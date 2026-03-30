use std::io::Read;

use url::Url;
use wezterm_term::color::ColorPalette;
use wezterm_term::{CursorPosition, StableRowIndex};

use crate::osc::NotificationEvent;
use crate::pane::{AdvanceResult, SequenceNo, TermError};

/// Trait abstracting the terminal backend (wezterm-term, libghostty, etc.).
///
/// Covers lifecycle, state queries, process management, change tracking,
/// text reading, and terminal control. Does NOT include raw screen access
/// (`screen()`) — that is backend-specific and accessed via the concrete type
/// for GPU rendering and cell-level iteration.
///
/// Step 1 of the libghostty POC: the trait uses wezterm-term types (CursorPosition,
/// ColorPalette, StableRowIndex). Step 2 will introduce amux-native equivalents.
pub trait TerminalBackend {
    // --- Lifecycle ---

    /// Read available bytes from the PTY and feed them to the terminal.
    fn advance(&mut self) -> AdvanceResult;

    /// Resize the terminal and PTY.
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), TermError>;

    /// Write bytes to the PTY (simulate keyboard input).
    fn write_bytes(&mut self, data: &[u8]) -> Result<(), TermError>;

    /// Feed raw bytes directly into the terminal state machine.
    fn feed_bytes(&mut self, data: &[u8]);

    /// Take the PTY reader for use in a background thread.
    /// After this, `advance()` returns `Eof`; use `feed_bytes()` instead.
    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>>;

    // --- State queries ---

    /// Window title (set by OSC 0/2).
    fn title(&self) -> &str;

    /// Working directory (set by OSC 7).
    fn working_dir(&self) -> Option<&Url>;

    /// Terminal dimensions as (cols, rows).
    fn dimensions(&self) -> (usize, usize);

    /// Current cursor position.
    fn cursor(&self) -> CursorPosition;

    /// Current color palette.
    fn palette(&self) -> ColorPalette;

    /// Whether the alternate screen buffer is active.
    fn is_alt_screen_active(&self) -> bool;

    /// Whether bracketed paste mode is enabled (DECSET 2004).
    fn bracketed_paste_enabled(&self) -> bool;

    // --- Process management ---

    /// Child process ID, if available.
    fn child_pid(&self) -> Option<u32>;

    /// Whether the child process is still running.
    fn is_alive(&mut self) -> bool;

    /// Child exit status, if it has exited.
    fn exit_status(&mut self) -> Option<portable_pty::ExitStatus>;

    // --- Change tracking ---

    /// Stable row indices of lines changed since last `mark_rendered()`.
    fn changed_lines(&self) -> Vec<StableRowIndex>;

    /// Mark the current state as rendered.
    fn mark_rendered(&mut self);

    /// Current sequence number.
    fn current_seqno(&self) -> SequenceNo;

    /// Last-rendered sequence number.
    fn rendered_seqno(&self) -> SequenceNo;

    // --- Text reading ---

    /// Total rows in scrollback + visible screen.
    fn scrollback_rows(&self) -> usize;

    /// Read lines by spec: "1-50", "-20" (last 20), or "5" (single line).
    fn read_screen_lines(&self, line_spec: &str, ansi: bool) -> String;

    /// Read visible screen content as plain text.
    fn read_screen_text(&self) -> String;

    /// Read scrollback with ANSI formatting, up to `max_lines`.
    fn read_scrollback_text(&self, max_lines: usize) -> String;

    /// Read a range of physical rows with ANSI formatting.
    fn read_scrollback_text_range(&self, start: usize, end: usize) -> String;

    /// Search scrollback for case-insensitive matches.
    /// Returns `(phys_row, start_col, end_col)` tuples.
    fn search_scrollback(&self, query: &str) -> Vec<(usize, usize, usize)>;

    // --- Terminal control ---

    /// Erase scrollback buffer, keeping visible screen.
    fn erase_scrollback(&mut self);

    /// Notify terminal of focus change (DECSET 1004).
    fn focus_changed(&mut self, focused: bool);

    // --- Notifications ---

    /// Drain pending notification events from the alert handler.
    fn drain_notifications(&self) -> Vec<NotificationEvent>;
}
