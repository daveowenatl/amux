//! Shared egui popup theming helpers.
//!
//! Egui does not source popup styling from a single place for every
//! API. `ui.menu_button` / `popup_below_widget` construct their
//! outer `Frame` from the PARENT ui's style at call time
//! (`Frame::popup(parent_ui.style())` in `egui::containers::popup`,
//! `Frame::menu(ui.style())` in `egui::menu`). By contrast,
//! `Response::context_menu` constructs its Frame from
//! `response.ctx.style()` — the parent ui's style has no effect.
//! If the relevant style source hasn't been themed, the popup falls
//! back to egui's default visuals, which is a light-mode look that
//! clashes badly with amux's dark chrome.
//!
//! There are therefore two theming entry points in this module:
//!
//! - [`apply_menu_palette`] mutates a `Ui`'s visuals. Use it on a
//!   PARENT ui BEFORE calling `ui.menu_button` or
//!   `popup_below_widget` — by the time the popup's closure runs,
//!   egui has already constructed the outer Frame from the parent
//!   style. Note: this mutates the ui's local style, which
//!   persists for subsequent widgets in that same ui. Call on a
//!   dedicated child `ui.scope(...)` if you want containment.
//!
//! - [`with_menu_palette`] wraps a closure with a save / apply /
//!   restore of the `egui::Context` style. This is how
//!   `Response::context_menu` is themed: the ctx style is temporarily
//!   overridden for the duration of the `.context_menu(...)` call
//!   (which builds the Frame synchronously), then restored so
//!   unrelated widgets painted later in the frame don't inherit the
//!   menu-specific visuals.
//!
//! For the inside of a popup (button text colors, hover highlights,
//! separator lines), the shared [`apply_menu_palette_to_visuals`]
//! helper also sets the widget stroke / fill variants that egui
//! Button uses for its label paint path, since `override_text_color`
//! alone isn't picked up by every egui widget.
//!
//! This module is intentionally cross-platform. The menu bar on
//! macOS uses `muda` native, but egui popups live on every platform
//! — sidebar context menus, tab bar menus, future tooltips, etc.

use egui::{Color32, Stroke, Ui};

use crate::theme::Theme;

/// Rec. 601 perceived luminance of a `Color32`. Shared between
/// `contrast_text` (dark vs. light foreground) and the hover-bg
/// branch in `MenuPalette::from_theme` so both use the same
/// luminance model and can't disagree on colors near the boundary.
fn perceived_luma(c: Color32) -> f32 {
    0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32
}

/// Pick a readable foreground color for a given background by
/// checking its perceived luminance. Uses Rec. 601 luma — the
/// approximation most UI toolkits use.
///
/// Returns soft-white for dark backgrounds and near-black for light
/// backgrounds. Deliberately NOT pure white / pure black — both
/// extremes are harsh against typical chrome colors.
pub(crate) fn contrast_text(bg: Color32) -> Color32 {
    if perceived_luma(bg) < 128.0 {
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
        let hover_bg = if perceived_luma(bg) < 128.0 {
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

/// Apply a `MenuPalette` to an `egui::Visuals` struct. Used by
/// [`apply_menu_palette`] (ui-scoped) and [`apply_menu_palette_to_ctx`]
/// (ctx-scoped) so both paths share the exact same set of visual
/// overrides and stay in sync.
pub(crate) fn apply_menu_palette_to_visuals(visuals: &mut egui::Visuals, palette: MenuPalette) {
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

/// Apply a `MenuPalette` globally to an `egui::Context`'s style.
///
/// Do NOT call this unconditionally at the top of a frame. Because
/// it also sets widget fills to transparent and widget strokes to
/// none, it will restyle non-menu widgets (modals, text fields,
/// buttons) that read from `ctx.style()`. Prefer [`with_menu_palette`],
/// which scopes the mutation to a single call site with a
/// save / apply / restore pair.
///
/// This function is exposed only for the `with_menu_palette`
/// implementation and for tests that need to inspect the post-apply
/// style directly.
fn apply_menu_palette_to_ctx(ctx: &egui::Context, palette: MenuPalette) {
    ctx.style_mut(|style| {
        apply_menu_palette_to_visuals(&mut style.visuals, palette);
    });
}

/// Run `f` with amux's menu palette temporarily applied to the
/// `egui::Context`'s style, then restore the previous style.
///
/// This is the correct way to theme `Response::context_menu` and
/// any other egui primitive that reads from `ctx.style()` to build
/// its outer `Frame`. `context_menu` constructs its menu Frame
/// synchronously during the `.context_menu(|ui| ...)` call (inside
/// egui's `menu_ui`), so by the time `f` returns, the Frame has
/// already been built, paint commands have been recorded against
/// the themed style, and it is safe to restore the ctx style for
/// the rest of the frame.
///
/// We save via `ctx.style()` (cheap `Arc<Style>` clone) and restore
/// via `ctx.set_style(...)`. The inner `apply_menu_palette_to_ctx`
/// uses `ctx.style_mut`, which will `Arc::make_mut` a fresh style
/// — the saved `Arc` keeps the original untouched until we restore.
///
/// Use this for:
///
/// - `Response::context_menu` (sidebar workspace right-click, tab
///   bar right-click, etc.)
/// - Any future egui popup / tooltip / window primitive that reads
///   from `ctx.style()` internally
///
/// For `ui.menu_button` and `popup_below_widget`, whose Frame is
/// built from the parent `Ui`'s style, use [`apply_menu_palette`]
/// on the parent ui before the call.
pub(crate) fn with_menu_palette<R>(
    ctx: &egui::Context,
    palette: MenuPalette,
    f: impl FnOnce() -> R,
) -> R {
    let saved = ctx.style();
    apply_menu_palette_to_ctx(ctx, palette);
    let result = f();
    ctx.set_style(saved);
    result
}

/// Apply only the modal-relevant subset of a `MenuPalette` to
/// `visuals`. Used by [`with_modal_palette`] so every `egui::Window`
/// call site shares the same set of overrides.
///
/// Unlike [`apply_menu_palette_to_visuals`], this does NOT touch
/// widget `bg_fill` / `bg_stroke`. A modal needs visible button
/// backgrounds and text-field outlines — the menu-style "transparent
/// button that becomes a hover highlight" look is wrong for a
/// modal's OK / Cancel controls.
fn apply_modal_palette_to_visuals(visuals: &mut egui::Visuals, palette: MenuPalette) {
    // Outer Window Frame.
    visuals.window_fill = palette.bg;
    visuals.panel_fill = palette.bg;
    visuals.window_stroke = Stroke::new(1.0, palette.divider);

    // Text color — labels, RichText, and the button-label path.
    // We set `override_text_color` (used by `ui.label`) AND the
    // per-widget `fg_stroke.color` (used by egui Button's label
    // paint path) so both code paths render readable text. Widget
    // `bg_fill` / `bg_stroke` are intentionally left untouched so
    // egui's default chrome for buttons and text fields still
    // renders normally.
    visuals.override_text_color = Some(palette.fg);
    visuals.widgets.noninteractive.fg_stroke.color = palette.fg;
    visuals.widgets.inactive.fg_stroke.color = palette.fg;
    visuals.widgets.hovered.fg_stroke.color = palette.fg;
    visuals.widgets.active.fg_stroke.color = palette.fg;
    visuals.widgets.open.fg_stroke.color = palette.fg;
}

/// Run `f` with amux's modal palette temporarily applied to the
/// `egui::Context`'s style, then restore the previous style.
///
/// This is the correct way to theme `egui::Window`-based modals
/// (rename modal, find bar, notification panel). `Window::show`
/// reads `ctx.style().visuals.window_fill` / `window_stroke` /
/// `panel_fill` synchronously during the call to build its outer
/// Frame, so save / apply / restore wraps the entire modal cleanly.
///
/// Unlike [`with_menu_palette`], this variant leaves widget
/// `bg_fill` / `bg_stroke` untouched — modals want normal-looking
/// buttons and text-field borders, not the flat hover-only look
/// that's correct for menus.
pub(crate) fn with_modal_palette<R>(
    ctx: &egui::Context,
    palette: MenuPalette,
    f: impl FnOnce() -> R,
) -> R {
    let saved = ctx.style();
    ctx.style_mut(|style| {
        apply_modal_palette_to_visuals(&mut style.visuals, palette);
    });
    let result = f();
    ctx.set_style(saved);
    result
}

/// Apply a `MenuPalette` to a UI's visuals so that popups built
/// from this UI's style (and widgets rendered inside them) pick up
/// amux's chrome colors instead of egui's light-theme defaults.
///
/// # When to call
///
/// Call on the **parent** UI BEFORE opening a popup / menu_button /
/// `egui::popup::popup_below_widget`. Egui reads `parent_ui.style()`
/// at the moment the outer Frame is constructed, which happens
/// BEFORE the popup's closure runs — so applying the palette inside
/// the closure is too late to affect the Frame's background or
/// stroke.
///
/// **This does NOT theme `Response::context_menu`** — egui's context
/// menu builds its Frame from `ctx.style()`, not parent-ui style.
/// Use [`apply_menu_palette_to_ctx`] for context menus (typically
/// called once per frame from `AmuxApp::update`).
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
    apply_menu_palette_to_visuals(&mut ui.style_mut().visuals, palette);
}
