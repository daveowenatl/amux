use std::sync::atomic::{AtomicUsize, Ordering};

use wezterm_term::color::{ColorPalette, RgbColor, SrgbaTuple};
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

/// Build a color palette using standard xterm ANSI colors (0-15).
/// wezterm-term's default palette uses softer, more pastel colors that make
/// reds look pinkish. This overrides with the widely-expected xterm defaults.
fn default_palette() -> ColorPalette {
    let mut palette = ColorPalette::default();

    // Standard xterm ANSI colors (same as iTerm2, Terminal.app, Ghostty defaults)
    let xterm_ansi: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00), // 0  Black
        (0xcd, 0x00, 0x00), // 1  Red
        (0x00, 0xcd, 0x00), // 2  Green
        (0xcd, 0xcd, 0x00), // 3  Yellow
        (0x00, 0x00, 0xee), // 4  Blue
        (0xcd, 0x00, 0xcd), // 5  Magenta
        (0x00, 0xcd, 0xcd), // 6  Cyan
        (0xe5, 0xe5, 0xe5), // 7  White
        (0x7f, 0x7f, 0x7f), // 8  Bright Black (Grey)
        (0xff, 0x00, 0x00), // 9  Bright Red
        (0x00, 0xff, 0x00), // 10 Bright Green
        (0xff, 0xff, 0x00), // 11 Bright Yellow
        (0x5c, 0x5c, 0xff), // 12 Bright Blue
        (0xff, 0x00, 0xff), // 13 Bright Magenta
        (0x00, 0xff, 0xff), // 14 Bright Cyan
        (0xff, 0xff, 0xff), // 15 Bright White
    ];

    for (i, (r, g, b)) in xterm_ansi.iter().enumerate() {
        palette.colors.0[i] = RgbColor::new_8bpc(*r, *g, *b).into();
    }

    // Use a lighter default foreground (standard xterm white-ish grey)
    palette.foreground = SrgbaTuple(
        0xe5 as f32 / 255.0,
        0xe5 as f32 / 255.0,
        0xe5 as f32 / 255.0,
        1.0,
    );

    palette
}

impl Default for AmuxTermConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            color_palette: default_palette(),
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
