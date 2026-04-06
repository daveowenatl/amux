/// Cross-platform native menu bar using `muda`.
///
/// macOS: top-of-screen system menu bar.
/// Windows: per-window native Win32 menu bar.
/// Linux: GTK menu bar (requires GTK window access — not yet wired up).
///
/// Menu item clicks are delivered via `muda::MenuEvent::receiver()`, drained
/// each frame in the egui update loop.
///
/// Accelerators match the keybindings in `handle_shortcuts()`: Cmd on macOS,
/// Ctrl+Shift on Windows/Linux for workspace/tab ops, Ctrl for view ops.
/// On both platforms the system menu bar consumes the key event before it
/// reaches the egui event loop, so the shortcut handlers will not double-fire.
use muda::accelerator::{Accelerator, Code, Modifiers};
#[cfg(target_os = "macos")]
use muda::AboutMetadata;
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

/// Actions that can be triggered from the native menu bar.
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

/// Stored menu item IDs, used to match incoming `MenuEvent`s to actions.
struct MenuItems {
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

static MENU_ITEMS: std::sync::OnceLock<MenuItems> = std::sync::OnceLock::new();

/// Platform modifier: Cmd on macOS, Ctrl on Windows/Linux.
#[cfg(target_os = "macos")]
const CMD: Modifiers = Modifiers::SUPER;
#[cfg(not(target_os = "macos"))]
const CMD: Modifiers = Modifiers::CONTROL;

/// Ctrl+Shift on non-macOS — matches `handle_shortcuts()` which uses
/// Ctrl+Shift for workspace/tab operations to avoid stealing terminal
/// control keys (Ctrl+N/T/W/S).
#[cfg(not(target_os = "macos"))]
const CMD_SHIFT: Modifiers = Modifiers::CONTROL.union(Modifiers::SHIFT);

fn accel(mods: Modifiers, code: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Some(mods), code))
}

/// Build and install the native menu bar. Call once at startup.
///
/// On macOS this sets the app-global menu bar via `init_for_nsapp()`.
/// On Windows, call `init_for_hwnd()` once the window handle is available
/// (see `attach_to_window()`).
///
/// Accelerators match the existing keybindings in `handle_shortcuts()`:
/// - macOS: Cmd+key
/// - Windows/Linux: Ctrl+Shift+key for workspace/tab ops; Ctrl+key for
///   view ops (sidebar, zoom) that don't conflict with terminal control chars.
pub(crate) fn build() -> Menu {
    // --- Custom action items ---
    // On non-macOS, workspace/tab operations use Ctrl+Shift to avoid
    // stealing terminal control keys (Ctrl+N = new line, Ctrl+W = word
    // erase, Ctrl+S = XOFF).
    #[cfg(target_os = "macos")]
    let new_workspace = MenuItem::new("New Workspace", true, accel(CMD, Code::KeyN));
    #[cfg(not(target_os = "macos"))]
    let new_workspace = MenuItem::new("New Workspace", true, accel(CMD_SHIFT, Code::KeyN));

    #[cfg(target_os = "macos")]
    let new_tab = MenuItem::new("New Tab", true, accel(CMD, Code::KeyT));
    #[cfg(not(target_os = "macos"))]
    let new_tab = MenuItem::new("New Tab", true, accel(CMD_SHIFT, Code::KeyT));

    let new_browser_tab = MenuItem::new(
        "New Browser Tab",
        true,
        accel(CMD.union(Modifiers::SHIFT), Code::KeyL),
    );

    #[cfg(target_os = "macos")]
    let close_tab = MenuItem::new("Close Tab", true, accel(CMD, Code::KeyW));
    #[cfg(not(target_os = "macos"))]
    let close_tab = MenuItem::new("Close Tab", true, accel(CMD_SHIFT, Code::KeyW));

    #[cfg(target_os = "macos")]
    let save_session = MenuItem::new("Save Session", true, accel(CMD, Code::KeyS));
    #[cfg(not(target_os = "macos"))]
    let save_session = MenuItem::new("Save Session", true, accel(CMD_SHIFT, Code::KeyS));

    let toggle_sidebar = MenuItem::new("Toggle Sidebar", true, accel(CMD, Code::KeyB));
    #[cfg(target_os = "macos")]
    let toggle_notifications = MenuItem::new("Toggle Notifications", true, accel(CMD, Code::KeyI));
    #[cfg(not(target_os = "macos"))]
    let toggle_notifications =
        MenuItem::new("Toggle Notifications", true, accel(CMD_SHIFT, Code::KeyI));
    let zoom_in = MenuItem::new("Zoom In", true, accel(CMD, Code::Equal));
    let zoom_out = MenuItem::new("Zoom Out", true, accel(CMD, Code::Minus));
    let zoom_reset = MenuItem::new("Actual Size", true, accel(CMD, Code::Digit0));

    // Edit menu items — use custom MenuItems (not PredefinedMenuItem) so we
    // receive the event in our handler instead of it being consumed by the OS.
    #[cfg(target_os = "macos")]
    let copy = MenuItem::new("Copy", true, accel(CMD, Code::KeyC));
    #[cfg(not(target_os = "macos"))]
    let copy = MenuItem::new("Copy", true, accel(CMD_SHIFT, Code::KeyC));
    #[cfg(target_os = "macos")]
    let paste = MenuItem::new("Paste", true, accel(CMD, Code::KeyV));
    #[cfg(not(target_os = "macos"))]
    let paste = MenuItem::new("Paste", true, accel(CMD_SHIFT, Code::KeyV));
    #[cfg(target_os = "macos")]
    let select_all = MenuItem::new("Select All", true, accel(CMD, Code::KeyA));
    #[cfg(not(target_os = "macos"))]
    let select_all = MenuItem::new("Select All", true, accel(CMD_SHIFT, Code::KeyA));

    // Store IDs for event matching
    if MENU_ITEMS
        .set(MenuItems {
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

    // --- App menu (macOS only, ignored on other platforms) ---
    #[cfg(target_os = "macos")]
    {
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
    }

    // --- File menu ---
    {
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
    }

    // --- Edit menu ---
    {
        let edit_menu = Submenu::new("Edit", true);
        let _ =
            edit_menu.append_items(&[&copy, &paste, &PredefinedMenuItem::separator(), &select_all]);
        let _ = menu.append(&edit_menu);
    }

    // --- View menu ---
    {
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
    }

    // --- Window menu ---
    {
        let window_menu = Submenu::new("Window", true);
        let _ = window_menu.append_items(&[
            &PredefinedMenuItem::minimize(None),
            &PredefinedMenuItem::maximize(None),
        ]);
        let _ = menu.append(&window_menu);

        #[cfg(target_os = "macos")]
        window_menu.set_as_windows_menu_for_nsapp();
    }

    // --- Help menu (macOS only — includes system search field; empty on other platforms) ---
    #[cfg(target_os = "macos")]
    {
        let help_menu = Submenu::new("Help", true);
        let _ = menu.append(&help_menu);
        help_menu.set_as_help_menu_for_nsapp();
    }

    // Install for macOS immediately (app-global menu bar).
    #[cfg(target_os = "macos")]
    menu.init_for_nsapp();

    // Linux: muda supports GTK menus via `init_for_gtk_window()`, but eframe
    // does not expose the underlying GtkWindow. When Linux support is needed,
    // the GTK window can be obtained from the raw Xlib/Wayland handle or by
    // patching eframe to surface it.

    menu
}

/// On Windows, attach the menu bar to the window once the HWND is available.
/// Call from the first `App::update()` frame. Returns `true` if the menu was
/// successfully attached; `false` if the window handle wasn't ready yet.
#[cfg(target_os = "windows")]
pub(crate) fn attach_to_window(menu: &Menu, frame: &eframe::Frame) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(handle) = frame.window_handle() {
        if let RawWindowHandle::Win32(win32) = handle.as_raw() {
            match unsafe { menu.init_for_hwnd(win32.hwnd.get() as _) } {
                Ok(()) => return true,
                Err(e) => {
                    tracing::error!("Failed to attach menu bar to HWND: {e}");
                    return false;
                }
            }
        }
    }
    false
}

/// Drain pending menu events and return the next recognized action, if any.
/// Skips unrecognized event IDs (e.g. from predefined items handled by the OS)
/// so they don't block processing of subsequent events in the queue.
pub(crate) fn take_pending_action() -> Option<MenuAction> {
    let items = MENU_ITEMS.get()?;
    loop {
        let event = MenuEvent::receiver().try_recv().ok()?;
        let id = &event.id;
        if *id == items.new_workspace {
            return Some(MenuAction::NewWorkspace);
        } else if *id == items.new_tab {
            return Some(MenuAction::NewTab);
        } else if *id == items.new_browser_tab {
            return Some(MenuAction::NewBrowserTab);
        } else if *id == items.close_tab {
            return Some(MenuAction::CloseTab);
        } else if *id == items.save_session {
            return Some(MenuAction::SaveSession);
        } else if *id == items.toggle_sidebar {
            return Some(MenuAction::ToggleSidebar);
        } else if *id == items.toggle_notifications {
            return Some(MenuAction::ToggleNotificationPanel);
        } else if *id == items.zoom_in {
            return Some(MenuAction::ZoomIn);
        } else if *id == items.zoom_out {
            return Some(MenuAction::ZoomOut);
        } else if *id == items.zoom_reset {
            return Some(MenuAction::ZoomReset);
        } else if *id == items.copy {
            return Some(MenuAction::Copy);
        } else if *id == items.paste {
            return Some(MenuAction::Paste);
        } else if *id == items.select_all {
            return Some(MenuAction::SelectAll);
        }
        // Unknown ID (predefined OS item, etc.) — skip and keep draining.
    }
}
