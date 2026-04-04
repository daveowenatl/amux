use std::collections::HashMap;

use egui::Color32;
use wezterm_term::color::SrgbaTuple;

/// Terminal color scheme — feeds into wezterm-term's ColorPalette.
/// Later: loadable from named schemes (Dracula, Solarized, etc.)
#[derive(Debug, Clone)]
pub(crate) struct TerminalColors {
    pub background: [u8; 3],
    pub foreground: [u8; 3],
    /// 16 ANSI colors: 0-7 normal, 8-15 bright.
    pub ansi: [[u8; 3]; 16],
    /// Extended palette overrides (indices 16-255) from Ghostty config.
    pub palette_overrides: HashMap<u8, [u8; 3]>,
    pub cursor_fg: [u8; 3],
    pub cursor_bg: [u8; 3],
    pub selection_fg: [u8; 3],
    pub selection_bg: [u8; 3],
}

/// UI chrome colors — tab bar, sidebar, dividers, accents.
/// Distinct from terminal colors (wezterm/ghostty pattern).
/// Fields that are `None` fall back to the terminal background (ghostty pattern).
#[derive(Debug, Clone)]
pub(crate) struct ChromeColors {
    pub sidebar_bg: Color32,
    /// Active/selected row background in the sidebar (derived from accent).
    pub sidebar_active_bg: Color32,
    /// Tab bar background. Falls back to terminal background when `None`.
    pub tab_bar_bg: Option<Color32>,
    /// Title bar / top padding background. Falls back to `tab_bar_bg` when `None`.
    pub titlebar_bg: Option<Color32>,
    pub tab_active_bg: Color32,
    /// 1px border around the tab bar (top and bottom edges).
    pub tab_bar_border: Color32,
    pub tab_border: Color32,
    pub divider: Color32,
    pub accent: Color32,
}

/// Combined theme: terminal colors + UI chrome.
#[derive(Debug, Clone)]
pub(crate) struct Theme {
    pub terminal: TerminalColors,
    pub chrome: ChromeColors,
}

impl Theme {
    /// Terminal background as `Color32`.
    pub fn terminal_bg(&self) -> Color32 {
        let [r, g, b] = self.terminal.background;
        Color32::from_rgb(r, g, b)
    }

    /// Terminal background as `SrgbaTuple` for wezterm palette.
    pub fn terminal_bg_srgba(&self) -> SrgbaTuple {
        let [r, g, b] = self.terminal.background;
        SrgbaTuple(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
    }

    /// Terminal foreground as `SrgbaTuple` for wezterm palette.
    pub fn terminal_fg_srgba(&self) -> SrgbaTuple {
        let [r, g, b] = self.terminal.foreground;
        SrgbaTuple(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
    }

    /// Apply terminal colors to a wezterm ColorPalette.
    pub fn apply_to_palette(&self, palette: &mut wezterm_term::color::ColorPalette) {
        palette.background = self.terminal_bg_srgba();
        palette.foreground = self.terminal_fg_srgba();
        for (i, [r, g, b]) in self.terminal.ansi.iter().enumerate() {
            palette.colors.0[i] =
                SrgbaTuple(*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0, 1.0);
        }
        // Apply extended palette overrides (16-255).
        for (&idx, &[r, g, b]) in &self.terminal.palette_overrides {
            if (idx as usize) < palette.colors.0.len() {
                palette.colors.0[idx as usize] =
                    SrgbaTuple(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0);
            }
        }
        let to_srgba = |c: [u8; 3]| {
            SrgbaTuple(
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
                1.0,
            )
        };
        palette.cursor_fg = to_srgba(self.terminal.cursor_fg);
        palette.cursor_bg = to_srgba(self.terminal.cursor_bg);
        palette.cursor_border = to_srgba(self.terminal.cursor_bg);
        palette.selection_fg = to_srgba(self.terminal.selection_fg);
        palette.selection_bg = to_srgba(self.terminal.selection_bg);
    }

    /// Resolved tab bar background: chrome override → terminal background.
    pub fn tab_bar_bg(&self) -> Color32 {
        self.chrome.tab_bar_bg.unwrap_or_else(|| self.terminal_bg())
    }

    /// Resolved titlebar background: chrome override → tab bar background.
    pub fn titlebar_bg(&self) -> Color32 {
        self.chrome.titlebar_bg.unwrap_or_else(|| self.tab_bar_bg())
    }
}

impl Theme {
    /// Create a theme from a loaded Ghostty config.
    ///
    /// Colors present in the Ghostty config override the defaults; missing
    /// colors fall back to the built-in Tokyo Night theme.
    pub(crate) fn from_ghostty(cfg: &amux_ghostty_config::GhosttyConfig) -> Self {
        let default = Self::default();
        let mut terminal = default.terminal.clone();

        if let Some(c) = cfg.background() {
            terminal.background = c;
        }
        if let Some(c) = cfg.foreground() {
            terminal.foreground = c;
        }
        if let Some(c) = cfg.cursor_color() {
            terminal.cursor_bg = c;
        }
        if let Some(c) = cfg.cursor_text() {
            terminal.cursor_fg = c;
        }
        if let Some(c) = cfg.selection_background() {
            terminal.selection_bg = c;
        }
        if let Some(c) = cfg.selection_foreground() {
            terminal.selection_fg = c;
        }

        // Apply ANSI palette overrides (0-15) and extended palette (16-255).
        for (&idx, &color) in cfg.palette_overrides() {
            if (idx as usize) < 16 {
                terminal.ansi[idx as usize] = color;
            } else {
                terminal.palette_overrides.insert(idx, color);
            }
        }

        // Derive chrome colors from terminal background for a cohesive look.
        let [br, bg, bb] = terminal.background;
        let accent = default.chrome.accent;
        let chrome = ChromeColors {
            sidebar_bg: darken_rgb(br, bg, bb, 0.15),
            sidebar_active_bg: accent,
            tab_bar_bg: None, // falls back to terminal background
            titlebar_bg: None,
            tab_active_bg: Color32::from_rgb(br, bg, bb), // match terminal background
            tab_bar_border: lighten_rgb(br, bg, bb, 0.15),
            tab_border: lighten_rgb(br, bg, bb, 0.15),
            divider: lighten_rgb(br, bg, bb, 0.18),
            accent,
        };

        Self { terminal, chrome }
    }
}

/// Darken an RGB color by a fraction (0.0 = no change, 1.0 = black).
fn darken_rgb(r: u8, g: u8, b: u8, amount: f32) -> Color32 {
    let f = 1.0 - amount;
    Color32::from_rgb(
        (r as f32 * f) as u8,
        (g as f32 * f) as u8,
        (b as f32 * f) as u8,
    )
}

/// Lighten an RGB color by a fraction (0.0 = no change, 1.0 = white).
fn lighten_rgb(r: u8, g: u8, b: u8, amount: f32) -> Color32 {
    Color32::from_rgb(
        (r as f32 + (255.0 - r as f32) * amount) as u8,
        (g as f32 + (255.0 - g as f32) * amount) as u8,
        (b as f32 + (255.0 - b as f32) * amount) as u8,
    )
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            terminal: TerminalColors {
                // Tokyo Night
                background: [0x1a, 0x1b, 0x26],
                foreground: [0xc0, 0xca, 0xf5],
                ansi: [
                    [0x15, 0x16, 0x1e], // 0  black
                    [0xf7, 0x76, 0x8e], // 1  red
                    [0x9e, 0xce, 0x6a], // 2  green
                    [0xe0, 0xaf, 0x68], // 3  yellow
                    [0x7a, 0xa2, 0xf7], // 4  blue
                    [0xbb, 0x9a, 0xf7], // 5  magenta
                    [0x7d, 0xcf, 0xff], // 6  cyan
                    [0xa9, 0xb1, 0xd6], // 7  white
                    [0x41, 0x48, 0x68], // 8  bright black
                    [0xf7, 0x76, 0x8e], // 9  bright red
                    [0x9e, 0xce, 0x6a], // 10 bright green
                    [0xe0, 0xaf, 0x68], // 11 bright yellow
                    [0x7a, 0xa2, 0xf7], // 12 bright blue
                    [0xbb, 0x9a, 0xf7], // 13 bright magenta
                    [0x7d, 0xcf, 0xff], // 14 bright cyan
                    [0xc0, 0xca, 0xf5], // 15 bright white
                ],
                palette_overrides: HashMap::new(),
                cursor_fg: [0x15, 0x16, 0x1e],
                cursor_bg: [0xc0, 0xca, 0xf5],
                selection_fg: [0xc0, 0xca, 0xf5],
                selection_bg: [0x33, 0x46, 0x7c],
            },
            chrome: ChromeColors {
                sidebar_bg: Color32::from_gray(35),
                sidebar_active_bg: Color32::from_rgb(0, 145, 255), // same as accent
                tab_bar_bg: None,  // falls back to terminal background
                titlebar_bg: None, // falls back to tab bar background
                tab_active_bg: Color32::from_rgb(0x1a, 0x1b, 0x26), // match terminal bg
                tab_bar_border: Color32::from_gray(55),
                tab_border: Color32::from_gray(55),
                divider: Color32::from_gray(60),
                accent: Color32::from_rgb(0, 145, 255),
            },
        }
    }
}
