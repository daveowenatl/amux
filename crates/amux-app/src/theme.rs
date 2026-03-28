use egui::Color32;
use wezterm_term::color::SrgbaTuple;

/// Terminal color scheme — feeds into wezterm-term's ColorPalette.
/// Later: loadable from named schemes (Dracula, Solarized, etc.)
#[derive(Debug, Clone)]
pub(crate) struct TerminalColors {
    pub background: [u8; 3],
    pub foreground: [u8; 3],
    // Future: ansi, brights, cursor_bg, cursor_fg, selection_bg, selection_fg
}

/// UI chrome colors — tab bar, sidebar, dividers, accents.
/// Distinct from terminal colors (wezterm/ghostty pattern).
/// Fields that are `None` fall back to the terminal background (ghostty pattern).
#[derive(Debug, Clone)]
pub(crate) struct ChromeColors {
    pub sidebar_bg: Color32,
    /// Tab bar background. Falls back to terminal background when `None`.
    pub tab_bar_bg: Option<Color32>,
    /// Title bar / top padding background. Falls back to `tab_bar_bg` when `None`.
    pub titlebar_bg: Option<Color32>,
    pub tab_active_bg: Color32,
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

    /// Resolved tab bar background: chrome override → terminal background.
    pub fn tab_bar_bg(&self) -> Color32 {
        self.chrome.tab_bar_bg.unwrap_or_else(|| self.terminal_bg())
    }

    /// Resolved titlebar background: chrome override → tab bar background.
    pub fn titlebar_bg(&self) -> Color32 {
        self.chrome.titlebar_bg.unwrap_or_else(|| self.tab_bar_bg())
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            terminal: TerminalColors {
                background: [35, 35, 35],
                foreground: [0xe5, 0xe5, 0xe5],
            },
            chrome: ChromeColors {
                sidebar_bg: Color32::from_rgba_premultiplied(20, 20, 20, 230),
                tab_bar_bg: None,  // falls back to terminal background
                titlebar_bg: None, // falls back to tab bar background
                tab_active_bg: Color32::from_gray(50),
                tab_border: Color32::from_gray(55),
                divider: Color32::from_gray(60),
                accent: Color32::from_rgb(0, 145, 255),
            },
        }
    }
}
