//! `AnyBackend` enum — runtime-switchable terminal backend.
//!
//! Uses an enum instead of `Box<dyn TerminalBackend>` to avoid lifetime
//! gymnastics (GhosttyPane has lifetime params), Send divergence, and to
//! allow pattern-matching for backend-specific features (e.g., Kitty images
//! on wezterm).

use std::io::Read;

use url::Url;

use crate::backend::{CursorPos, Palette, ProcessExit, ScreenRow, StableRow, TerminalBackend};
#[cfg(feature = "libghostty")]
use crate::ghostty_pane::GhosttyPane;
use crate::osc::NotificationEvent;
use crate::pane::{AdvanceResult, SequenceNo, TermError, TerminalPane};

/// Runtime-selectable terminal backend.
///
/// Both variants are boxed to keep enum size small and avoid
/// clippy::large_enum_variant.
pub enum AnyBackend {
    Wezterm(Box<TerminalPane>),
    #[cfg(feature = "libghostty")]
    Ghostty(Box<GhosttyPane<'static, 'static>>),
}

impl AnyBackend {
    /// Returns a reference to the inner `TerminalPane` if this is a wezterm backend.
    pub fn as_wezterm(&self) -> Option<&TerminalPane> {
        match self {
            Self::Wezterm(p) => Some(p),
            #[cfg(feature = "libghostty")]
            _ => None,
        }
    }

    /// Returns a mutable reference to the inner `TerminalPane` if this is a wezterm backend.
    pub fn as_wezterm_mut(&mut self) -> Option<&mut TerminalPane> {
        match self {
            Self::Wezterm(p) => Some(p),
            #[cfg(feature = "libghostty")]
            _ => None,
        }
    }
}

/// Delegate all `TerminalBackend` methods to the inner variant.
macro_rules! delegate {
    ($self:ident, $method:ident $(, $arg:ident : $ty:ty )* $(,)? ) => {
        match $self {
            AnyBackend::Wezterm(inner) => inner.$method( $( $arg ),* ),
            #[cfg(feature = "libghostty")]
            AnyBackend::Ghostty(inner) => inner.$method( $( $arg ),* ),
        }
    };
}

impl TerminalBackend for AnyBackend {
    fn advance(&mut self) -> AdvanceResult {
        delegate!(self, advance)
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), TermError> {
        delegate!(self, resize, cols: u16, rows: u16)
    }

    fn write_bytes(&mut self, data: &[u8]) -> Result<(), TermError> {
        delegate!(self, write_bytes, data: &[u8])
    }

    fn feed_bytes(&mut self, data: &[u8]) {
        delegate!(self, feed_bytes, data: &[u8])
    }

    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        delegate!(self, take_reader)
    }

    fn title(&self) -> &str {
        delegate!(self, title)
    }

    fn working_dir(&self) -> Option<&Url> {
        delegate!(self, working_dir)
    }

    fn dimensions(&self) -> (usize, usize) {
        delegate!(self, dimensions)
    }

    fn cursor(&self) -> CursorPos {
        delegate!(self, cursor)
    }

    fn palette(&self) -> Palette {
        delegate!(self, palette)
    }

    fn is_alt_screen_active(&self) -> bool {
        delegate!(self, is_alt_screen_active)
    }

    fn bracketed_paste_enabled(&self) -> bool {
        delegate!(self, bracketed_paste_enabled)
    }

    fn child_pid(&self) -> Option<u32> {
        delegate!(self, child_pid)
    }

    fn is_alive(&mut self) -> bool {
        delegate!(self, is_alive)
    }

    fn exit_status(&mut self) -> Option<ProcessExit> {
        delegate!(self, exit_status)
    }

    fn changed_lines(&self) -> Vec<StableRow> {
        delegate!(self, changed_lines)
    }

    fn mark_rendered(&mut self) {
        delegate!(self, mark_rendered)
    }

    fn current_seqno(&self) -> SequenceNo {
        delegate!(self, current_seqno)
    }

    fn rendered_seqno(&self) -> SequenceNo {
        delegate!(self, rendered_seqno)
    }

    fn scrollback_rows(&self) -> usize {
        delegate!(self, scrollback_rows)
    }

    fn read_screen_lines(&self, line_spec: &str, ansi: bool) -> String {
        delegate!(self, read_screen_lines, line_spec: &str, ansi: bool)
    }

    fn read_screen_text(&self) -> String {
        delegate!(self, read_screen_text)
    }

    fn read_scrollback_text(&self, max_lines: usize) -> String {
        delegate!(self, read_scrollback_text, max_lines: usize)
    }

    fn read_scrollback_text_range(&self, start: usize, end: usize) -> String {
        delegate!(self, read_scrollback_text_range, start: usize, end: usize)
    }

    fn search_scrollback(&self, query: &str) -> Vec<(usize, usize, usize)> {
        delegate!(self, search_scrollback, query: &str)
    }

    fn read_screen_cells(&self, scroll_offset: usize) -> Vec<ScreenRow> {
        delegate!(self, read_screen_cells, scroll_offset: usize)
    }

    fn read_cells_range(&self, start_row: usize, end_row: usize) -> Vec<ScreenRow> {
        delegate!(self, read_cells_range, start_row: usize, end_row: usize)
    }

    fn erase_scrollback(&mut self) {
        delegate!(self, erase_scrollback)
    }

    fn focus_changed(&mut self, focused: bool) {
        delegate!(self, focus_changed, focused: bool)
    }

    fn drain_notifications(&self) -> Vec<NotificationEvent> {
        delegate!(self, drain_notifications)
    }
}
