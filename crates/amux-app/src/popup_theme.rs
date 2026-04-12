//! Shared egui popup theming helpers.
//!
//! Egui's popup / menu / context_menu primitives build their outer
//! `Frame` from the PARENT ui's style at construction time
//! (`Frame::popup(parent_ui.style())` in `egui::containers::popup`,
//! `Frame::menu(ui.style())` in `egui::menu`). If the parent ui
//! hasn't been themed, popups fall back to egui's default visuals —
//! which is a light-mode look that clashes badly with amux's dark
//! chrome.
//!
//! [`apply_menu_palette`] takes a [`MenuPalette`] (derived from the
//! current theme) and mutates the ui's visuals so that any popup
//! built afterwards picks up our colors. Call it on the PARENT ui
//! BEFORE opening the popup, not inside the closure — by the time
//! the closure runs, egui has already constructed the outer Frame.
//!
//! For the inside of a popup (button text colors, hover highlights,
//! separator lines), [`apply_menu_palette`] also sets the widget
//! stroke / fill variants that egui Button uses for its label paint
//! path, since `override_text_color` alone isn't picked up by every
//! egui widget.
//!
//! This module is intentionally cross-platform. The menu bar on
//! macOS uses `muda` native, but egui popups live on every platform
//! — sidebar context menus, tab bar menus, future tooltips, etc.

use egui::{Color32, Stroke, Ui};

use crate::theme::Theme;

/// Pick a readable foreground color for a given background by
/// checking its perceived luminance. Uses Rec. 601 luma — the
/// approximation most UI toolkits use.
///
/// Returns soft-white for dark backgrounds and near-black for light
/// backgrounds. Deliberately NOT pure white / pure black — both
/// extremes are harsh against typical chrome colors.
pub(crate) fn contrast_text(bg: Color32) -> Color32 {
    let r = bg.r() as f32;
    let g = bg.g() as f32;
    let b = bg.b() as f32;
    let luma = 0.299 * r + 0.587 * g + 0.114 * b;
    if luma < 128.0 {
        Color32::from_rgb(0xE6, 0xE6, 0xE6)
    } else {
        Color32::from_rgb(0x20, 0x20, 0x20)
    }
}

/// Bundle of theme-derived colors used by every popup rendering
/// path. Computed once per top-level render call so per-item loops
/// don't redo the luma math.
#[derive(Clone, Copy)]
pub(crate) struct MenuPalette {
    /// Popup background + button-base color.
    pub bg: Color32,
    /// Foreground text color (buttons, labels, separators).
    pub fg: Color32,
    /// Hover / active / open background fill for widgets inside the
    /// popup. A gamma-shifted variant of `bg` that's slightly
    /// lighter on dark themes and slightly darker on light themes.
    pub hover_bg: Color32,
    /// Subtle divider color for popup borders and separator lines.
    /// Alpha-blended so it doesn't overpower.
    pub divider: Color32,
}

impl MenuPalette {
    /// Derive a palette from the user's configured theme. Uses the
    /// theme's titlebar background as the popup background so
    /// popups feel visually attached to amux's top chrome.
    pub fn from_theme(theme: &Theme) -> Self {
        let bg = theme.titlebar_bg();
        let fg = contrast_text(bg);
        let luma_sum: u16 = bg.r() as u16 + bg.g() as u16 + (bg.b() as u16);
        let hover_bg = if luma_sum < 384 {
            bg.gamma_multiply(1.5) // dark theme → brighten on hover
        } else {
            bg.gamma_multiply(0.85) // light theme → darken on hover
        };
        // Divider at low alpha (24/255). A 1px stroke on popup frames
        // is loud even at modest alphas — tune down if future
        // complaints surface.
        let divider = Color32::from_rgba_unmultiplied(fg.r(), fg.g(), fg.b(), 24);
        Self {
            bg,
            fg,
            hover_bg,
            divider,
        }
    }
}

/// Apply a `MenuPalette` to a UI's visuals so that popups built
/// from this UI's style (and widgets rendered inside them) pick up
/// amux's chrome colors instead of egui's light-theme defaults.
///
/// # When to call
///
/// Call on the **parent** UI BEFORE opening a popup / menu_button /
/// context_menu. Egui reads `parent_ui.style()` at the moment the
/// outer Frame is constructed, which happens BEFORE the popup's
/// closure runs — so applying the palette inside the closure is
/// too late to affect the Frame's background or stroke.
///
/// Also call a second time INSIDE the popup closure if the popup
/// opens nested popups (nested `ui.menu_button`), so children
/// inherit the palette too. Applying multiple times is idempotent
/// and safe.
///
/// # What it sets
///
/// - `visuals.window_fill` — popup background
/// - `visuals.window_stroke` — popup border (thin, divider color)
/// - `visuals.panel_fill` — panels inside the popup
/// - `visuals.override_text_color` — default text color for all widgets
/// - `visuals.widgets.{state}.fg_stroke.color` — button label text
///   (NOT the same as `override_text_color`; egui Button reads both)
/// - `visuals.widgets.{state}.weak_bg_fill` / `bg_fill` — button
///   hover / active / open backgrounds
/// - `visuals.widgets.{state}.bg_stroke` — button borders set to
///   NONE so buttons look like plain text links rather than boxed
///   controls
/// - `visuals.widgets.noninteractive.bg_stroke` — `ui.separator()`
///   line color
pub(crate) fn apply_menu_palette(ui: &mut Ui, palette: MenuPalette) {
    let visuals = &mut ui.style_mut().visuals;

    // Popup container styling (used by egui's popup Frame).
    visuals.window_fill = palette.bg;
    visuals.panel_fill = palette.bg;
    visuals.window_stroke = Stroke::new(1.0, palette.divider);

    // Text color — set both `override_text_color` (used by labels,
    // RichText, etc.) and `widgets.{state}.fg_stroke.color` (used
    // by Button's label paint path).
    visuals.override_text_color = Some(palette.fg);
    visuals.widgets.inactive.fg_stroke.color = palette.fg;
    visuals.widgets.hovered.fg_stroke.color = palette.fg;
    visuals.widgets.active.fg_stroke.color = palette.fg;
    visuals.widgets.open.fg_stroke.color = palette.fg;

    // Widget backgrounds.
    visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.hovered.weak_bg_fill = palette.hover_bg;
    visuals.widgets.hovered.bg_fill = palette.hover_bg;
    visuals.widgets.active.weak_bg_fill = palette.hover_bg;
    visuals.widgets.active.bg_fill = palette.hover_bg;
    visuals.widgets.open.weak_bg_fill = palette.hover_bg;
    visuals.widgets.open.bg_fill = palette.hover_bg;

    // Widget borders: buttons should look like plain text links,
    // not boxed controls.
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    visuals.widgets.active.bg_stroke = Stroke::NONE;
    visuals.widgets.open.bg_stroke = Stroke::NONE;

    // Separator line color — `ui.separator()` draws using
    // `noninteractive.bg_stroke`.
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.divider);
}
