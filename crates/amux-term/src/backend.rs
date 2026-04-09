use std::io::Read;

use url::Url;

use crate::osc::NotificationEvent;

/// Typed errors for terminal pane operations.
#[derive(Debug, thiserror::Error)]
pub enum TermError {
    #[error("PTY setup failed: {0}")]
    PtySetupFailed(#[source] anyhow::Error),

    #[error("resize failed: {0}")]
    ResizeFailed(#[source] anyhow::Error),

    #[error("write failed: {0}")]
    WriteFailed(#[source] std::io::Error),
}

/// Sequence number type (matches rendering generation counter).
pub type SequenceNo = usize;

/// Result of advancing the terminal (reading PTY bytes).
pub enum AdvanceResult {
    /// Bytes were read and fed to the terminal.
    Read(usize),
    /// The PTY read would block (no data available).
    WouldBlock,
    /// The PTY has closed (child exited).
    Eof,
}

// --- Amux-native types ---

/// RGBA color as sRGB f32 values in [0.0, 1.0].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub f32, pub f32, pub f32, pub f32);

impl Color {
    pub const BLACK: Self = Self(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Self = Self(1.0, 1.0, 1.0, 1.0);
    pub const TRANSPARENT: Self = Self(0.0, 0.0, 0.0, 0.0);
}

/// Terminal cursor position.
#[derive(Clone, Copy, Debug, Default)]
pub struct CursorPos {
    pub x: usize,
    pub y: i64,
    pub shape: CursorShape,
    pub visible: bool,
}

/// Cursor rendering shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Default,
    BlinkingBlock,
    SteadyBlock,
    BlinkingUnderline,
    SteadyUnderline,
    BlinkingBar,
    SteadyBar,
}

/// Terminal color palette — the semantic colors a backend exposes.
#[derive(Clone, Debug)]
pub struct Palette {
    pub foreground: Color,
    pub background: Color,
    pub cursor_fg: Color,
    pub cursor_bg: Color,
    pub cursor_border: Color,
    pub selection_fg: Color,
    pub selection_bg: Color,
    /// 256-entry xterm color table (ANSI 0-15 + 216 cube + 24 grayscale).
    pub colors: Vec<Color>,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            foreground: Color::WHITE,
            background: Color::BLACK,
            cursor_fg: Color::BLACK,
            cursor_bg: Color::WHITE,
            cursor_border: Color::WHITE,
            selection_fg: Color::BLACK,
            selection_bg: Color(0.4, 0.6, 1.0, 1.0),
            colors: Vec::new(),
        }
    }
}

/// Process exit status.
#[derive(Clone, Debug)]
pub struct ProcessExit {
    code: i32,
    signal: Option<String>,
}

impl ProcessExit {
    pub fn new(code: i32, signal: Option<String>) -> Self {
        Self { code, signal }
    }

    /// True if the process exited with code 0 and no signal.
    pub fn success(&self) -> bool {
        self.signal.is_none() && self.code == 0
    }

    /// Exit code (0 = success).
    pub fn exit_code(&self) -> i32 {
        self.code
    }

    /// Signal name that killed the process, if any (e.g. "SIGTERM").
    pub fn signal(&self) -> Option<&str> {
        self.signal.as_deref()
    }
}

/// Stable row index — identifies a row across scrollback changes.
pub type StableRow = i64;

// --- Screen cell types for rendering ---

/// Underline decoration style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Dotted,
    Dashed,
    Curly,
}

/// A single cell's data, backend-agnostic.
#[derive(Clone, Debug)]
pub struct ScreenCell {
    /// Grapheme cluster (usually one char, can be multi-codepoint).
    pub text: String,
    /// Resolved foreground color (already palette-resolved).
    pub fg: Color,
    /// Resolved background color.
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: UnderlineStyle,
    /// Underline color override (None = use foreground color).
    pub underline_color: Option<Color>,
    pub strikethrough: bool,
    /// Faint/dim text (SGR 2).
    pub faint: bool,
    pub reverse: bool,
    pub hyperlink_url: Option<String>,
}

/// A row of cells for rendering.
#[derive(Clone, Debug)]
pub struct ScreenRow {
    pub cells: Vec<ScreenCell>,
    /// Whether this line wraps to the next.
    pub wrapped: bool,
}

impl From<portable_pty::ExitStatus> for ProcessExit {
    fn from(s: portable_pty::ExitStatus) -> Self {
        Self {
            code: s.exit_code() as i32,
            signal: s.signal().map(|name| name.to_string()),
        }
    }
}

// --- The trait ---

/// Trait abstracting the terminal backend.
///
/// Covers lifecycle, state queries, process management, change tracking,
/// text reading, and terminal control.
///
/// Uses amux-native types (CursorPos, Palette, etc.).
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

    /// Current cursor position, shape, and visibility.
    fn cursor(&self) -> CursorPos;

    /// Current color palette.
    fn palette(&self) -> Palette;

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
    fn exit_status(&mut self) -> Option<ProcessExit>;

    // --- Change tracking ---

    /// Stable row indices of lines changed since last `mark_rendered()`.
    fn changed_lines(&self) -> Vec<StableRow>;

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

    // --- Cell-level screen access (for rendering) ---

    /// Read visible screen as structured rows/cells with resolved colors.
    /// `scroll_offset` is lines scrolled back from the bottom (0 = latest).
    /// Returns rows in display order. Colors are palette-resolved.
    fn read_screen_cells(&self, scroll_offset: usize) -> Vec<ScreenRow>;

    /// Read an arbitrary range of physical rows as structured cells.
    /// `start_row` and `end_row` are 0-based physical row indices (end exclusive).
    /// Used for selection text extraction, hyperlink detection, etc.
    ///
    /// **Note:** Not all backends can access scrollback history. The ghostty
    /// backend returns only viewport rows that overlap the requested range,
    /// and returns empty for rows in scrollback history. Callers should
    /// handle receiving fewer rows than requested.
    fn read_cells_range(&self, start_row: usize, end_row: usize) -> Vec<ScreenRow>;

    // --- Scrolling ---

    /// Whether this backend manages its own viewport scrolling.
    /// When true, callers should use `scroll_viewport()` instead of tracking
    /// `scroll_offset` externally. The render state / screen cells will
    /// reflect the scrolled viewport automatically.
    fn manages_own_scroll(&self) -> bool {
        false
    }

    /// Scroll the viewport by `delta` lines (positive = scroll down / toward
    /// bottom, negative = scroll up / toward history). Only meaningful when
    /// `manages_own_scroll()` returns true.
    fn scroll_viewport(&mut self, _delta: isize) {}

    /// Snap the viewport to the bottom (most recent output). Only meaningful
    /// when `manages_own_scroll()` returns true.
    fn scroll_to_bottom(&mut self) {}

    // --- Terminal control ---

    /// Erase scrollback buffer, keeping visible screen.
    fn erase_scrollback(&mut self);

    /// Notify terminal of focus change (DECSET 1004).
    fn focus_changed(&mut self, focused: bool);

    // --- Notifications ---

    /// Drain pending notification events from the alert handler.
    fn drain_notifications(&self) -> Vec<NotificationEvent>;
}
