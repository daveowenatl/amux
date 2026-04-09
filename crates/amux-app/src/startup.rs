//! Application bootstrap: CLI parsing, window creation, session restore, and PTY spawning.

use crate::*;

/// Per-user scrollback temp directory with restrictive permissions.
fn scrollback_temp_dir() -> std::path::PathBuf {
    // Prefer XDG_RUNTIME_DIR (per-user, typically 0700) on Linux
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        return std::path::PathBuf::from(runtime).join("amux-scrollback");
    }
    // Fall back to user-specific subdir in system temp
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string());
    std::env::temp_dir().join(format!("amux-scrollback-{user}"))
}

/// Write scrollback text to a temp file for shell-based replay.
/// Returns the file path, or `None` on failure.
fn write_scrollback_temp_file(text: &str) -> Option<std::path::PathBuf> {
    let dir = scrollback_temp_dir();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        if builder.create(&dir).is_err() {
            tracing::warn!("Failed to create scrollback temp dir");
            return None;
        }
    }
    #[cfg(not(unix))]
    if std::fs::create_dir_all(&dir).is_err() {
        tracing::warn!("Failed to create scrollback temp dir");
        return None;
    }

    let filename = format!("{}.txt", uuid::Uuid::new_v4());
    let path = dir.join(filename);

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut f) => {
                if let Err(e) = f.write_all(text.as_bytes()) {
                    tracing::warn!("Failed to write scrollback temp file: {e}");
                    return None;
                }
                // Ensure file ends with newline so shell prompt appears on a new line
                if !text.ends_with('\n') {
                    let _ = f.write_all(b"\n");
                }
            }
            Err(e) => {
                tracing::warn!("Failed to create scrollback temp file: {e}");
                return None;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let content = if text.ends_with('\n') {
            text.to_string()
        } else {
            format!("{text}\n")
        };
        if let Err(e) = std::fs::write(&path, &content) {
            tracing::warn!("Failed to write scrollback temp file: {e}");
            return None;
        }
    }

    Some(path)
}

/// Remove scrollback temp files older than 1 hour.
fn cleanup_stale_scrollback_files() {
    let dir = scrollback_temp_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let Some(cutoff) =
        std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(3600))
    else {
        return;
    };
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                if modified < cutoff {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

pub(crate) fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    cleanup_stale_scrollback_files();

    let mut app_config = config::load_app_config();
    let mut font_size = app_config.font_size;

    // Initialize sound player with configured sound setting
    let mut sound_player = system_notify::SoundPlayer::new();
    if let Some(player) = &mut sound_player {
        player.configure(&app_config.notifications.sound.sound);
    }

    let socket_token = uuid::Uuid::new_v4().to_string();
    let (ipc_rx, ipc_addr, event_broadcaster) = amux_ipc::start_server(socket_token.clone())?;
    tracing::info!("IPC server: {}", ipc_addr);

    let mut theme = match app_config.theme_source.as_str() {
        "ghostty" => {
            if let Some(ghostty_cfg) = amux_ghostty_config::GhosttyConfig::load() {
                // Override font settings from Ghostty config if present.
                if let Some(family) = ghostty_cfg.font_family() {
                    app_config.font_family = family.to_owned();
                }
                if let Some(size) = ghostty_cfg.font_size() {
                    app_config.font_size = config::validate_font_size(size);
                    font_size = app_config.font_size;
                }
                theme::Theme::from_ghostty(&ghostty_cfg)
            } else {
                tracing::warn!(
                    "theme_source = \"ghostty\" but no Ghostty config found, using default"
                );
                theme::Theme::default()
            }
        }
        _ => theme::Theme::default(),
    };

    // Apply user color overrides from [colors] config section
    theme.apply_color_config(&app_config.colors);

    // Resolve keybindings: user overrides merged with platform defaults.
    let keybindings = app_config.keybindings.resolved();

    // FontConfig is only consumed by the GPU renderer; gate to avoid unused
    // warnings in non-GPU builds. Created after theme loading so Ghostty
    // font overrides are picked up.
    #[cfg(feature = "gpu-renderer")]
    let font_config = font::FontConfig {
        family: app_config.font_family.clone(),
        size: app_config.font_size,
    };

    let mut term_config = AmuxTermConfig {
        backend: app_config.backend.clone(),
        ..Default::default()
    };
    theme.apply_to_palette(&mut term_config.color_palette);
    let config = Arc::new(term_config);

    // Try to restore a previous session
    let restored = match amux_session::load() {
        Ok(Some(session)) => {
            tracing::info!("Restoring session from {}", session.saved_at);
            Some(session)
        }
        Ok(None) => None,
        Err(amux_session::SessionError::VersionMismatch { version, expected }) => {
            tracing::warn!(
                "Session version {} not supported (expected {}), starting fresh",
                version,
                expected
            );
            None
        }
        Err(amux_session::SessionError::Corrupted(e)) => {
            tracing::error!("Session file corrupted: {}, starting fresh", e);
            None
        }
        Err(e) => {
            tracing::warn!("Failed to load session, starting fresh: {}", e);
            None
        }
    };

    let state = if let Some(session) = restored {
        restore_session(&session, &ipc_addr, &socket_token, &config)
    } else {
        fresh_startup(&ipc_addr, &socket_token, &config)?
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
            fonts::install_system_font_fallback(&_cc.egui_ctx);

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
                event_broadcaster,
                socket_addr: ipc_addr,
                socket_token,
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
                keybindings,
                system_notifier: system_notify::SystemNotifier::new(),
                last_badge_count: 0,
                cursor_blink_since: Instant::now(),
                sound_player,
                menu,
                #[cfg(target_os = "windows")]
                menu_attached: false,
                #[cfg(feature = "gpu-renderer")]
                gpu_renderer,
                pending_browser_panes: Vec::new(),
                pending_browser_restores: state.pending_browser_restores,
                omnibar_state: HashMap::new(),
                browser_history: amux_browser::history::BrowserHistory::load(),
                favicon_cache: HashMap::new(),
                favicon_pending: std::collections::HashSet::new(),
                pending_text_field_paste: None,
                pending_text_field_select_all: false,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{}", e));

    cleanup_addr(&ipc_addr_cleanup);
    result
}

/// Bundled startup state to avoid complex return tuples.
pub(crate) struct StartupState {
    pub(crate) workspaces: Vec<Workspace>,
    pub(crate) active_workspace_idx: usize,
    pub(crate) panes: HashMap<PaneId, PaneEntry>,
    pub(crate) next_pane_id: PaneId,
    pub(crate) next_workspace_id: u64,
    pub(crate) next_surface_id: u64,
    pub(crate) sidebar: SidebarState,
    pub(crate) notifications: NotificationStore,
    /// Browser panes that need to be created once the window handle is available.
    /// Tuple: (parent_pane_id, saved_tab).
    pub(crate) pending_browser_restores: Vec<(PaneId, amux_session::SavedBrowserTab)>,
}

/// Create a fresh default startup (one workspace, one pane).
pub(crate) fn fresh_startup(
    ipc_addr: &amux_ipc::IpcAddr,
    socket_token: &str,
    config: &Arc<AmuxTermConfig>,
) -> anyhow::Result<StartupState> {
    let initial_pane_id: PaneId = 0;
    let surface = spawn_surface(80, 24, ipc_addr, socket_token, config, 0, 0, None, None)?;

    let managed = PaneEntry::Terminal(ManagedPane {
        tabs: vec![managed_pane::TabEntry::Terminal(Box::new(surface))],
        active_tab_idx: 0,
        selection: None,
    });

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
        pending_browser_restores: Vec::new(),
    })
}

/// Restore app state from a saved session. Falls back to fresh startup on any failure.
pub(crate) fn restore_session(
    session: &SessionData,
    ipc_addr: &amux_ipc::IpcAddr,
    socket_token: &str,
    config: &Arc<AmuxTermConfig>,
) -> StartupState {
    let mut workspaces = Vec::new();
    let mut panes: HashMap<PaneId, PaneEntry> = HashMap::new();
    let mut pending_browser_restores: Vec<(PaneId, amux_session::SavedBrowserTab)> = Vec::new();

    for saved_ws in &session.workspaces {
        for (&pane_id, saved_pane) in &saved_ws.panes {
            // Legacy standalone browser panes: skip (they're now tabs within panes)
            if saved_pane.panel_type == amux_session::PANEL_TYPE_BROWSER {
                tracing::info!(
                    "Skipping legacy standalone browser pane {} (not supported in tab model)",
                    pane_id
                );
                continue;
            }

            if saved_pane.panel_type != amux_session::PANEL_TYPE_TERMINAL {
                tracing::warn!(
                    "Skipping pane {} with unsupported panel type {:?}",
                    pane_id,
                    saved_pane.panel_type,
                );
                continue;
            }
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
                    socket_token,
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

            // Build unified tab list: terminals first, then browser tabs
            // (matches old layout order for backward compat with old sessions)
            let mut tabs: Vec<TabEntry> = surfaces
                .into_iter()
                .map(|s| TabEntry::Terminal(Box::new(s)))
                .collect();
            for bt in &saved_pane.browser_tabs {
                pending_browser_restores.push((pane_id, bt.clone()));
                tabs.push(TabEntry::Browser(bt.pane_id));
            }

            let max_idx = tabs.len().saturating_sub(1);
            let active_idx = saved_pane.active_surface_idx.min(max_idx);
            panes.insert(
                pane_id,
                PaneEntry::Terminal(ManagedPane {
                    tabs,
                    active_tab_idx: active_idx,
                    selection: None,
                }),
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
        return match fresh_startup(ipc_addr, socket_token, config) {
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
        store.push_restored(
            saved_n.workspace_id,
            saved_n.pane_id,
            saved_n.surface_id,
            saved_n.title.clone(),
            saved_n.subtitle.clone(),
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
        pending_browser_restores,
    }
}

pub(crate) fn cleanup_addr(addr: &amux_ipc::IpcAddr) {
    match addr {
        #[cfg(unix)]
        amux_ipc::IpcAddr::Unix(path) => {
            let _ = std::fs::remove_file(path);
        }
        #[cfg(windows)]
        amux_ipc::IpcAddr::NamedPipe(_) => {}
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_surface(
    cols: u16,
    rows: u16,
    ipc_addr: &amux_ipc::IpcAddr,
    socket_token: &str,
    config: &Arc<AmuxTermConfig>,
    workspace_id: u64,
    surface_id: u64,
    cwd: Option<&str>,
    scrollback: Option<&str>,
) -> anyhow::Result<PaneSurface> {
    let shell = shell::default_shell();
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("AMUX_SOCKET_PATH", ipc_addr.to_string());
    cmd.env("AMUX_SOCKET_TOKEN", socket_token);
    cmd.env("AMUX_WORKSPACE_ID", workspace_id.to_string());
    cmd.env("AMUX_SURFACE_ID", surface_id.to_string());
    cmd.env("TERM", "xterm-256color");
    cmd.env("TERM_PROGRAM", "amux");
    cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));

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
    if let Some(bin_dir) = shell::ensure_claude_wrapper_dir() {
        let current_path = std::env::var("PATH").unwrap_or_default();
        let bin_str = bin_dir.to_string_lossy();
        if !current_path.split(':').any(|d| d == bin_str.as_ref()) {
            let sep = if current_path.is_empty() { "" } else { ":" };
            cmd.env("PATH", format!("{bin_str}{sep}{current_path}"));
        }
    }

    // Auto-inject shell integration (matching cmux's ZDOTDIR/PROMPT_COMMAND approach)
    shell::inject_shell_integration(&shell, &mut cmd);

    // Write scrollback to temp file for shell-based replay.
    // Only for shells with integration scripts that will replay and delete the file.
    let shell_name = std::path::Path::new(&shell)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");
    if matches!(shell_name, "zsh" | "bash") {
        if let Some(text) = scrollback {
            if !text.is_empty() {
                if let Some(path) = write_scrollback_temp_file(text) {
                    cmd.env(
                        "AMUX_RESTORE_SCROLLBACK_FILE",
                        path.to_string_lossy().as_ref(),
                    );
                }
            }
        }
    }

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

    let mut pane: amux_term::AnyBackend = match config.backend.as_str() {
        #[cfg(feature = "libghostty")]
        "ghostty" => {
            let mut ghostty_pane = amux_term::ghostty_pane::GhosttyPane::spawn(cols, rows, cmd)?;
            // Apply amux theme colors to the ghostty backend (which otherwise
            // uses libghostty-vt's built-in defaults).
            let palette: amux_term::backend::Palette = config.color_palette.clone().into();
            ghostty_pane.set_palette(palette);
            amux_term::AnyBackend::Ghostty(Box::new(ghostty_pane))
        }
        _ => {
            let wez_pane = TerminalPane::spawn(cols, rows, cmd, config.clone())?;
            amux_term::AnyBackend::Wezterm(Box::new(wez_pane))
        }
    };

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
        exited: None,
    })
}
