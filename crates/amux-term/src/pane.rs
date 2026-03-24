use std::io::{Read, Write};
use std::sync::{mpsc, Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use url::Url;
use wezterm_term::color::ColorPalette;
use wezterm_term::terminal::Terminal;
use wezterm_term::{CursorPosition, StableRowIndex, TerminalSize};

use crate::config::AmuxTermConfig;
use crate::osc::{ChannelAlertHandler, NotificationEvent};

/// Sequence number type (matches wezterm_surface::SequenceNo = usize).
pub type SequenceNo = usize;

/// Result of `TerminalPane::advance()`.
pub enum AdvanceResult {
    /// Bytes were read and fed to the terminal.
    Read(usize),
    /// The PTY read would block (no data available).
    WouldBlock,
    /// The PTY has closed (child exited).
    Eof,
}

/// A terminal pane wrapping wezterm-term + portable-pty.
///
/// Owns the PTY master, child process, and terminal state machine.
/// The reader is used to pull bytes from the PTY and feed them to the terminal.
/// Write handle that can be cloned and shared between Terminal (for responses)
/// and keyboard input. Wraps the single PTY writer via Arc<Mutex>.
struct SharedWriter(Arc<Mutex<Box<dyn Write + Send>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}

/// A terminal pane wrapping wezterm-term + portable-pty.
///
/// Owns the PTY master, child process, and terminal state machine.
/// The reader is used to pull bytes from the PTY and feed them to the terminal.
pub struct TerminalPane {
    terminal: Terminal,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    seqno: SequenceNo,
    notification_rx: mpsc::Receiver<NotificationEvent>,
}

impl TerminalPane {
    /// Spawn a new terminal pane running the given command.
    pub fn spawn(
        cols: u16,
        rows: u16,
        cmd: CommandBuilder,
        config: Arc<AmuxTermConfig>,
    ) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();
        let pty_size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system.openpty(pty_size)?;

        let child = pair.slave.spawn_command(cmd)?;
        let reader = pair.master.try_clone_reader()?;

        let terminal_size = TerminalSize {
            rows: rows as usize,
            cols: cols as usize,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        };

        let writer = pair.master.take_writer()?;
        let shared = Arc::new(Mutex::new(writer));
        let terminal_writer = SharedWriter(Arc::clone(&shared));
        let mut terminal = Terminal::new(
            terminal_size,
            config,
            "amux",
            "0.1.0",
            Box::new(terminal_writer),
        );

        let (tx, rx) = mpsc::channel();
        terminal.set_notification_handler(Box::new(ChannelAlertHandler::new(tx)));

        Ok(Self {
            terminal,
            master: pair.master,
            child,
            reader: Some(reader),
            writer: shared,
            seqno: 0,
            notification_rx: rx,
        })
    }

    /// Read available bytes from the PTY and feed them to the terminal state machine.
    ///
    /// Returns `AdvanceResult::Read(n)` with the number of bytes consumed,
    /// `WouldBlock` if no data was available, or `Eof` if the PTY closed.
    pub fn advance(&mut self) -> AdvanceResult {
        let reader = match &mut self.reader {
            Some(r) => r,
            None => return AdvanceResult::Eof,
        };
        let mut buf = [0u8; 8192];
        match reader.read(&mut buf) {
            Ok(0) => AdvanceResult::Eof,
            Ok(n) => {
                self.terminal.advance_bytes(&buf[..n]);
                AdvanceResult::Read(n)
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => AdvanceResult::WouldBlock,
            Err(_) => AdvanceResult::Eof,
        }
    }

    /// Resize the terminal and PTY to the given dimensions.
    pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        let pty_size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        self.master.resize(pty_size)?;

        let terminal_size = TerminalSize {
            rows: rows as usize,
            cols: cols as usize,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        };
        self.terminal.resize(terminal_size);
        Ok(())
    }

    /// Write bytes to the PTY (i.e. simulate keyboard input).
    pub fn write_bytes(&mut self, data: &[u8]) -> anyhow::Result<()> {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(data)?;
        Ok(())
    }

    /// Take the PTY reader out of the pane for use in a background thread.
    ///
    /// After calling this, `advance()` will always return `Eof`.
    /// Use `feed_bytes()` to feed data from the reader back to the terminal.
    pub fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    /// Feed raw bytes directly into the terminal state machine.
    ///
    /// Use this when running the PTY reader in a background thread.
    pub fn feed_bytes(&mut self, data: &[u8]) {
        self.terminal.advance_bytes(data);
    }

    /// Borrow the current terminal screen.
    pub fn screen(&self) -> &wezterm_term::screen::Screen {
        self.terminal.screen()
    }

    /// Whether the alternate screen buffer is active.
    pub fn is_alt_screen_active(&self) -> bool {
        self.terminal.is_alt_screen_active()
    }

    /// Get the window title (set by OSC 0/2).
    pub fn title(&self) -> &str {
        self.terminal.get_title()
    }

    /// Get the working directory (set by OSC 7).
    pub fn working_dir(&self) -> Option<&Url> {
        self.terminal.get_current_dir()
    }

    /// Check whether the child process is still alive.
    pub fn is_alive(&mut self) -> bool {
        // try_wait returns Ok(Some(status)) if exited, Ok(None) if still running
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Get the child exit status, if it has exited.
    pub fn exit_status(&mut self) -> Option<portable_pty::ExitStatus> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status),
            _ => None,
        }
    }

    /// Get stable row indices of lines changed since the last `mark_rendered()` call.
    pub fn changed_lines(&self) -> Vec<StableRowIndex> {
        let size = self.terminal.get_size();
        let screen = self.terminal.screen();
        // Compute the visible stable row range
        let first_visible = screen.visible_row_to_stable_row(0);
        let last_visible = screen.visible_row_to_stable_row(size.rows as i64 - 1);
        screen.get_changed_stable_rows(first_visible..last_visible + 1, self.seqno)
    }

    /// Advance the internal seqno to mark the current state as rendered.
    pub fn mark_rendered(&mut self) {
        self.seqno = self.terminal.current_seqno();
    }

    /// Get the current cursor position.
    pub fn cursor(&self) -> CursorPosition {
        self.terminal.cursor_pos()
    }

    /// Get terminal dimensions as (cols, rows).
    pub fn dimensions(&self) -> (usize, usize) {
        let size = self.terminal.get_size();
        (size.cols, size.rows)
    }

    /// Get the current color palette.
    pub fn palette(&self) -> ColorPalette {
        self.terminal.palette()
    }

    /// Drain pending notification events from the alert handler channel.
    pub fn drain_notifications(&self) -> Vec<NotificationEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.notification_rx.try_recv() {
            events.push(event);
        }
        events
    }

    /// Get the current sequence number.
    pub fn current_seqno(&self) -> SequenceNo {
        self.terminal.current_seqno()
    }

    /// Get the last-rendered sequence number.
    pub fn rendered_seqno(&self) -> SequenceNo {
        self.seqno
    }

    /// Read the visible screen content as a string (lines joined by newlines).
    pub fn read_screen_text(&self) -> String {
        let (cols, rows) = self.dimensions();
        let screen = self.terminal.screen();
        let total = screen.scrollback_rows();
        let start = total.saturating_sub(rows);
        let lines = screen.lines_in_phys_range(start..total);

        let mut result = String::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                result.push('\n');
            }
            let mut line_text = String::new();
            for cell in line.visible_cells() {
                if cell.cell_index() >= cols {
                    break;
                }
                line_text.push_str(cell.str());
            }
            result.push_str(line_text.trim_end());
        }
        result
    }

    /// Read scrollback + visible screen as text, up to `max_lines` lines.
    /// Unlike `read_screen_text` which only reads the visible viewport,
    /// this captures the full scrollback buffer for session persistence.
    pub fn read_scrollback_text(&self, max_lines: usize) -> String {
        let (cols, _) = self.dimensions();
        let screen = self.terminal.screen();
        let total = screen.scrollback_rows();
        let start = total.saturating_sub(max_lines);
        let lines = screen.lines_in_phys_range(start..total);

        let mut result = String::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                result.push('\n');
            }
            let mut line_text = String::new();
            for cell in line.visible_cells() {
                if cell.cell_index() >= cols {
                    break;
                }
                line_text.push_str(cell.str());
            }
            result.push_str(line_text.trim_end());
        }
        result
    }
}
