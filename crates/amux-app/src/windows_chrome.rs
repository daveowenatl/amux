//! Windows-only dark-mode chrome helpers.
//!
//! Enables dark-mode themed controls (title bar + common controls, best-
//! effort for the native menu bar) on Windows 10/11. Called once at
//! startup from `menu_bar::attach_to_window` right before the menu bar
//! is initialized on the window's HWND, so the menu paints in dark mode
//! from its first frame.
//!
//! Approach:
//!
//! 1. **Title bar**: documented `DwmSetWindowAttribute` with
//!    `DWMWA_USE_IMMERSIVE_DARK_MODE` (attribute 20). Stable since
//!    Windows 10 2004.
//! 2. **Process-wide dark mode**: undocumented `SetPreferredAppMode` at
//!    uxtheme.dll ordinal 135 (Windows 10 1903+), called with
//!    `AllowDark`. This opts the process into dark-themed common
//!    controls and is what VS Code, Windows Terminal, and File Explorer
//!    all use internally.
//! 3. **Per-window dark mode**: undocumented `AllowDarkModeForWindow`
//!    at uxtheme.dll ordinal 133, called on the HWND. Required in
//!    addition to the process-wide call for some themed controls.
//! 4. **Policy refresh**: undocumented `RefreshImmersiveColorPolicyState`
//!    at uxtheme.dll ordinal 104, to force Windows to reapply dark mode
//!    to already-created windows.
//!
//! The undocumented ordinals may break or be renumbered on future
//! Windows builds. Every call site is defensive: if the function
//! can't be loaded, we silently no-op and the user sees whatever
//! chrome they would have seen without amux trying. No crashes.
//!
//! If this still doesn't make the native menu bar follow dark mode
//! (Windows 11 has made the Win32 menu rendering path more stubborn
//! than it used to be), the follow-up fix is to replace muda's native
//! menu with an egui-drawn top bar. Tracked in amux #190.

#![cfg(target_os = "windows")]

use windows_sys::Win32::Foundation::{HMODULE, HWND};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

// `BOOL` / `TRUE` used to live in `windows_sys::Win32::Foundation`, but
// windows-sys 0.60 moved or renamed them. We only need them as a plain
// 4-byte integer for the DWM attribute, so declare them locally instead
// of chasing the re-export. `BOOL` on Win32 is always a signed 32-bit
// int where 0 = FALSE and any non-zero value = TRUE.
#[allow(non_camel_case_types)]
type BOOL = i32;
const TRUE: BOOL = 1;

/// PreferredAppMode — the arg to `SetPreferredAppMode` (uxtheme.dll
/// ordinal 135). Values are from leaked Windows SDK headers and match
/// the enum used internally by Explorer.
#[repr(C)]
#[allow(dead_code)]
enum PreferredAppMode {
    Default = 0,
    AllowDark = 1,
    ForceDark = 2,
    ForceLight = 3,
}

/// UTF-16 NUL-terminated literal for `LoadLibraryW`.
const UXTHEME_DLL_W: [u16; 12] = [
    b'u' as u16,
    b'x' as u16,
    b't' as u16,
    b'h' as u16,
    b'e' as u16,
    b'm' as u16,
    b'e' as u16,
    b'.' as u16,
    b'd' as u16,
    b'l' as u16,
    b'l' as u16,
    0,
];

/// Load uxtheme.dll once. Returns null if the load fails (unrealistic
/// on any Windows system that runs amux, but we handle it anyway).
///
/// We intentionally never `FreeLibrary` the handle: uxtheme is a core
/// system DLL that stays loaded for the process lifetime regardless,
/// so leaking our one reference is a no-op.
unsafe fn uxtheme() -> HMODULE {
    LoadLibraryW(UXTHEME_DLL_W.as_ptr())
}

/// Enable process-wide dark mode. Call once at startup, before any
/// windows are created or shown. Safe to call multiple times — the
/// underlying ordinals are idempotent.
///
/// Reaches two undocumented uxtheme ordinals: 135 `SetPreferredAppMode`
/// and 104 `RefreshImmersiveColorPolicyState`. If either ordinal can't
/// be resolved (future Windows build that removed or renumbered them),
/// we silently skip that step and the process stays in light mode —
/// no crash, no tracing-level noise, just the pre-amux baseline.
pub fn enable_process_dark_mode() {
    unsafe {
        let dll = uxtheme();
        if dll.is_null() {
            return;
        }

        // Ordinal 135: SetPreferredAppMode(PreferredAppMode) -> PreferredAppMode
        // Returns the previous mode, which we discard. The ordinal is passed
        // to `GetProcAddress` as a LPCSTR whose low WORD is the ordinal
        // number (Win32 convention; equivalent to the C `MAKEINTRESOURCEA`
        // macro).
        if let Some(fn_ptr) = GetProcAddress(dll, 135usize as *const u8) {
            type SetPreferredAppMode =
                unsafe extern "system" fn(PreferredAppMode) -> PreferredAppMode;
            let set_preferred_app_mode: SetPreferredAppMode = std::mem::transmute(fn_ptr);
            // AllowDark = follow system setting when system is in dark mode.
            // We deliberately don't use ForceDark because some users may
            // configure Windows in light mode intentionally — following the
            // system preference is the least surprising choice.
            let _previous = set_preferred_app_mode(PreferredAppMode::AllowDark);
        }

        // Ordinal 104: RefreshImmersiveColorPolicyState() -> void
        // Forces Windows to reapply the policy to existing windows.
        if let Some(fn_ptr) = GetProcAddress(dll, 104usize as *const u8) {
            type RefreshImmersiveColorPolicyState = unsafe extern "system" fn();
            let refresh: RefreshImmersiveColorPolicyState = std::mem::transmute(fn_ptr);
            refresh();
        }
    }
}

/// Enable dark mode for a specific window. Applies both the documented
/// DWM title-bar flag AND the undocumented uxtheme per-window
/// `AllowDarkModeForWindow` ordinal, which is required in addition to
/// the process-wide `SetPreferredAppMode` for themed Win32 controls
/// hosted inside the window.
///
/// Takes the HWND as a raw `isize` to match the shape that
/// `raw_window_handle::Win32WindowHandle::hwnd` and `muda::Menu::init_for_hwnd`
/// both use — this lets the caller pass the same handle to both APIs
/// without fighting two different pointer types. We convert internally
/// to `windows-sys`'s `HWND` (which is `*mut c_void`).
///
/// Call after `enable_process_dark_mode()`, once the window's HWND is
/// available (first `App::update()` frame).
pub fn apply_dark_mode_to_window(hwnd_raw: isize) {
    let hwnd: HWND = hwnd_raw as HWND;
    unsafe {
        // Documented path: immersive dark mode for the title bar.
        // Attribute 20 (`DWMWA_USE_IMMERSIVE_DARK_MODE`) on Windows
        // 10 2004+. Older Windows 10 builds used attribute 19 — we
        // only target modern builds.
        //
        // windows-sys 0.60 declares the attribute constant as `i32`
        // but `DwmSetWindowAttribute` takes `u32`, so we have to cast.
        let enable: BOOL = TRUE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &enable as *const BOOL as _,
            std::mem::size_of::<BOOL>() as u32,
        );

        // Undocumented path: per-window dark mode opt-in. Required for
        // themed common controls to draw in dark mode when hosted
        // inside this specific HWND.
        let dll = uxtheme();
        if dll.is_null() {
            return;
        }

        // Ordinal 133: AllowDarkModeForWindow(HWND, BOOL) -> BOOL
        if let Some(fn_ptr) = GetProcAddress(dll, 133usize as *const u8) {
            type AllowDarkModeForWindow = unsafe extern "system" fn(HWND, BOOL) -> BOOL;
            let allow_dark_mode_for_window: AllowDarkModeForWindow = std::mem::transmute(fn_ptr);
            let _ = allow_dark_mode_for_window(hwnd, TRUE);
        }
    }
}
