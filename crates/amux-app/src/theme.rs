use std::collections::HashMap;

use amux_core::config;
use amux_term::backend::{Color, Palette};
use egui::Color32;

/// Terminal color scheme — feeds into the amux-native Palette.
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
/// Distinct from terminal colors.
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

fn rgb_to_color(c: [u8; 3]) -> Color {
    Color(
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        1.0,
    )
}

impl Theme {
    /// Terminal background as `Color32`.
    pub fn terminal_bg(&self) -> Color32 {
        let [r, g, b] = self.terminal.background;
        Color32::from_rgb(r, g, b)
    }

    /// Apply terminal colors to an amux-native Palette.
    pub fn apply_to_palette(&self, palette: &mut Palette) {
        palette.background = rgb_to_color(self.terminal.background);
        palette.foreground = rgb_to_color(self.terminal.foreground);
        for (i, [r, g, b]) in self.terminal.ansi.iter().enumerate() {
            if i < palette.colors.len() {
                palette.colors[i] =
                    Color(*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0, 1.0);
            }
        }
        // Apply extended palette overrides (16-255).
        for (&idx, &[r, g, b]) in &self.terminal.palette_overrides {
            if (idx as usize) < palette.colors.len() {
                palette.colors[idx as usize] =
                    Color(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0);
            }
        }
        palette.cursor_fg = rgb_to_color(self.terminal.cursor_fg);
        palette.cursor_bg = rgb_to_color(self.terminal.cursor_bg);
        palette.cursor_border = rgb_to_color(self.terminal.cursor_bg);
        palette.selection_fg = rgb_to_color(self.terminal.selection_fg);
        palette.selection_bg = rgb_to_color(self.terminal.selection_bg);
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
    /// Colors present in the Ghostty config override the defaults;
    /// missing colors fall back to the amux built-in default
    /// palette (see `Theme::default`).
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
        let accent = default.chrome.accent;
        let chrome = ChromeColors::from_background(terminal.background, accent);

        Self { terminal, chrome }
    }
}

impl ChromeColors {
    /// Derive chrome colors from a terminal background RGB value.
    /// Uses the same algorithm as `Theme::from_ghostty`.
    fn from_background(bg: [u8; 3], accent: Color32) -> Self {
        let [br, bg_g, bb] = bg;
        Self {
            sidebar_bg: darken_rgb(br, bg_g, bb, 0.15),
            sidebar_active_bg: accent,
            tab_bar_bg: None,
            titlebar_bg: None,
            tab_active_bg: Color32::from_rgb(br, bg_g, bb),
            tab_bar_border: lighten_rgb(br, bg_g, bb, 0.15),
            tab_border: lighten_rgb(br, bg_g, bb, 0.15),
            divider: lighten_rgb(br, bg_g, bb, 0.18),
            accent,
        }
    }
}

impl Theme {
    /// Apply user color overrides from config.
    pub fn apply_color_config(&mut self, colors: &config::ColorsConfig) {
        if let Some(ref hex) = colors.foreground {
            if let Some(rgb) = config::ColorsConfig::parse_hex(hex) {
                self.terminal.foreground = rgb;
            }
        }
        if let Some(ref hex) = colors.background {
            if let Some(rgb) = config::ColorsConfig::parse_hex(hex) {
                self.terminal.background = rgb;
                // Re-derive chrome colors from new background
                self.chrome = ChromeColors::from_background(rgb, self.chrome.accent);
            }
        }
        if let Some(ref hex) = colors.cursor_fg {
            if let Some(rgb) = config::ColorsConfig::parse_hex(hex) {
                self.terminal.cursor_fg = rgb;
            }
        }
        if let Some(ref hex) = colors.cursor_bg {
            if let Some(rgb) = config::ColorsConfig::parse_hex(hex) {
                self.terminal.cursor_bg = rgb;
            }
        }
        if let Some(ref hex) = colors.selection_fg {
            if let Some(rgb) = config::ColorsConfig::parse_hex(hex) {
                self.terminal.selection_fg = rgb;
            }
        }
        if let Some(ref hex) = colors.selection_bg {
            if let Some(rgb) = config::ColorsConfig::parse_hex(hex) {
                self.terminal.selection_bg = rgb;
            }
        }
        // Apply palette overrides (indices 0-15)
        for (i, hex) in colors.palette.iter().enumerate() {
            if i >= 16 {
                break;
            }
            if let Some(rgb) = config::ColorsConfig::parse_hex(hex) {
                self.terminal.ansi[i] = rgb;
            }
        }
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
    /// amux's built-in default theme: a neutral dark palette with a
    /// blue-gray background (`#14161E`), soft off-white foreground
    /// (`#D0D4E0`), and a blue accent for active/selected chrome.
    ///
    /// This replaces a previous "Tokyo Night" default. The amux
    /// default is intentionally not derived from any third-party
    /// theme so there's no branding confusion — users who want
    /// Tokyo Night, Nord, Solarized, etc. should set
    /// `theme_source = "ghostty"` in their `amux/config.toml` and
    /// configure the theme in Ghostty, OR override individual
    /// colors in the `[colors]` section of `amux/config.toml`.
    /// See `docs/configuration.md` for the full layering.
    ///
    /// ANSI palette leans toward balanced saturation so both the
    /// dim and bright variants of each color are readable against
    /// the dark background. Selection background is a muted blue
    /// (`#2A3550`) so selected terminal text stays legible.
    fn default() -> Self {
        Self {
            terminal: TerminalColors {
                // amux default
                background: [0x14, 0x16, 0x1e],
                foreground: [0xd0, 0xd4, 0xe0],
                ansi: [
                    [0x0f, 0x11, 0x17], // 0  black
                    [0xe0, 0x6c, 0x75], // 1  red
                    [0x98, 0xc3, 0x79], // 2  green
                    [0xe5, 0xc0, 0x7b], // 3  yellow
                    [0x61, 0xaf, 0xef], // 4  blue
                    [0xc6, 0x78, 0xdd], // 5  magenta
                    [0x56, 0xb6, 0xc2], // 6  cyan
                    [0xab, 0xb2, 0xbf], // 7  white
                    [0x3a, 0x40, 0x4e], // 8  bright black
                    [0xe8, 0x80, 0x88], // 9  bright red
                    [0xa8, 0xd1, 0x8e], // 10 bright green
                    [0xed, 0xcd, 0x8f], // 11 bright yellow
                    [0x77, 0xbd, 0xf2], // 12 bright blue
                    [0xd0, 0x8a, 0xe6], // 13 bright magenta
                    [0x74, 0xc6, 0xd1], // 14 bright cyan
                    [0xd0, 0xd4, 0xe0], // 15 bright white
                ],
                palette_overrides: HashMap::new(),
                cursor_fg: [0x14, 0x16, 0x1e],
                cursor_bg: [0xd0, 0xd4, 0xe0],
                selection_fg: [0xd0, 0xd4, 0xe0],
                selection_bg: [0x2a, 0x35, 0x50],
            },
            chrome: ChromeColors {
                sidebar_bg: Color32::from_gray(30),
                sidebar_active_bg: Color32::from_rgb(0x3d, 0x7d, 0xff),
                tab_bar_bg: None,  // falls back to terminal background
                titlebar_bg: None, // falls back to tab bar background
                tab_active_bg: Color32::from_rgb(0x14, 0x16, 0x1e), // match terminal bg
                tab_bar_border: Color32::from_gray(50),
                tab_border: Color32::from_gray(50),
                divider: Color32::from_gray(55),
                accent: Color32::from_rgb(0x3d, 0x7d, 0xff),
            },
        }
    }
}
