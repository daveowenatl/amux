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
    /// Sidebar row background when the row is hovered but not the active
    /// workspace. Subtle white overlay applied on top of `sidebar_bg`.
    pub sidebar_hover_bg: Color32,
    /// Active/selected row background in the sidebar (derived from accent).
    pub sidebar_active_bg: Color32,
    /// Sidebar row background when the row is active AND hovered. Derived
    /// by lightening `sidebar_active_bg` so hover stays visible over the
    /// selected row instead of collapsing into the plain active state.
    pub sidebar_active_hover_bg: Color32,
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
    /// Notification ring shown on panes with unread notifications.
    pub notification_ring: Color32,
    /// Unfocused pane dim overlay alpha (0 = no dimming, 255 = fully opaque).
    pub pane_dim_alpha: u8,
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
            sidebar_hover_bg: Color32::from_rgba_premultiplied(255, 255, 255, 20),
            sidebar_active_bg: accent,
            sidebar_active_hover_bg: lighten_color(accent, 0.12),
            tab_bar_bg: None,
            titlebar_bg: None,
            tab_active_bg: Color32::from_rgb(br, bg_g, bb),
            tab_bar_border: lighten_rgb(br, bg_g, bb, 0.15),
            tab_border: lighten_rgb(br, bg_g, bb, 0.15),
            divider: lighten_rgb(br, bg_g, bb, 0.18),
            accent,
            notification_ring: Color32::from_rgb(40, 120, 255),
            pane_dim_alpha: 100,
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

        // Chrome color overrides
        if let Some(ref hex) = colors.accent {
            if let Some([r, g, b]) = config::ColorsConfig::parse_hex(hex) {
                let c = Color32::from_rgb(r, g, b);
                self.chrome.accent = c;
                self.chrome.sidebar_active_bg = c;
                self.chrome.sidebar_active_hover_bg = lighten_color(c, 0.12);
            }
        }
        if let Some(ref hex) = colors.sidebar_bg {
            if let Some([r, g, b]) = config::ColorsConfig::parse_hex(hex) {
                self.chrome.sidebar_bg = Color32::from_rgb(r, g, b);
            }
        }
        if let Some(ref hex) = colors.notification_ring {
            if let Some([r, g, b]) = config::ColorsConfig::parse_hex(hex) {
                self.chrome.notification_ring = Color32::from_rgb(r, g, b);
            }
        }
        if let Some(alpha) = colors.pane_dim_alpha {
            self.chrome.pane_dim_alpha = alpha;
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

/// Lighten an opaque `Color32` toward white by `amount` (0.0–1.0).
fn lighten_color(c: Color32, amount: f32) -> Color32 {
    lighten_rgb(c.r(), c.g(), c.b(), amount)
}

impl Default for Theme {
    /// amux's built-in default theme: Monokai Classic — the same
    /// palette that ships as cmux's default and that the author
    /// uses via Ghostty on macOS. Windows and Linux users get the
    /// same look out of the box without needing to install Ghostty
    /// or write a config file; users who want something different
    /// can set `theme_source = "ghostty"` to read their Ghostty
    /// config instead, or override individual colors in the
    /// `[colors]` section of `amux/config.toml`.
    /// See `docs/configuration.md` for the full layering.
    ///
    /// Terminal colors mirror the Ghostty config cmux ships as its
    /// default: dark gray background (`#252830`), warm off-white
    /// foreground (`#fdfff1`), Monokai ANSI palette (pink/green/
    /// yellow/orange/purple/cyan). The chrome accent stays amux
    /// blue (`#3d7dff`) so the active workspace/tab highlight is
    /// visually distinct from the Monokai orange.
    fn default() -> Self {
        Self {
            terminal: TerminalColors {
                // Monokai Classic (cmux default)
                background: [0x25, 0x28, 0x30],
                foreground: [0xfd, 0xff, 0xf1],
                ansi: [
                    [0x27, 0x28, 0x22], // 0  black
                    [0xf9, 0x26, 0x72], // 1  red    (Monokai pink)
                    [0xa6, 0xe2, 0x2e], // 2  green
                    [0xe6, 0xdb, 0x74], // 3  yellow
                    [0xfd, 0x97, 0x1f], // 4  blue   (Monokai orange — slot 4 by convention)
                    [0xae, 0x81, 0xff], // 5  magenta (Monokai purple)
                    [0x66, 0xd9, 0xef], // 6  cyan
                    [0xfd, 0xff, 0xf1], // 7  white
                    [0x6e, 0x70, 0x66], // 8  bright black
                    [0xf9, 0x26, 0x72], // 9  bright red
                    [0xa6, 0xe2, 0x2e], // 10 bright green
                    [0xe6, 0xdb, 0x74], // 11 bright yellow
                    [0xfd, 0x97, 0x1f], // 12 bright blue
                    [0xae, 0x81, 0xff], // 13 bright magenta
                    [0x66, 0xd9, 0xef], // 14 bright cyan
                    [0xfd, 0xff, 0xf1], // 15 bright white
                ],
                palette_overrides: HashMap::new(),
                cursor_fg: [0x8d, 0x8e, 0x82],
                cursor_bg: [0xc0, 0xc1, 0xb5],
                selection_fg: [0xfd, 0xff, 0xf1],
                selection_bg: [0x57, 0x58, 0x4f],
            },
            chrome: ChromeColors {
                // Sidebar: slightly darker than the terminal bg so
                // the sidebar reads as a distinct panel rather than
                // blending into the terminal.
                sidebar_bg: Color32::from_rgb(0x1d, 0x1f, 0x25),
                sidebar_hover_bg: Color32::from_rgba_premultiplied(255, 255, 255, 20),
                // Accent: amux blue — kept distinct from the Monokai
                // terminal palette so the active workspace/tab
                // highlight doesn't blend into the orange ANSI cells.
                sidebar_active_bg: Color32::from_rgb(0x3d, 0x7d, 0xff),
                sidebar_active_hover_bg: Color32::from_rgb(0x5a, 0x93, 0xff),
                tab_bar_bg: None,  // falls back to terminal background
                titlebar_bg: None, // falls back to tab bar background
                tab_active_bg: Color32::from_rgb(0x25, 0x28, 0x30), // match terminal bg
                tab_bar_border: Color32::from_rgb(0x3a, 0x3c, 0x43),
                tab_border: Color32::from_rgb(0x3a, 0x3c, 0x43),
                divider: Color32::from_rgb(0x3a, 0x3c, 0x43),
                accent: Color32::from_rgb(0x3d, 0x7d, 0xff),
                notification_ring: Color32::from_rgb(40, 120, 255),
                pane_dim_alpha: 100,
            },
        }
    }
}
