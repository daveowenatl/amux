fn main() {
    // Embed the amux icon into the Windows .exe so it shows in
    // Explorer, the taskbar, and Alt+Tab. No-op on other platforms.
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("../../packaging/msix/Assets/amux.ico");
        if let Err(e) = res.compile() {
            eprintln!("cargo:warning=winres failed: {e}");
        }
    }
}
