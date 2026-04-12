//! Cross-platform menu bar.
//!
//! # Two rendering paths
//!
//! - **macOS**: native app-global menu bar via [`muda`], installed into
//!   NSApp's top-of-screen strip. That's the idiomatic macOS pattern —
//!   Mac apps don't draw menus inside their own windows.
//! - **Windows / Linux**: egui-drawn [`egui::TopBottomPanel::top`] +
//!   [`egui::menu::bar`], rendered from [`AmuxApp::update`] every frame.
//!   Native menu chrome is off the table on Windows 11 because the
//!   native menu rendering path ignores the undocumented dark-mode
//!   ordinals VS Code / Windows Terminal used to use — the egui-drawn
//!   approach gives us full theme control and cross-platform visual
//!   parity. On Linux it means amux has a working menu bar at all:
//!   `muda`'s GTK path isn't wired through `eframe`, so the native
//!   approach produces nothing.
//!
//! # Shared action model
//!
//! Both paths emit [`MenuAction`] values into the process-wide
//! [`PENDING_ACTIONS`] queue, which is drained per frame by
//! [`AmuxApp::handle_menu_actions`]. The dispatcher code in
//! `workspace_ops.rs` doesn't know or care which path produced the
//! action.
//!
//! # Keyboard shortcuts
//!
//! Menu items display shortcut text (e.g. `Ctrl+Shift+N`) for
//! discoverability, but the menu bar does **not** dispatch keyboard
//! events. All shortcuts go through [`AmuxApp::handle_shortcuts`] in
//! `input.rs`, which reads the configured keybindings and matches
//! against `egui::Event::Key` directly. The shortcut strings here are
//! purely cosmetic — changing them does not affect what keys fire what
//! action.

use std::collections::VecDeque;
use std::sync::Mutex;

#[cfg(target_os = "macos")]
use muda::{
    accelerator::{Accelerator, Code, Modifiers},
    AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};

// ---------------------------------------------------------------------
// Action vocabulary
// ---------------------------------------------------------------------

/// Actions that can be triggered from the menu bar. Stable across both
/// rendering paths so `workspace_ops::handle_menu_actions` can treat
/// them uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuAction {
    NewWorkspace,
    NewTab,
    NewBrowserTab,
    CloseTab,
    SaveSession,
    ToggleSidebar,
    ToggleNotificationPanel,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Copy,
    Paste,
    SelectAll,
}

// ---------------------------------------------------------------------
// Platform-agnostic menu model (non-macOS only)
// ---------------------------------------------------------------------
//
// macOS builds its native menu via the inlined code in `build()`
// below using muda's typed accelerators directly, so these data
// types would be dead code on that platform. Gated to non-macOS so
// the compiler doesn't flag them.

/// A single item inside a submenu — either a clickable action or a
/// visual separator.
#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone, Copy)]
pub(crate) enum MenuItemDef {
    Separator,
    Action {
        label: &'static str,
        /// Display-only shortcut hint (e.g. `"Ctrl+Shift+N"`). Purely
        /// cosmetic — actual dispatch is in `input::handle_shortcuts`.
        shortcut: Option<&'static str>,
        action: MenuAction,
    },
}

/// A top-level submenu (`File`, `Edit`, `View`, `Window`).
#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct SubmenuDef {
    pub label: &'static str,
    pub items: &'static [MenuItemDef],
}

// Platform-specific shortcut display strings. macOS uses the Cmd glyph
// (⌘) + letter; Windows/Linux use Ctrl+Shift+letter for workspace/tab
// ops to avoid stealing terminal control keys (Ctrl+N = new line,
// Ctrl+W = word erase, Ctrl+S = XOFF), and Ctrl+letter for view ops
// that don't conflict. These strings must match what
// `input::handle_shortcuts` actually listens for.

// Shortcut display strings — only used by the non-macOS egui menu
// renderer. macOS uses muda's typed `Accelerator` directly in
// `build()` below and doesn't need these string forms.
#[cfg(not(target_os = "macos"))]
mod shortcuts {
    pub const NEW_WORKSPACE: &str = "Ctrl+Shift+N";
    pub const NEW_TAB: &str = "Ctrl+Shift+T";
    pub const NEW_BROWSER_TAB: &str = "Ctrl+Shift+L";
    pub const CLOSE_TAB: &str = "Ctrl+Shift+W";
    pub const SAVE_SESSION: &str = "Ctrl+Shift+S";
    pub const TOGGLE_SIDEBAR: &str = "Ctrl+B";
    pub const TOGGLE_NOTIFICATIONS: &str = "Ctrl+Shift+I";
    pub const ZOOM_IN: &str = "Ctrl+=";
    pub const ZOOM_OUT: &str = "Ctrl+-";
    pub const ZOOM_RESET: &str = "Ctrl+0";
    pub const COPY: &str = "Ctrl+Shift+C";
    pub const PASTE: &str = "Ctrl+Shift+V";
    pub const SELECT_ALL: &str = "Ctrl+Shift+A";
}

/// The non-macOS menu structure consumed by [`draw_egui_menu_bar`].
/// macOS builds its native menu separately in [`build`] using muda's
/// typed accelerators, so this const isn't referenced there.
///
/// Window > Minimize / Maximize is not in this list — on Windows the
/// title bar already provides those controls, and on Linux there's no
/// portable winit API to wire from an egui click. If we need them on
/// non-macOS later, route through `eframe::Frame::set_minimized` or
/// similar in the dispatcher.
#[cfg(not(target_os = "macos"))]
pub(crate) const MENU_MODEL: &[SubmenuDef] = &[
    SubmenuDef {
        label: "File",
        items: &[
            MenuItemDef::Action {
                label: "New Workspace",
                shortcut: Some(shortcuts::NEW_WORKSPACE),
                action: MenuAction::NewWorkspace,
            },
            MenuItemDef::Action {
                label: "New Tab",
                shortcut: Some(shortcuts::NEW_TAB),
                action: MenuAction::NewTab,
            },
            MenuItemDef::Action {
                label: "New Browser Tab",
                shortcut: Some(shortcuts::NEW_BROWSER_TAB),
                action: MenuAction::NewBrowserTab,
            },
            MenuItemDef::Separator,
            MenuItemDef::Action {
                label: "Close Tab",
                shortcut: Some(shortcuts::CLOSE_TAB),
                action: MenuAction::CloseTab,
            },
            MenuItemDef::Separator,
            MenuItemDef::Action {
                label: "Save Session",
                shortcut: Some(shortcuts::SAVE_SESSION),
                action: MenuAction::SaveSession,
            },
        ],
    },
    SubmenuDef {
        label: "Edit",
        items: &[
            MenuItemDef::Action {
                label: "Copy",
                shortcut: Some(shortcuts::COPY),
                action: MenuAction::Copy,
            },
            MenuItemDef::Action {
                label: "Paste",
                shortcut: Some(shortcuts::PASTE),
                action: MenuAction::Paste,
            },
            MenuItemDef::Separator,
            MenuItemDef::Action {
                label: "Select All",
                shortcut: Some(shortcuts::SELECT_ALL),
                action: MenuAction::SelectAll,
            },
        ],
    },
    SubmenuDef {
        label: "View",
        items: &[
            MenuItemDef::Action {
                label: "Toggle Sidebar",
                shortcut: Some(shortcuts::TOGGLE_SIDEBAR),
                action: MenuAction::ToggleSidebar,
            },
            MenuItemDef::Action {
                label: "Toggle Notifications",
                shortcut: Some(shortcuts::TOGGLE_NOTIFICATIONS),
                action: MenuAction::ToggleNotificationPanel,
            },
            MenuItemDef::Separator,
            MenuItemDef::Action {
                label: "Zoom In",
                shortcut: Some(shortcuts::ZOOM_IN),
                action: MenuAction::ZoomIn,
            },
            MenuItemDef::Action {
                label: "Zoom Out",
                shortcut: Some(shortcuts::ZOOM_OUT),
                action: MenuAction::ZoomOut,
            },
            MenuItemDef::Action {
                label: "Actual Size",
                shortcut: Some(shortcuts::ZOOM_RESET),
                action: MenuAction::ZoomReset,
            },
        ],
    },
];

// ---------------------------------------------------------------------
// Shared action queue
// ---------------------------------------------------------------------

/// Process-wide queue of menu-driven actions. Both the macOS muda path
/// (via `drain_muda_events`) and the non-macOS egui path (via direct
/// `push_action` calls from click handlers) push here. The queue is
/// drained once per frame by `AmuxApp::handle_menu_actions`.
static PENDING_ACTIONS: Mutex<VecDeque<MenuAction>> = Mutex::new(VecDeque::new());

/// Enqueue a menu action. Called by both render paths.
pub(crate) fn push_action(action: MenuAction) {
    if let Ok(mut queue) = PENDING_ACTIONS.lock() {
        queue.push_back(action);
    }
}

/// Pop the next queued menu action, if any. On macOS, also drains
/// `muda::MenuEvent`s from muda's channel into the queue first.
pub(crate) fn take_pending_action() -> Option<MenuAction> {
    #[cfg(target_os = "macos")]
    drain_muda_events();

    PENDING_ACTIONS.lock().ok()?.pop_front()
}

// ---------------------------------------------------------------------
// macOS path (muda native menu bar)
// ---------------------------------------------------------------------

#[cfg(target_os = "macos")]
struct MacMenuItems {
    new_workspace: muda::MenuId,
    new_tab: muda::MenuId,
    new_browser_tab: muda::MenuId,
    close_tab: muda::MenuId,
    save_session: muda::MenuId,
    toggle_sidebar: muda::MenuId,
    toggle_notifications: muda::MenuId,
    zoom_in: muda::MenuId,
    zoom_out: muda::MenuId,
    zoom_reset: muda::MenuId,
    copy: muda::MenuId,
    paste: muda::MenuId,
    select_all: muda::MenuId,
}

#[cfg(target_os = "macos")]
static MAC_MENU_ITEMS: std::sync::OnceLock<MacMenuItems> = std::sync::OnceLock::new();

#[cfg(target_os = "macos")]
const CMD: Modifiers = Modifiers::SUPER;

#[cfg(target_os = "macos")]
fn accel(mods: Modifiers, code: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Some(mods), code))
}

/// Build and install the macOS native menu bar. Call once at startup.
/// Returns the [`muda::Menu`] — the caller must keep it alive for the
/// process lifetime (muda drops its event wiring when the Menu is
/// dropped).
///
/// `init_for_nsapp` is called internally, so the menu bar is visible
/// immediately when this returns.
#[cfg(target_os = "macos")]
pub(crate) fn build() -> Menu {
    // Build menu items with accelerators.
    let new_workspace = MenuItem::new("New Workspace", true, accel(CMD, Code::KeyN));
    let new_tab = MenuItem::new("New Tab", true, accel(CMD, Code::KeyT));
    let new_browser_tab = MenuItem::new(
        "New Browser Tab",
        true,
        accel(CMD.union(Modifiers::SHIFT), Code::KeyL),
    );
    let close_tab = MenuItem::new("Close Tab", true, accel(CMD, Code::KeyW));
    let save_session = MenuItem::new("Save Session", true, accel(CMD, Code::KeyS));
    let toggle_sidebar = MenuItem::new("Toggle Sidebar", true, accel(CMD, Code::KeyB));
    let toggle_notifications = MenuItem::new("Toggle Notifications", true, accel(CMD, Code::KeyI));
    let zoom_in = MenuItem::new("Zoom In", true, accel(CMD, Code::Equal));
    let zoom_out = MenuItem::new("Zoom Out", true, accel(CMD, Code::Minus));
    let zoom_reset = MenuItem::new("Actual Size", true, accel(CMD, Code::Digit0));
    let copy = MenuItem::new("Copy", true, accel(CMD, Code::KeyC));
    let paste = MenuItem::new("Paste", true, accel(CMD, Code::KeyV));
    let select_all = MenuItem::new("Select All", true, accel(CMD, Code::KeyA));

    // Stash IDs for event matching before the items get moved into
    // their submenus — muda's MenuId is cheap to clone but we need to
    // capture it before handing ownership off.
    if MAC_MENU_ITEMS
        .set(MacMenuItems {
            new_workspace: new_workspace.id().clone(),
            new_tab: new_tab.id().clone(),
            new_browser_tab: new_browser_tab.id().clone(),
            close_tab: close_tab.id().clone(),
            save_session: save_session.id().clone(),
            toggle_sidebar: toggle_sidebar.id().clone(),
            toggle_notifications: toggle_notifications.id().clone(),
            zoom_in: zoom_in.id().clone(),
            zoom_out: zoom_out.id().clone(),
            zoom_reset: zoom_reset.id().clone(),
            copy: copy.id().clone(),
            paste: paste.id().clone(),
            select_all: select_all.id().clone(),
        })
        .is_err()
    {
        tracing::warn!("menu_bar::build() called more than once; ignoring duplicate");
    }

    let menu = Menu::new();

    // App menu (macOS standard: About / Services / Hide / Quit).
    let app_menu = Submenu::new("amux", true);
    let _ = app_menu.append_items(&[
        &PredefinedMenuItem::about(
            None,
            Some(AboutMetadata {
                name: Some("amux".to_string()),
                ..Default::default()
            }),
        ),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::services(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(None),
        &PredefinedMenuItem::hide_others(None),
        &PredefinedMenuItem::show_all(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::quit(None),
    ]);
    let _ = menu.append(&app_menu);

    // File menu.
    let file_menu = Submenu::new("File", true);
    let _ = file_menu.append_items(&[
        &new_workspace,
        &new_tab,
        &new_browser_tab,
        &PredefinedMenuItem::separator(),
        &close_tab,
        &PredefinedMenuItem::separator(),
        &save_session,
    ]);
    let _ = menu.append(&file_menu);

    // Edit menu.
    let edit_menu = Submenu::new("Edit", true);
    let _ = edit_menu.append_items(&[&copy, &paste, &PredefinedMenuItem::separator(), &select_all]);
    let _ = menu.append(&edit_menu);

    // View menu.
    let view_menu = Submenu::new("View", true);
    let _ = view_menu.append_items(&[
        &toggle_sidebar,
        &toggle_notifications,
        &PredefinedMenuItem::separator(),
        &zoom_in,
        &zoom_out,
        &zoom_reset,
    ]);
    let _ = menu.append(&view_menu);

    // Window menu — minimize/maximize/window list. macOS has native
    // predefined items for these that hook into NSWindow.
    let window_menu = Submenu::new("Window", true);
    let _ = window_menu.append_items(&[
        &PredefinedMenuItem::minimize(None),
        &PredefinedMenuItem::maximize(None),
    ]);
    let _ = menu.append(&window_menu);
    window_menu.set_as_windows_menu_for_nsapp();

    // Help menu — empty container, but set_as_help_menu_for_nsapp
    // wires the system-provided search field into it.
    let help_menu = Submenu::new("Help", true);
    let _ = menu.append(&help_menu);
    help_menu.set_as_help_menu_for_nsapp();

    menu.init_for_nsapp();
    menu
}

/// Drain any pending muda events into the shared action queue. No-op
/// if the menu hasn't been built yet (early in startup).
#[cfg(target_os = "macos")]
fn drain_muda_events() {
    let Some(items) = MAC_MENU_ITEMS.get() else {
        return;
    };
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        let id = &event.id;
        let action = if *id == items.new_workspace {
            Some(MenuAction::NewWorkspace)
        } else if *id == items.new_tab {
            Some(MenuAction::NewTab)
        } else if *id == items.new_browser_tab {
            Some(MenuAction::NewBrowserTab)
        } else if *id == items.close_tab {
            Some(MenuAction::CloseTab)
        } else if *id == items.save_session {
            Some(MenuAction::SaveSession)
        } else if *id == items.toggle_sidebar {
            Some(MenuAction::ToggleSidebar)
        } else if *id == items.toggle_notifications {
            Some(MenuAction::ToggleNotificationPanel)
        } else if *id == items.zoom_in {
            Some(MenuAction::ZoomIn)
        } else if *id == items.zoom_out {
            Some(MenuAction::ZoomOut)
        } else if *id == items.zoom_reset {
            Some(MenuAction::ZoomReset)
        } else if *id == items.copy {
            Some(MenuAction::Copy)
        } else if *id == items.paste {
            Some(MenuAction::Paste)
        } else if *id == items.select_all {
            Some(MenuAction::SelectAll)
        } else {
            // Predefined OS item or unknown — no local handler.
            None
        };
        if let Some(action) = action {
            push_action(action);
        }
    }
}

// ---------------------------------------------------------------------
// Non-macOS path (integrated into the existing titlebar strip)
// ---------------------------------------------------------------------
//
// amux already owns the top-of-window chrome on Windows/Linux: a
// 28px-tall strip (`TERMINAL_TOP_PAD`) whose background is painted in
// a background layer from `frame_update.rs`, with the sidebar/bell/+
// icons drawn over it via a fixed-position `egui::Area` from
// `notifications_ui::render_titlebar_icons`. A first attempt at the
// menu bar used `egui::TopBottomPanel::top` as a separate panel, but
// that created two competing top strips and the menu labels ended up
// invisible — the foreground icon Area painted over them and the
// layout math got confused.
//
// This implementation plugs the menu directly into the same titlebar
// strip: `draw_menu_buttons` below is called from
// `render_titlebar_icons_inner` after the icons are laid out, at the
// same `y` position, drawing File/Edit/View as clickable labels in
// the remaining horizontal space. One strip, one coordinate system,
// no layer fights.

/// Pick a readable foreground color for a given background by checking
/// its perceived luminance. Matches what amux does elsewhere (sidebar,
/// tab bar) to keep chrome text legible against any user theme.
#[cfg(not(target_os = "macos"))]
pub(crate) fn contrast_text(bg: egui::Color32) -> egui::Color32 {
    // Rec. 601 luma — the approximation most UI toolkits use.
    let r = bg.r() as f32;
    let g = bg.g() as f32;
    let b = bg.b() as f32;
    let luma = 0.299 * r + 0.587 * g + 0.114 * b;
    if luma < 128.0 {
        egui::Color32::from_rgb(0xE6, 0xE6, 0xE6) // soft white
    } else {
        egui::Color32::from_rgb(0x20, 0x20, 0x20) // near black
    }
}

/// Bundle of theme-derived colors used by every menu-related
/// rendering path below. Computed once per top-level call so the
/// per-item loop doesn't redo the luma math.
#[cfg(not(target_os = "macos"))]
#[derive(Clone, Copy)]
struct MenuPalette {
    bg: egui::Color32,
    fg: egui::Color32,
    hover_bg: egui::Color32,
    divider: egui::Color32,
}

#[cfg(not(target_os = "macos"))]
impl MenuPalette {
    fn from_theme(theme: &crate::theme::Theme) -> Self {
        let bg = theme.titlebar_bg();
        let fg = contrast_text(bg);
        let luma_sum: u16 = bg.r() as u16 + bg.g() as u16 + (bg.b() as u16);
        let hover_bg = if luma_sum < 384 {
            bg.gamma_multiply(1.5) // dark theme → brighten on hover
        } else {
            bg.gamma_multiply(0.85) // light theme → darken on hover
        };
        // Divider: a faint version of fg for popup borders and
        // separator lines. Low alpha so it doesn't overpower — this
        // color is used for a 1px stroke on popup frames, so even
        // a small alpha bump looks loud. Tune down if future
        // complaints surface.
        let divider = egui::Color32::from_rgba_unmultiplied(fg.r(), fg.g(), fg.b(), 24);
        Self {
            bg,
            fg,
            hover_bg,
            divider,
        }
    }
}

/// Apply a `MenuPalette` to a UI's visuals so that every widget
/// drawn inside is rendered in amux's chrome colors. Call this at
/// the top of any popup closure.
///
/// This also styles the popup's OUTER Frame (which egui's
/// `popup_below_widget` / `menu_button` builds automatically from
/// `visuals.window_fill` + `visuals.window_stroke`). We don't wrap
/// our content in an additional inner Frame — doing that stacks
/// two frames + their inner_margin gap and produces what looks
/// like an absurdly thick border.
#[cfg(not(target_os = "macos"))]
fn apply_menu_palette(ui: &mut egui::Ui, palette: MenuPalette) {
    let visuals = &mut ui.style_mut().visuals;

    // --- Popup container styling (used by egui's popup Frame) ---
    visuals.window_fill = palette.bg;
    visuals.panel_fill = palette.bg;
    // Thin 1px stroke at the divider color — deliberately subtle
    // so the popup doesn't look like it's wearing a picture frame.
    visuals.window_stroke = egui::Stroke::new(1.0, palette.divider);

    // --- Text color ---
    // egui `Button` uses `widgets.{state}.fg_stroke.color` for label
    // rendering, NOT `override_text_color` in every code path. Set
    // both so every widget picks it up.
    visuals.override_text_color = Some(palette.fg);
    visuals.widgets.inactive.fg_stroke.color = palette.fg;
    visuals.widgets.hovered.fg_stroke.color = palette.fg;
    visuals.widgets.active.fg_stroke.color = palette.fg;
    visuals.widgets.open.fg_stroke.color = palette.fg;

    // --- Widget backgrounds ---
    // Inactive state: transparent (button looks like plain text
    // until the user hovers it).
    visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
    // Hover / active / open: subtle highlight.
    visuals.widgets.hovered.weak_bg_fill = palette.hover_bg;
    visuals.widgets.hovered.bg_fill = palette.hover_bg;
    visuals.widgets.active.weak_bg_fill = palette.hover_bg;
    visuals.widgets.active.bg_fill = palette.hover_bg;
    visuals.widgets.open.weak_bg_fill = palette.hover_bg;
    visuals.widgets.open.bg_fill = palette.hover_bg;

    // --- Widget border strokes ---
    // No borders on buttons — they should look like plain text
    // links, not boxed controls.
    visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.open.bg_stroke = egui::Stroke::NONE;

    // --- Separator color ---
    // `ui.separator()` draws using `noninteractive.bg_stroke`.
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, palette.divider);
}

/// Render the items of a single submenu (labels + shortcuts +
/// separators) into the current UI. Shared between the Menubar
/// path and the Hamburger path (where it's called once per
/// nested submenu).
#[cfg(not(target_os = "macos"))]
fn render_submenu_items(ui: &mut egui::Ui, items: &[MenuItemDef], close_popup_on_click: bool) {
    for item in items {
        match item {
            MenuItemDef::Separator => {
                ui.separator();
            }
            MenuItemDef::Action {
                label,
                shortcut,
                action,
            } => {
                let button = match shortcut {
                    Some(sc) => egui::Button::new(*label).shortcut_text(*sc),
                    None => egui::Button::new(*label),
                };
                if ui.add(button).clicked() {
                    push_action(*action);
                    if close_popup_on_click {
                        ui.memory_mut(|m| m.close_popup());
                    } else {
                        // For nested menus (Hamburger mode), closing
                        // the top-level popup also closes the nested
                        // one in egui's popup model. Fall through to
                        // the default click → collapse the parent
                        // menu_button behavior.
                        ui.close_menu();
                    }
                }
            }
        }
    }
}

/// Draw the popup body for a single submenu. The outer framed
/// container is egui's default popup Frame (built from
/// `visuals.window_fill` / `visuals.window_stroke` that we set in
/// `apply_menu_palette`), so we don't stack an additional inner
/// Frame here — doing that produces a visibly thick border made of
/// two stacked strokes plus the inner Frame's margin.
#[cfg(not(target_os = "macos"))]
fn draw_submenu_popup(ui: &mut egui::Ui, items: &[MenuItemDef], palette: MenuPalette) {
    apply_menu_palette(ui, palette);
    ui.set_min_width(200.0);
    render_submenu_items(ui, items, /* close_popup_on_click */ true);
}

/// Draw the `File Edit View` labels as clickable text. Used by the
/// `Menubar` mode — called once per frame from
/// `notifications_ui::render_titlebar_icons` into the dedicated
/// menu strip above the icon row.
///
/// `start_x` / `y` / `row_height` describe the strip geometry. Each
/// label is sized to fit its text and draws its own hover rect.
#[cfg(not(target_os = "macos"))]
pub(crate) fn draw_menu_buttons(
    ui: &mut egui::Ui,
    start_x: f32,
    y: f32,
    row_height: f32,
    theme: &crate::theme::Theme,
) {
    let palette = MenuPalette::from_theme(theme);

    // Layout constants for the label row.
    const LABEL_GAP: f32 = 8.0;
    const LABEL_PAD_X: f32 = 10.0;
    const LABEL_FONT_SIZE: f32 = 13.5;

    let mut x = start_x + LABEL_GAP;
    for submenu in MENU_MODEL {
        let galley = ui.painter().layout_no_wrap(
            submenu.label.to_string(),
            egui::FontId::proportional(LABEL_FONT_SIZE),
            palette.fg,
        );
        let label_w = galley.size().x + LABEL_PAD_X * 2.0;
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(label_w, row_height));

        let id = ui.id().with(("amux_menu_label", submenu.label));
        let response = ui.interact(rect, id, egui::Sense::click());

        // Hover background — draws inside the strip itself.
        if response.hovered() {
            ui.painter().rect_filled(rect, 4.0, palette.hover_bg);
        }

        // Label text, centered vertically inside the hit rect.
        let text_pos = egui::pos2(
            rect.min.x + LABEL_PAD_X,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(text_pos, galley, palette.fg);

        // Click → toggle this label's popup.
        let popup_id = ui.make_persistent_id(("amux_menu_popup", submenu.label));
        if response.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup_id));
        }
        // Apply our palette to the PARENT UI (this one) before
        // calling popup_below_widget. Egui's popup implementation
        // reads `parent_ui.style()` to build its Frame
        // (`Frame::popup(parent_ui.style())` in egui 0.31 —
        // `containers/popup.rs:415`), so we have to set the style
        // on this UI, not on the popup's child UI inside the
        // closure. The child UI inherits from the parent so our
        // widget text/bg overrides still apply.
        apply_menu_palette(ui, palette);
        egui::popup::popup_below_widget(
            ui,
            popup_id,
            &response,
            egui::PopupCloseBehavior::CloseOnClickOutside,
            |ui| draw_submenu_popup(ui, submenu.items, palette),
        );

        x += label_w;
    }
}

/// Draw the hamburger (`≡`) button used in `Hamburger` mode. Sized
/// to match the titlebar icon row's icon size so it nests cleanly
/// at the leftmost position of the row.
///
/// Clicking the button toggles a single large popup that contains
/// all submenus as nested `ui.menu_button` blocks — one click opens
/// the hamburger, hovering a submenu expands it. Full menu access
/// with zero extra vertical chrome.
#[cfg(not(target_os = "macos"))]
pub(crate) fn draw_hamburger_button(
    ui: &mut egui::Ui,
    icon_size: egui::Vec2,
    theme: &crate::theme::Theme,
) {
    let palette = MenuPalette::from_theme(theme);
    let origin = ui.min_rect().min;
    let rect = egui::Rect::from_min_size(origin, icon_size);
    let id = ui.id().with("amux_hamburger_btn");
    let response = ui.interact(rect, id, egui::Sense::click());

    // Hover background.
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, palette.hover_bg);
    }

    // Draw three horizontal lines for the hamburger glyph.
    let center = rect.center();
    let line_w = icon_size.x * 0.5;
    let line_stroke = egui::Stroke::new(1.5, palette.fg);
    let spacing = 4.0;
    for dy in [-spacing, 0.0, spacing] {
        let y = center.y + dy;
        ui.painter().line_segment(
            [
                egui::pos2(center.x - line_w / 2.0, y),
                egui::pos2(center.x + line_w / 2.0, y),
            ],
            line_stroke,
        );
    }

    // Popup handling.
    let popup_id = ui.make_persistent_id("amux_hamburger_popup");
    if response.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    // Apply palette to the PARENT UI before calling
    // popup_below_widget — egui reads `parent_ui.style()` to
    // construct its popup Frame. See the same comment in
    // draw_menu_buttons above.
    apply_menu_palette(ui, palette);
    egui::popup::popup_below_widget(
        ui,
        popup_id,
        &response,
        egui::PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            apply_menu_palette(ui, palette);
            ui.set_min_width(220.0);
            // Each top-level submenu is a nested `ui.menu_button`
            // so the user can hover/click to expand inline. This
            // keeps the vertical footprint minimal — only the three
            // top-level labels are visible until the user actively
            // drills in.
            //
            // No wrapping `egui::Frame` here — egui's popup already
            // paints a Frame using `visuals.window_fill` /
            // `visuals.window_stroke`, which `apply_menu_palette`
            // sets. Stacking another Frame on top produces a
            // visible double border.
            for submenu in MENU_MODEL {
                ui.menu_button(submenu.label, |ui| {
                    apply_menu_palette(ui, palette);
                    render_submenu_items(ui, submenu.items, /* close_popup_on_click */ false);
                });
            }
        },
    );
}
