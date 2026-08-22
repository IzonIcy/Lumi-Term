use crate::config::TerminalConfig;
use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
};
use vt100::{Color, Parser};

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
    _child: Box<dyn portable_pty::Child + Send>,
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
        let writer = pty_pair.master.take_writer().context("taking PTY writer")?;
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
            _child: child,
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
        has_updates
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

    /// Scroll backwards through the buffer to the nearest line containing
    /// `query` (case-insensitive). On a hit the screen is left scrolled at the
    /// match so it stays visible; returns that scrollback offset. Returns None
    /// (and restores the previous position) when nothing matches.
    pub fn search_scrollback(&mut self, query: &str) -> Option<usize> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return None;
        }

        let screen = self.parser.screen_mut();
        let original = screen.scrollback();
        let cols = screen.size().1;

        // Find the top of the scrollback: set_scrollback saturates, so probe
        // forward in chunks until the requested offset stops being honored.
        let mut probe = original;
        loop {
            probe += 512;
            screen.set_scrollback(probe);
            if screen.scrollback() < probe {
                break;
            }
            if probe > 50_000_000 {
                break; // absurd guard; real buffers never get here
            }
        }
        let top = screen.scrollback();

        // Walk down from the top toward where the user was, first match wins.
        let mut offset = top;
        loop {
            screen.set_scrollback(offset);
            let haystack: String = screen
                .rows(0, cols)
                .collect::<Vec<_>>()
                .join("\n")
                .to_lowercase();
            if haystack.contains(&needle) {
                return Some(offset);
            }
            if offset == 0 || offset <= original {
                break;
            }
            offset -= 1;
        }

        screen.set_scrollback(original);
        None
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
}

/// Converts the current vt100 screen state into coalesced styled spans.
///
/// Pure function of the parser state so rendering logic can be unit-tested
/// without a live PTY.
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
    use super::{CellStyle, screen_to_snapshot};
    use vt100::{Color, Parser};

    fn parse_with_size(rows: u16, cols: u16, input: &str) -> Parser {
        let mut parser = Parser::new(rows, cols, 10_000);
        parser.process(input.as_bytes());
        parser
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
        // Bold (1) and dim (2) share one intensity field in vt100 — they are
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
}
