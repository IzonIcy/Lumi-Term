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

#[derive(Clone, Debug)]
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
        let screen = self.parser.screen();
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
}
