/// Native macOS menu bar (File, Edit, View, Help) at the top of the screen.
///
/// Uses NSMenu/NSMenuItem via objc2. Menu item clicks store a pending action
/// in a static AtomicU32 that the egui update loop drains each frame.
///
/// On non-macOS platforms this module compiles to no-ops.
use std::sync::atomic::{AtomicU32, Ordering};

/// Pending menu action, polled each frame by the egui update loop.
static PENDING_ACTION: AtomicU32 = AtomicU32::new(0);

/// Actions that can be triggered from the native menu bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum MenuAction {
    None = 0,
    NewWorkspace = 1,
    NewTab = 2,
    CloseTab = 3,
    SaveSession = 4,
    ToggleSidebar = 10,
    ToggleNotificationPanel = 11,
    ZoomIn = 12,
    ZoomOut = 13,
    ZoomReset = 14,
}

impl MenuAction {
    fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::NewWorkspace,
            2 => Self::NewTab,
            3 => Self::CloseTab,
            4 => Self::SaveSession,
            10 => Self::ToggleSidebar,
            11 => Self::ToggleNotificationPanel,
            12 => Self::ZoomIn,
            13 => Self::ZoomOut,
            14 => Self::ZoomReset,
            _ => Self::None,
        }
    }
}

/// Drain the pending menu action (returns `None` if no action pending).
pub(crate) fn take_pending_action() -> Option<MenuAction> {
    let v = PENDING_ACTION.swap(0, Ordering::Relaxed);
    match MenuAction::from_u32(v) {
        MenuAction::None => None,
        action => Some(action),
    }
}

// ---------------------------------------------------------------------------
// macOS implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub(crate) fn install() {
    use objc2::sel;
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
    use objc2_foundation::{MainThreadMarker, NSString};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let app = NSApplication::sharedApplication(mtm);

    // Register the ObjC action handler class once
    register_handler_class();

    let main_menu = NSMenu::new(mtm);

    // --- amux menu (app name) ---
    {
        let menu = NSMenu::new(mtm);
        menu.setTitle(&NSString::from_str("amux"));

        add_system_item(
            &menu,
            "About amux",
            sel!(orderFrontStandardAboutPanel:),
            "",
            None,
            mtm,
        );
        add_separator(&menu, mtm);
        add_system_item(&menu, "Hide amux", sel!(hide:), "h", None, mtm);
        add_system_item(
            &menu,
            "Hide Others",
            sel!(hideOtherApplications:),
            "h",
            Some(NSEventModifierFlags::Command | NSEventModifierFlags::Option),
            mtm,
        );
        add_system_item(
            &menu,
            "Show All",
            sel!(unhideAllApplications:),
            "",
            None,
            mtm,
        );
        add_separator(&menu, mtm);
        add_system_item(&menu, "Quit amux", sel!(terminate:), "q", None, mtm);

        let app_item = NSMenuItem::new(mtm);
        app_item.setSubmenu(Some(&menu));
        main_menu.addItem(&app_item);
    }

    // --- File menu ---
    {
        let menu = NSMenu::new(mtm);
        menu.setTitle(&NSString::from_str("File"));

        add_action_item(&menu, "New Workspace", "n", MenuAction::NewWorkspace, mtm);
        add_action_item(&menu, "New Tab", "t", MenuAction::NewTab, mtm);
        add_separator(&menu, mtm);
        add_action_item(&menu, "Close Tab", "w", MenuAction::CloseTab, mtm);
        add_separator(&menu, mtm);
        add_action_item(&menu, "Save Session", "s", MenuAction::SaveSession, mtm);

        let file_item = NSMenuItem::new(mtm);
        file_item.setSubmenu(Some(&menu));
        main_menu.addItem(&file_item);
    }

    // --- Edit menu ---
    {
        let menu = NSMenu::new(mtm);
        menu.setTitle(&NSString::from_str("Edit"));

        add_system_item(&menu, "Copy", sel!(copy:), "c", None, mtm);
        add_system_item(&menu, "Paste", sel!(paste:), "v", None, mtm);
        add_system_item(&menu, "Select All", sel!(selectAll:), "a", None, mtm);

        let edit_item = NSMenuItem::new(mtm);
        edit_item.setSubmenu(Some(&menu));
        main_menu.addItem(&edit_item);
    }

    // --- View menu ---
    {
        let menu = NSMenu::new(mtm);
        menu.setTitle(&NSString::from_str("View"));

        add_action_item(&menu, "Toggle Sidebar", "b", MenuAction::ToggleSidebar, mtm);
        add_action_item(
            &menu,
            "Toggle Notifications",
            "",
            MenuAction::ToggleNotificationPanel,
            mtm,
        );
        add_separator(&menu, mtm);
        add_action_item(&menu, "Zoom In", "+", MenuAction::ZoomIn, mtm);
        add_action_item(&menu, "Zoom Out", "-", MenuAction::ZoomOut, mtm);
        add_action_item(&menu, "Actual Size", "0", MenuAction::ZoomReset, mtm);

        let view_item = NSMenuItem::new(mtm);
        view_item.setSubmenu(Some(&menu));
        main_menu.addItem(&view_item);
    }

    // --- Window menu ---
    {
        let menu = NSMenu::new(mtm);
        menu.setTitle(&NSString::from_str("Window"));

        add_system_item(&menu, "Minimize", sel!(performMiniaturize:), "m", None, mtm);
        add_system_item(&menu, "Zoom", sel!(performZoom:), "", None, mtm);

        let window_item = NSMenuItem::new(mtm);
        window_item.setSubmenu(Some(&menu));
        main_menu.addItem(&window_item);
        app.setWindowsMenu(Some(&menu));
    }

    // --- Help menu ---
    {
        let menu = NSMenu::new(mtm);
        menu.setTitle(&NSString::from_str("Help"));

        let help_item = NSMenuItem::new(mtm);
        help_item.setSubmenu(Some(&menu));
        main_menu.addItem(&help_item);
        app.setHelpMenu(Some(&menu));
    }

    app.setMainMenu(Some(&main_menu));
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a menu item that invokes a standard AppKit selector (copy:, paste:, etc.)
#[cfg(target_os = "macos")]
fn add_system_item(
    menu: &objc2_app_kit::NSMenu,
    title: &str,
    action: objc2::runtime::Sel,
    key: &str,
    modifiers: Option<objc2_app_kit::NSEventModifierFlags>,
    mtm: objc2_foundation::MainThreadMarker,
) {
    use objc2_app_kit::NSMenuItem;
    use objc2_foundation::NSString;

    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    unsafe { item.setAction(Some(action)) };
    item.setKeyEquivalent(&NSString::from_str(key));
    if let Some(mods) = modifiers {
        item.setKeyEquivalentModifierMask(mods);
    }
    menu.addItem(&item);
}

#[cfg(target_os = "macos")]
fn add_separator(menu: &objc2_app_kit::NSMenu, mtm: objc2_foundation::MainThreadMarker) {
    use objc2_app_kit::NSMenuItem;
    menu.addItem(&NSMenuItem::separatorItem(mtm));
}

/// Create a menu item that dispatches a custom `MenuAction` via our ObjC handler.
#[cfg(target_os = "macos")]
fn add_action_item(
    menu: &objc2_app_kit::NSMenu,
    title: &str,
    key: &str,
    action: MenuAction,
    mtm: objc2_foundation::MainThreadMarker,
) {
    use objc2::sel;
    use objc2_app_kit::NSMenuItem;
    use objc2_foundation::NSString;

    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    unsafe { item.setAction(Some(sel!(handleMenuAction:))) };
    item.setKeyEquivalent(&NSString::from_str(key));
    item.setTag(action as isize);

    unsafe { item.setTarget(Some(get_handler_instance())) };

    menu.addItem(&item);
}

// ---------------------------------------------------------------------------
// ObjC handler class — receives menu item actions and stores them in the atomic
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
static HANDLER_REGISTERED: std::sync::Once = std::sync::Once::new();

#[cfg(target_os = "macos")]
fn register_handler_class() {
    use objc2::runtime::{ClassBuilder, NSObject};
    use objc2::sel;
    use objc2::ClassType;

    HANDLER_REGISTERED.call_once(|| {
        let superclass = NSObject::class();
        let mut builder =
            ClassBuilder::new(c"AmuxMenuHandler", superclass).expect("class already registered");

        unsafe {
            builder.add_method(
                sel!(handleMenuAction:),
                handle_menu_action
                    as unsafe extern "C" fn(
                        *const objc2::runtime::AnyObject,
                        objc2::runtime::Sel,
                        *const objc2::runtime::AnyObject,
                    ),
            );
        }

        let _ = builder.register();
    });
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn handle_menu_action(
    _this: *const objc2::runtime::AnyObject,
    _sel: objc2::runtime::Sel,
    sender: *const objc2::runtime::AnyObject,
) {
    if sender.is_null() {
        return;
    }
    // sender is the NSMenuItem — read its tag to get the MenuAction
    let item: &objc2_app_kit::NSMenuItem =
        unsafe { &*(sender as *const objc2_app_kit::NSMenuItem) };
    let tag = item.tag() as u32;
    PENDING_ACTION.store(tag, Ordering::Relaxed);
}

/// Shared handler instance — kept alive for the lifetime of the process.
/// NSMenuItem target is an unretained reference, so we must ensure the
/// handler outlives all menu items. We leak the Retained to get a 'static ref.
#[cfg(target_os = "macos")]
fn get_handler_instance() -> &'static objc2::runtime::AnyObject {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    use std::sync::atomic::AtomicPtr;

    static HANDLER_PTR: AtomicPtr<objc2::runtime::AnyObject> = AtomicPtr::new(std::ptr::null_mut());

    let ptr = HANDLER_PTR.load(Ordering::Acquire);
    if !ptr.is_null() {
        return unsafe { &*ptr };
    }

    let cls = AnyClass::get(c"AmuxMenuHandler").expect("AmuxMenuHandler not registered");
    let obj: objc2::rc::Retained<objc2::runtime::AnyObject> = unsafe { msg_send![cls, new] };
    let raw = objc2::rc::Retained::into_raw(obj);
    // Intentionally leaked — lives for the process lifetime
    HANDLER_PTR.store(raw as *mut _, Ordering::Release);
    unsafe { &*raw }
}

// ---------------------------------------------------------------------------
// Non-macOS stub
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "macos"))]
pub(crate) fn install() {
    // No native menu bar on Windows/Linux — future: in-app menu bar
}
