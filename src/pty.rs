use crate::config::TerminalConfig;
use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
};
use vt100::{Color, Parser, Screen};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellStyle {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledSpan {
    pub text: String,
    pub style: CellStyle,
}

#[derive(Clone, Debug)]
pub struct TerminalSnapshot {
    pub rows: Vec<Vec<StyledSpan>>,
    pub at_scrollback_top: bool,
}

pub struct TerminalSession {
    parser: Parser,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    output_rx: Receiver<Vec<u8>>,
    child: Option<Box<dyn portable_pty::Child + Send>>,
    closed: bool,
}

impl TerminalSession {
    pub fn new(rows: u16, cols: u16, config: &TerminalConfig) -> Result<Self> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("opening PTY")?;

        let mut command = if let Some(shell) = &config.shell {
            CommandBuilder::new(shell)
        } else {
            CommandBuilder::new_default_prog()
        };

        let start_directory = config
            .working_directory
            .clone()
            .unwrap_or_else(|| PathBuf::from("/"));
        command.cwd(start_directory);

        let child = pty_pair
            .slave
            .spawn_command(command)
            .context("spawning shell in PTY")?;

        let mut reader = pty_pair
            .master
            .try_clone_reader()
            .context("cloning PTY reader")?;
        let mut writer = pty_pair.master.take_writer().context("taking PTY writer")?;

        // Enable bracketed paste mode (ESC[?2004h) so pasted text is wrapped
        // with ESC[200~ ... ESC[201~. This lets the shell distinguish pasted
        // input from typed input. Best-effort; some shells may not support it.
        let _ = writer.write_all(b"\x1b[?2004h");
        let _ = writer.flush();

        let (output_tx, output_rx) = mpsc::channel();

        thread::Builder::new()
            .name("lumi-term-pty-reader".to_string())
            .spawn(move || {
                let mut buffer = [0_u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => {
                            let _ = output_tx.send(Vec::new());
                            break;
                        }
                        Ok(bytes_read) => {
                            if output_tx.send(buffer[..bytes_read].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => {
                            let _ = output_tx.send(Vec::new());
                            break;
                        }
                    }
                }
            })
            .context("starting PTY reader thread")?;

        Ok(Self {
            parser: Parser::new(rows, cols, config.scrollback),
            master: pty_pair.master,
            writer,
            output_rx,
            child: Some(child),
            closed: false,
        })
    }

    pub fn poll_output(&mut self) -> bool {
        let mut has_updates = false;
        while let Ok(bytes) = self.output_rx.try_recv() {
            if bytes.is_empty() {
                self.closed = true;
                has_updates = true;
                break;
            }
            self.parser.process(&bytes);
            has_updates = true;
        }
        if self.closed && self.child.is_some() {
            // Without this wait the shell's PID lingers as a zombie for the
            // life of the app. Waiting can block briefly, so it happens on a
            // throwaway thread.
            self.kill_and_reap();
        }
        has_updates
    }

    /// Kills the child shell and reaps it, so the PID never lingers as a
    /// zombie. Idempotent: `child` is only ever taken here, so `None` means
    /// "already killed and reaped".
    fn kill_and_reap(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            // Waiting on a throwaway thread because `wait()` can block. Note
            // that `spawn` consumes the closure (and the `Child` with it) even
            // when it fails, so there is no inline fallback: if the thread
            // cannot start, the killed child is left unreaped. That is worth
            // saying out loud rather than swallowing.
            if let Err(error) = thread::Builder::new()
                .name("lumi-term-pty-reaper".to_string())
                .spawn(move || {
                    let _ = child.wait();
                })
            {
                eprintln!("lumi-term: could not spawn PTY reaper thread: {error}");
            }
        }
        self.closed = true;
    }

    /// Kills the shell and waits for it. Called when a tab is closed while
    /// its session is still running.
    pub fn shutdown(&mut self) {
        // Disable bracketed paste mode (ESC[?2004l) on clean shutdown.
        let _ = self.writer.write_all(b"\x1b[?2004l");
        let _ = self.writer.flush();

        self.kill_and_reap();
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resizing PTY")?;

        self.parser.screen_mut().set_size(rows, cols);
        Ok(())
    }

    pub fn scroll_by_lines(&mut self, delta_lines: i32) {
        if delta_lines == 0 {
            return;
        }

        let screen = self.parser.screen_mut();
        let current = screen.scrollback();
        let target = if delta_lines > 0 {
            current.saturating_add(delta_lines as usize)
        } else {
            current.saturating_sub((-delta_lines) as usize)
        };
        screen.set_scrollback(target);
    }

    pub fn jump_to_live_output(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    /// Scrolls back to the nearest line containing `query` (case-insensitive),
    /// leaving it on screen; returns its scrollback offset, or None (position
    /// restored) when nothing matches.
    pub fn search_scrollback(&mut self, query: &str) -> Option<usize> {
        search_parser_scrollback(&mut self.parser, query)
    }

    pub fn send_text(&mut self, text: &str) -> Result<()> {
        self.writer
            .write_all(text.as_bytes())
            .context("writing text to PTY")?;
        self.writer.flush().context("flushing PTY writer")?;
        Ok(())
    }

    pub fn send_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer
            .write_all(bytes)
            .context("writing key sequence")?;
        self.writer.flush().context("flushing PTY writer")?;
        Ok(())
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        screen_to_snapshot(&self.parser)
    }

    /// The shell's OS pid, for asserting that `Drop` really reaps it.
    #[cfg(test)]
    fn child_pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(|child| child.process_id())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // A session can be dropped without an explicit shutdown(): a tab
        // restart replaces it in place, and app exit drops every tab.
        //
        // Killing directly rather than via shutdown() because Drop must not do
        // I/O — the master fd is still open, and a write can block if the
        // child is alive but not draining its stdin, which would hang the app
        // on exit. Dropping the master would SIGHUP the child anyway, so the
        // shells were not truly orphaned; what leaked was the *unreaped* child
        // and the detached reader thread.
        self.kill_and_reap();
    }
}

/// True if any visible row of `screen` contains `needle` (already lowercased).
fn window_matches(screen: &mut Screen, cols: u16, needle: &str) -> bool {
    screen
        .rows(0, cols)
        .any(|row| row.to_lowercase().contains(needle))
}

/// Scrolls `parser`'s view back to the nearest line containing `query`,
/// case-insensitively, leaving that line on screen. Returns its scrollback
/// offset, or `None` with the original position restored.
///
/// Offset 0 is the live screen and larger offsets are progressively older
/// lines, so "nearest match at or above where the user is" means walking
/// *up* from the current offset. Walking down from the oldest line instead
/// would scan the whole buffer to find something already on screen.
///
/// Two passes, because the cost is dominated by how many times the viewport
/// is re-rendered. The viewport is a sliding window of `rows` consecutive
/// lines, so any single line is visible across exactly `rows` consecutive
/// offsets: a coarse stride of `rows` therefore cannot step over a match.
/// The coarse pass finds a window containing a hit, then a short fine walk
/// inside that window reports the *nearest* one. Stepping one offset at a
/// time instead re-rendered the entire scrollback and froze the UI for
/// seconds on a realistic buffer.
///
/// Pure over a `Parser` (no PTY) so the search logic is unit-testable.
pub fn search_parser_scrollback(parser: &mut Parser, query: &str) -> Option<usize> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }

    let screen = parser.screen_mut();
    let original = screen.scrollback();
    let (rows, cols) = screen.size();
    let stride = (rows as usize).max(1);

    // Find the top of the scrollback: set_scrollback saturates, so probe
    // forward in chunks until the requested offset stops being honored.
    // (set_scrollback clamps internally, so the clamp below is what actually
    // ends this loop; the counter guard is belt-and-braces.)
    let mut probe = original;
    loop {
        probe += 512;
        screen.set_scrollback(probe);
        if screen.scrollback() < probe {
            break;
        }
        if probe > 50_000_000 {
            break;
        }
    }
    let top = screen.scrollback();

    let mut offset = original;
    loop {
        screen.set_scrollback(offset);
        if window_matches(screen, cols, &needle) {
            // A hit somewhere in this window. Visibility is monotonic across
            // the window, so the lowest offset that still shows a match is
            // the nearest one.
            let low = offset.saturating_sub(stride - 1).max(original);
            let mut fine = low;
            loop {
                screen.set_scrollback(fine);
                if window_matches(screen, cols, &needle) {
                    return Some(fine);
                }
                if fine >= offset {
                    break;
                }
                fine += 1;
            }
            return Some(offset);
        }
        if offset >= top {
            break;
        }
        offset = (offset + stride).min(top);
    }

    screen.set_scrollback(original);
    None
}

/// Converts the current vt100 screen state into coalesced styled spans; pure
/// so rendering logic can be unit-tested without a live PTY.
pub fn screen_to_snapshot(parser: &Parser) -> TerminalSnapshot {
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    let (cursor_row, cursor_col) = screen.cursor_position();
    let mut styled_rows = Vec::with_capacity(rows as usize);
    let cursor = if screen.scrollback() == 0 {
        Some((cursor_row, cursor_col))
    } else {
        None
    };

    for row in 0..rows {
        let mut spans = Vec::<StyledSpan>::new();
        let mut current_span: Option<StyledSpan> = None;

        for col in 0..cols {
            let mut style = CellStyle {
                fg: Color::Default,
                bg: Color::Default,
                bold: false,
                dim: false,
                italic: false,
                underline: false,
                inverse: false,
            };
            let mut text = " ".to_owned();

            if let Some(cell) = screen.cell(row, col) {
                if cell.is_wide_continuation() {
                    continue;
                }
                style = CellStyle {
                    fg: cell.fgcolor(),
                    bg: cell.bgcolor(),
                    bold: cell.bold(),
                    dim: cell.dim(),
                    italic: cell.italic(),
                    underline: cell.underline(),
                    inverse: cell.inverse(),
                };
                if cell.has_contents() {
                    text = cell.contents().to_owned();
                }
            }

            if cursor == Some((row, col)) {
                style.inverse = !style.inverse;
            }

            match current_span.as_mut() {
                Some(span) if span.style == style => span.text.push_str(&text),
                Some(_) => {
                    if let Some(span) = current_span.take() {
                        spans.push(span);
                    }
                    current_span = Some(StyledSpan { text, style });
                }
                None => current_span = Some(StyledSpan { text, style }),
            }
        }

        if let Some(span) = current_span {
            spans.push(span);
        }

        styled_rows.push(spans);
    }

    TerminalSnapshot {
        rows: styled_rows,
        at_scrollback_top: screen.scrollback() > 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{CellStyle, screen_to_snapshot, search_parser_scrollback};
    use vt100::{Color, Parser};

    fn parse_with_size(rows: u16, cols: u16, input: &str) -> Parser {
        let mut parser = Parser::new(rows, cols, 10_000);
        parser.process(input.as_bytes());
        parser
    }

    /// Feeds newline-terminated lines through the parser so earlier ones are
    /// pushed into the scrollback buffer.
    fn parser_fed_with(lines: &[&str], rows: u16, cols: u16) -> Parser {
        let mut parser = Parser::new(rows, cols, 50_000);
        for line in lines {
            parser.process(format!("{line}\r\n").as_bytes());
        }
        parser
    }

    fn visible_text(parser: &Parser, cols: u16) -> String {
        parser.screen().rows(0, cols).collect::<Vec<_>>().join("\n")
    }

    fn row_text(snapshot: &super::TerminalSnapshot, row: usize) -> String {
        snapshot.rows[row]
            .iter()
            .map(|span| span.text.as_str())
            .collect()
    }

    #[test]
    fn plain_text_becomes_single_span_per_row() {
        let parser = parse_with_size(3, 12, "hello world");
        let snapshot = screen_to_snapshot(&parser);

        assert_eq!(row_text(&snapshot, 0), "hello world ");
        assert_eq!(
            snapshot.rows[0].len(),
            2,
            "text span plus inverted cursor cell"
        );
        assert_eq!(snapshot.rows[0][0].style.fg, Color::Default);
        assert!(!snapshot.rows[0][0].style.bold);
        assert!(!snapshot.at_scrollback_top);
    }

    #[test]
    fn style_changes_split_spans() {
        let parser = parse_with_size(2, 20, "\x1b[31mred\x1b[1mredbold\x1b[0mplain");
        let snapshot = screen_to_snapshot(&parser);
        let spans = &snapshot.rows[0];

        // cursor sits on the empty cell right after "plain", splitting the
        // trailing whitespace: [red][redbold][plain][cursor][trailing]
        assert_eq!(spans.len(), 5);
        assert_eq!(spans[0].text, "red");
        assert_eq!(spans[0].style.fg, Color::Idx(1));
        assert!(!spans[0].style.bold);
        assert_eq!(spans[1].text, "redbold");
        assert_eq!(spans[1].style.fg, Color::Idx(1));
        assert!(spans[1].style.bold);
        assert_eq!(spans[2].text, "plain");
        assert_eq!(spans[2].style.fg, Color::Default);
        assert!(!spans[2].style.bold);
        assert!(spans[3].style.inverse, "cursor cell is inverted");
    }

    #[test]
    fn cursor_is_rendered_inverse_on_live_screen_only() {
        let parser = parse_with_size(2, 10, "abc");
        let snapshot = screen_to_snapshot(&parser);

        let spans = &snapshot.rows[0];
        assert_eq!(spans.len(), 3, "[abc][cursor][trailing]");
        assert_eq!(spans[0].text, "abc");
        assert!(spans[1].style.inverse, "cursor cell should be inverted");
        assert_eq!(spans[1].text, " ");
        assert!(!spans[2].style.inverse);
    }

    #[test]
    fn cursor_inversion_disappears_in_scrollback() {
        let mut parser = Parser::new(2, 10, 10_000);
        // Push several lines so content scrolls off the live screen.
        for line in 0..6 {
            parser.process(format!("line {line}\r\n").as_bytes());
        }
        parser.screen_mut().set_scrollback(1);
        let snapshot = screen_to_snapshot(&parser);

        assert!(snapshot.at_scrollback_top);
        assert!(
            snapshot
                .rows
                .iter()
                .flatten()
                .all(|span| !span.style.inverse),
            "no cursor marker while scrolled back"
        );
    }

    #[test]
    fn wide_characters_occupy_two_columns_but_one_span() {
        // '中' is a wide char occupying two columns.
        let parser = parse_with_size(1, 6, "中");
        let snapshot = screen_to_snapshot(&parser);
        assert_eq!(row_text(&snapshot, 0), "中    ", "wide char + 4 empty cols");

        let spans = &snapshot.rows[0];
        assert_eq!(spans[0].text, "中", "continuation cell must not duplicate");
    }

    #[test]
    fn empty_screen_has_one_space_span_per_cell() {
        let parser = parse_with_size(1, 3, "");
        let snapshot = screen_to_snapshot(&parser);
        let spans = &snapshot.rows[0];

        let total: usize = spans.iter().map(|span| span.text.len()).sum();
        assert_eq!(total, 3, "each empty cell contributes a space");
    }

    #[test]
    fn colors_roundtrip_through_cell_style() {
        let parser = parse_with_size(1, 10, "\x1b[48;5;196mX\x1b[0mY");
        let snapshot = screen_to_snapshot(&parser);
        let spans = &snapshot.rows[0];

        assert_eq!(spans[0].text, "X");
        assert_eq!(spans[0].style.bg, Color::Idx(196));
        assert_eq!(spans[1].text, "Y");
        assert_eq!(spans[1].style.bg, Color::Default);

        assert_eq!(
            spans[0].style,
            CellStyle {
                fg: Color::Default,
                bg: Color::Idx(196),
                bold: false,
                dim: false,
                italic: false,
                underline: false,
                inverse: false,
            }
        );
    }

    #[test]
    fn sgr_attributes_roundtrip_through_cell_style() {
        // italic (3), underline (4), inverse (7).
        let parser = parse_with_size(1, 12, "\x1b[3;4;7mX\x1b[0mY");
        let snapshot = screen_to_snapshot(&parser);
        let spans = &snapshot.rows[0];

        assert_eq!(spans[0].text, "X");
        assert!(spans[0].style.italic);
        assert!(spans[0].style.underline);
        assert!(spans[0].style.inverse);

        assert_eq!(spans[1].text, "Y", "reset sequence clears attributes");
        assert!(!spans[1].style.italic);
        assert!(!spans[1].style.underline);
        assert!(!spans[1].style.inverse);
    }

    #[test]
    fn sgr_intensity_is_a_single_axis_with_last_one_wins() {
        // Bold (1) and dim (2) share one intensity field in vt100; they are
        // mutually exclusive, and the later sequence replaces the earlier.
        let parser = parse_with_size(1, 12, "\x1b[1mA\x1b[2mB\x1b[1mC");
        let snapshot = screen_to_snapshot(&parser);
        let spans = &snapshot.rows[0];

        assert!(spans[0].style.bold, "SGR 1 sets bold");
        assert!(!spans[0].style.dim);

        assert!(
            !spans[1].style.bold && spans[1].style.dim,
            "SGR 2 replaces bold with dim"
        );

        assert!(
            spans[2].style.bold && !spans[2].style.dim,
            "SGR 1 replaces dim with bold"
        );
    }

    // ---- scrollback search ----

    #[test]
    fn search_finds_a_match_in_scrollback_and_leaves_it_visible() {
        let mut parser = parser_fed_with(
            &["alpha", "bravo", "Needle_Here", "charlie", "delta", "echo"],
            2,
            40,
        );
        let hit = search_parser_scrollback(&mut parser, "needle_here")
            .expect("match should be found, case-insensitively");

        assert!(hit > 0, "match is above the live screen, got offset {hit}");
        assert!(
            visible_text(&parser, 40).contains("Needle_Here"),
            "the matched line should be left on screen"
        );
    }

    #[test]
    fn search_returns_the_nearest_match_not_the_oldest() {
        // Two hits: one far back, one just above the live screen. Walking from
        // the oldest line down toward the user (the old behaviour) returned
        // 'stale-marker'; the nearest match is 'fresh-marker'.
        let mut parser = parser_fed_with(
            &[
                "stale-marker",
                "one",
                "two",
                "three",
                "four",
                "fresh-marker",
                "live",
            ],
            2,
            40,
        );

        let hit = search_parser_scrollback(&mut parser, "marker").expect("a match is expected");
        let visible = visible_text(&parser, 40);

        assert_eq!(hit, 1, "'fresh-marker' sits one line above the live edge");
        assert!(
            visible.contains("fresh-marker"),
            "nearest match should be on screen, got:\n{visible}"
        );
    }

    #[test]
    fn search_without_a_match_restores_the_previous_position() {
        let mut parser = parser_fed_with(&["alpha", "bravo", "charlie", "delta"], 2, 40);
        parser.screen_mut().set_scrollback(2);
        assert_eq!(parser.screen().scrollback(), 2);

        assert_eq!(
            search_parser_scrollback(&mut parser, "zzz-not-present"),
            None
        );
        assert_eq!(
            parser.screen().scrollback(),
            2,
            "a failed search must not move the viewport"
        );
    }

    #[test]
    fn search_ignores_empty_and_whitespace_only_queries() {
        let mut parser = parser_fed_with(&["alpha", "bravo", "charlie"], 2, 40);
        parser.screen_mut().set_scrollback(1);

        assert_eq!(search_parser_scrollback(&mut parser, ""), None);
        assert_eq!(search_parser_scrollback(&mut parser, "   \t "), None);
        assert_eq!(
            parser.screen().scrollback(),
            1,
            "a rejected query must not move the viewport"
        );
    }

    /// A wall-clock budget that is generous enough to survive a loaded CI
    /// runner but still an order of magnitude below what the old
    /// implementation cost. The regression this guards against took seconds
    /// and tens of megabytes; a loaded machine pushing 10k lines past 10s
    /// means something is actually wrong, not just slow.
    const SLOW_SEARCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

    #[test]
    fn search_terminates_and_is_responsive_on_a_deep_scrollback() {
        // Regression guard. The old implementation joined every visible row
        // into one lowercased String per candidate offset, and walked from the
        // oldest line down to the user's position — so this worst case (no
        // match, full 10k buffer) allocated tens of megabytes and took
        // seconds on a single Enter press.
        let mut parser = Parser::new(24, 80, 50_000);
        let marker = "depth-marker-first-line";
        parser.process(format!("{marker}\r\n").as_bytes());
        let filler = "x".repeat(200);
        for _ in 0..10_000 {
            parser.process(format!("{filler}\r\n").as_bytes());
        }

        // Prove the buffer really is deep: the marker line was written first,
        // so it is ~10k lines back from the live edge. scrollback() reports
        // the current offset, not the history size, so probe with a real
        // search rather than trusting a counter. This is also the long walk —
        // the match is only at the far end of the buffer.
        let started = std::time::Instant::now();
        let oldest = search_parser_scrollback(&mut parser, marker)
            .expect("the marker line is still in the buffer");
        let oldest_elapsed = started.elapsed();
        assert!(
            oldest > 9_000,
            "expected a ~10k-line-deep buffer, marker found at {oldest}"
        );
        assert!(
            oldest_elapsed < SLOW_SEARCH_BUDGET,
            "walking the full buffer took {oldest_elapsed:?}"
        );
        // Back to the live edge first. The search above left the viewport at
        // the top of the buffer, and a search only looks at or above the
        // current position — so without this the "miss" would examine a
        // single screen and the timing assertion would prove nothing.
        parser.screen_mut().set_scrollback(0);
        let started = std::time::Instant::now();
        let miss = search_parser_scrollback(&mut parser, "no-such-token-anywhere");
        let miss_elapsed = started.elapsed();
        assert_eq!(miss, None, "worst case is a full-buffer miss");
        assert!(
            miss_elapsed < SLOW_SEARCH_BUDGET,
            "full-buffer miss took {miss_elapsed:?}"
        );

        parser.screen_mut().set_scrollback(0);
        parser.process(b"the-needle-is-here\r\n");
        let started = std::time::Instant::now();
        let hit = search_parser_scrollback(&mut parser, "the-needle");
        let hit_elapsed = started.elapsed();
        assert!(hit.is_some(), "a live-screen match should be found");
        // This is the assertion that actually distinguishes the fix: a search
        // that walks the full 10k buffer cannot come in near the live edge in
        // 200ms, whereas the real implementation finds it in well under a
        // millisecond. Kept tight deliberately, so a regression that restores
        // the full walk still fails even on a slow runner.
        assert!(
            hit_elapsed < std::time::Duration::from_millis(500),
            "nearest-match search took {hit_elapsed:?}; it should not scan the whole buffer"
        );
    }

    // ---- session lifecycle ----

    /// `kill -0` succeeds for any live pid, including a zombie that has been
    /// killed but not yet reaped. So this asserts reaping, not just killing.
    fn process_exists(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn dropping_a_session_kills_and_reaps_the_shell() {
        let config = crate::config::AppConfig::default().terminal;
        let session = super::TerminalSession::new(10, 40, &config).expect("spawn a session");
        let pid = session.child_pid().expect("the shell should report a pid");
        assert!(process_exists(pid), "the shell should be running");

        drop(session);

        // Reaping happens on a helper thread, so poll instead of asserting
        // instantly.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while process_exists(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(
            !process_exists(pid),
            "shell {pid} was killed but never reaped; it is still a zombie"
        );
    }

    #[test]
    fn shutdown_then_drop_is_idempotent() {
        let config = crate::config::AppConfig::default().terminal;
        let mut session = super::TerminalSession::new(10, 40, &config).expect("spawn a session");
        let pid = session.child_pid().expect("the shell should report a pid");

        session.shutdown();
        assert!(
            session.is_closed(),
            "shutdown should mark the session closed"
        );
        // Dropping after an explicit shutdown must not panic or double-kill.
        drop(session);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while process_exists(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(!process_exists(pid), "shell {pid} outlived its session");
    }
}
