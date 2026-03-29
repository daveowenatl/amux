mod managed_pane;
mod menu_bar;
mod sidebar;
mod system_notify;
mod theme;

use std::collections::HashMap;
use std::io::Read;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use amux_ipc::IpcCommand;
use amux_layout::{NavDirection, PaneId, PaneTree, SplitDirection};
use amux_notify::{
    flash_alpha, FlashReason, NotificationSource, NotificationStore, FLASH_DURATION,
};
use amux_session::SessionData;
use amux_term::color::resolve_color;
use amux_term::config::AmuxTermConfig;
use amux_term::font;
use amux_term::osc::NotificationEvent;
use amux_term::pane::TerminalPane;
use managed_pane::*;
use portable_pty::CommandBuilder;
use wezterm_surface::{CursorShape, CursorVisibility};
use wezterm_term::color::SrgbaTuple;

#[cfg(feature = "gpu-renderer")]
use amux_render_gpu::GpuRenderer;

/// Try to get the current working directory of a process by PID.
/// Falls back across platform-specific mechanisms.
#[allow(unused_variables)]
fn get_cwd_from_pid(pid: u32) -> Option<String> {
    // Linux: readlink /proc/{pid}/cwd
    #[cfg(target_os = "linux")]
    {
        let link = std::fs::read_link(format!("/proc/{}/cwd", pid)).ok()?;
        return Some(link.to_string_lossy().to_string());
    }

    // macOS: use lsof to query the cwd
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("lsof")
            .args(["-a", "-d", "cwd", "-p", &pid.to_string(), "-Fn"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        // lsof -Fn outputs lines like "p1234\nn/path/to/dir"
        for line in text.lines() {
            if let Some(path) = line.strip_prefix('n') {
                if path.starts_with('/') {
                    return Some(path.to_string());
                }
            }
        }
        return None;
    }

    // Windows / other: no fallback yet
    #[allow(unreachable_code)]
    None
}

const DEFAULT_SIDEBAR_WIDTH: f32 = 200.0;
const TAB_BAR_HEIGHT: f32 = 24.0;
/// Content top inset: tab bar height + 1px border between tab bar and content.
const TAB_CONTENT_TOP_INSET: f32 = TAB_BAR_HEIGHT + 1.0;
/// Visual padding above the tab bar. On macOS with fullSizeContentView,
/// this covers the native title bar area where traffic light buttons sit.
const TERMINAL_TOP_PAD: f32 = 28.0;
/// Visual padding below the terminal grid (does not reduce PTY rows).
/// Painted with terminal background color so it blends with the terminal.
const TERMINAL_BOTTOM_PAD: f32 = 4.0;

// ---------------------------------------------------------------------------
// App config (loaded from ~/.config/amux/config.toml)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
#[serde(default)]
struct AppConfig {
    font_size: f32,
    /// Font family for terminal text (e.g. "JetBrains Mono", "Menlo").
    /// Resolved against system-installed fonts by cosmic-text.
    font_family: String,
    notifications: NotificationConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            font_size: font::DEFAULT_FONT_SIZE,
            font_family: font::DEFAULT_FONT_FAMILY.to_owned(),
            notifications: NotificationConfig::default(),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(default)]
struct NotificationConfig {
    /// Deliver OS-native toast notifications when the app is unfocused.
    system_notifications: bool,
    /// Automatically move workspaces to the top of the sidebar on notification.
    auto_reorder_workspaces: bool,
    /// Show unread count on macOS dock icon / Windows taskbar.
    dock_badge: bool,
    /// Shell command to run on each notification (receives AMUX_NOTIFICATION_* env vars).
    custom_command: Option<String>,
    /// Notification sound settings.
    sound: NotificationSoundConfig,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            system_notifications: true,
            auto_reorder_workspaces: true,
            dock_badge: true,
            custom_command: None,
            sound: NotificationSoundConfig::default(),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(default)]
struct NotificationSoundConfig {
    /// "system", "none", or path to a .wav/.ogg/.mp3 file.
    sound: String,
    /// Play sound even when app is focused (suppressed notification feedback).
    play_when_focused: bool,
}

impl Default for NotificationSoundConfig {
    fn default() -> Self {
        Self {
            sound: "system".to_string(),
            play_when_focused: true,
        }
    }
}

fn load_app_config() -> AppConfig {
    let config_path = if cfg!(target_os = "windows") {
        dirs::config_dir().map(|d| d.join("amux").join("config.toml"))
    } else {
        dirs::home_dir().map(|d| d.join(".config").join("amux").join("config.toml"))
    };

    if let Some(path) = config_path {
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str::<AppConfig>(&contents) {
                Ok(mut config) => {
                    tracing::info!("Loaded config from {}", path.display());
                    config.font_size = validate_font_size(config.font_size);
                    // Trim whitespace; treat empty as default.
                    config.font_family = config.font_family.trim().to_owned();
                    if config.font_family.is_empty() {
                        config.font_family = font::DEFAULT_FONT_FAMILY.to_owned();
                    }
                    return config;
                }
                Err(e) => {
                    tracing::warn!("Failed to parse {}: {}", path.display(), e);
                }
            },
            Err(_) => {
                tracing::debug!("No config file at {}", path.display());
            }
        }
    }

    AppConfig::default()
}

fn validate_font_size(size: f32) -> f32 {
    const MIN_FONT_SIZE: f32 = 4.0;
    const MAX_FONT_SIZE: f32 = 96.0;
    if !size.is_finite() || size <= 0.0 {
        font::DEFAULT_FONT_SIZE
    } else {
        size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    }
}

/// Cached font family for semibold/bold UI text (sidebar titles, active tabs).
/// Uses a static Arc<str> to avoid allocating on every call.
pub(crate) fn bold_font(size: f32) -> egui::FontId {
    use std::sync::LazyLock;
    static BOLD_FAMILY: LazyLock<egui::FontFamily> =
        LazyLock::new(|| egui::FontFamily::Name("Bold".into()));
    egui::FontId::new(size, BOLD_FAMILY.clone())
}

/// Load system fonts as fallbacks to egui's font families.
/// This provides coverage for braille patterns, geometric shapes, and other symbols
/// that egui's bundled Hack font doesn't include.
///
/// Also registers bundled IBM Plex Sans (Regular + SemiBold) for consistent
/// cross-platform UI text, and a custom "Bold" font family for titles.
fn install_system_font_fallback(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut loaded = Vec::new();

    // --- Bundled IBM Plex Sans for UI chrome ---
    fonts.font_data.insert(
        "plex_sans".to_owned(),
        egui::FontData::from_static(font::SANS_REGULAR).into(),
    );
    fonts.font_data.insert(
        "plex_sans_semibold".to_owned(),
        egui::FontData::from_static(font::SANS_SEMIBOLD).into(),
    );

    // Platform-specific font candidates: (path, name, is_symbol_font, is_proportional)
    // We try to load a monospace font + a symbols font for maximum coverage.
    let candidates: &[(&str, &str, bool, bool)] = if cfg!(target_os = "macos") {
        &[
            // SF Pro (system font) for UI chrome text
            ("/System/Library/Fonts/SFNS.ttf", "sf_pro", false, true),
            // SF Mono: single .ttf with good Unicode coverage
            (
                "/System/Library/Fonts/SFNSMono.ttf",
                "sf_mono",
                false,
                false,
            ),
            // Apple Symbols: broad Unicode symbol coverage
            (
                "/System/Library/Fonts/Apple Symbols.ttf",
                "apple_symbols",
                true,
                false,
            ),
            // Supplemental Andale Mono as another option
            (
                "/System/Library/Fonts/Supplemental/Andale Mono.ttf",
                "andale_mono",
                false,
                false,
            ),
        ]
    } else if cfg!(target_os = "windows") {
        &[
            ("C:\\Windows\\Fonts\\segoeui.ttf", "segoe_ui", false, true),
            ("C:\\Windows\\Fonts\\consola.ttf", "consolas", false, false),
            (
                "C:\\Windows\\Fonts\\segmdl2.ttf",
                "segoe_symbols",
                true,
                false,
            ),
        ]
    } else {
        &[
            (
                "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
                "dejavu_mono",
                false,
                false,
            ),
            (
                "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
                "dejavu_mono",
                false,
                false,
            ),
        ]
    };

    // Bold system font candidates: (path, name)
    // Loaded separately so they can be added to the "Bold" family.
    let bold_candidates: &[(&str, &str)] = if cfg!(target_os = "macos") {
        // macOS provides bold weights / variable font axes for SF Pro, but we
        // intentionally avoid hard-coding those paths or relying on axis selection.
        // Bundled Plex Sans SemiBold is used instead for bold UI text.
        &[]
    } else if cfg!(target_os = "windows") {
        &[("C:\\Windows\\Fonts\\segoeuib.ttf", "segoe_ui_bold")]
    } else {
        &[(
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "dejavu_bold",
        )]
    };

    let mut proportional_fonts = Vec::new();
    let mut mono_fonts = Vec::new();
    let mut bold_fonts = Vec::new();

    for &(path, name, _is_symbol, is_proportional) in candidates {
        if fonts.font_data.contains_key(name) {
            continue;
        }
        match std::fs::read(path) {
            Ok(data) => {
                fonts
                    .font_data
                    .insert(name.to_owned(), egui::FontData::from_owned(data).into());
                loaded.push(name);
                if is_proportional {
                    proportional_fonts.push(name);
                } else {
                    mono_fonts.push(name);
                }
            }
            Err(e) => {
                tracing::debug!("Font fallback not found: {} ({})", path, e);
            }
        }
    }

    for &(path, name) in bold_candidates {
        if fonts.font_data.contains_key(name) {
            continue;
        }
        match std::fs::read(path) {
            Ok(data) => {
                fonts
                    .font_data
                    .insert(name.to_owned(), egui::FontData::from_owned(data).into());
                bold_fonts.push(name);
                loaded.push(name);
            }
            Err(e) => {
                tracing::debug!("Bold font not found: {} ({})", path, e);
            }
        }
    }

    // Proportional fonts: bundled Plex Sans first, then system UI font, then mono
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "plex_sans".to_owned());
        for name in &proportional_fonts {
            family.insert(1, (*name).to_owned());
        }
        for name in &mono_fonts {
            family.push((*name).to_owned());
        }
    }
    // Monospace fonts: add all as fallbacks
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        for name in &loaded {
            family.push((*name).to_owned());
        }
    }

    // "Bold" family: bundled Plex Sans SemiBold first, then system bold fonts,
    // then fall back to all proportional + mono fonts for symbol coverage.
    let mut bold_family = vec!["plex_sans_semibold".to_owned()];
    for name in &bold_fonts {
        bold_family.push((*name).to_owned());
    }
    for name in &proportional_fonts {
        bold_family.push((*name).to_owned());
    }
    for name in &mono_fonts {
        bold_family.push((*name).to_owned());
    }
    // Include egui's bundled fonts as final fallback
    if let Some(prop) = fonts.families.get(&egui::FontFamily::Proportional) {
        for name in prop {
            if !bold_family.contains(name) {
                bold_family.push(name.clone());
            }
        }
    }
    fonts
        .families
        .insert(egui::FontFamily::Name("Bold".into()), bold_family);

    if loaded.is_empty() {
        tracing::warn!("No system font fallbacks found; egui may miss symbol/emoji coverage");
    }

    tracing::info!("Loaded font fallbacks: {:?}", loaded);
    ctx.set_fonts(fonts);
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let app_config = load_app_config();
    let font_size = app_config.font_size;
    // FontConfig is only consumed by the GPU renderer; gate to avoid unused
    // warnings in non-GPU builds. font_family is GPU-only — the egui fallback
    // renderer uses its built-in monospace font.
    #[cfg(feature = "gpu-renderer")]
    let font_config = font::FontConfig {
        family: app_config.font_family.clone(),
        size: app_config.font_size,
    };

    // Initialize sound player with configured sound setting
    let mut sound_player = system_notify::SoundPlayer::new();
    if let Some(player) = &mut sound_player {
        player.configure(&app_config.notifications.sound.sound);
    }

    let (ipc_rx, ipc_addr) = amux_ipc::start_server()?;
    tracing::info!("IPC server: {}", ipc_addr);

    let theme = theme::Theme::default();
    let mut term_config = AmuxTermConfig::default();
    theme.apply_to_palette(&mut term_config.color_palette);
    let config = Arc::new(term_config);

    // Try to restore a previous session
    let restored = match amux_session::load() {
        Ok(Some(session)) => {
            tracing::info!("Restoring session from {}", session.saved_at);
            Some(session)
        }
        Ok(None) => None,
        Err(e) => {
            tracing::warn!("Failed to load session, starting fresh: {}", e);
            None
        }
    };

    let state = if let Some(session) = restored {
        restore_session(&session, &ipc_addr, &config)
    } else {
        fresh_startup(&ipc_addr, &config)?
    };

    // Force dark appearance on macOS so the title bar matches the app's dark chrome.
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSAppearance, NSApplication};
        use objc2_foundation::{MainThreadMarker, NSString};

        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            let dark =
                NSAppearance::appearanceNamed(&NSString::from_str("NSAppearanceNameDarkAqua"));
            app.setAppearance(dark.as_deref());
        }
    }

    // Build the native menu bar (cross-platform via muda).
    let menu = menu_bar::build();

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1000.0, 600.0])
        .with_title("amux");
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);

    let options = eframe::NativeOptions {
        viewport,
        // Suppress winit's default macOS menu (we provide our own via muda).
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            builder.with_default_menu(false);
        })),
        ..Default::default()
    };

    let ipc_addr_cleanup = ipc_addr.clone();
    let result = eframe::run_native(
        "amux",
        options,
        Box::new(move |_cc| {
            // Add system monospace font as fallback for braille/symbol coverage
            install_system_font_fallback(&_cc.egui_ctx);

            // Hide the panel resize handle entirely (cursor still changes on hover).
            _cc.egui_ctx.style_mut(|style| {
                style.visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::TRANSPARENT;
                style.visuals.widgets.inactive.fg_stroke.color = egui::Color32::TRANSPARENT;
                style.visuals.widgets.hovered.fg_stroke.color = egui::Color32::TRANSPARENT;
                style.visuals.widgets.active.fg_stroke.color = egui::Color32::TRANSPARENT;
            });

            #[cfg(feature = "gpu-renderer")]
            let gpu_renderer = _cc.wgpu_render_state.as_ref().map(|rs| {
                tracing::info!("GPU renderer initialized (wgpu backend)");
                GpuRenderer::new(rs.clone(), &font_config)
            });

            Ok(Box::new(AmuxApp {
                workspaces: state.workspaces,
                active_workspace_idx: state.active_workspace_idx,
                panes: state.panes,
                next_pane_id: state.next_pane_id,
                next_workspace_id: state.next_workspace_id,
                next_surface_id: state.next_surface_id,
                sidebar: state.sidebar,
                ipc_rx,
                socket_addr: ipc_addr,
                config,
                theme,
                last_panel_rect: None,
                notifications: state.notifications,
                show_notification_panel: false,
                last_click_time: Instant::now(),
                last_click_pos: egui::Pos2::ZERO,
                click_count: 0,
                wants_exit: false,
                font_size,
                find_state: None,
                copy_mode: None,
                hovered_hyperlink: None,
                ime_preedit: None,
                selection_changed: false,
                tab_drag: None,
                rename_modal: None,
                app_focused: true,
                app_config,
                system_notifier: system_notify::SystemNotifier::new(),
                last_badge_count: 0,
                sound_player,
                menu,
                #[cfg(target_os = "windows")]
                menu_attached: false,
                #[cfg(feature = "gpu-renderer")]
                gpu_renderer,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{}", e));

    cleanup_addr(&ipc_addr_cleanup);
    result
}

/// Bundled startup state to avoid complex return tuples.
struct StartupState {
    workspaces: Vec<Workspace>,
    active_workspace_idx: usize,
    panes: HashMap<PaneId, ManagedPane>,
    next_pane_id: PaneId,
    next_workspace_id: u64,
    next_surface_id: u64,
    sidebar: SidebarState,
    notifications: NotificationStore,
}

/// Create a fresh default startup (one workspace, one pane).
fn fresh_startup(
    ipc_addr: &amux_ipc::IpcAddr,
    config: &Arc<AmuxTermConfig>,
) -> anyhow::Result<StartupState> {
    let initial_pane_id: PaneId = 0;
    let surface = spawn_surface(80, 24, ipc_addr, config, 0, 0, None, None)?;

    let managed = ManagedPane {
        surfaces: vec![surface],
        active_surface_idx: 0,
        selection: None,
    };

    let mut panes = HashMap::new();
    panes.insert(initial_pane_id, managed);

    let workspace = Workspace {
        id: 0,
        title: "Terminal 1".to_string(),
        tree: PaneTree::new(initial_pane_id),
        focused_pane: initial_pane_id,
        zoomed: None,
        dragging_divider: None,
        last_pane_sizes: HashMap::new(),
        color: None,
    };

    Ok(StartupState {
        workspaces: vec![workspace],
        active_workspace_idx: 0,
        panes,
        next_pane_id: 1,
        next_workspace_id: 1,
        next_surface_id: 1,
        sidebar: SidebarState {
            visible: true,
            width: DEFAULT_SIDEBAR_WIDTH,
            drag: None,
        },
        notifications: NotificationStore::new(),
    })
}

/// Restore app state from a saved session. Falls back to fresh startup on any failure.
fn restore_session(
    session: &SessionData,
    ipc_addr: &amux_ipc::IpcAddr,
    config: &Arc<AmuxTermConfig>,
) -> StartupState {
    let mut workspaces = Vec::new();
    let mut panes: HashMap<PaneId, ManagedPane> = HashMap::new();

    for saved_ws in &session.workspaces {
        for (&pane_id, saved_pane) in &saved_ws.panes {
            let mut surfaces = Vec::new();
            for saved_sf in &saved_pane.surfaces {
                let cwd = saved_sf.working_dir.as_deref();
                let scrollback = if saved_sf.scrollback.is_empty() {
                    None
                } else {
                    Some(saved_sf.scrollback.as_str())
                };

                match spawn_surface(
                    saved_sf.cols,
                    saved_sf.rows,
                    ipc_addr,
                    config,
                    saved_ws.id,
                    saved_sf.id,
                    cwd,
                    scrollback,
                ) {
                    Ok(mut surface) => {
                        // Restore git/PR metadata from session
                        surface.metadata.git_branch = saved_sf.git_branch.clone();
                        surface.metadata.git_dirty = saved_sf.git_dirty;
                        surface.metadata.pr_number = saved_sf.pr_number;
                        surface.metadata.pr_title = saved_sf.pr_title.clone();
                        surface.metadata.pr_state = saved_sf.pr_state.clone();
                        surface.user_title = saved_sf.user_title.clone();
                        surfaces.push(surface);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to restore surface {} in pane {}: {}",
                            saved_sf.id,
                            pane_id,
                            e
                        );
                    }
                }
            }

            if surfaces.is_empty() {
                tracing::warn!("All surfaces failed for pane {}, skipping", pane_id);
                continue;
            }

            let active_idx = saved_pane
                .active_surface_idx
                .min(surfaces.len().saturating_sub(1));
            panes.insert(
                pane_id,
                ManagedPane {
                    surfaces,
                    active_surface_idx: active_idx,
                    selection: None,
                },
            );
        }

        // Verify all pane IDs in the tree were actually restored
        let tree_pane_ids = saved_ws.tree.iter_panes();
        let all_panes_restored = tree_pane_ids.iter().all(|id| panes.contains_key(id));
        if !all_panes_restored {
            tracing::warn!(
                "Skipping workspace {} (tree references missing panes)",
                saved_ws.title
            );
            // Clean up any panes we did restore for this workspace
            for id in &tree_pane_ids {
                panes.remove(id);
            }
            continue;
        }

        let focused = if panes.contains_key(&saved_ws.focused_pane) {
            saved_ws.focused_pane
        } else {
            *tree_pane_ids.first().unwrap_or(&0)
        };

        workspaces.push(Workspace {
            id: saved_ws.id,
            title: saved_ws.title.clone(),
            tree: saved_ws.tree.clone(),
            focused_pane: focused,
            zoomed: saved_ws.zoomed.filter(|z| panes.contains_key(z)),
            dragging_divider: None,
            last_pane_sizes: HashMap::new(),
            color: saved_ws.color,
        });
    }

    // If nothing restored, fall back to fresh
    if workspaces.is_empty() {
        tracing::warn!("Session restore produced no workspaces, starting fresh");
        return match fresh_startup(ipc_addr, config) {
            Ok(result) => result,
            Err(e) => {
                tracing::error!("Fresh startup also failed: {}", e);
                panic!("Cannot start amux: {}", e);
            }
        };
    }

    let sidebar = SidebarState {
        visible: session.sidebar.visible,
        width: session.sidebar.width,
        drag: None,
    };

    // Restore only already-read notifications (unread ones are stale — the
    // agent that created them is gone after restart).
    let mut store = NotificationStore::new();
    for saved_n in &session.notifications {
        if !saved_n.read {
            continue;
        }
        let source = match saved_n.source.as_str() {
            "toast" => NotificationSource::Toast,
            "bell" => NotificationSource::Bell,
            _ => NotificationSource::Cli,
        };
        store.push_read(
            saved_n.workspace_id,
            saved_n.pane_id,
            saved_n.surface_id,
            saved_n.title.clone(),
            saved_n.body.clone(),
            source,
        );
    }

    // Don't restore workspace statuses — agent processes don't survive restart,
    // so any Active/Waiting state would be stale. They start as Idle implicitly.

    let active_idx = session
        .active_workspace_idx
        .min(workspaces.len().saturating_sub(1));

    StartupState {
        workspaces,
        active_workspace_idx: active_idx,
        panes,
        next_pane_id: session.next_pane_id,
        next_workspace_id: session.next_workspace_id,
        next_surface_id: session.next_surface_id,
        sidebar,
        notifications: store,
    }
}

fn default_shell() -> String {
    #[cfg(unix)]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
}

/// Write shell integration files to ~/.config/amux/shell/ and set env vars to
/// auto-inject them. For zsh: ZDOTDIR override. For bash: PROMPT_COMMAND bootstrap.
/// Matching cmux's approach — no user dotfile modification required.
fn inject_shell_integration(shell: &str, cmd: &mut CommandBuilder) {
    let shell_name = std::path::Path::new(shell)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");

    let Some(integration_dir) = ensure_shell_integration_dir() else {
        return;
    };

    cmd.env(
        "AMUX_SHELL_INTEGRATION_DIR",
        integration_dir.to_string_lossy().as_ref(),
    );

    match shell_name {
        "zsh" => {
            let zsh_dir = integration_dir.join("zsh");
            // Save user's original ZDOTDIR (if set) so bootstrap can restore it
            if let Ok(original) = std::env::var("ZDOTDIR") {
                cmd.env("AMUX_ZSH_ZDOTDIR", &original);
            }
            cmd.env("ZDOTDIR", zsh_dir.to_string_lossy().as_ref());
        }
        "bash" => {
            // Bootstrap integration via PROMPT_COMMAND on first interactive prompt.
            // This runs once, then restores any original PROMPT_COMMAND.
            let bash_script = integration_dir.join("amux-bash-integration.bash");
            let bootstrap = format!(
                concat!(
                    "unset PROMPT_COMMAND; ",
                    "if [[ -r \"{}\" ]]; then source \"{}\"; fi",
                ),
                bash_script.display(),
                bash_script.display(),
            );
            cmd.env("PROMPT_COMMAND", &bootstrap);
        }
        _ => {}
    }
}

/// Ensure shell integration scripts are written to ~/.config/amux/shell/.
/// Returns the directory path, or None on failure.
fn ensure_shell_integration_dir() -> Option<std::path::PathBuf> {
    let config_dir = dirs::config_dir()?.join("amux").join("shell");

    // Write zsh bootstrap files
    let zsh_dir = config_dir.join("zsh");
    if std::fs::create_dir_all(&zsh_dir).is_err() {
        return None;
    }

    // Embed integration scripts at compile time
    let files: &[(&str, &str)] = &[
        (
            "zsh/.zshenv",
            include_str!("../../../resources/shell-integration/zsh/.zshenv"),
        ),
        (
            "zsh/.zprofile",
            include_str!("../../../resources/shell-integration/zsh/.zprofile"),
        ),
        (
            "zsh/.zshrc",
            include_str!("../../../resources/shell-integration/zsh/.zshrc"),
        ),
        (
            "zsh/.zlogin",
            include_str!("../../../resources/shell-integration/zsh/.zlogin"),
        ),
        (
            "amux-zsh-integration.zsh",
            include_str!("../../../resources/shell-integration/amux-zsh-integration.zsh"),
        ),
        (
            "amux-bash-integration.bash",
            include_str!("../../../resources/shell-integration/amux-bash-integration.bash"),
        ),
    ];

    for (name, content) in files {
        let path = config_dir.join(name);
        // Only write if content changed (avoid unnecessary disk writes)
        let needs_write = std::fs::read_to_string(&path)
            .map(|existing| existing != *content)
            .unwrap_or(true);
        if needs_write && std::fs::write(&path, content).is_err() {
            return None;
        }
    }

    Some(config_dir)
}

/// Ensure the claude wrapper script is written to ~/.config/amux/bin/claude.
/// Returns the bin directory path, or None on failure.
fn ensure_claude_wrapper_dir() -> Option<std::path::PathBuf> {
    // Use ~/.config/amux/bin/ instead of dirs::config_dir() because on macOS
    // that returns ~/Library/Application Support/ which has a space — spaces
    // in PATH entries break many tools.
    let bin_dir = dirs::home_dir()?.join(".config").join("amux").join("bin");
    if std::fs::create_dir_all(&bin_dir).is_err() {
        return None;
    }

    let wrapper_path = bin_dir.join("claude");
    let wrapper_content = include_str!("../../../resources/bin/claude");

    let needs_write = std::fs::read_to_string(&wrapper_path)
        .map(|existing| existing != wrapper_content)
        .unwrap_or(true);

    if needs_write && std::fs::write(&wrapper_path, wrapper_content).is_err() {
        return None;
    }
    // Make executable (unix)
    #[cfg(unix)]
    if needs_write {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&wrapper_path, std::fs::Permissions::from_mode(0o755));
    }

    Some(bin_dir)
}
fn cleanup_addr(addr: &amux_ipc::IpcAddr) {
    match addr {
        #[cfg(unix)]
        amux_ipc::IpcAddr::Unix(path) => {
            let _ = std::fs::remove_file(path);
        }
        #[cfg(windows)]
        amux_ipc::IpcAddr::NamedPipe(_) => {}
    }
}

// --- Data Model ---
// Hierarchy: Workspace > PaneTree (splits) > Pane (each has tab bar) > Surface (terminal tab)
// Core pane types (ManagedPane, PaneSurface, SurfaceMetadata, SelectionState, etc.)
// live in managed_pane.rs.

/// A workspace shown in the sidebar. Owns the split tree.
pub(crate) struct Workspace {
    pub(crate) id: u64,
    pub(crate) title: String,
    pub(crate) tree: PaneTree,
    pub(crate) focused_pane: PaneId,
    pub(crate) zoomed: Option<PaneId>,
    pub(crate) dragging_divider: Option<DragState>,
    pub(crate) last_pane_sizes: HashMap<PaneId, (usize, usize)>,
    /// Optional workspace color for sidebar indicator.
    pub(crate) color: Option<[u8; 4]>,
}

pub(crate) struct SidebarState {
    pub(crate) visible: bool,
    pub(crate) width: f32,
    /// Drag reorder state.
    pub(crate) drag: Option<SidebarDragState>,
}

pub(crate) struct SidebarDragState {
    /// Index of workspace being dragged.
    pub(crate) source_idx: usize,
    /// Current pointer Y position.
    pub(crate) current_y: f32,
    /// Computed drop target index.
    pub(crate) drop_target_idx: usize,
    /// Y midpoints of each row for computing drop position.
    pub(crate) row_midpoints: Vec<f32>,
}

struct DragState {
    node_path: Vec<usize>,
    direction: SplitDirection,
}

#[allow(clippy::too_many_arguments)]
fn spawn_surface(
    cols: u16,
    rows: u16,
    ipc_addr: &amux_ipc::IpcAddr,
    config: &Arc<AmuxTermConfig>,
    workspace_id: u64,
    surface_id: u64,
    cwd: Option<&str>,
    scrollback: Option<&str>,
) -> anyhow::Result<PaneSurface> {
    let shell = default_shell();
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("AMUX_SOCKET_PATH", ipc_addr.to_string());
    cmd.env("AMUX_WORKSPACE_ID", workspace_id.to_string());
    cmd.env("AMUX_SURFACE_ID", surface_id.to_string());
    cmd.env("TERM", "xterm-256color");

    // Point AMUX_BIN to the CLI binary so shell integration scripts can invoke it
    // without relying on PATH (macOS path_helper in /etc/zprofile rebuilds PATH).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let cli_bin = exe_dir.join("amux");
            if cli_bin.exists() {
                cmd.env("AMUX_BIN", cli_bin.to_string_lossy().as_ref());
            }
        }
    }

    // Prepend amux bin dir (containing claude wrapper) to PATH so hooks are
    // injected at runtime via --settings, scoped to amux sessions only.
    if let Some(bin_dir) = ensure_claude_wrapper_dir() {
        let current_path = std::env::var("PATH").unwrap_or_default();
        let bin_str = bin_dir.to_string_lossy();
        if !current_path.split(':').any(|d| d == bin_str.as_ref()) {
            let sep = if current_path.is_empty() { "" } else { ":" };
            cmd.env("PATH", format!("{bin_str}{sep}{current_path}"));
        }
    }

    // Auto-inject shell integration (matching cmux's ZDOTDIR/PROMPT_COMMAND approach)
    inject_shell_integration(&shell, &mut cmd);

    let actual_cwd = if let Some(dir) = cwd {
        let path = std::path::Path::new(dir);
        if path.is_dir() {
            cmd.cwd(path);
            Some(dir.to_string())
        } else {
            tracing::warn!("Saved working dir no longer exists: {}", dir);
            if let Some(home) = dirs::home_dir() {
                cmd.cwd(&home);
                Some(home.to_string_lossy().to_string())
            } else {
                None
            }
        }
    } else {
        None
    };

    let mut pane = TerminalPane::spawn(cols, rows, cmd, config.clone())?;

    // Inject scrollback text before starting the reader thread.
    // feed_bytes writes directly to the terminal state machine, not through the PTY.
    if let Some(text) = scrollback {
        if !text.is_empty() {
            // Convert \n to \r\n for terminal processing, avoiding extra trailing blank line.
            let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
            let trimmed = normalized.trim_end_matches('\n');
            let buffer = trimmed.replace('\n', "\r\n");
            if !buffer.is_empty() {
                pane.feed_bytes(buffer.as_bytes());
                pane.feed_bytes(b"\r\n");
            }
        }
    }

    let mut reader = pane.take_reader().expect("reader already taken");
    let (byte_tx, byte_rx) = mpsc::sync_channel::<Vec<u8>>(64);

    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if byte_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Initialize metadata with the actual CWD used (may differ from saved if dir was removed)
    let metadata = SurfaceMetadata {
        cwd: actual_cwd,
        ..Default::default()
    };

    Ok(PaneSurface {
        id: surface_id,
        pane,
        byte_rx,
        scroll_offset: 0,
        scroll_accum: 0.0,
        metadata,
        user_title: None,
    })
}

// --- App ---

struct AmuxApp {
    workspaces: Vec<Workspace>,
    active_workspace_idx: usize,
    panes: HashMap<PaneId, ManagedPane>,
    next_pane_id: PaneId,
    next_workspace_id: u64,
    next_surface_id: u64,
    sidebar: SidebarState,
    ipc_rx: std::sync::mpsc::Receiver<IpcCommand>,
    socket_addr: amux_ipc::IpcAddr,
    config: Arc<AmuxTermConfig>,
    theme: theme::Theme,
    last_panel_rect: Option<egui::Rect>,
    notifications: NotificationStore,
    show_notification_panel: bool,
    last_click_time: Instant,
    last_click_pos: egui::Pos2,
    click_count: u32,
    wants_exit: bool,
    font_size: f32,
    find_state: Option<FindState>,
    copy_mode: Option<CopyModeState>,
    hovered_hyperlink: Option<String>,
    ime_preedit: Option<String>,
    /// Set during update() when selection changes; used for smart repaint.
    selection_changed: bool,
    /// Drag state for tab reordering within a pane.
    tab_drag: Option<TabDragState>,
    /// Rename modal state for workspaces and tabs.
    rename_modal: Option<RenameModal>,
    /// Whether the app window currently has OS-level focus.
    app_focused: bool,
    /// Persisted application configuration.
    app_config: AppConfig,
    /// Cross-platform system notification sender.
    system_notifier: system_notify::SystemNotifier,
    /// Cached badge count to avoid redundant dock badge updates every frame.
    last_badge_count: usize,
    /// Notification sound player (None if no audio device).
    sound_player: Option<system_notify::SoundPlayer>,
    /// Native menu bar (kept alive for the process lifetime).
    #[allow(dead_code)]
    menu: muda::Menu,
    /// Whether the menu has been attached to the window (Windows only).
    #[cfg(target_os = "windows")]
    menu_attached: bool,
    #[cfg(feature = "gpu-renderer")]
    gpu_renderer: Option<GpuRenderer>,
}

/// What is being renamed — workspace or tab (surface).
/// Uses stable IDs rather than indices so background reorder/close can't
/// cause the modal to rename the wrong item.
enum RenameTarget {
    Workspace(u64),
    Tab { pane_id: PaneId, surface_id: u64 },
}

struct RenameModal {
    target: RenameTarget,
    buf: String,
    just_opened: bool,
}

struct TabDragState {
    pane_id: PaneId,
    source_idx: usize,
    drop_target_idx: usize,
}

impl AmuxApp {
    /// Get cell dimensions in logical points, using GPU renderer measurements
    /// when available, falling back to egui font measurements.
    fn cell_dimensions(&self, ui: &egui::Ui) -> (f32, f32) {
        #[cfg(feature = "gpu-renderer")]
        if let Some(gpu) = &self.gpu_renderer {
            let cw = gpu.cell_width();
            let ch = gpu.cell_height();
            if cw > 0.0 && ch > 0.0 {
                return (cw, ch);
            }
        }
        let font_id = egui::FontId::monospace(self.font_size);
        let cell_width = ui.fonts(|f| f.glyph_width(&font_id, 'M'));
        let cell_height = ui.fonts(|f| f.row_height(&font_id));
        (cell_width, cell_height)
    }

    fn active_workspace(&self) -> &Workspace {
        &self.workspaces[self.active_workspace_idx]
    }

    fn active_workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active_workspace_idx]
    }

    fn set_focus(&mut self, pane_id: PaneId) {
        let ws = self.active_workspace_mut();
        if ws.focused_pane != pane_id {
            let old_id = ws.focused_pane;
            ws.focused_pane = pane_id;

            // Send DECSET 1004 focus-out to old pane
            if let Some(managed) = self.panes.get_mut(&old_id) {
                managed.active_surface_mut().pane.focus_changed(false);
            }
            // Send DECSET 1004 focus-in to new pane
            if let Some(managed) = self.panes.get_mut(&pane_id) {
                managed.active_surface_mut().pane.focus_changed(true);
            }

            // Clear notifications on the newly focused pane
            self.notifications.mark_pane_read(pane_id);
            // Navigation flash — but suppress if other panes have unread
            let pane_ids: Vec<u64> = self.active_workspace().tree.iter_panes();
            if !self.notifications.has_unread_excluding(&pane_ids, pane_id) {
                self.notifications
                    .flash_pane(pane_id, FlashReason::Navigation);
            }
        }
    }

    fn flash_focus(&mut self) {
        let pane_id = self.focused_pane_id();
        self.notifications
            .flash_pane(pane_id, FlashReason::Navigation);
    }

    fn focused_pane_id(&self) -> PaneId {
        self.active_workspace().focused_pane
    }

    /// Drain any pending PTY bytes into the terminal state machine so that
    /// title, working directory, and scrollback are up to date before save.
    fn flush_pending_io(&mut self) {
        for managed in self.panes.values_mut() {
            for surface in &mut managed.surfaces {
                while let Ok(bytes) = surface.byte_rx.try_recv() {
                    surface.pane.feed_bytes(&bytes);
                }
            }
        }
    }

    fn build_session_data(&self) -> SessionData {
        let mut saved_workspaces = Vec::new();
        for ws in &self.workspaces {
            let mut saved_panes = std::collections::HashMap::new();
            for &pane_id in &ws.tree.iter_panes() {
                if let Some(managed) = self.panes.get(&pane_id) {
                    let surfaces: Vec<amux_session::SavedSurface> = managed
                        .surfaces
                        .iter()
                        .map(|sf| {
                            // Prefer shell-reported CWD (metadata.cwd from set-cwd/OSC 7),
                            // then fall back to pane.working_dir() and OS-level queries.
                            let working_dir = sf.metadata.cwd.clone().or_else(|| {
                                sf.pane
                                    .working_dir()
                                    .and_then(|url| url.to_file_path().ok())
                                    .map(|p| p.to_string_lossy().to_string())
                                    .or_else(|| sf.pane.child_pid().and_then(get_cwd_from_pid))
                            });
                            let scrollback = sf.pane.read_scrollback_text(4096);
                            let (cols, rows) = sf.pane.dimensions();
                            amux_session::SavedSurface {
                                id: sf.id,
                                title: sf.pane.title().to_string(),
                                working_dir,
                                scrollback,
                                cols: cols as u16,
                                rows: rows as u16,
                                git_branch: sf.metadata.git_branch.clone(),
                                git_dirty: sf.metadata.git_dirty,
                                pr_number: sf.metadata.pr_number,
                                pr_title: sf.metadata.pr_title.clone(),
                                pr_state: sf.metadata.pr_state.clone(),
                                user_title: sf.user_title.clone(),
                            }
                        })
                        .collect();
                    saved_panes.insert(
                        pane_id,
                        amux_session::SavedManagedPane {
                            panel_type: managed.panel_type().to_string(),
                            surfaces,
                            active_surface_idx: managed.active_surface_idx,
                        },
                    );
                }
            }
            saved_workspaces.push(amux_session::SavedWorkspace {
                id: ws.id,
                title: ws.title.clone(),
                tree: ws.tree.clone(),
                focused_pane: ws.focused_pane,
                zoomed: ws.zoomed,
                panes: saved_panes,
                color: ws.color,
            });
        }

        let notifications: Vec<amux_session::SavedNotification> = self
            .notifications
            .all_notifications()
            .iter()
            .map(|n| amux_session::SavedNotification {
                id: n.id,
                workspace_id: n.workspace_id,
                pane_id: n.pane_id,
                surface_id: n.surface_id,
                title: n.title.clone(),
                body: n.body.clone(),
                source: match n.source {
                    NotificationSource::Toast => "toast",
                    NotificationSource::Bell => "bell",
                    NotificationSource::Cli => "cli",
                }
                .to_string(),
                read: n.read,
            })
            .collect();

        let workspace_statuses: std::collections::HashMap<u64, amux_session::SavedWorkspaceStatus> =
            self.workspaces
                .iter()
                .filter_map(|ws| {
                    self.notifications.workspace_status(ws.id).map(|status| {
                        (
                            ws.id,
                            amux_session::SavedWorkspaceStatus {
                                state: match status.state {
                                    amux_notify::AgentState::Active => "active",
                                    amux_notify::AgentState::Waiting => "waiting",
                                    amux_notify::AgentState::Idle => "idle",
                                }
                                .to_string(),
                                label: status.label.clone(),
                                // task/message are transient agent state — don't persist
                                task: None,
                                message: None,
                            },
                        )
                    })
                })
                .collect();

        SessionData {
            version: 1,
            saved_at: chrono_now(),
            workspaces: saved_workspaces,
            active_workspace_idx: self.active_workspace_idx,
            next_pane_id: self.next_pane_id,
            next_workspace_id: self.next_workspace_id,
            next_surface_id: self.next_surface_id,
            sidebar: amux_session::SavedSidebar {
                visible: self.sidebar.visible,
                width: self.sidebar.width,
            },
            notifications,
            workspace_statuses,
        }
    }
}

/// Simple ISO 8601 timestamp without a chrono dependency.
fn chrono_now() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    // Approximate UTC datetime (good enough for session metadata)
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let mins = (time_secs % 3600) / 60;
    let s = time_secs % 60;
    // Simple days-since-epoch to date (approximate, ignoring leap seconds)
    let mut y = 1970i64;
    let mut remaining_days = days as i64;
    loop {
        let year_days = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
        if remaining_days < year_days {
            break;
        }
        remaining_days -= year_days;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining_days < md as i64 {
            m = i + 1;
            break;
        }
        remaining_days -= md as i64;
    }
    let d = remaining_days + 1;
    format!("{y:04}-{m:02}-{d:02}T{hours:02}:{mins:02}:{s:02}Z")
}

impl eframe::App for AmuxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.wants_exit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Attach native menu bar to the window (Windows: per-HWND).
        // Retries each frame until the HWND is available.
        #[cfg(target_os = "windows")]
        if !self.menu_attached {
            self.menu_attached = menu_bar::attach_to_window(&self.menu, _frame);
        }

        self.selection_changed = false;
        self.app_focused = ctx.input(|i| i.focused);

        // Drain PTY output from all surfaces in all panes
        let mut got_data = false;
        for managed in self.panes.values_mut() {
            for surface in &mut managed.surfaces {
                while let Ok(bytes) = surface.byte_rx.try_recv() {
                    got_data = true;
                    surface.pane.feed_bytes(&bytes);
                }
            }
        }

        // Handle clicks on system notifications (navigate to workspace/pane).
        // Process before draining new notifications so focus state is current.
        for action in self.system_notifier.drain_actions() {
            if let Some(idx) = self
                .workspaces
                .iter()
                .position(|ws| ws.id == action.workspace_id)
            {
                self.active_workspace_idx = idx;
                let ws = &mut self.workspaces[idx];
                if ws.tree.iter_panes().contains(&action.pane_id) {
                    ws.focused_pane = action.pane_id;
                }
                self.notifications.mark_pane_read(action.pane_id);
            }
        }

        // Drain notification events from all surfaces
        self.drain_notifications();

        // Update dock/taskbar badge with total unread count (only when changed)
        if self.app_config.notifications.dock_badge {
            let count = self.notifications.total_unread();
            if count != self.last_badge_count {
                self.last_badge_count = count;
                system_notify::set_badge_count(count);
            }
        }

        // Process IPC commands
        self.process_ipc_commands();

        // Handle keyboard shortcuts BEFORE terminal input
        let shortcut_consumed = self.handle_shortcuts(ctx);

        // Drain native menu bar actions
        self.handle_menu_actions();

        // Handle keyboard/paste input -> focused pane's active surface only
        // (blocked during copy mode — all keys go through handle_copy_mode_key)
        let mut sent_input = false;
        if !shortcut_consumed
            && self.copy_mode.is_none()
            && self.rename_modal.is_none()
            && self.find_state.is_none()
        {
            sent_input = self.handle_input(ctx);
        }

        // Render sidebar
        if self.sidebar.visible {
            // Build workspace metadata map for sidebar display
            let workspace_metadata: std::collections::HashMap<u64, SurfaceMetadata> = self
                .workspaces
                .iter()
                .map(|ws| (ws.id, self.workspace_metadata(ws)))
                .collect();
            let sidebar_actions = sidebar::render_sidebar(
                ctx,
                &mut self.sidebar,
                &self.workspaces,
                self.active_workspace_idx,
                &self.notifications,
                &workspace_metadata,
                &self.theme,
            );
            for action in sidebar_actions {
                match action {
                    sidebar::SidebarAction::SwitchWorkspace(idx) => {
                        self.active_workspace_idx = idx;
                        // Mark notifications read when switching to a workspace
                        if idx < self.workspaces.len() {
                            let pane_ids: Vec<u64> = self.workspaces[idx].tree.iter_panes();
                            self.notifications.mark_workspace_read(&pane_ids);
                        }
                    }
                    sidebar::SidebarAction::CreateWorkspace => {
                        self.create_workspace(None);
                    }
                    sidebar::SidebarAction::CloseWorkspace(idx) => {
                        self.close_workspace_at(idx);
                    }
                    sidebar::SidebarAction::StartRenameWorkspace(idx) => {
                        if idx < self.workspaces.len() {
                            let ws_id = self.workspaces[idx].id;
                            self.rename_modal = Some(RenameModal {
                                target: RenameTarget::Workspace(ws_id),
                                buf: self.workspaces[idx].title.clone(),
                                just_opened: true,
                            });
                        }
                    }
                    sidebar::SidebarAction::MarkWorkspaceRead(idx) => {
                        if idx < self.workspaces.len() {
                            let pane_ids: Vec<u64> = self.workspaces[idx].tree.iter_panes();
                            self.notifications.mark_workspace_read(&pane_ids);
                        }
                    }
                    sidebar::SidebarAction::ReorderWorkspace(from, to) => {
                        if from < self.workspaces.len() && to <= self.workspaces.len() {
                            let ws = self.workspaces.remove(from);
                            // After removal, adjust insertion index for the shift
                            let insert_idx = if from < to {
                                (to - 1).min(self.workspaces.len())
                            } else {
                                to.min(self.workspaces.len())
                            };
                            self.workspaces.insert(insert_idx, ws);
                            if self.active_workspace_idx == from {
                                self.active_workspace_idx = insert_idx;
                            } else if from < self.active_workspace_idx
                                && insert_idx >= self.active_workspace_idx
                            {
                                self.active_workspace_idx -= 1;
                            } else if from > self.active_workspace_idx
                                && insert_idx <= self.active_workspace_idx
                            {
                                self.active_workspace_idx += 1;
                            }
                        }
                    }
                    sidebar::SidebarAction::SetWorkspaceColor(idx, color) => {
                        if idx < self.workspaces.len() {
                            self.workspaces[idx].color = color;
                        }
                    }
                }
            }
        }

        // Render main content
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let full_rect = ui.available_rect_before_wrap();
                // Paint top padding strip with titlebar color.
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(
                        full_rect.min,
                        egui::pos2(full_rect.max.x, full_rect.min.y + TERMINAL_TOP_PAD),
                    ),
                    0.0,
                    self.theme.titlebar_bg(),
                );
                // Shift content area down by the top padding.
                let panel_rect = egui::Rect::from_min_max(
                    egui::pos2(full_rect.min.x, full_rect.min.y + TERMINAL_TOP_PAD),
                    full_rect.max,
                );
                self.last_panel_rect = Some(panel_rect);

                // Handle divider dragging
                self.handle_divider_drag(ui, panel_rect);

                let zoomed = self.active_workspace().zoomed;
                if let Some(zoomed_id) = zoomed {
                    // Zoomed mode: render single pane fullscreen
                    let content_rect = egui::Rect::from_min_max(
                        egui::pos2(panel_rect.min.x, panel_rect.min.y + TAB_BAR_HEIGHT),
                        egui::pos2(panel_rect.max.x, panel_rect.max.y - TERMINAL_BOTTOM_PAD),
                    );
                    let sel_changed = self.handle_selection_mouse(ui, zoomed_id, content_rect);
                    if sel_changed {
                        self.selection_changed = true;
                    }
                    self.render_single_pane(ui, zoomed_id, panel_rect, true);
                    self.resize_pane_if_needed(zoomed_id, panel_rect, ui);
                } else {
                    // Normal mode: render all panes at computed rects
                    let layout = self.active_workspace().tree.layout(panel_rect);
                    let focused = self.focused_pane_id();

                    // Click-to-focus + selection start
                    if ui.input(|i| i.pointer.primary_pressed()) {
                        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                            for &(id, rect) in &layout {
                                if rect.contains(pos) && id != focused {
                                    // Clear selection on old pane
                                    let old_focused = focused;
                                    if let Some(m) = self.panes.get_mut(&old_focused) {
                                        m.selection = None;
                                    }
                                    self.set_focus(id);
                                    break;
                                }
                            }
                        }
                    }

                    // Handle selection mouse for focused pane
                    let focused = self.focused_pane_id();
                    for &(id, rect) in &layout {
                        if id == focused {
                            let content_rect = egui::Rect::from_min_max(
                                egui::pos2(rect.min.x, rect.min.y + TAB_BAR_HEIGHT),
                                egui::pos2(rect.max.x, rect.max.y - TERMINAL_BOTTOM_PAD),
                            );
                            let sel_changed = self.handle_selection_mouse(ui, id, content_rect);
                            if sel_changed {
                                self.selection_changed = true;
                            }
                            break;
                        }
                    }

                    // Render dividers
                    let dividers = self.active_workspace().tree.dividers(panel_rect);
                    let painter = ui.painter();
                    for div in &dividers {
                        painter.rect_filled(div.rect, 0.0, self.theme.chrome.divider);
                    }

                    // Render each pane (with its own tab bar)
                    let focused = self.focused_pane_id();
                    for &(id, rect) in &layout {
                        let is_focused = id == focused;
                        self.render_single_pane(ui, id, rect, is_focused);
                        self.resize_pane_if_needed(id, rect, ui);
                    }
                }

                ui.allocate_rect(panel_rect, egui::Sense::hover());
            });

        // Notification panel overlay
        if self.show_notification_panel {
            self.render_notification_panel(ctx);
        }

        // Find bar overlay
        if self.find_state.is_some() {
            self.render_find_bar(ctx);
        }

        // Rename modal
        if self.rename_modal.is_some() {
            self.render_rename_modal(ctx);
        }

        // Hyperlink hover detection + Cmd+click handling
        self.handle_hyperlinks(ctx);

        // Update window title from focused pane's active surface
        let focused_id = self.focused_pane_id();
        if let Some(managed) = self.panes.get(&focused_id) {
            let title = managed.active_surface().pane.title();
            if !title.is_empty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("amux — {}", title)));
            }
        }

        // Position IME candidate window at the terminal cursor
        self.update_ime_position(ctx);

        // Render IME preedit overlay
        if let Some(preedit) = self.ime_preedit.clone() {
            self.render_ime_preedit(ctx, &preedit);
        }

        // Clean up GPU resources for closed panes.
        #[cfg(feature = "gpu-renderer")]
        if let Some(ref gpu) = self.gpu_renderer {
            let live_ids: Vec<u64> = self.panes.keys().copied().collect();
            gpu.retain_panes(&live_ids);
        }

        // Smart repaint: immediate when data arrived or input was sent (to
        // catch the PTY echo on the very next frame), otherwise poll at 50ms.
        if got_data || sent_input || shortcut_consumed || self.selection_changed {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn on_exit(&mut self) {
        if self.wants_exit {
            // User explicitly closed everything — clear session so next
            // launch starts fresh instead of restoring an empty state.
            if let Err(e) = amux_session::clear() {
                tracing::error!("Session clear failed: {}", e);
            }
        } else {
            self.flush_pending_io();
            let data = self.build_session_data();
            if let Err(e) = amux_session::save(&data) {
                tracing::error!("Session save failed: {}", e);
            }
        }
    }
}

impl AmuxApp {
    /// Render a single pane: tab bar (if >1 surface) + terminal content.
    fn render_single_pane(
        &mut self,
        ui: &mut egui::Ui,
        pane_id: PaneId,
        rect: egui::Rect,
        is_focused: bool,
    ) {
        let managed = match self.panes.get_mut(&pane_id) {
            Some(m) => m,
            None => return,
        };

        // Always show tab bar
        let tab_rect =
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), TAB_BAR_HEIGHT));
        let content_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x, rect.min.y + TAB_CONTENT_TOP_INSET),
            egui::pos2(rect.max.x, rect.max.y - TERMINAL_BOTTOM_PAD),
        );
        // Paint bottom padding strip with terminal background color.
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.min.x, rect.max.y - TERMINAL_BOTTOM_PAD),
                rect.max,
            ),
            0.0,
            self.theme.terminal_bg(),
        );

        {
            let painter = ui.painter();
            painter.rect_filled(tab_rect, 0.0, self.theme.tab_bar_bg());
            let bar_stroke = egui::Stroke::new(1.0, self.theme.chrome.tab_bar_border);
            painter.hline(tab_rect.x_range(), tab_rect.min.y, bar_stroke);
            painter.hline(tab_rect.x_range(), tab_rect.max.y, bar_stroke);

            let active_idx = managed.active_surface_idx;
            let tab_font = egui::FontId::proportional(11.0);
            let tab_font_bold = bold_font(11.0);
            let mut x = tab_rect.min.x + 2.0;

            // Get pointer state for hover detection and drag
            let hover_pos = ui.input(|i| i.pointer.hover_pos());
            let primary_pressed = ui.input(|i| i.pointer.primary_pressed());

            let any_released = ui.input(|i| i.pointer.any_released());
            let primary_down = ui.input(|i| i.pointer.primary_down());
            let interact_pos = ui.input(|i| i.pointer.interact_pos());

            // Track actions to apply after rendering
            let mut switch_to: Option<usize> = None;

            let mut close_tab: Option<usize> = None;
            let mut tab_rects: Vec<egui::Rect> = Vec::new();
            let mut drag_start: Option<usize> = None;
            let mut start_rename_tab: Option<usize> = None;

            for (idx, surface) in managed.surfaces.iter().enumerate() {
                let is_active = idx == active_idx;
                let raw_title = surface
                    .user_title
                    .as_deref()
                    .unwrap_or_else(|| surface.pane.title());
                let label = if raw_title.is_empty() {
                    format!("tab {}", surface.id + 1)
                } else if raw_title.chars().count() > 20 {
                    let prefix: String = raw_title.chars().take(17).collect();
                    format!("{prefix}...")
                } else {
                    raw_title.to_string()
                };

                let text_galley =
                    painter.layout_no_wrap(label.clone(), tab_font.clone(), egui::Color32::WHITE);
                let text_width = text_galley.size().x;
                let tab_w = (text_width + 24.0).max(120.0);

                let this_tab = egui::Rect::from_min_size(
                    egui::pos2(x, tab_rect.min.y),
                    egui::vec2(tab_w, TAB_BAR_HEIGHT),
                );
                tab_rects.push(this_tab);

                let tab_hovered = hover_pos.is_some_and(|p| this_tab.contains(p));

                // Tab background + border
                let border_color = self.theme.chrome.tab_border;
                if is_active {
                    painter.rect_filled(this_tab, 0.0, self.theme.chrome.tab_active_bg);
                    // Active highlight at the top
                    let topline = egui::Rect::from_min_size(
                        egui::pos2(x, tab_rect.min.y),
                        egui::vec2(tab_w, 2.0),
                    );
                    painter.rect_filled(topline, 0.0, self.theme.chrome.accent);
                }
                // 1px border around each tab
                painter.rect_stroke(
                    this_tab,
                    0.0,
                    egui::Stroke::new(1.0, border_color),
                    egui::StrokeKind::Outside,
                );

                let (text_color, text_font) = if is_active {
                    (egui::Color32::WHITE, tab_font_bold.clone())
                } else {
                    (egui::Color32::from_gray(130), tab_font.clone())
                };
                painter.text(
                    egui::pos2(x + 6.0, tab_rect.min.y + 5.0),
                    egui::Align2::LEFT_TOP,
                    &label,
                    text_font,
                    text_color,
                );

                // Close button — only visible on hover
                let close_center = egui::pos2(x + tab_w - 10.0, tab_rect.center().y);
                let close_rect = egui::Rect::from_center_size(close_center, egui::vec2(12.0, 12.0));
                if tab_hovered {
                    let close_hovered = hover_pos.is_some_and(|p| close_rect.contains(p));
                    let close_color = if close_hovered {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_gray(90)
                    };
                    sidebar::paint_close_x(painter, close_center, 3.5, close_color);
                }

                // Right-click context menu
                let tab_id = ui.id().with("tab_ctx").with(pane_id).with(idx);
                let tab_response = ui.interact(this_tab, tab_id, egui::Sense::click());
                tab_response.context_menu(|ui| {
                    if ui.button("Rename Tab").clicked() {
                        start_rename_tab = Some(idx);
                        ui.close_menu();
                    }
                    if ui.button("Close Tab").clicked() {
                        close_tab = Some(idx);
                        ui.close_menu();
                    }
                });

                // Hit testing (primary button only)
                if primary_pressed {
                    if let Some(pos) = interact_pos {
                        if tab_hovered && close_rect.contains(pos) {
                            close_tab = Some(idx);
                        } else if this_tab.contains(pos) && !is_active {
                            switch_to = Some(idx);
                        }
                        // Start tab drag
                        if this_tab.contains(pos)
                            && !close_rect.contains(pos)
                            && self.tab_drag.is_none()
                        {
                            drag_start = Some(idx);
                        }
                    }
                }

                x += tab_w + 1.0;
            }

            // Tab drag reorder logic
            if let Some(src) = drag_start {
                self.tab_drag = Some(TabDragState {
                    pane_id,
                    source_idx: src,
                    drop_target_idx: src,
                });
            }

            if let Some(drag) = &mut self.tab_drag {
                if drag.pane_id == pane_id {
                    if any_released || !primary_down {
                        let from = drag.source_idx;
                        let to = drag.drop_target_idx;
                        self.tab_drag = None;
                        if from != to {
                            if let Some(m) = self.panes.get_mut(&pane_id) {
                                let surface = m.surfaces.remove(from);
                                let insert_idx = if from < to {
                                    (to - 1).min(m.surfaces.len())
                                } else {
                                    to.min(m.surfaces.len())
                                };
                                m.surfaces.insert(insert_idx, surface);
                                if m.active_surface_idx == from {
                                    m.active_surface_idx = insert_idx;
                                } else if from < m.active_surface_idx
                                    && insert_idx >= m.active_surface_idx
                                {
                                    m.active_surface_idx -= 1;
                                } else if from > m.active_surface_idx
                                    && insert_idx <= m.active_surface_idx
                                {
                                    m.active_surface_idx += 1;
                                }
                            }
                            return;
                        }
                    } else if let Some(pos) = hover_pos {
                        // Compute drop target from X position vs tab midpoints
                        let tab_midpoints: Vec<f32> =
                            tab_rects.iter().map(|r| r.center().x).collect();
                        let mut target = tab_midpoints.len();
                        for (i, &mid) in tab_midpoints.iter().enumerate() {
                            if pos.x < mid {
                                target = i;
                                break;
                            }
                        }
                        drag.drop_target_idx = target;

                        // Paint drop indicator
                        let drop_x = if target == 0 {
                            tab_rects.first().map(|r| r.min.x).unwrap_or(x)
                        } else if target < tab_rects.len() {
                            let left = tab_rects[target - 1].max.x;
                            let right = tab_rects[target].min.x;
                            (left + right) / 2.0
                        } else {
                            tab_rects.last().map(|r| r.max.x).unwrap_or(x)
                        };
                        let indicator_rect = egui::Rect::from_min_size(
                            egui::pos2(drop_x - 1.0, tab_rect.min.y + 2.0),
                            egui::vec2(2.0, TAB_BAR_HEIGHT - 4.0),
                        );
                        painter.rect_filled(indicator_rect, 1.0, self.theme.chrome.accent);
                    }
                }
            }

            // "+" button to add tab
            let plus_rect = egui::Rect::from_min_size(
                egui::pos2(x + 2.0, tab_rect.min.y),
                egui::vec2(20.0, TAB_BAR_HEIGHT),
            );
            painter.text(
                plus_rect.center(),
                egui::Align2::CENTER_CENTER,
                "+",
                egui::FontId::proportional(14.0),
                egui::Color32::from_gray(100),
            );
            if primary_pressed {
                if let Some(pos) = interact_pos {
                    if plus_rect.contains(pos) {
                        let ws_id = self.active_workspace().id;
                        let sf_id = self.next_surface_id;
                        self.next_surface_id += 1;
                        if let Ok(surface) = spawn_surface(
                            80,
                            24,
                            &self.socket_addr,
                            &self.config,
                            ws_id,
                            sf_id,
                            None,
                            None,
                        ) {
                            // Re-borrow managed after spawn_surface
                            if let Some(m) = self.panes.get_mut(&pane_id) {
                                m.surfaces.push(surface);
                                m.active_surface_idx = m.surfaces.len() - 1;
                            }
                        }
                        return; // skip further rendering this frame
                    }
                }
            }

            // Apply tab switch/close (need to re-borrow managed)
            if let Some(idx) = close_tab {
                let is_last = self
                    .panes
                    .get(&pane_id)
                    .is_some_and(|m| m.surfaces.len() <= 1);
                if is_last {
                    self.close_pane(pane_id);
                    return;
                }
                let managed = self.panes.get_mut(&pane_id).unwrap();
                managed.surfaces.remove(idx);
                if idx < managed.active_surface_idx {
                    managed.active_surface_idx -= 1;
                } else if managed.active_surface_idx >= managed.surfaces.len() {
                    managed.active_surface_idx = managed.surfaces.len() - 1;
                }
            } else if let Some(idx) = switch_to {
                let managed = self.panes.get_mut(&pane_id).unwrap();
                managed.active_surface_idx = idx;
            }

            // Open rename modal for tab
            if let Some(idx) = start_rename_tab {
                if let Some(managed) = self.panes.get(&pane_id) {
                    if idx < managed.surfaces.len() {
                        let surface = &managed.surfaces[idx];
                        let current_title = surface
                            .user_title
                            .clone()
                            .unwrap_or_else(|| surface.pane.title().to_string());
                        self.rename_modal = Some(RenameModal {
                            target: RenameTarget::Tab {
                                pane_id,
                                surface_id: surface.id,
                            },
                            buf: current_title,
                            just_opened: true,
                        });
                    }
                }
            }
        }

        // Collect find highlights for this pane
        let (find_highlights, current_highlight) = self
            .find_state
            .as_ref()
            .filter(|f| f.pane_id == pane_id)
            .map(|f| (f.matches.clone(), Some(f.current_match)))
            .unwrap_or_default();

        // Build selection: use copy mode visual selection if active, else normal selection
        let copy_mode_sel = self
            .copy_mode
            .as_ref()
            .filter(|cm| cm.pane_id == pane_id && cm.visual_anchor.is_some())
            .map(|cm| {
                let anchor = cm.visual_anchor.unwrap();
                let cursor = cm.cursor;
                let (start, end) =
                    if anchor.1 < cursor.1 || (anchor.1 == cursor.1 && anchor.0 <= cursor.0) {
                        (anchor, cursor)
                    } else {
                        (cursor, anchor)
                    };
                SelectionState {
                    anchor: start,
                    end,
                    mode: if cm.line_visual {
                        SelectionMode::Line
                    } else {
                        SelectionMode::Cell
                    },
                    active: false,
                }
            });

        // Render terminal content for the active surface
        let managed = match self.panes.get_mut(&pane_id) {
            Some(m) => m,
            None => return,
        };
        let selection = copy_mode_sel
            .as_ref()
            .or(managed.selection.as_ref())
            .cloned();
        let surface = managed.active_surface_mut();
        render_pane(
            ui,
            &mut surface.pane,
            content_rect,
            is_focused,
            surface.scroll_offset,
            selection.as_ref(),
            self.font_size,
            &find_highlights,
            current_highlight,
            #[cfg(feature = "gpu-renderer")]
            self.gpu_renderer.as_ref(),
            #[cfg(feature = "gpu-renderer")]
            pane_id,
        );

        // Render copy mode cursor overlay
        if let Some(cm) = self.copy_mode.as_ref().filter(|cm| cm.pane_id == pane_id) {
            let (cell_w, cell_h) = self.cell_dimensions(ui);
            if let Some(managed) = self.panes.get(&pane_id) {
                let surface = managed.active_surface();
                let (_, rows) = surface.pane.dimensions();
                let total = surface.pane.screen().scrollback_rows();
                let end = total.saturating_sub(surface.scroll_offset);
                let start = end.saturating_sub(rows);
                if cm.cursor.1 >= start && cm.cursor.1 < end {
                    let row_in_view = cm.cursor.1 - start;
                    let x = content_rect.min.x + cm.cursor.0 as f32 * cell_w;
                    let y = content_rect.min.y + row_in_view as f32 * cell_h;
                    let cursor_rect =
                        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell_w, cell_h));
                    ui.painter().rect_stroke(
                        cursor_rect,
                        0.0,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 255, 0)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }

        // Notification ring + flash animation (matching cmux)
        let pane_u64 = pane_id;
        let ring_rect = rect.shrink(2.0);

        // 1. Persistent unread ring (NOT on focused pane)
        if !is_focused && self.notifications.pane_unread(pane_u64) > 0 {
            // Steady blue ring with glow
            let ring_color = egui::Color32::from_rgba_unmultiplied(40, 120, 255, 89); // 0.35 * 255
            ui.painter().rect_stroke(
                ring_rect,
                6.0,
                egui::Stroke::new(2.5, ring_color),
                egui::StrokeKind::Inside,
            );
            ui.ctx().request_repaint();
        }

        // 2. Flash animation (on ANY pane including focused)
        if let Some(state) = self.notifications.pane_state(pane_u64) {
            if let Some(started) = state.flash_started_at {
                let elapsed = started.elapsed().as_secs_f32();
                if elapsed < FLASH_DURATION {
                    let alpha = flash_alpha(elapsed);
                    let base_color = match state.flash_reason {
                        Some(FlashReason::Navigation) => [0u8, 128, 128], // teal
                        _ => [40, 120, 255],                              // blue
                    };
                    let glow_alpha = (alpha * 0.6 * 255.0) as u8;
                    let ring_alpha = (alpha * 255.0) as u8;
                    // Glow (wider, more transparent)
                    ui.painter().rect_stroke(
                        ring_rect.expand(1.0),
                        6.0,
                        egui::Stroke::new(
                            4.0,
                            egui::Color32::from_rgba_unmultiplied(
                                base_color[0],
                                base_color[1],
                                base_color[2],
                                glow_alpha,
                            ),
                        ),
                        egui::StrokeKind::Outside,
                    );
                    // Ring
                    ui.painter().rect_stroke(
                        ring_rect,
                        6.0,
                        egui::Stroke::new(
                            2.5,
                            egui::Color32::from_rgba_unmultiplied(
                                base_color[0],
                                base_color[1],
                                base_color[2],
                                ring_alpha,
                            ),
                        ),
                        egui::StrokeKind::Inside,
                    );
                    ui.ctx().request_repaint();
                }
            }
        }
    }

    // --- Resize ---

    fn resize_pane_if_needed(&mut self, id: PaneId, rect: egui::Rect, ui: &egui::Ui) {
        let (cell_width, cell_height) = self.cell_dimensions(ui);

        // Account for tab bar height (always shown) and visual bottom padding.
        let content_height = rect.height() - TAB_BAR_HEIGHT - TERMINAL_BOTTOM_PAD;

        let cols = (rect.width() / cell_width).floor() as usize;
        let rows = (content_height / cell_height).floor() as usize;

        if cols == 0 || rows == 0 {
            return;
        }

        // Resize the active surface if its dimensions don't match the pane rect.
        // This handles both pane rect changes and tab switches (new surface at 80x24).
        if let Some(managed) = self.panes.get_mut(&id) {
            let surface = managed.active_surface_mut();
            let (cur_cols, cur_rows) = surface.pane.dimensions();
            if cur_cols != cols || cur_rows != rows {
                let _ = surface.pane.resize(cols as u16, rows as u16);
            }
        }
    }

    // --- Pane/Workspace management ---

    fn spawn_pane_with_surface(&mut self) -> Option<PaneId> {
        let ws_id = self.active_workspace().id;
        let sf_id = self.next_surface_id;

        match spawn_surface(
            80,
            24,
            &self.socket_addr,
            &self.config,
            ws_id,
            sf_id,
            None,
            None,
        ) {
            Ok(surface) => {
                let pane_id = self.next_pane_id;
                self.next_pane_id += 1;
                self.next_surface_id += 1;
                self.panes.insert(
                    pane_id,
                    ManagedPane {
                        surfaces: vec![surface],
                        active_surface_idx: 0,
                        selection: None,
                    },
                );
                Some(pane_id)
            }
            Err(e) => {
                tracing::error!("Failed to spawn pane: {}", e);
                None
            }
        }
    }

    fn create_workspace(&mut self, title: Option<String>) -> Option<u64> {
        let ws_id = self.next_workspace_id;
        let title = title.unwrap_or_else(|| format!("Terminal {}", self.workspaces.len() + 1));

        let sf_id = self.next_surface_id;

        match spawn_surface(
            80,
            24,
            &self.socket_addr,
            &self.config,
            ws_id,
            sf_id,
            None,
            None,
        ) {
            Ok(surface) => {
                self.next_workspace_id += 1;
                let pane_id = self.next_pane_id;
                self.next_pane_id += 1;
                self.next_surface_id += 1;
                self.panes.insert(
                    pane_id,
                    ManagedPane {
                        surfaces: vec![surface],
                        active_surface_idx: 0,
                        selection: None,
                    },
                );

                let workspace = Workspace {
                    id: ws_id,
                    title,
                    tree: PaneTree::new(pane_id),
                    focused_pane: pane_id,
                    zoomed: None,
                    dragging_divider: None,
                    last_pane_sizes: HashMap::new(),
                    color: None,
                };

                self.workspaces.push(workspace);
                self.active_workspace_idx = self.workspaces.len() - 1;
                Some(ws_id)
            }
            Err(e) => {
                tracing::error!("Failed to spawn pane for workspace: {}", e);
                None
            }
        }
    }

    fn add_surface_to_focused_pane(&mut self) -> Option<u64> {
        let sf_id = self.next_surface_id;
        self.next_surface_id += 1;
        let ws_id = self.active_workspace().id;
        let focused = self.focused_pane_id();

        match spawn_surface(
            80,
            24,
            &self.socket_addr,
            &self.config,
            ws_id,
            sf_id,
            None,
            None,
        ) {
            Ok(surface) => {
                if let Some(managed) = self.panes.get_mut(&focused) {
                    managed.surfaces.push(surface);
                    managed.active_surface_idx = managed.surfaces.len() - 1;
                    Some(sf_id)
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::error!("Failed to spawn surface: {}", e);
                None
            }
        }
    }

    fn close_workspace_at(&mut self, ws_idx: usize) {
        let ws_id = self.workspaces[ws_idx].id;
        if self.workspaces.len() <= 1 {
            // Last workspace — clean up and signal exit
            let pane_ids: Vec<PaneId> = self.workspaces[ws_idx].tree.iter_panes();
            for id in &pane_ids {
                self.panes.remove(id);
                self.notifications.remove_pane(*id);
            }
            self.notifications.remove_workspace(ws_id);
            self.wants_exit = true;
            return;
        }
        let pane_ids: Vec<PaneId> = self.workspaces[ws_idx].tree.iter_panes();
        for id in &pane_ids {
            self.panes.remove(id);
            self.notifications.remove_pane(*id);
        }
        self.notifications.remove_workspace(ws_id);
        self.workspaces.remove(ws_idx);
        if self.active_workspace_idx >= self.workspaces.len() {
            self.active_workspace_idx = self.workspaces.len() - 1;
        }
    }

    // --- Menu bar actions ---

    fn handle_menu_actions(&mut self) {
        while let Some(action) = menu_bar::take_pending_action() {
            match action {
                menu_bar::MenuAction::NewWorkspace => {
                    self.create_workspace(None);
                }
                menu_bar::MenuAction::NewTab => {
                    self.add_surface_to_focused_pane();
                }
                menu_bar::MenuAction::CloseTab => {
                    self.do_close_cascade();
                }
                menu_bar::MenuAction::SaveSession => {
                    self.flush_pending_io();
                    let data = self.build_session_data();
                    if let Err(e) = amux_session::save(&data) {
                        tracing::error!("Failed to save session: {}", e);
                    }
                }
                menu_bar::MenuAction::ToggleSidebar => {
                    self.sidebar.visible = !self.sidebar.visible;
                }
                menu_bar::MenuAction::ToggleNotificationPanel => {
                    self.show_notification_panel = !self.show_notification_panel;
                }
                menu_bar::MenuAction::ZoomIn => {
                    self.font_size = (self.font_size + 1.0).min(96.0);
                    #[cfg(feature = "gpu-renderer")]
                    if let Some(gpu) = &mut self.gpu_renderer {
                        gpu.set_font_size(self.font_size);
                    }
                }
                menu_bar::MenuAction::ZoomOut => {
                    self.font_size = (self.font_size - 1.0).max(4.0);
                    #[cfg(feature = "gpu-renderer")]
                    if let Some(gpu) = &mut self.gpu_renderer {
                        gpu.set_font_size(self.font_size);
                    }
                }
                menu_bar::MenuAction::ZoomReset => {
                    self.font_size = font::DEFAULT_FONT_SIZE;
                    #[cfg(feature = "gpu-renderer")]
                    if let Some(gpu) = &mut self.gpu_renderer {
                        gpu.set_font_size(self.font_size);
                    }
                }
            }
        }
    }

    // --- Shortcuts ---

    fn handle_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        // Skip terminal shortcuts when a modal text field has focus — let egui
        // handle Cmd+V, Cmd+C, etc. for the text widget instead.
        if self.rename_modal.is_some() || self.find_state.is_some() {
            return false;
        }
        let events = ctx.input(|i| i.events.clone());

        for event in &events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                #[cfg(target_os = "macos")]
                let is_cmd = modifiers.mac_cmd || modifiers.command;
                #[cfg(not(target_os = "macos"))]
                let is_cmd = modifiers.ctrl && modifiers.shift;

                // Copy: Cmd+C (with selection) / Cmd+Shift+C (always copy)
                #[cfg(target_os = "macos")]
                let is_copy = is_cmd && (*key == egui::Key::C);
                #[cfg(not(target_os = "macos"))]
                let is_copy = modifiers.ctrl && modifiers.shift && (*key == egui::Key::C);

                // Copy selection if active; otherwise fall through to send Ctrl+C
                if is_copy && self.copy_selection() {
                    return true;
                }

                // Paste: Cmd+V (macOS) / Ctrl+Shift+V (other)
                #[cfg(target_os = "macos")]
                let is_paste = is_cmd && !modifiers.shift && *key == egui::Key::V;
                #[cfg(not(target_os = "macos"))]
                let is_paste = modifiers.ctrl && modifiers.shift && *key == egui::Key::V;

                if is_paste {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        if let Ok(text) = clipboard.get_text() {
                            if !text.is_empty() {
                                self.do_paste(&text);
                            }
                        }
                    }
                    return true;
                }

                // Copy mode: intercept all keys when active
                if self.copy_mode.is_some() {
                    return self.handle_copy_mode_key(key, modifiers);
                }

                // Escape: close find bar, exit copy mode, or clear selection
                if *key == egui::Key::Escape
                    && !modifiers.shift
                    && !modifiers.ctrl
                    && !modifiers.alt
                {
                    if self.find_state.is_some() {
                        self.find_state = None;
                        return true;
                    }
                    let focused = self.focused_pane_id();
                    if let Some(m) = self.panes.get_mut(&focused) {
                        if m.selection.is_some() {
                            m.selection = None;
                            return true;
                        }
                    }
                }

                // Find: Cmd+F (macOS) / Ctrl+Shift+F (other)
                if is_cmd && !modifiers.shift && *key == egui::Key::F {
                    let pane_id = self.focused_pane_id();
                    self.find_state = Some(FindState {
                        query: String::new(),
                        matches: Vec::new(),
                        current_match: 0,
                        pane_id,
                        just_opened: true,
                    });
                    return true;
                }

                // Select all: Cmd+A
                #[cfg(target_os = "macos")]
                if is_cmd && !modifiers.shift && *key == egui::Key::A {
                    self.select_all_visible();
                    return true;
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::A {
                    self.select_all_visible();
                    return true;
                }

                // Enter copy mode: Cmd+Shift+X (macOS) / Ctrl+Shift+X (other)
                if is_cmd && modifiers.shift && *key == egui::Key::X {
                    self.enter_copy_mode();
                    return true;
                }

                // Toggle sidebar: Cmd+B / Ctrl+B
                #[cfg(target_os = "macos")]
                if is_cmd && !modifiers.shift && *key == egui::Key::B {
                    self.sidebar.visible = !self.sidebar.visible;
                    return true;
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && !modifiers.shift && *key == egui::Key::B {
                    self.sidebar.visible = !self.sidebar.visible;
                    return true;
                }

                // New workspace: Cmd+N / Ctrl+Shift+N
                #[cfg(target_os = "macos")]
                if is_cmd && !modifiers.shift && *key == egui::Key::N {
                    self.create_workspace(None);
                    return true;
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::N {
                    self.create_workspace(None);
                    return true;
                }

                // New tab in focused pane: Cmd+T / Ctrl+Shift+T
                #[cfg(target_os = "macos")]
                if is_cmd && !modifiers.shift && *key == egui::Key::T {
                    self.add_surface_to_focused_pane();
                    return true;
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::T {
                    self.add_surface_to_focused_pane();
                    return true;
                }

                // Next workspace: Cmd+Shift+]
                #[cfg(target_os = "macos")]
                if is_cmd && modifiers.shift && *key == egui::Key::CloseBracket {
                    if self.workspaces.len() > 1 {
                        self.active_workspace_idx =
                            (self.active_workspace_idx + 1) % self.workspaces.len();
                    }
                    return true;
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::CloseBracket {
                    if self.workspaces.len() > 1 {
                        self.active_workspace_idx =
                            (self.active_workspace_idx + 1) % self.workspaces.len();
                    }
                    return true;
                }

                // Prev workspace: Cmd+Shift+[
                #[cfg(target_os = "macos")]
                if is_cmd && modifiers.shift && *key == egui::Key::OpenBracket {
                    if self.workspaces.len() > 1 {
                        self.active_workspace_idx = if self.active_workspace_idx == 0 {
                            self.workspaces.len() - 1
                        } else {
                            self.active_workspace_idx - 1
                        };
                    }
                    return true;
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::OpenBracket {
                    if self.workspaces.len() > 1 {
                        self.active_workspace_idx = if self.active_workspace_idx == 0 {
                            self.workspaces.len() - 1
                        } else {
                            self.active_workspace_idx - 1
                        };
                    }
                    return true;
                }

                // Jump to workspace 1-9 (Cmd+9 = last workspace)
                #[cfg(target_os = "macos")]
                let is_jump_mod = is_cmd && !modifiers.shift;
                #[cfg(not(target_os = "macos"))]
                let is_jump_mod = modifiers.ctrl && !modifiers.shift;

                if is_jump_mod {
                    let num = match key {
                        egui::Key::Num1 => Some(0usize),
                        egui::Key::Num2 => Some(1),
                        egui::Key::Num3 => Some(2),
                        egui::Key::Num4 => Some(3),
                        egui::Key::Num5 => Some(4),
                        egui::Key::Num6 => Some(5),
                        egui::Key::Num7 => Some(6),
                        egui::Key::Num8 => Some(7),
                        egui::Key::Num9 => Some(usize::MAX), // last workspace
                        _ => None,
                    };
                    if let Some(mut idx) = num {
                        if idx == usize::MAX {
                            idx = self.workspaces.len().saturating_sub(1);
                        }
                        if idx < self.workspaces.len() {
                            self.active_workspace_idx = idx;
                            return true;
                        }
                    }
                }

                // Next tab in focused pane: Ctrl+Tab
                if modifiers.ctrl && !modifiers.shift && *key == egui::Key::Tab {
                    if let Some(managed) = self.panes.get_mut(&self.focused_pane_id()) {
                        if managed.surfaces.len() > 1 {
                            managed.active_surface_idx =
                                (managed.active_surface_idx + 1) % managed.surfaces.len();
                        }
                    }
                    return true;
                }

                // Prev tab in focused pane: Ctrl+Shift+Tab
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::Tab {
                    if let Some(managed) = self.panes.get_mut(&self.focused_pane_id()) {
                        if managed.surfaces.len() > 1 {
                            managed.active_surface_idx = if managed.active_surface_idx == 0 {
                                managed.surfaces.len() - 1
                            } else {
                                managed.active_surface_idx - 1
                            };
                        }
                    }
                    return true;
                }

                // --- Pane shortcuts ---

                // Split right: Cmd+D (macOS) / Ctrl+Shift+D (other)
                #[cfg(target_os = "macos")]
                if is_cmd && !modifiers.shift && *key == egui::Key::D {
                    return self.do_split(SplitDirection::Horizontal);
                }
                #[cfg(not(target_os = "macos"))]
                if is_cmd && *key == egui::Key::D {
                    return self.do_split(SplitDirection::Horizontal);
                }
                // Split down: Cmd+Shift+D (macOS) / Ctrl+Shift+Down (other)
                #[cfg(target_os = "macos")]
                if is_cmd && modifiers.shift && *key == egui::Key::D {
                    return self.do_split(SplitDirection::Vertical);
                }
                #[cfg(not(target_os = "macos"))]
                if modifiers.ctrl && modifiers.shift && *key == egui::Key::ArrowDown {
                    return self.do_split(SplitDirection::Vertical);
                }

                // Close: Cmd+W — cascade: tab -> pane -> workspace
                if is_cmd && *key == egui::Key::W {
                    return self.do_close_cascade();
                }

                // Navigate: Option+Cmd+Arrow / Ctrl+Alt+Arrow
                #[cfg(target_os = "macos")]
                let is_nav = is_cmd && modifiers.alt;
                #[cfg(not(target_os = "macos"))]
                let is_nav = modifiers.ctrl && modifiers.alt;

                if is_nav {
                    let dir = match key {
                        egui::Key::ArrowLeft => Some(NavDirection::Left),
                        egui::Key::ArrowRight => Some(NavDirection::Right),
                        egui::Key::ArrowUp => Some(NavDirection::Up),
                        egui::Key::ArrowDown => Some(NavDirection::Down),
                        _ => None,
                    };
                    if let Some(dir) = dir {
                        return self.do_navigate(dir);
                    }
                }

                // Zoom toggle: Cmd+Shift+Enter / Ctrl+Shift+Enter
                #[cfg(target_os = "macos")]
                let is_zoom = is_cmd && modifiers.shift && *key == egui::Key::Enter;
                #[cfg(not(target_os = "macos"))]
                let is_zoom = modifiers.ctrl && modifiers.shift && *key == egui::Key::Enter;

                if is_zoom {
                    return self.do_toggle_zoom();
                }

                // Notification panel: Cmd+I / Ctrl+I
                if is_cmd && !modifiers.shift && *key == egui::Key::I {
                    self.show_notification_panel = !self.show_notification_panel;
                    return true;
                }

                // Jump to latest unread: Cmd+Shift+U / Ctrl+Shift+U
                if is_cmd && modifiers.shift && *key == egui::Key::U {
                    self.jump_to_latest_unread();
                    return true;
                }

                // Clear scrollback: Cmd+K (macOS) / Ctrl+Shift+K (other)
                if is_cmd && !modifiers.shift && *key == egui::Key::K {
                    self.do_clear_scrollback();
                    return true;
                }

                // Scroll
                if modifiers.shift && *key == egui::Key::PageUp {
                    return self.do_scroll(-1);
                }
                if modifiers.shift && *key == egui::Key::PageDown {
                    return self.do_scroll(1);
                }
            }
        }

        // Mouse wheel scrolling — scroll the pane under the cursor
        let scroll_delta = ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll_delta != 0.0 {
            let hover_pos = ctx.input(|i| i.pointer.hover_pos());
            let target_pane = hover_pos.and_then(|pos| {
                let panel_rect = self.last_panel_rect?;
                let ws = self.active_workspace();
                if let Some(zoomed_id) = ws.zoomed {
                    // In zoomed mode, only the zoomed pane is visible
                    if panel_rect.contains(pos) {
                        return Some(zoomed_id);
                    }
                    return None;
                }
                let layout = ws.tree.layout(panel_rect);
                layout
                    .iter()
                    .find(|(_, rect)| rect.contains(pos))
                    .map(|(id, _)| *id)
            });
            if let Some(pane_id) = target_pane {
                if let Some(managed) = self.panes.get_mut(&pane_id) {
                    let surface = managed.active_surface_mut();
                    let font_id = egui::FontId::monospace(self.font_size);
                    let cell_height = ctx.fonts(|f| f.row_height(&font_id));

                    surface.scroll_accum += -scroll_delta / cell_height;
                    let whole_lines = surface.scroll_accum.trunc() as isize;
                    if whole_lines != 0 {
                        surface.scroll_accum -= whole_lines as f32;
                        self.do_scroll_lines_for(pane_id, whole_lines);
                    }
                }
            }
        }

        false
    }

    fn do_split(&mut self, direction: SplitDirection) -> bool {
        let Some(new_id) = self.spawn_pane_with_surface() else {
            return false;
        };
        let ws = self.active_workspace_mut();
        if ws.tree.split(ws.focused_pane, direction, new_id) {
            self.set_focus(new_id);
            true
        } else {
            // Split failed — clean up the spawned pane
            self.panes.remove(&new_id);
            false
        }
    }

    fn do_close_cascade(&mut self) -> bool {
        let focused_id = self.focused_pane_id();

        // First check: close a tab if >1 tab in focused pane
        if let Some(managed) = self.panes.get_mut(&focused_id) {
            if managed.surfaces.len() > 1 {
                managed.surfaces.remove(managed.active_surface_idx);
                if managed.active_surface_idx >= managed.surfaces.len() {
                    managed.active_surface_idx = managed.surfaces.len() - 1;
                }
                return true;
            }
        }

        self.close_pane(focused_id);
        true
    }

    /// Close a pane entirely. Finds the owning workspace (not necessarily the
    /// active one). If it's the last pane in that workspace, close the workspace.
    fn close_pane(&mut self, pane_id: PaneId) {
        // Find the workspace that owns this pane
        let ws_idx = match self
            .workspaces
            .iter()
            .position(|ws| ws.tree.iter_panes().contains(&pane_id))
        {
            Some(idx) => idx,
            None => return, // pane not in any workspace
        };

        let pane_count = self.workspaces[ws_idx].tree.iter_panes().len();
        if pane_count > 1 {
            let ws = &mut self.workspaces[ws_idx];
            if let Some(new_focus) = ws.tree.close(pane_id) {
                ws.last_pane_sizes.remove(&pane_id);
                if ws.zoomed == Some(pane_id) {
                    ws.zoomed = None;
                }
                self.panes.remove(&pane_id);
                self.notifications.remove_pane(pane_id);
                if ws_idx == self.active_workspace_idx {
                    self.set_focus(new_focus);
                }
            }
        } else {
            // Last pane in workspace -> close workspace
            self.close_workspace_at(ws_idx);
        }
    }

    fn do_navigate(&mut self, dir: NavDirection) -> bool {
        if let Some(rect) = self.last_panel_rect {
            let ws = self.active_workspace();
            if let Some(neighbor) = ws.tree.neighbor(ws.focused_pane, dir, rect) {
                self.set_focus(neighbor);
            } else {
                self.flash_focus();
            }
        }
        true
    }

    fn do_toggle_zoom(&mut self) -> bool {
        let ws = self.active_workspace_mut();
        if ws.zoomed.is_some() {
            ws.zoomed = None;
        } else {
            ws.zoomed = Some(ws.focused_pane);
        }
        true
    }

    fn do_scroll(&mut self, pages: isize) -> bool {
        let focused_id = self.focused_pane_id();
        if let Some(managed) = self.panes.get_mut(&focused_id) {
            let surface = managed.active_surface_mut();
            let (_, rows) = surface.pane.dimensions();
            let page_size = rows.saturating_sub(1).max(1);
            let lines = pages * page_size as isize;
            let total = surface.pane.screen().scrollback_rows();
            let max_offset = total.saturating_sub(rows);
            let new_offset = surface.scroll_offset as isize - lines;
            surface.scroll_offset = (new_offset.max(0) as usize).min(max_offset);
        }
        true
    }

    fn do_scroll_lines_for(&mut self, pane_id: PaneId, lines: isize) {
        if let Some(managed) = self.panes.get_mut(&pane_id) {
            let surface = managed.active_surface_mut();
            let (_, rows) = surface.pane.dimensions();
            let total = surface.pane.screen().scrollback_rows();
            let max_offset = total.saturating_sub(rows);
            let new_offset = surface.scroll_offset as isize - lines;
            surface.scroll_offset = (new_offset.max(0) as usize).min(max_offset);
        }
    }

    fn do_clear_scrollback(&mut self) {
        let focused_id = self.focused_pane_id();
        if let Some(managed) = self.panes.get_mut(&focused_id) {
            let surface = managed.active_surface_mut();
            // 1. Clear visible screen and move cursor home via terminal state machine
            surface.pane.feed_bytes(b"\x1b[2J\x1b[H");
            // 2. Erase scrollback buffer
            surface.pane.erase_scrollback();
            surface.scroll_offset = 0;
            surface.scroll_accum = 0.0;
            // 3. Send Ctrl+L to the PTY so the shell redraws its prompt
            let _ = surface.pane.write_bytes(b"\x0c");
        }
    }

    // --- Notifications ---

    fn drain_notifications(&mut self) {
        // Collect events first to avoid borrow conflicts
        let mut events: Vec<(u64, u64, u64, NotificationEvent)> = Vec::new();

        for (&pane_id, managed) in &self.panes {
            let ws_id = self.workspace_for_pane(pane_id).unwrap_or(0);
            for surface in &managed.surfaces {
                for event in surface.pane.drain_notifications() {
                    events.push((ws_id, pane_id, surface.id, event));
                }
            }
        }

        for (ws_id, pane_id, surface_id, event) in events {
            let (title, body, source) = match event {
                NotificationEvent::Toast { title, body } => {
                    (title.unwrap_or_default(), body, NotificationSource::Toast)
                }
                NotificationEvent::Bell => {
                    (String::new(), "Bell".to_string(), NotificationSource::Bell)
                }
                NotificationEvent::TitleChanged(_) => {
                    continue;
                }
                NotificationEvent::WorkingDirectoryChanged => {
                    // Store the CWD from OSC 7 into surface metadata
                    if let Some(managed) = self.panes.get_mut(&pane_id) {
                        for surface in &mut managed.surfaces {
                            if surface.id == surface_id {
                                let cwd = surface
                                    .pane
                                    .working_dir()
                                    .and_then(|url| url.to_file_path().ok())
                                    .map(|p| p.to_string_lossy().to_string());
                                if cwd.is_some() {
                                    surface.metadata.cwd = cwd;
                                }
                            }
                        }
                    }
                    continue;
                }
            };

            let skip_toast = matches!(source, NotificationSource::Bell);
            self.deliver_notification(ws_id, pane_id, surface_id, title, body, source, skip_toast);
        }
    }

    /// Three-tier notification delivery (matching cmux):
    /// 1. App unfocused → system toast + custom command + unread
    /// 2. App focused, different pane → in-app sound + custom command + unread
    /// 3. App focused, same pane → mark read (flash only, no ring)
    ///
    /// `skip_toast` suppresses the system toast (used for bell notifications).
    #[allow(clippy::too_many_arguments)]
    fn deliver_notification(
        &mut self,
        ws_id: u64,
        pane_id: PaneId,
        surface_id: u64,
        title: String,
        body: String,
        source: NotificationSource,
        skip_toast: bool,
    ) -> u64 {
        let focused = self.focused_pane_id();
        let source_str = source.as_str();

        if !self.app_focused {
            // Tier 1: app is unfocused — always treat as background
            if self.app_config.notifications.system_notifications && !skip_toast {
                self.system_notifier.send(&title, &body, ws_id, pane_id);
            }
            if let Some(cmd) = &self.app_config.notifications.custom_command {
                self.system_notifier
                    .run_custom_command(cmd, &title, &body, source_str);
            }
            let nid = self
                .notifications
                .push(ws_id, pane_id, surface_id, title, body, source);
            if self.app_config.notifications.auto_reorder_workspaces {
                self.bubble_workspace(ws_id);
            }
            nid
        } else if pane_id == focused {
            // Tier 3: app focused, same pane — mark read (flash only)
            self.notifications
                .push_read(ws_id, pane_id, surface_id, title, body, source)
        } else {
            // Tier 2: app focused, different pane — in-app sound + command
            if self.app_config.notifications.sound.play_when_focused {
                if let Some(player) = &self.sound_player {
                    player.play();
                }
            }
            if let Some(cmd) = &self.app_config.notifications.custom_command {
                self.system_notifier
                    .run_custom_command(cmd, &title, &body, source_str);
            }
            let nid = self
                .notifications
                .push(ws_id, pane_id, surface_id, title, body, source);
            if self.app_config.notifications.auto_reorder_workspaces {
                self.bubble_workspace(ws_id);
            }
            nid
        }
    }

    /// Move a workspace to the top of the sidebar (just index 0 for now,
    /// since amux doesn't have pinned workspaces yet). Adjusts
    /// `active_workspace_idx` to keep the active workspace correct.
    fn bubble_workspace(&mut self, workspace_id: u64) {
        let active_ws_id = self.workspaces[self.active_workspace_idx].id;
        // Don't bubble the active workspace
        if workspace_id == active_ws_id {
            return;
        }
        let Some(from) = self.workspaces.iter().position(|ws| ws.id == workspace_id) else {
            return;
        };
        if from == 0 {
            return;
        }
        let ws = self.workspaces.remove(from);
        self.workspaces.insert(0, ws);
        // Fix active_workspace_idx: the active workspace shifted right by 1
        // if it was before the removed position.
        self.active_workspace_idx = self
            .workspaces
            .iter()
            .position(|ws| ws.id == active_ws_id)
            .unwrap_or(0);
    }

    fn workspace_for_pane(&self, pane_id: PaneId) -> Option<u64> {
        self.workspaces
            .iter()
            .find(|ws| ws.tree.iter_panes().contains(&pane_id))
            .map(|ws| ws.id)
    }

    /// Aggregate metadata from the focused surface of the focused pane in a workspace.
    fn workspace_metadata(&self, workspace: &Workspace) -> SurfaceMetadata {
        self.panes
            .get(&workspace.focused_pane)
            .map(|mp| {
                let sf = mp.active_surface();
                let mut meta = sf.metadata.clone();
                // Capture the surface's OSC title for sidebar display
                let title = sf.pane.title();
                if !title.is_empty() {
                    meta.surface_title = Some(title.to_string());
                }
                meta
            })
            .unwrap_or_default()
    }

    fn jump_to_latest_unread(&mut self) {
        if let Some(notif) = self.notifications.most_recent_unread() {
            let ws_id = notif.workspace_id;
            let pane_id = notif.pane_id as PaneId;

            // Switch to the notification's workspace
            if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) {
                self.active_workspace_idx = idx;
            }
            self.set_focus(pane_id);
        }
    }

    fn update_ime_position(&self, ctx: &egui::Context) {
        let focused_id = self.focused_pane_id();
        let panel_rect = match self.last_panel_rect {
            Some(r) => r,
            None => return,
        };

        // Find the focused pane's rect
        let pane_rect = if let Some(zoomed_id) = self.active_workspace().zoomed {
            if zoomed_id == focused_id {
                panel_rect
            } else {
                return;
            }
        } else {
            let layout = self.active_workspace().tree.layout(panel_rect);
            match layout.iter().find(|(id, _)| *id == focused_id) {
                Some((_, r)) => *r,
                None => return,
            }
        };

        if let Some(managed) = self.panes.get(&focused_id) {
            let surface = managed.active_surface();
            let cursor = surface.pane.cursor();
            let (dim_cols, dim_rows) = surface.pane.dimensions();
            let cols = dim_cols.max(1) as f32;
            let rows = dim_rows.max(1) as f32;
            let cell_w = pane_rect.width() / cols;
            let cell_h = (pane_rect.height() - TAB_BAR_HEIGHT - TERMINAL_BOTTOM_PAD) / rows;
            let x = pane_rect.min.x + cursor.x as f32 * cell_w;
            let y = pane_rect.min.y + TAB_BAR_HEIGHT + cursor.y as f32 * cell_h;
            ctx.send_viewport_cmd(egui::ViewportCommand::IMERect(egui::Rect::from_min_size(
                egui::pos2(x, y),
                egui::vec2(cell_w, cell_h),
            )));
        }
    }

    fn render_ime_preedit(&self, ctx: &egui::Context, preedit: &str) {
        let focused_id = self.focused_pane_id();
        let panel_rect = match self.last_panel_rect {
            Some(r) => r,
            None => return,
        };

        let pane_rect = if let Some(zoomed_id) = self.active_workspace().zoomed {
            if zoomed_id == focused_id {
                panel_rect
            } else {
                return;
            }
        } else {
            let layout = self.active_workspace().tree.layout(panel_rect);
            match layout.iter().find(|(id, _)| *id == focused_id) {
                Some((_, r)) => *r,
                None => return,
            }
        };

        if let Some(managed) = self.panes.get(&focused_id) {
            let surface = managed.active_surface();
            let cursor = surface.pane.cursor();
            let (dim_cols, dim_rows) = surface.pane.dimensions();
            let cols = dim_cols.max(1) as f32;
            let rows = dim_rows.max(1) as f32;
            let cell_w = pane_rect.width() / cols;
            let cell_h = (pane_rect.height() - TAB_BAR_HEIGHT - TERMINAL_BOTTOM_PAD) / rows;
            let x = pane_rect.min.x + cursor.x as f32 * cell_w;
            let y = pane_rect.min.y + TAB_BAR_HEIGHT + cursor.y as f32 * cell_h;

            egui::Area::new(egui::Id::new("ime_preedit"))
                .fixed_pos(egui::pos2(x, y))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(preedit)
                                .monospace()
                                .size(self.font_size)
                                .underline(),
                        );
                    });
                });
        }
    }

    fn handle_hyperlinks(&mut self, ctx: &egui::Context) {
        self.hovered_hyperlink = None;

        let hover_pos = match ctx.input(|i| i.pointer.hover_pos()) {
            Some(pos) => pos,
            None => return,
        };

        let panel_rect = match self.last_panel_rect {
            Some(r) => r,
            None => return,
        };

        // Find which pane the mouse is over
        let ws = self.active_workspace();
        let pane_id = if let Some(zoomed_id) = ws.zoomed {
            if panel_rect.contains(hover_pos) {
                zoomed_id
            } else {
                return;
            }
        } else {
            let layout = ws.tree.layout(panel_rect);
            match layout
                .iter()
                .find(|(_, rect)| rect.contains(hover_pos))
                .map(|(id, _)| *id)
            {
                Some(id) => id,
                None => return,
            }
        };

        // Resolve cell coordinates from pixel position
        let (cell_w, cell_h) = ctx.fonts(|f| {
            let fid = egui::FontId::monospace(self.font_size);
            (f.glyph_width(&fid, 'M'), f.row_height(&fid))
        });

        #[cfg(feature = "gpu-renderer")]
        let (cell_w, cell_h) = if let Some(gpu) = &self.gpu_renderer {
            let cw = gpu.cell_width();
            let ch = gpu.cell_height();
            if cw > 0.0 && ch > 0.0 {
                (cw, ch)
            } else {
                (cell_w, cell_h)
            }
        } else {
            (cell_w, cell_h)
        };

        if cell_w <= 0.0 || cell_h <= 0.0 {
            return;
        }

        // Get the content rect (below tab bar) for this pane
        let pane_rect = if let Some(zoomed_id) = self.active_workspace().zoomed {
            if zoomed_id == pane_id {
                panel_rect
            } else {
                return;
            }
        } else {
            let layout = self.active_workspace().tree.layout(panel_rect);
            match layout.iter().find(|(id, _)| *id == pane_id) {
                Some((_, r)) => *r,
                None => return,
            }
        };
        let content_top = pane_rect.min.y + TAB_BAR_HEIGHT;
        if hover_pos.y < content_top || hover_pos.x < pane_rect.min.x {
            return;
        }
        let col = ((hover_pos.x - pane_rect.min.x) / cell_w) as usize;
        let row = ((hover_pos.y - content_top) / cell_h) as usize;

        // Check if cell has a hyperlink
        if let Some(managed) = self.panes.get(&pane_id) {
            let surface = managed.active_surface();
            let (cols, rows) = surface.pane.dimensions();
            if col >= cols || row >= rows {
                return;
            }
            let screen = surface.pane.screen();
            let total = screen.scrollback_rows();
            let end = total.saturating_sub(surface.scroll_offset);
            let start = end.saturating_sub(rows);
            let phys_row = start + row;
            let lines = screen.lines_in_phys_range(phys_row..phys_row + 1);
            if let Some(line) = lines.first() {
                for cell in line.visible_cells() {
                    if cell.cell_index() == col {
                        if let Some(link) = cell.attrs().hyperlink() {
                            let url = link.uri().to_string();
                            self.hovered_hyperlink = Some(url.clone());

                            // Set pointer cursor
                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);

                            // Cmd+click opens URL
                            let cmd_held = ctx.input(|i| {
                                #[cfg(target_os = "macos")]
                                {
                                    i.modifiers.mac_cmd || i.modifiers.command
                                }
                                #[cfg(not(target_os = "macos"))]
                                {
                                    i.modifiers.ctrl
                                }
                            });
                            if cmd_held && ctx.input(|i| i.pointer.primary_clicked()) {
                                // Only open safe URL schemes.
                                if url.starts_with("http://")
                                    || url.starts_with("https://")
                                    || url.starts_with("mailto:")
                                {
                                    let _ = open::that(&url);
                                }
                            }
                        }
                        break;
                    }
                }
            }
        }
    }

    fn enter_copy_mode(&mut self) {
        let pane_id = self.focused_pane_id();
        if let Some(managed) = self.panes.get(&pane_id) {
            let surface = managed.active_surface();
            let cursor = surface.pane.cursor();
            let screen = surface.pane.screen();
            let (_, rows) = surface.pane.dimensions();
            let total = screen.scrollback_rows();
            let end = total.saturating_sub(surface.scroll_offset);
            let start = end.saturating_sub(rows);
            // Place copy mode cursor at terminal cursor position in phys coords
            let phys_row = start + (cursor.y.max(0) as usize).min(rows.saturating_sub(1));
            self.copy_mode = Some(CopyModeState {
                pane_id,
                cursor: (cursor.x, phys_row),
                visual_anchor: None,
                line_visual: false,
            });
        }
    }

    fn handle_copy_mode_key(&mut self, key: &egui::Key, modifiers: &egui::Modifiers) -> bool {
        let cm = match self.copy_mode.as_mut() {
            Some(cm) => cm,
            None => return false,
        };
        let pane_id = cm.pane_id;

        // Get dimensions for bounds checking
        let (cols, rows, total_rows) = match self.panes.get(&pane_id) {
            Some(m) => {
                let s = m.active_surface();
                let (c, r) = s.pane.dimensions();
                let t = s.pane.screen().scrollback_rows();
                (c, r, t)
            }
            None => {
                self.copy_mode = None;
                return true;
            }
        };
        let cm = self.copy_mode.as_mut().unwrap();

        match key {
            // Exit copy mode
            egui::Key::Escape | egui::Key::Q => {
                self.copy_mode = None;
                return true;
            }
            // Movement
            egui::Key::H | egui::Key::ArrowLeft => {
                cm.cursor.0 = cm.cursor.0.saturating_sub(1);
            }
            egui::Key::L | egui::Key::ArrowRight => {
                cm.cursor.0 = (cm.cursor.0 + 1).min(cols.saturating_sub(1));
            }
            egui::Key::K | egui::Key::ArrowUp => {
                cm.cursor.1 = cm.cursor.1.saturating_sub(1);
            }
            egui::Key::J | egui::Key::ArrowDown => {
                cm.cursor.1 = (cm.cursor.1 + 1).min(total_rows.saturating_sub(1));
            }
            // Half-page up/down
            egui::Key::U if modifiers.ctrl => {
                let half = rows / 2;
                cm.cursor.1 = cm.cursor.1.saturating_sub(half);
            }
            egui::Key::D if modifiers.ctrl => {
                let half = rows / 2;
                cm.cursor.1 = (cm.cursor.1 + half).min(total_rows.saturating_sub(1));
            }
            // End of scrollback (Shift+G = vim 'G')
            egui::Key::G if modifiers.shift => {
                cm.cursor.1 = total_rows.saturating_sub(1);
                cm.cursor.0 = 0;
            }
            // Start of scrollback (g = vim 'gg', second g handled by repeat)
            egui::Key::G => {
                cm.cursor.1 = 0;
                cm.cursor.0 = 0;
            }
            // Line start/end
            egui::Key::Num0 => {
                cm.cursor.0 = 0;
            }
            // Visual mode toggle
            egui::Key::V if modifiers.shift => {
                // Line visual
                if cm.line_visual {
                    cm.visual_anchor = None;
                    cm.line_visual = false;
                } else {
                    cm.visual_anchor = Some(cm.cursor);
                    cm.line_visual = true;
                }
            }
            egui::Key::V => {
                // Character visual
                if cm.visual_anchor.is_some() && !cm.line_visual {
                    cm.visual_anchor = None;
                } else {
                    cm.visual_anchor = Some(cm.cursor);
                    cm.line_visual = false;
                }
            }
            // Yank selection
            egui::Key::Y => {
                let anchor = cm.visual_anchor;
                let line_visual = cm.line_visual;
                if let Some(anchor) = anchor {
                    let text = self.extract_copy_mode_text(pane_id, anchor, cols, line_visual);
                    if let Some(text) = text {
                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                            let _ = clipboard.set_text(&text);
                        }
                    }
                    self.copy_mode = None;
                    return true;
                }
            }
            _ => {}
        }

        // Scroll viewport to keep cursor visible
        if let Some(managed) = self.panes.get_mut(&pane_id) {
            let cm = self.copy_mode.as_ref().unwrap();
            let surface = managed.active_surface_mut();
            let end = total_rows.saturating_sub(surface.scroll_offset);
            let start = end.saturating_sub(rows);
            if cm.cursor.1 < start {
                surface.scroll_offset = total_rows.saturating_sub(cm.cursor.1 + rows);
            } else if cm.cursor.1 >= end {
                surface.scroll_offset = total_rows.saturating_sub(cm.cursor.1 + 1);
            }
        }

        true
    }

    fn extract_copy_mode_text(
        &self,
        pane_id: PaneId,
        anchor: (usize, usize),
        cols: usize,
        line_visual: bool,
    ) -> Option<String> {
        let cm = self.copy_mode.as_ref()?;
        let managed = self.panes.get(&pane_id)?;
        let surface = managed.active_surface();
        let screen = surface.pane.screen();

        let (start, end) =
            if anchor.1 < cm.cursor.1 || (anchor.1 == cm.cursor.1 && anchor.0 <= cm.cursor.0) {
                (anchor, cm.cursor)
            } else {
                (cm.cursor, anchor)
            };

        let lines = screen.lines_in_phys_range(start.1..end.1 + 1);
        let mut result = String::new();

        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                result.push('\n');
            }
            let phys_row = start.1 + i;
            for cell in line.visible_cells() {
                let col = cell.cell_index();
                if col >= cols {
                    break;
                }
                if line_visual {
                    result.push_str(cell.str());
                } else {
                    // Character visual: clip to selection bounds
                    if phys_row == start.1 && col < start.0 {
                        continue;
                    }
                    if phys_row == end.1 && col > end.0 {
                        break;
                    }
                    result.push_str(cell.str());
                }
            }
        }

        // Trim trailing whitespace per line
        let trimmed: Vec<&str> = result.lines().map(|l| l.trim_end()).collect();
        Some(trimmed.join("\n"))
    }

    fn render_rename_modal(&mut self, ctx: &egui::Context) {
        let mut apply: Option<String> = None;
        let mut cancel = false;

        let title = match &self.rename_modal.as_ref().unwrap().target {
            RenameTarget::Workspace(_) => "Rename Workspace",
            RenameTarget::Tab { .. } => "Rename Tab",
        };

        let modal = self.rename_modal.as_mut().unwrap();
        let just_opened = modal.just_opened;

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([280.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    let response = ui.text_edit_singleline(&mut modal.buf);
                    if just_opened {
                        response.request_focus();
                        modal.just_opened = false;
                    }
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        apply = Some(modal.buf.trim().to_string());
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        apply = Some(modal.buf.trim().to_string());
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        // Also close on Escape
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }

        if let Some(new_name) = apply {
            if !new_name.is_empty() {
                match &self.rename_modal.as_ref().unwrap().target {
                    RenameTarget::Workspace(ws_id) => {
                        let ws_id = *ws_id;
                        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id) {
                            ws.title = new_name;
                        }
                    }
                    RenameTarget::Tab {
                        pane_id,
                        surface_id,
                    } => {
                        let pane_id = *pane_id;
                        let surface_id = *surface_id;
                        if let Some(managed) = self.panes.get_mut(&pane_id) {
                            if let Some(surface) =
                                managed.surfaces.iter_mut().find(|s| s.id == surface_id)
                            {
                                surface.user_title = Some(new_name);
                            }
                        }
                    }
                }
            }
            self.rename_modal = None;
        } else if cancel {
            self.rename_modal = None;
        }
    }

    fn render_find_bar(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let mut navigate: Option<isize> = None; // +1 = next, -1 = prev

        egui::Window::new("Find")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_TOP, [-8.0, 8.0])
            .fixed_size([300.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let response =
                        ui.text_edit_singleline(&mut self.find_state.as_mut().unwrap().query);

                    // Auto-focus the text field on first show
                    if let Some(fs) = self.find_state.as_mut() {
                        if fs.just_opened {
                            response.request_focus();
                            fs.just_opened = false;
                        }
                    }

                    // Enter = next, Shift+Enter = prev
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if ui.input(|i| i.modifiers.shift) {
                            navigate = Some(-1);
                        } else {
                            navigate = Some(1);
                        }
                        response.request_focus();
                    }

                    // Trigger search on text change
                    if response.changed() {
                        let find = self.find_state.as_ref().unwrap();
                        let query = find.query.clone();
                        let pane_id = find.pane_id;
                        if let Some(managed) = self.panes.get(&pane_id) {
                            let matches = managed.active_surface().pane.search_scrollback(&query);
                            let find = self.find_state.as_mut().unwrap();
                            find.matches = matches;
                            find.current_match = 0;
                        }
                    }

                    if ui.button("X").clicked() {
                        close = true;
                    }
                });

                // Show match count
                if let Some(find) = &self.find_state {
                    let total = find.matches.len();
                    if total > 0 {
                        ui.horizontal(|ui| {
                            ui.label(format!("{}/{}", find.current_match + 1, total));
                            if ui.button("<").clicked() {
                                navigate = Some(-1);
                            }
                            if ui.button(">").clicked() {
                                navigate = Some(1);
                            }
                        });
                    } else if !find.query.is_empty() {
                        ui.label("No matches");
                    }
                }
            });

        if close {
            self.find_state = None;
            return;
        }

        // Navigate matches
        if let Some(dir) = navigate {
            if let Some(find) = self.find_state.as_mut() {
                if !find.matches.is_empty() {
                    let total = find.matches.len();
                    if dir > 0 {
                        find.current_match = (find.current_match + 1) % total;
                    } else {
                        find.current_match = if find.current_match == 0 {
                            total - 1
                        } else {
                            find.current_match - 1
                        };
                    }

                    // Scroll to the current match
                    let (phys_row, _, _) = find.matches[find.current_match];
                    let pane_id = find.pane_id;
                    if let Some(managed) = self.panes.get_mut(&pane_id) {
                        let surface = managed.active_surface_mut();
                        let (_, rows) = surface.pane.dimensions();
                        let total_rows = surface.pane.screen().scrollback_rows();
                        // Calculate scroll offset to center the match
                        let target_end = phys_row + rows / 2;
                        let actual_end = target_end.min(total_rows);
                        surface.scroll_offset = total_rows.saturating_sub(actual_end);
                    }
                }
            }
        }
    }

    fn render_notification_panel(&mut self, ctx: &egui::Context) {
        let mut close_panel = false;
        let mut mark_all = false;
        let mut jump_to: Option<(u64, u64)> = None; // (workspace_id, pane_id)
        let mut remove_id: Option<u64> = None;

        egui::Window::new("Notifications")
            .collapsible(false)
            .resizable(true)
            .default_size([380.0, 460.0])
            .anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(
                        egui::RichText::new("Notifications").color(egui::Color32::from_gray(220)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Clear All").clicked() {
                            mark_all = true;
                        }
                        if ui.small_button("Jump to Latest").clicked() {
                            jump_to = self
                                .notifications
                                .most_recent_unread()
                                .map(|n| (n.workspace_id, n.pane_id));
                            close_panel = true;
                        }
                    });
                });
                ui.separator();

                let notifications = self.notifications.all_notifications();
                if notifications.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.label(
                            egui::RichText::new("\u{1f515}") // 🔕
                                .size(32.0),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("No notifications")
                                .color(egui::Color32::from_gray(140))
                                .size(14.0),
                        );
                        ui.label(
                            egui::RichText::new("Agent notifications will appear here")
                                .color(egui::Color32::from_gray(80))
                                .size(11.0),
                        );
                    });
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        // Group by workspace, most recent notifications first within each
                        let mut grouped: Vec<_> = notifications.iter().rev().collect();
                        grouped.sort_by_key(|n| n.workspace_id);
                        let mut last_ws_id: Option<u64> = None;
                        for notif in &grouped {
                            // Workspace section header
                            if last_ws_id != Some(notif.workspace_id) {
                                last_ws_id = Some(notif.workspace_id);
                                if let Some(ws) =
                                    self.workspaces.iter().find(|w| w.id == notif.workspace_id)
                                {
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(&ws.title)
                                            .font(bold_font(11.0))
                                            .color(egui::Color32::from_gray(120)),
                                    );
                                    ui.add_space(2.0);
                                }
                            }

                            let response = ui.horizontal(|ui| {
                                // Source icon + unread dot
                                let source_icon = match notif.source {
                                    NotificationSource::Bell => "\u{1f514}",  // 🔔
                                    NotificationSource::Toast => "\u{1f4ac}", // 💬
                                    NotificationSource::Cli => "\u{2328}",    // ⌨
                                };
                                let dot_color = if notif.read {
                                    egui::Color32::from_gray(60)
                                } else {
                                    self.theme.chrome.accent
                                };
                                ui.label(
                                    egui::RichText::new(source_icon).size(10.0).color(dot_color),
                                );

                                ui.vertical(|ui| {
                                    let title = if notif.title.is_empty() {
                                        &notif.body
                                    } else {
                                        &notif.title
                                    };
                                    ui.label(
                                        egui::RichText::new(title)
                                            .color(egui::Color32::from_gray(200)),
                                    );
                                    if !notif.title.is_empty() && !notif.body.is_empty() {
                                        let body_display = if notif.body.len() > 100 {
                                            format!("{}...", &notif.body[..97])
                                        } else {
                                            notif.body.clone()
                                        };
                                        ui.label(
                                            egui::RichText::new(body_display)
                                                .small()
                                                .color(egui::Color32::from_gray(140)),
                                        );
                                    }
                                    let age = format_duration(notif.created_at.elapsed());
                                    ui.label(
                                        egui::RichText::new(age)
                                            .small()
                                            .color(egui::Color32::from_gray(80)),
                                    );
                                });

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("×").clicked() {
                                            remove_id = Some(notif.id);
                                        }
                                    },
                                );
                            });
                            if response.response.interact(egui::Sense::click()).clicked() {
                                jump_to = Some((notif.workspace_id, notif.pane_id));
                                close_panel = true;
                            }
                            ui.separator();
                        }
                    });
                }
            });

        if mark_all {
            self.notifications.mark_all_read();
        }
        if let Some(id) = remove_id {
            self.notifications.remove_notification(id);
        }
        if let Some((ws_id, pane_id)) = jump_to {
            if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) {
                self.active_workspace_idx = idx;
            }
            self.set_focus(pane_id as PaneId);
        }
        if close_panel {
            self.show_notification_panel = false;
        }
    }

    // --- Divider Drag ---

    fn handle_divider_drag(&mut self, ui: &egui::Ui, panel_rect: egui::Rect) {
        let zoomed = self.active_workspace().zoomed;
        if zoomed.is_some() {
            return;
        }

        let dividers = self.active_workspace().tree.dividers(panel_rect);
        let pointer_pos = ui.input(|i| i.pointer.hover_pos());
        let primary_down = ui.input(|i| i.pointer.primary_down());
        let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
        let primary_released = ui.input(|i| i.pointer.primary_released());

        let is_dragging = self.active_workspace().dragging_divider.is_some();

        if let Some(pos) = pointer_pos {
            if !is_dragging {
                if let Some(div) = dividers.iter().find(|d| d.rect.contains(pos)) {
                    match div.direction {
                        SplitDirection::Horizontal => {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                        }
                        SplitDirection::Vertical => {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                        }
                    }
                }
            }
        }

        if primary_pressed && !is_dragging {
            if let Some(pos) = pointer_pos {
                if let Some(div) = dividers.iter().find(|d| d.rect.expand(4.0).contains(pos)) {
                    self.active_workspace_mut().dragging_divider = Some(DragState {
                        node_path: div.node_path.clone(),
                        direction: div.direction,
                    });
                }
            }
        }

        if primary_down {
            let ws = self.active_workspace_mut();
            if let Some(ref drag) = ws.dragging_divider {
                let delta = ui.input(|i| i.pointer.delta());
                let px_delta = match drag.direction {
                    SplitDirection::Horizontal => delta.x,
                    SplitDirection::Vertical => delta.y,
                };
                if px_delta != 0.0 {
                    let path = drag.node_path.clone();
                    let dir = drag.direction;
                    ws.tree.resize_divider(&path, px_delta, panel_rect);
                    match dir {
                        SplitDirection::Horizontal => {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                        }
                        SplitDirection::Vertical => {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                        }
                    }
                }
            }
        }

        if primary_released {
            self.active_workspace_mut().dragging_divider = None;
        }
    }

    // --- Selection ---

    fn copy_selection(&mut self) -> bool {
        let focused = self.focused_pane_id();
        let managed = match self.panes.get_mut(&focused) {
            Some(m) => m,
            None => return false,
        };
        let sel = match &managed.selection {
            Some(s) => s.clone(),
            None => return false,
        };

        let (cols, _) = managed.active_surface().pane.dimensions();
        let (start, end) = sel.normalized();
        let text = extract_selection_text(&managed.active_surface().pane, start, end, cols);

        if text.is_empty() {
            return false;
        }

        match arboard::Clipboard::new() {
            Ok(mut clipboard) => {
                if clipboard.set_text(&text).is_ok() {
                    managed.selection = None; // Only clear on successful copy
                }
            }
            Err(e) => {
                tracing::warn!("Clipboard error: {}", e);
            }
        }
        true
    }

    fn do_paste(&mut self, text: &str) {
        let focused_id = self.focused_pane_id();
        if let Some(managed) = self.panes.get_mut(&focused_id) {
            let surface = managed.active_surface_mut();
            surface.scroll_offset = 0;
            surface.scroll_accum = 0.0;
            if surface.pane.bracketed_paste_enabled() {
                let _ = surface.pane.write_bytes(b"\x1b[200~");
                let _ = surface.pane.write_bytes(text.as_bytes());
                let _ = surface.pane.write_bytes(b"\x1b[201~");
            } else {
                let _ = surface.pane.write_bytes(text.as_bytes());
            }
        }
    }

    fn select_all_visible(&mut self) {
        let focused = self.focused_pane_id();
        let managed = match self.panes.get_mut(&focused) {
            Some(m) => m,
            None => return,
        };
        let surface = managed.active_surface();
        let (cols, visible_rows) = surface.pane.dimensions();
        let total = surface.pane.screen().scrollback_rows();
        let scroll_offset = surface.scroll_offset;
        let end_row = total.saturating_sub(scroll_offset);
        let start_row = end_row.saturating_sub(visible_rows);

        managed.selection = Some(SelectionState {
            anchor: (0, start_row),
            end: (cols.saturating_sub(1), end_row.saturating_sub(1)),
            mode: SelectionMode::Cell,
            active: false,
        });
    }

    fn clear_selection_on_focused(&mut self) {
        let focused = self.focused_pane_id();
        if let Some(m) = self.panes.get_mut(&focused) {
            m.selection = None;
        }
    }

    // --- Selection Mouse ---

    fn handle_selection_mouse(
        &mut self,
        ui: &egui::Ui,
        pane_id: PaneId,
        content_rect: egui::Rect,
    ) -> bool {
        let (cell_width, cell_height) = self.cell_dimensions(ui);

        let managed = match self.panes.get(&pane_id) {
            Some(m) => m,
            None => return false,
        };
        let surface = managed.active_surface();
        let (cols, visible_rows) = surface.pane.dimensions();
        let total_rows = surface.pane.screen().scrollback_rows();
        let scroll_offset = surface.scroll_offset;

        let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
        let primary_down = ui.input(|i| i.pointer.primary_down());
        let primary_released = ui.input(|i| i.pointer.primary_released());
        let pointer_pos = ui.input(|i| i.pointer.hover_pos());

        // Check if we're dragging a divider — skip selection if so
        if self.active_workspace().dragging_divider.is_some() {
            return false;
        }

        if primary_pressed {
            if let Some(pos) = pointer_pos {
                if !content_rect.contains(pos) {
                    // Click outside content — clear any existing selection
                    if let Some(m) = self.panes.get_mut(&pane_id) {
                        m.selection = None;
                    }
                    return true;
                }

                let (col, stable_row) = pointer_to_cell(
                    pos,
                    content_rect,
                    cell_width,
                    cell_height,
                    scroll_offset,
                    total_rows,
                    visible_rows,
                );
                let col = col.min(cols.saturating_sub(1));

                // Click count tracking
                let now = Instant::now();
                let dt = now.duration_since(self.last_click_time).as_millis();
                let dist = (pos - self.last_click_pos).length();
                if dt < 400 && dist < 5.0 {
                    self.click_count = (self.click_count + 1).min(3);
                } else {
                    self.click_count = 1;
                }
                self.last_click_time = now;
                self.last_click_pos = pos;

                let (anchor, end, mode) = match self.click_count {
                    2 => {
                        // Word selection
                        let text =
                            line_text_string(&managed.active_surface().pane, stable_row, cols);
                        let (wstart, wend) = word_bounds_in_line(&text, col);
                        (
                            (wstart, stable_row),
                            (wend, stable_row),
                            SelectionMode::Word,
                        )
                    }
                    3 => {
                        // Line selection
                        (
                            (0, stable_row),
                            (cols.saturating_sub(1), stable_row),
                            SelectionMode::Line,
                        )
                    }
                    _ => {
                        // Cell selection
                        ((col, stable_row), (col, stable_row), SelectionMode::Cell)
                    }
                };

                if let Some(m) = self.panes.get_mut(&pane_id) {
                    m.selection = Some(SelectionState {
                        anchor,
                        end,
                        mode,
                        active: true,
                    });
                }
                return true;
            }
        } else if primary_down {
            // Drag — update selection end
            let has_active_selection = self
                .panes
                .get(&pane_id)
                .and_then(|m| m.selection.as_ref())
                .is_some_and(|s| s.active);

            if has_active_selection {
                if let Some(pos) = pointer_pos {
                    let (col, stable_row) = pointer_to_cell(
                        pos,
                        content_rect,
                        cell_width,
                        cell_height,
                        scroll_offset,
                        total_rows,
                        visible_rows,
                    );
                    let col = col.min(cols.saturating_sub(1));

                    if let Some(m) = self.panes.get_mut(&pane_id) {
                        if let Some(ref mut sel) = m.selection {
                            match sel.mode {
                                SelectionMode::Cell => {
                                    sel.end = (col, stable_row);
                                }
                                SelectionMode::Word => {
                                    let text = line_text_string(
                                        &m.surfaces[m.active_surface_idx].pane,
                                        stable_row,
                                        cols,
                                    );
                                    let (_, wend) = word_bounds_in_line(&text, col);
                                    // Extend: keep anchor word start, update end word boundary
                                    if stable_row > sel.anchor.1
                                        || (stable_row == sel.anchor.1 && col >= sel.anchor.0)
                                    {
                                        sel.end = (wend, stable_row);
                                    } else {
                                        let (wstart, _) = word_bounds_in_line(&text, col);
                                        sel.end = (wstart, stable_row);
                                    }
                                }
                                SelectionMode::Line => {
                                    if stable_row >= sel.anchor.1 {
                                        sel.end = (cols.saturating_sub(1), stable_row);
                                    } else {
                                        sel.end = (0, stable_row);
                                    }
                                }
                            }
                        }
                    }
                    return true;
                }
            }
        } else if primary_released {
            if let Some(m) = self.panes.get_mut(&pane_id) {
                if let Some(ref mut sel) = m.selection {
                    sel.active = false;
                    // If no actual drag (anchor == end), clear selection
                    if sel.anchor == sel.end && sel.mode == SelectionMode::Cell {
                        m.selection = None;
                    }
                }
            }
        }
        false
    }

    // --- Input ---

    fn handle_input(&mut self, ctx: &egui::Context) -> bool {
        let events = ctx.input(|i| i.events.clone());
        let focused_id = self.focused_pane_id();

        // Clear selection when user types
        let has_input = events.iter().any(|e| {
            matches!(
                e,
                egui::Event::Text(_)
                    | egui::Event::Paste(_)
                    | egui::Event::Key { pressed: true, .. }
            )
        });
        if has_input {
            self.clear_selection_on_focused();
        }

        let managed = match self.panes.get_mut(&focused_id) {
            Some(m) => m,
            None => return has_input,
        };
        let surface = managed.active_surface_mut();

        for event in &events {
            match event {
                egui::Event::Text(text) => {
                    surface.scroll_offset = 0;
                    surface.scroll_accum = 0.0;
                    let _ = surface.pane.write_bytes(text.as_bytes());
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if let Some(bytes) = encode_egui_key(key, modifiers) {
                        surface.scroll_offset = 0;
                        surface.scroll_accum = 0.0;
                        let _ = surface.pane.write_bytes(&bytes);
                    }
                }
                egui::Event::Paste(text) => {
                    surface.scroll_offset = 0;
                    surface.scroll_accum = 0.0;
                    if surface.pane.bracketed_paste_enabled() {
                        let _ = surface.pane.write_bytes(b"\x1b[200~");
                        let _ = surface.pane.write_bytes(text.as_bytes());
                        let _ = surface.pane.write_bytes(b"\x1b[201~");
                    } else {
                        let _ = surface.pane.write_bytes(text.as_bytes());
                    }
                }
                egui::Event::Ime(ime_event) => match ime_event {
                    egui::ImeEvent::Commit(text) => {
                        surface.scroll_offset = 0;
                        surface.scroll_accum = 0.0;
                        self.ime_preedit = None;
                        let _ = surface.pane.write_bytes(text.as_bytes());
                    }
                    egui::ImeEvent::Preedit(text) => {
                        self.ime_preedit = if text.is_empty() {
                            None
                        } else {
                            Some(text.clone())
                        };
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        has_input
    }

    // --- IPC ---

    fn process_ipc_commands(&mut self) {
        while let Ok(cmd) = self.ipc_rx.try_recv() {
            let response = self.dispatch_ipc(&cmd.request);
            let _ = cmd.reply_tx.send(response);
        }
    }

    fn dispatch_ipc(&mut self, req: &amux_ipc::Request) -> amux_ipc::Response {
        use amux_ipc::Response;
        match req.method.as_str() {
            "system.ping" => Response::ok(req.id.clone(), serde_json::json!({"pong": true})),
            "system.capabilities" => Response::ok(
                req.id.clone(),
                serde_json::json!({"methods": amux_ipc::methods::METHODS}),
            ),
            "system.identify" => {
                let ws = self.active_workspace();
                let focused = ws.focused_pane;
                let sf_id = self
                    .panes
                    .get(&focused)
                    .map(|m| m.active_surface().id)
                    .unwrap_or(0);
                Response::ok(
                    req.id.clone(),
                    serde_json::json!({
                        "workspace_id": ws.id.to_string(),
                        "surface_id": sf_id.to_string(),
                    }),
                )
            }
            "surface.list" => {
                let mut surfaces = Vec::new();
                for ws in &self.workspaces {
                    for pane_id in ws.tree.iter_panes() {
                        if let Some(managed) = self.panes.get_mut(&pane_id) {
                            for (sf_idx, sf) in managed.surfaces.iter_mut().enumerate() {
                                let (cols, rows) = sf.pane.dimensions();
                                surfaces.push(serde_json::json!({
                                    "id": sf.id.to_string(),
                                    "pane_id": pane_id.to_string(),
                                    "workspace_id": ws.id.to_string(),
                                    "title": sf.pane.title(),
                                    "cols": cols,
                                    "rows": rows,
                                    "alive": sf.pane.is_alive(),
                                    "active": sf_idx == managed.active_surface_idx,
                                }));
                            }
                        }
                    }
                }
                Response::ok(req.id.clone(), serde_json::json!({"surfaces": surfaces}))
            }
            "surface.set_cwd" => {
                match serde_json::from_value::<amux_ipc::methods::SetCwdParams>(req.params.clone())
                {
                    Ok(params) => {
                        let surface = self.resolve_surface_mut(&params.surface_id);
                        match surface {
                            Some(sf) => {
                                sf.metadata.cwd = params.cwd.filter(|s| !s.is_empty());
                                Response::ok(req.id.clone(), serde_json::json!({}))
                            }
                            None => Response::err(req.id.clone(), "not_found", "surface not found"),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.set_git" => {
                match serde_json::from_value::<amux_ipc::methods::SetGitParams>(req.params.clone())
                {
                    Ok(params) => {
                        let surface = self.resolve_surface_mut(&params.surface_id);
                        match surface {
                            Some(sf) => {
                                sf.metadata.git_branch = params.branch;
                                sf.metadata.git_dirty = params.dirty;
                                Response::ok(req.id.clone(), serde_json::json!({}))
                            }
                            None => Response::err(req.id.clone(), "not_found", "surface not found"),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.set_pr" => {
                match serde_json::from_value::<amux_ipc::methods::SetPrParams>(req.params.clone()) {
                    Ok(params) => {
                        let surface = self.resolve_surface_mut(&params.surface_id);
                        match surface {
                            Some(sf) => {
                                sf.metadata.pr_number = params.number;
                                sf.metadata.pr_title = params.title;
                                sf.metadata.pr_state = params.state;
                                Response::ok(req.id.clone(), serde_json::json!({}))
                            }
                            None => Response::err(req.id.clone(), "not_found", "surface not found"),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.send_text" => {
                match serde_json::from_value::<amux_ipc::methods::SendTextParams>(
                    req.params.clone(),
                ) {
                    Ok(params) => {
                        let surface = self.resolve_surface_mut(&params.surface_id);
                        match surface {
                            Some(sf) => match sf.pane.write_bytes(params.text.as_bytes()) {
                                Ok(_) => Response::ok(req.id.clone(), serde_json::json!({})),
                                Err(e) => {
                                    Response::err(req.id.clone(), "write_error", &e.to_string())
                                }
                            },
                            None => Response::err(req.id.clone(), "not_found", "surface not found"),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.read_text" => {
                match serde_json::from_value::<amux_ipc::methods::ReadTextParams>(
                    req.params.clone(),
                ) {
                    Ok(params) => {
                        let surface = self.resolve_surface(&params.surface_id);
                        match surface {
                            Some(sf) => {
                                let text = if let Some(ref line_spec) = params.lines {
                                    sf.pane.read_screen_lines(line_spec, params.ansi)
                                } else if params.ansi {
                                    let (_, rows) = sf.pane.dimensions();
                                    sf.pane.read_scrollback_text(rows)
                                } else {
                                    sf.pane.read_screen_text()
                                };
                                Response::ok(req.id.clone(), serde_json::json!({"text": text}))
                            }
                            None => Response::err(req.id.clone(), "not_found", "surface not found"),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.split" => {
                #[derive(serde::Deserialize)]
                struct SplitParams {
                    #[serde(default = "default_direction")]
                    direction: String,
                }
                fn default_direction() -> String {
                    "right".to_string()
                }
                match serde_json::from_value::<SplitParams>(req.params.clone()) {
                    Ok(params) => {
                        let dir = match params.direction.as_str() {
                            "down" | "vertical" => SplitDirection::Vertical,
                            _ => SplitDirection::Horizontal,
                        };
                        match self.spawn_pane_with_surface() {
                            Some(new_id) => {
                                let ws = self.active_workspace_mut();
                                if ws.tree.split(ws.focused_pane, dir, new_id) {
                                    self.set_focus(new_id);
                                    Response::ok(
                                        req.id.clone(),
                                        serde_json::json!({"pane_id": new_id.to_string()}),
                                    )
                                } else {
                                    self.panes.remove(&new_id);
                                    Response::err(
                                        req.id.clone(),
                                        "split_failed",
                                        "failed to split pane tree",
                                    )
                                }
                            }
                            None => Response::err(
                                req.id.clone(),
                                "spawn_failed",
                                "failed to spawn pane",
                            ),
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.close" => {
                #[derive(serde::Deserialize)]
                struct CloseParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                }
                match serde_json::from_value::<CloseParams>(req.params.clone()) {
                    Ok(params) => {
                        let focused = self.focused_pane_id();
                        let target = params
                            .pane_id
                            .and_then(|s| s.parse::<PaneId>().ok())
                            .unwrap_or(focused);
                        let ws = self.active_workspace_mut();
                        if ws.tree.iter_panes().len() <= 1 {
                            return Response::err(
                                req.id.clone(),
                                "last_pane",
                                "cannot close the last pane",
                            );
                        }
                        if let Some(new_focus) = ws.tree.close(target) {
                            let should_refocus = ws.focused_pane == target;
                            ws.last_pane_sizes.remove(&target);
                            if ws.zoomed == Some(target) {
                                ws.zoomed = None;
                            }
                            self.panes.remove(&target);
                            self.notifications.remove_pane(target);
                            if should_refocus {
                                self.set_focus(new_focus);
                            }
                            Response::ok(
                                req.id.clone(),
                                serde_json::json!({"focused": self.focused_pane_id().to_string()}),
                            )
                        } else {
                            Response::err(req.id.clone(), "not_found", "pane not found in tree")
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.focus" => {
                #[derive(serde::Deserialize)]
                struct FocusParams {
                    pane_id: String,
                }
                match serde_json::from_value::<FocusParams>(req.params.clone()) {
                    Ok(params) => match params.pane_id.parse::<PaneId>() {
                        Ok(id) if self.active_workspace().tree.contains(id) => {
                            self.set_focus(id);
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        }
                        _ => Response::err(req.id.clone(), "not_found", "pane not found"),
                    },
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "pane.list" => {
                let ws = self.active_workspace();
                let pane_ids = ws.tree.iter_panes();
                let focused = ws.focused_pane;
                let mut pane_list = Vec::new();
                for id in &pane_ids {
                    if let Some(managed) = self.panes.get_mut(id) {
                        let sf = managed.active_surface_mut();
                        let (cols, rows) = sf.pane.dimensions();
                        let alive = sf.pane.is_alive();
                        let tab_count = managed.surfaces.len();
                        pane_list.push(serde_json::json!({
                            "id": id.to_string(),
                            "focused": *id == focused,
                            "cols": cols,
                            "rows": rows,
                            "alive": alive,
                            "tab_count": tab_count,
                        }));
                    }
                }
                Response::ok(req.id.clone(), serde_json::json!({"panes": pane_list}))
            }
            "workspace.create" => {
                #[derive(serde::Deserialize)]
                struct CreateParams {
                    #[serde(default)]
                    title: Option<String>,
                }
                match serde_json::from_value::<CreateParams>(req.params.clone()) {
                    Ok(params) => {
                        if let Some(ws_id) = self.create_workspace(params.title) {
                            Response::ok(
                                req.id.clone(),
                                serde_json::json!({"workspace_id": ws_id.to_string()}),
                            )
                        } else {
                            Response::err(
                                req.id.clone(),
                                "spawn_failed",
                                "Failed to spawn pane for workspace",
                            )
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "workspace.list" => {
                let list: Vec<serde_json::Value> = self
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(idx, ws)| {
                        serde_json::json!({
                            "id": ws.id.to_string(),
                            "title": ws.title,
                            "pane_count": ws.tree.iter_panes().len(),
                            "active": idx == self.active_workspace_idx,
                        })
                    })
                    .collect();
                Response::ok(req.id.clone(), serde_json::json!({"workspaces": list}))
            }
            "workspace.close" => {
                #[derive(serde::Deserialize)]
                struct CloseParams {
                    workspace_id: String,
                }
                match serde_json::from_value::<CloseParams>(req.params.clone()) {
                    Ok(params) => {
                        if let Some(idx) = self
                            .workspaces
                            .iter()
                            .position(|ws| ws.id.to_string() == params.workspace_id)
                        {
                            if self.workspaces.len() <= 1 {
                                return Response::err(
                                    req.id.clone(),
                                    "last_workspace",
                                    "cannot close the last workspace",
                                );
                            }
                            self.close_workspace_at(idx);
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        } else {
                            Response::err(req.id.clone(), "not_found", "workspace not found")
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "workspace.focus" => {
                #[derive(serde::Deserialize)]
                struct FocusParams {
                    workspace_id: String,
                }
                match serde_json::from_value::<FocusParams>(req.params.clone()) {
                    Ok(params) => {
                        if let Some(idx) = self
                            .workspaces
                            .iter()
                            .position(|ws| ws.id.to_string() == params.workspace_id)
                        {
                            self.active_workspace_idx = idx;
                            Response::ok(req.id.clone(), serde_json::json!({}))
                        } else {
                            Response::err(req.id.clone(), "not_found", "workspace not found")
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.create" => {
                #[derive(serde::Deserialize)]
                struct CreateParams {
                    #[serde(default)]
                    pane_id: Option<String>,
                }
                match serde_json::from_value::<CreateParams>(req.params.clone()) {
                    Ok(params) => {
                        let target_pane = params
                            .pane_id
                            .and_then(|s| s.parse::<PaneId>().ok())
                            .unwrap_or(self.focused_pane_id());

                        // Validate pane exists before spawning a surface
                        if !self.panes.contains_key(&target_pane) {
                            Response::err(req.id.clone(), "not_found", "pane not found")
                        } else {
                            // Find the workspace that owns this pane
                            let ws_id = self
                                .workspaces
                                .iter()
                                .find(|ws| ws.tree.iter_panes().contains(&target_pane))
                                .map(|ws| ws.id)
                                .unwrap_or_else(|| self.active_workspace().id);

                            let sf_id = self.next_surface_id;
                            self.next_surface_id += 1;

                            match spawn_surface(
                                80,
                                24,
                                &self.socket_addr,
                                &self.config,
                                ws_id,
                                sf_id,
                                None,
                                None,
                            ) {
                                Ok(surface) => {
                                    let managed = self.panes.get_mut(&target_pane).unwrap();
                                    managed.surfaces.push(surface);
                                    managed.active_surface_idx = managed.surfaces.len() - 1;
                                    Response::ok(
                                        req.id.clone(),
                                        serde_json::json!({"surface_id": sf_id.to_string()}),
                                    )
                                }
                                Err(e) => {
                                    Response::err(req.id.clone(), "spawn_error", &e.to_string())
                                }
                            }
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.close" => {
                #[derive(serde::Deserialize)]
                struct CloseParams {
                    #[serde(default)]
                    surface_id: Option<String>,
                }
                match serde_json::from_value::<CloseParams>(req.params.clone()) {
                    Ok(params) => {
                        let focused = self.focused_pane_id();
                        if let Some(sf_id_str) = params.surface_id {
                            // Find and close the specific surface
                            if let Ok(sf_id) = sf_id_str.parse::<u64>() {
                                // Find which pane owns this surface
                                let target = self.panes.iter().find_map(|(&pid, m)| {
                                    m.surfaces
                                        .iter()
                                        .position(|s| s.id == sf_id)
                                        .map(|idx| (pid, idx))
                                });
                                if let Some((pane_id, idx)) = target {
                                    let managed = self.panes.get_mut(&pane_id).unwrap();
                                    if managed.surfaces.len() <= 1 {
                                        self.close_pane(pane_id);
                                    } else {
                                        managed.surfaces.remove(idx);
                                        if managed.active_surface_idx >= managed.surfaces.len() {
                                            managed.active_surface_idx = managed.surfaces.len() - 1;
                                        }
                                    }
                                    Response::ok(req.id.clone(), serde_json::json!({}))
                                } else {
                                    Response::err(req.id.clone(), "not_found", "surface not found")
                                }
                            } else {
                                Response::err(
                                    req.id.clone(),
                                    "invalid_params",
                                    "invalid surface_id",
                                )
                            }
                        } else {
                            // Close active surface in focused pane
                            if let Some(managed) = self.panes.get_mut(&focused) {
                                if managed.surfaces.len() <= 1 {
                                    self.close_pane(focused);
                                } else {
                                    managed.surfaces.remove(managed.active_surface_idx);
                                    if managed.active_surface_idx >= managed.surfaces.len() {
                                        managed.active_surface_idx = managed.surfaces.len() - 1;
                                    }
                                }
                                Response::ok(req.id.clone(), serde_json::json!({}))
                            } else {
                                Response::err(req.id.clone(), "not_found", "pane not found")
                            }
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "surface.focus" => {
                #[derive(serde::Deserialize)]
                struct FocusParams {
                    surface_id: String,
                }
                match serde_json::from_value::<FocusParams>(req.params.clone()) {
                    Ok(params) => {
                        if let Ok(sf_id) = params.surface_id.parse::<u64>() {
                            // Find the pane that owns this surface
                            let found = self.panes.iter_mut().find_map(|(pid, managed)| {
                                managed
                                    .surfaces
                                    .iter()
                                    .position(|s| s.id == sf_id)
                                    .map(|idx| (*pid, idx))
                            });
                            if let Some((pid, idx)) = found {
                                self.panes.get_mut(&pid).unwrap().active_surface_idx = idx;
                                // Switch to the owning workspace before setting focus
                                if let Some(ws_idx) = self
                                    .workspaces
                                    .iter()
                                    .position(|ws| ws.tree.iter_panes().contains(&pid))
                                {
                                    self.active_workspace_idx = ws_idx;
                                }
                                self.set_focus(pid);
                                return Response::ok(req.id.clone(), serde_json::json!({}));
                            }
                            Response::err(req.id.clone(), "not_found", "surface not found")
                        } else {
                            Response::err(req.id.clone(), "invalid_params", "invalid surface_id")
                        }
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "status.set" => {
                match serde_json::from_value::<amux_ipc::methods::StatusSetParams>(
                    req.params.clone(),
                ) {
                    Ok(params) => {
                        let ws_id = params.workspace_id.parse::<u64>().unwrap_or(0);
                        let state = match params.state.as_str() {
                            "active" => amux_notify::AgentState::Active,
                            "waiting" => amux_notify::AgentState::Waiting,
                            _ => amux_notify::AgentState::Idle,
                        };
                        self.notifications.set_status(
                            ws_id,
                            state,
                            params.label,
                            params.task,
                            params.message,
                        );
                        Response::ok(req.id.clone(), serde_json::json!({}))
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "notify.send" => {
                match serde_json::from_value::<amux_ipc::methods::NotifySendParams>(
                    req.params.clone(),
                ) {
                    Ok(params) => {
                        let ws_id = params.workspace_id.parse::<u64>().unwrap_or(0);
                        let pane_id = params
                            .pane_id
                            .parse::<u64>()
                            .unwrap_or(self.focused_pane_id());
                        let title = params.title.unwrap_or_default();
                        let nid = self.deliver_notification(
                            ws_id,
                            pane_id,
                            0,
                            title,
                            params.body,
                            NotificationSource::Cli,
                            false,
                        );
                        Response::ok(req.id.clone(), serde_json::json!({"notification_id": nid}))
                    }
                    Err(e) => Response::err(req.id.clone(), "invalid_params", &e.to_string()),
                }
            }
            "notify.list" => {
                let entries: Vec<serde_json::Value> = self
                    .notifications
                    .all_notifications()
                    .iter()
                    .map(|n| {
                        let source_str = match n.source {
                            NotificationSource::Toast => "toast",
                            NotificationSource::Bell => "bell",
                            NotificationSource::Cli => "cli",
                        };
                        serde_json::json!({
                            "id": n.id,
                            "workspace_id": n.workspace_id.to_string(),
                            "pane_id": n.pane_id.to_string(),
                            "title": n.title,
                            "body": n.body,
                            "source": source_str,
                            "read": n.read,
                        })
                    })
                    .collect();
                Response::ok(
                    req.id.clone(),
                    serde_json::json!({"notifications": entries}),
                )
            }
            "notify.clear" => {
                self.notifications.clear_all();
                Response::ok(req.id.clone(), serde_json::json!({}))
            }
            "session.save" => {
                self.flush_pending_io();
                let data = self.build_session_data();
                match amux_session::save(&data) {
                    Ok(()) => Response::ok(
                        req.id.clone(),
                        serde_json::json!({"path": amux_session::session_path().to_string_lossy()}),
                    ),
                    Err(e) => Response::err(req.id.clone(), "save_failed", &e.to_string()),
                }
            }
            _ => Response::err(
                req.id.clone(),
                "method_not_found",
                &format!("unknown method: {}", req.method),
            ),
        }
    }

    fn resolve_surface_mut(&mut self, surface_id: &str) -> Option<&mut PaneSurface> {
        if surface_id == "default" || surface_id.is_empty() {
            let focused = self.focused_pane_id();
            return self.panes.get_mut(&focused).map(|m| m.active_surface_mut());
        }

        // Try as surface ID first: find which pane contains it
        if let Ok(sf_id) = surface_id.parse::<u64>() {
            let target_pane = self
                .panes
                .iter()
                .find(|(_, m)| m.surfaces.iter().any(|s| s.id == sf_id))
                .map(|(pid, _)| *pid);

            if let Some(pid) = target_pane {
                return self
                    .panes
                    .get_mut(&pid)
                    .and_then(|m| m.surfaces.iter_mut().find(|s| s.id == sf_id));
            }

            // Fall back to treating it as a pane ID
            if let Ok(pane_id) = surface_id.parse::<PaneId>() {
                return self.panes.get_mut(&pane_id).map(|m| m.active_surface_mut());
            }
        }

        None
    }

    fn resolve_surface(&self, surface_id: &str) -> Option<&PaneSurface> {
        if surface_id == "default" || surface_id.is_empty() {
            let focused = self.focused_pane_id();
            self.panes.get(&focused).map(|m| m.active_surface())
        } else if let Ok(sf_id) = surface_id.parse::<u64>() {
            for managed in self.panes.values() {
                if let Some(sf) = managed.surfaces.iter().find(|s| s.id == sf_id) {
                    return Some(sf);
                }
            }
            if let Ok(pane_id) = surface_id.parse::<PaneId>() {
                self.panes.get(&pane_id).map(|m| m.active_surface())
            } else {
                None
            }
        } else {
            None
        }
    }
}

// --- Selection helpers ---

/// Convert a pointer position to terminal cell coordinates (col, stable_row).
fn pointer_to_cell(
    pointer_pos: egui::Pos2,
    content_rect: egui::Rect,
    cell_width: f32,
    cell_height: f32,
    scroll_offset: usize,
    total_rows: usize,
    visible_rows: usize,
) -> (usize, usize) {
    let col = ((pointer_pos.x - content_rect.min.x) / cell_width)
        .floor()
        .max(0.0) as usize;
    let visible_row = ((pointer_pos.y - content_rect.min.y) / cell_height)
        .floor()
        .max(0.0) as usize;
    let visible_row = visible_row.min(visible_rows.saturating_sub(1));
    let stable_row = total_rows
        .saturating_sub(visible_rows)
        .saturating_sub(scroll_offset)
        + visible_row;
    (col, stable_row)
}

/// Find word boundaries around a column in a line's text.
/// Returns (start_col, end_col) inclusive.
fn word_bounds_in_line(line_text: &str, col: usize) -> (usize, usize) {
    let chars: Vec<char> = line_text.chars().collect();
    if chars.is_empty() || col >= chars.len() {
        return (col, col);
    }

    let is_delim = |ch: char| WORD_DELIMITERS.contains(ch);
    let at_delim = is_delim(chars[col]);

    // Walk left
    let mut start = col;
    while start > 0 && is_delim(chars[start - 1]) == at_delim {
        start -= 1;
    }

    // Walk right
    let mut end = col;
    while end + 1 < chars.len() && is_delim(chars[end + 1]) == at_delim {
        end += 1;
    }

    (start, end)
}

/// Extract text from the terminal screen within a selection range.
fn extract_selection_text(
    pane: &TerminalPane,
    start: (usize, usize),
    end: (usize, usize),
    cols: usize,
) -> String {
    let screen = pane.screen();
    let lines = screen.lines_in_phys_range(start.1..end.1 + 1);
    let mut result = String::new();

    for (i, line) in lines.iter().enumerate() {
        let row = start.1 + i;
        let mut line_text = String::new();
        for cell in line.visible_cells() {
            let ci = cell.cell_index();
            if ci >= cols {
                break;
            }
            // Determine if this cell is in the selection
            let in_sel = if start.1 == end.1 {
                ci >= start.0 && ci <= end.0
            } else if row == start.1 {
                ci >= start.0
            } else if row == end.1 {
                ci <= end.0
            } else {
                true
            };
            if in_sel {
                line_text.push_str(cell.str());
            }
        }

        if i > 0 {
            // Check if previous line was wrapped — if so, don't add newline
            if i > 0 {
                let prev_line = &lines[i - 1];
                if !prev_line.last_cell_was_wrapped() {
                    result.push('\n');
                }
            }
        }
        result.push_str(line_text.trim_end());
    }

    result
}

/// Build a flat string of a line's cell text for word boundary detection.
fn line_text_string(pane: &TerminalPane, stable_row: usize, cols: usize) -> String {
    let screen = pane.screen();
    let lines = screen.lines_in_phys_range(stable_row..stable_row + 1);
    if lines.is_empty() {
        return String::new();
    }
    let line = &lines[0];
    let mut text = String::new();
    for cell in line.visible_cells() {
        if cell.cell_index() >= cols {
            break;
        }
        let s = cell.str();
        if s.is_empty() {
            text.push(' ');
        } else {
            text.push_str(s);
        }
    }
    text
}

// --- Rendering ---

#[allow(clippy::too_many_arguments)]
fn render_pane(
    ui: &mut egui::Ui,
    pane: &mut TerminalPane,
    rect: egui::Rect,
    is_focused: bool,
    scroll_offset: usize,
    selection: Option<&SelectionState>,
    font_size: f32,
    find_highlights: &[(usize, usize, usize)],
    current_highlight: Option<usize>,
    #[cfg(feature = "gpu-renderer")] gpu_renderer: Option<&GpuRenderer>,
    #[cfg(feature = "gpu-renderer")] pane_id: u64,
) {
    // GPU renderer path: build snapshot, emit a paint callback, and return early.
    #[cfg(feature = "gpu-renderer")]
    if let Some(gpu) = gpu_renderer {
        let (actual_cols, actual_rows) = pane.dimensions();
        if actual_cols == 0 || actual_rows == 0 {
            return;
        }
        let palette = pane.palette();
        let cursor = pane.cursor();
        let screen = pane.screen();
        let gpu_selection = selection.map(|sel| {
            let (start, end) = sel.normalized();
            amux_render_gpu::snapshot::SelectionRange { start, end }
        });
        let seqno = pane.current_seqno();
        let snapshot = amux_render_gpu::TerminalSnapshot::from_screen(
            screen,
            &palette,
            &cursor,
            actual_cols,
            actual_rows,
            scroll_offset,
            is_focused,
            gpu_selection,
            pane_id,
            seqno,
            find_highlights.to_vec(),
            current_highlight,
        );
        let pixels_per_point = ui.ctx().pixels_per_point();
        let callback = gpu.paint_callback(rect, snapshot, pixels_per_point);
        ui.painter().add(egui::Shape::Callback(callback));
        return;
    }

    let font_id = egui::FontId::monospace(font_size);
    let cell_width = ui.fonts(|f| f.glyph_width(&font_id, 'M'));
    let cell_height = ui.fonts(|f| f.row_height(&font_id));

    let (actual_cols, actual_rows) = pane.dimensions();
    if actual_cols == 0 || actual_rows == 0 {
        return;
    }

    let palette = pane.palette();
    let cursor = pane.cursor();
    let screen = pane.screen();

    let painter = ui.painter();
    let origin = rect.min;
    let bg_default = srgba_to_egui(palette.background);

    // Fill the full allocated rect first to avoid artifacts when terminal is smaller
    painter.rect_filled(rect, 0.0, bg_default);

    // Dim unfocused panes with a semi-transparent overlay
    if !is_focused {
        let dim_overlay = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 100);
        painter.rect_filled(rect, 0.0, dim_overlay);
    }

    let total = screen.scrollback_rows();
    let end = total.saturating_sub(scroll_offset);
    let start = end.saturating_sub(actual_rows);
    let lines = screen.lines_in_phys_range(start..end);

    for (row_idx, line) in lines.iter().enumerate() {
        let y = origin.y + row_idx as f32 * cell_height;
        if y + cell_height < rect.min.y || y > rect.max.y {
            continue;
        }

        for cell_ref in line.visible_cells() {
            let col_idx = cell_ref.cell_index();
            if col_idx >= actual_cols {
                break;
            }

            let x = origin.x + col_idx as f32 * cell_width;
            if x + cell_width < rect.min.x || x > rect.max.x {
                continue;
            }

            let attrs = cell_ref.attrs();
            let reverse = attrs.reverse();
            let mut bg =
                srgba_to_egui(resolve_color(&attrs.background(), &palette, false, reverse));
            let mut fg = srgba_to_egui(resolve_color(&attrs.foreground(), &palette, true, reverse));

            // Selection: swap fg/bg for selected cells (reverse video)
            let stable_row = start + row_idx;
            if let Some(sel) = selection {
                if sel.contains(col_idx, stable_row) {
                    std::mem::swap(&mut fg, &mut bg);
                    // Ensure selected empty cells have visible bg
                    if bg == bg_default {
                        bg = srgba_to_egui(palette.foreground);
                        fg = bg_default;
                    }
                }
            }

            // Find/search highlighting
            for (i, &(h_row, h_start, h_end)) in find_highlights.iter().enumerate() {
                if h_row == stable_row && col_idx >= h_start && col_idx < h_end {
                    if current_highlight == Some(i) {
                        bg = egui::Color32::from_rgb(255, 153, 0); // orange
                    } else {
                        bg = egui::Color32::from_rgba_unmultiplied(255, 255, 0, 180);
                        // yellow
                    }
                    fg = egui::Color32::BLACK;
                    break;
                }
            }

            if bg != bg_default {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, y),
                        egui::vec2(cell_width, cell_height),
                    ),
                    0.0,
                    bg,
                );
            }

            let text = cell_ref.str();
            if !text.is_empty() && text != " " {
                painter.text(
                    egui::pos2(x, y),
                    egui::Align2::LEFT_TOP,
                    text,
                    font_id.clone(),
                    fg,
                );
            }
        }
    }

    // Draw cursor
    if is_focused
        && scroll_offset == 0
        && cursor.visibility == CursorVisibility::Visible
        && cursor.y >= 0
        && (cursor.y as usize) < actual_rows
        && cursor.x < actual_cols
    {
        let cx = origin.x + cursor.x as f32 * cell_width;
        let cy = origin.y + cursor.y as f32 * cell_height;
        let cursor_color = srgba_to_egui(palette.cursor_bg);

        match cursor.shape {
            CursorShape::BlinkingBar | CursorShape::SteadyBar => {
                let bar_rect =
                    egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(2.0, cell_height));
                painter.rect_filled(bar_rect, 0.0, cursor_color);
            }
            CursorShape::BlinkingUnderline | CursorShape::SteadyUnderline => {
                let underline_rect = egui::Rect::from_min_size(
                    egui::pos2(cx, cy + cell_height - 2.0),
                    egui::vec2(cell_width, 2.0),
                );
                painter.rect_filled(underline_rect, 0.0, cursor_color);
            }
            CursorShape::Default | CursorShape::BlinkingBlock | CursorShape::SteadyBlock => {
                let cursor_rect = egui::Rect::from_min_size(
                    egui::pos2(cx, cy),
                    egui::vec2(cell_width, cell_height),
                );
                let cursor_fg = srgba_to_egui(palette.cursor_fg);
                painter.rect_filled(cursor_rect, 0.0, cursor_color);

                let cursor_line_idx = cursor.y as usize;
                if cursor_line_idx < lines.len() {
                    let line = &lines[cursor_line_idx];
                    for cell_ref in line.visible_cells() {
                        if cell_ref.cell_index() == cursor.x {
                            let text = cell_ref.str();
                            if !text.is_empty() && text != " " {
                                painter.text(
                                    egui::pos2(cx, cy),
                                    egui::Align2::LEFT_TOP,
                                    text,
                                    font_id.clone(),
                                    cursor_fg,
                                );
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    // Scroll indicator
    if scroll_offset > 0 {
        let indicator = format!("[+{}]", scroll_offset);
        let indicator_font = egui::FontId::monospace(font_size * 0.8);
        let text_color = egui::Color32::from_rgba_unmultiplied(255, 200, 50, 200);
        let bg_color = egui::Color32::from_rgba_unmultiplied(40, 40, 40, 180);

        let galley = painter.layout_no_wrap(indicator, indicator_font, text_color);
        let text_size = galley.size();
        let padding = 4.0;
        let indicator_rect = egui::Rect::from_min_size(
            egui::pos2(
                rect.right() - text_size.x - padding * 2.0,
                rect.top() + padding,
            ),
            egui::vec2(text_size.x + padding * 2.0, text_size.y + padding),
        );
        painter.rect_filled(indicator_rect, 3.0, bg_color);
        painter.galley(
            egui::pos2(
                indicator_rect.left() + padding,
                indicator_rect.top() + padding * 0.5,
            ),
            galley,
            text_color,
        );
    }
}

fn srgba_to_egui(color: SrgbaTuple) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (color.0 * 255.0).round() as u8,
        (color.1 * 255.0).round() as u8,
        (color.2 * 255.0).round() as u8,
        (color.3 * 255.0).round() as u8,
    )
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

// --- Key encoding (egui events -> terminal bytes) ---

fn encode_egui_key(key: &egui::Key, modifiers: &egui::Modifiers) -> Option<Vec<u8>> {
    if modifiers.ctrl && !modifiers.alt {
        if let Some(byte) = ctrl_byte_for_key(key) {
            return Some(vec![byte]);
        }
    }

    if modifiers.alt && !modifiers.ctrl {
        if let Some(ch) = key_to_char(key) {
            return Some(vec![0x1b, ch as u8]);
        }
    }

    if modifiers.ctrl && modifiers.alt {
        if let Some(byte) = ctrl_byte_for_key(key) {
            return Some(vec![0x1b, byte]);
        }
    }

    let modifier_param = egui_modifier_param(modifiers);

    match key {
        egui::Key::Enter => Some(vec![0x0d]),
        egui::Key::Tab => {
            if modifiers.shift {
                Some(b"\x1b[Z".to_vec())
            } else {
                Some(vec![0x09])
            }
        }
        egui::Key::Escape => Some(vec![0x1b]),
        egui::Key::Backspace => {
            if modifiers.alt {
                Some(vec![0x1b, 0x7f])
            } else {
                Some(vec![0x7f])
            }
        }
        egui::Key::Space if modifiers.ctrl => Some(vec![0x00]),

        egui::Key::ArrowUp => Some(encode_arrow(b'A', modifier_param)),
        egui::Key::ArrowDown => Some(encode_arrow(b'B', modifier_param)),
        egui::Key::ArrowRight => Some(encode_arrow(b'C', modifier_param)),
        egui::Key::ArrowLeft => Some(encode_arrow(b'D', modifier_param)),

        egui::Key::Home => Some(encode_csi_letter(b'H', modifier_param)),
        egui::Key::End => Some(encode_csi_letter(b'F', modifier_param)),
        egui::Key::Insert => Some(encode_csi_tilde(2, modifier_param)),
        egui::Key::Delete => Some(encode_csi_tilde(3, modifier_param)),
        egui::Key::PageUp => Some(encode_csi_tilde(5, modifier_param)),
        egui::Key::PageDown => Some(encode_csi_tilde(6, modifier_param)),

        egui::Key::F1 => Some(encode_fn_key(b'P', 11, modifier_param)),
        egui::Key::F2 => Some(encode_fn_key(b'Q', 12, modifier_param)),
        egui::Key::F3 => Some(encode_fn_key(b'R', 13, modifier_param)),
        egui::Key::F4 => Some(encode_fn_key(b'S', 14, modifier_param)),
        egui::Key::F5 => Some(encode_csi_tilde(15, modifier_param)),
        egui::Key::F6 => Some(encode_csi_tilde(17, modifier_param)),
        egui::Key::F7 => Some(encode_csi_tilde(18, modifier_param)),
        egui::Key::F8 => Some(encode_csi_tilde(19, modifier_param)),
        egui::Key::F9 => Some(encode_csi_tilde(20, modifier_param)),
        egui::Key::F10 => Some(encode_csi_tilde(21, modifier_param)),
        egui::Key::F11 => Some(encode_csi_tilde(23, modifier_param)),
        egui::Key::F12 => Some(encode_csi_tilde(24, modifier_param)),

        _ => None,
    }
}

fn encode_arrow(letter: u8, modifier_param: Option<u8>) -> Vec<u8> {
    match modifier_param {
        Some(m) => format!("\x1b[1;{}{}", m, letter as char).into_bytes(),
        None => vec![0x1b, b'[', letter],
    }
}

fn encode_csi_letter(letter: u8, modifier_param: Option<u8>) -> Vec<u8> {
    match modifier_param {
        Some(m) => format!("\x1b[1;{}{}", m, letter as char).into_bytes(),
        None => vec![0x1b, b'[', letter],
    }
}

fn encode_csi_tilde(number: u8, modifier_param: Option<u8>) -> Vec<u8> {
    match modifier_param {
        Some(m) => format!("\x1b[{};{}~", number, m).into_bytes(),
        None => format!("\x1b[{}~", number).into_bytes(),
    }
}

fn encode_fn_key(ss3_letter: u8, csi_number: u8, modifier_param: Option<u8>) -> Vec<u8> {
    match modifier_param {
        Some(m) => format!("\x1b[{};{}~", csi_number, m).into_bytes(),
        None => vec![0x1b, b'O', ss3_letter],
    }
}

fn egui_modifier_param(modifiers: &egui::Modifiers) -> Option<u8> {
    let mut param: u8 = 1;
    if modifiers.shift {
        param += 1;
    }
    if modifiers.alt {
        param += 2;
    }
    if modifiers.ctrl {
        param += 4;
    }
    if param == 1 {
        None
    } else {
        Some(param)
    }
}

fn ctrl_byte_for_key(key: &egui::Key) -> Option<u8> {
    match key {
        egui::Key::A => Some(0x01),
        egui::Key::B => Some(0x02),
        egui::Key::C => Some(0x03),
        egui::Key::D => Some(0x04),
        egui::Key::E => Some(0x05),
        egui::Key::F => Some(0x06),
        egui::Key::G => Some(0x07),
        egui::Key::H => Some(0x08),
        egui::Key::I => Some(0x09),
        egui::Key::J => Some(0x0a),
        egui::Key::K => Some(0x0b),
        egui::Key::L => Some(0x0c),
        egui::Key::M => Some(0x0d),
        egui::Key::N => Some(0x0e),
        egui::Key::O => Some(0x0f),
        egui::Key::P => Some(0x10),
        egui::Key::Q => Some(0x11),
        egui::Key::R => Some(0x12),
        egui::Key::S => Some(0x13),
        egui::Key::T => Some(0x14),
        egui::Key::U => Some(0x15),
        egui::Key::V => Some(0x16),
        egui::Key::W => Some(0x17),
        egui::Key::X => Some(0x18),
        egui::Key::Y => Some(0x19),
        egui::Key::Z => Some(0x1a),
        _ => None,
    }
}

fn key_to_char(key: &egui::Key) -> Option<char> {
    match key {
        egui::Key::A => Some('a'),
        egui::Key::B => Some('b'),
        egui::Key::C => Some('c'),
        egui::Key::D => Some('d'),
        egui::Key::E => Some('e'),
        egui::Key::F => Some('f'),
        egui::Key::G => Some('g'),
        egui::Key::H => Some('h'),
        egui::Key::I => Some('i'),
        egui::Key::J => Some('j'),
        egui::Key::K => Some('k'),
        egui::Key::L => Some('l'),
        egui::Key::M => Some('m'),
        egui::Key::N => Some('n'),
        egui::Key::O => Some('o'),
        egui::Key::P => Some('p'),
        egui::Key::Q => Some('q'),
        egui::Key::R => Some('r'),
        egui::Key::S => Some('s'),
        egui::Key::T => Some('t'),
        egui::Key::U => Some('u'),
        egui::Key::V => Some('v'),
        egui::Key::W => Some('w'),
        egui::Key::X => Some('x'),
        egui::Key::Y => Some('y'),
        egui::Key::Z => Some('z'),
        _ => None,
    }
}
