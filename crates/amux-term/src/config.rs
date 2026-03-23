use std::sync::atomic::{AtomicUsize, Ordering};

use wezterm_term::color::ColorPalette;
use wezterm_term::config::{BidiMode, NewlineCanon};
use wezterm_term::TerminalConfiguration;

static GENERATION: AtomicUsize = AtomicUsize::new(0);

/// Terminal configuration for amux panes.
///
/// Implements `wezterm_term::TerminalConfiguration` to feed terminal defaults
/// (palette, scrollback depth, unicode version, etc.) into the wezterm-term engine.
#[derive(Debug)]
pub struct AmuxTermConfig {
    pub scrollback_lines: usize,
    pub color_palette: ColorPalette,
    pub enable_kitty_keyboard: bool,
}

impl Default for AmuxTermConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            color_palette: ColorPalette::default(),
            enable_kitty_keyboard: true,
        }
    }
}

impl AmuxTermConfig {
    /// Bump the config generation so that the terminal re-reads values.
    pub fn notify_changed(&self) {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

impl TerminalConfiguration for AmuxTermConfig {
    fn generation(&self) -> usize {
        GENERATION.load(Ordering::Relaxed)
    }

    fn scrollback_size(&self) -> usize {
        self.scrollback_lines
    }

    fn color_palette(&self) -> ColorPalette {
        self.color_palette.clone()
    }

    fn enable_kitty_keyboard(&self) -> bool {
        self.enable_kitty_keyboard
    }

    fn enable_kitty_graphics(&self) -> bool {
        true
    }

    fn canonicalize_pasted_newlines(&self) -> NewlineCanon {
        NewlineCanon::None
    }

    fn bidi_mode(&self) -> BidiMode {
        BidiMode {
            enabled: false,
            hint: Default::default(),
        }
    }
}
