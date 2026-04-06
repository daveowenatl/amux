mod find_bar;
mod fonts;
mod frame_update;
mod hyperlinks;
mod ime;
mod input;
mod ipc_dispatch;
mod key_encode;
mod layout_ops;
mod managed_pane;
mod menu_bar;
mod notifications_ui;
mod pane_render;
mod rename_modal;
mod render;
mod selection;
mod sidebar;
mod startup;
mod system_notify;
mod theme;
mod workspace_ops;

use std::collections::HashMap;
use std::io::Read;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use amux_core::config::{self, AppConfig};
use amux_core::model::{DragState, SidebarState, Workspace};
use amux_core::shell;
use amux_ipc::IpcCommand;
use amux_layout::{NavDirection, PaneId, PaneTree, SplitDirection};
use amux_notify::{
    flash_alpha, FlashReason, NotificationSource, NotificationStore, FLASH_DURATION,
};
use amux_session::SessionData;
use amux_term::config::AmuxTermConfig;
use amux_term::font;
use amux_term::osc::NotificationEvent;
use amux_term::pane::TerminalPane;
use amux_term::TerminalBackend;
use managed_pane::*;
use portable_pty::CommandBuilder;
use rename_modal::{RenameModal, RenameTarget};

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
const TAB_BAR_HEIGHT: f32 = 26.0;
const TAB_MIN_WIDTH: f32 = 100.0;
const TAB_MAX_WIDTH: f32 = 240.0;
/// Content top inset: tab bar height + 1px border between tab bar and content.
const TAB_CONTENT_TOP_INSET: f32 = TAB_BAR_HEIGHT + 1.0;
/// Visual padding above the tab bar. On macOS with fullSizeContentView,
/// this covers the native title bar area where traffic light buttons sit.
const TERMINAL_TOP_PAD: f32 = 28.0;
/// Visual padding below the terminal grid (does not reduce PTY rows).
/// Painted with terminal background color so it blends with the terminal.
const TERMINAL_BOTTOM_PAD: f32 = 4.0;

fn main() -> anyhow::Result<()> {
    startup::run()
}

// --- App ---

struct AmuxApp {
    workspaces: Vec<Workspace>,
    active_workspace_idx: usize,
    panes: HashMap<PaneId, PaneEntry>,
    next_pane_id: PaneId,
    next_workspace_id: u64,
    next_surface_id: u64,
    sidebar: SidebarState,
    ipc_rx: std::sync::mpsc::Receiver<IpcCommand>,
    event_broadcaster: amux_ipc::EventBroadcaster,
    socket_addr: amux_ipc::IpcAddr,
    socket_token: String,
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
    /// Timestamp of last keystroke — cursor blink resets on input.
    cursor_blink_since: Instant,
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
    /// Pending browser pane creation requests (URL). Processed in update()
    /// where the window handle is available.
    pending_browser_panes: Vec<String>,
    /// Browser panes being restored from session (pane_id already in tree).
    pending_browser_restores: Vec<(PaneId, String)>,
    /// Per-browser-pane omnibar editing state.
    omnibar_state: HashMap<PaneId, OmnibarState>,
}

/// Editing state for a browser pane's omnibar.
struct OmnibarState {
    /// Text currently in the omnibar input.
    text: String,
    /// Whether the omnibar input is focused (editing mode).
    focused: bool,
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
            if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&old_id) {
                managed.active_surface_mut().pane.focus_changed(false);
            }
            // Send DECSET 1004 focus-in to new pane
            if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&pane_id) {
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

    /// Queue a browser pane creation. Deferred to update() where the window handle
    /// is available.
    fn queue_browser_pane(&mut self, url: String) {
        self.pending_browser_panes.push(url);
    }

    /// Open a URL in an existing browser pane, or queue creation of a new one.
    fn open_url_in_browser_pane(&mut self, url: &str) {
        // Find an existing browser pane in the current workspace
        let ws = self.active_workspace();
        let pane_ids = ws.tree.iter_panes();
        let existing_browser = pane_ids
            .iter()
            .find(|&&id| self.panes.get(&id).is_some_and(|e| e.is_browser()))
            .copied();

        if let Some(browser_id) = existing_browser {
            if let Some(PaneEntry::Browser(browser)) = self.panes.get(&browser_id) {
                browser.navigate(url);
            }
            self.set_focus(browser_id);
        } else {
            self.queue_browser_pane(url.to_string());
        }
    }

    /// Process pending browser pane creation requests.
    /// Must be called from update() where `frame` (with HasWindowHandle) is available.
    fn create_pending_browser_panes(&mut self, frame: &eframe::Frame) {
        let bounds = amux_browser::BrowserRect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };

        // New browser panes (from IPC/CLI)
        let urls: Vec<String> = self.pending_browser_panes.drain(..).collect();
        for url in urls {
            let pane_id = self.next_pane_id;
            self.next_pane_id += 1;

            match amux_browser::BrowserPane::new(frame, bounds, &url, None) {
                Ok(browser) => {
                    self.panes.insert(pane_id, PaneEntry::Browser(browser));
                    let ws = self.active_workspace_mut();
                    ws.tree
                        .split(ws.focused_pane, SplitDirection::Vertical, pane_id);
                    ws.focused_pane = pane_id;
                    tracing::info!("Created browser pane {} with URL: {}", pane_id, url);
                }
                Err(e) => {
                    tracing::error!("Failed to create browser pane: {}", e);
                }
            }
        }

        // Restored browser panes (pane_id already in tree)
        let restores: Vec<(PaneId, String)> = self.pending_browser_restores.drain(..).collect();
        for (pane_id, url) in restores {
            match amux_browser::BrowserPane::new(frame, bounds, &url, None) {
                Ok(browser) => {
                    self.panes.insert(pane_id, PaneEntry::Browser(browser));
                    tracing::info!("Restored browser pane {} with URL: {}", pane_id, url);
                }
                Err(e) => {
                    tracing::error!("Failed to restore browser pane {}: {}", pane_id, e);
                }
            }
        }
    }

    /// Drain any pending PTY bytes into the terminal state machine so that
    /// title, working directory, and scrollback are up to date before save.
    fn flush_pending_io(&mut self) {
        for entry in self.panes.values_mut() {
            let PaneEntry::Terminal(managed) = entry else {
                continue;
            };
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
                match self.panes.get(&pane_id) {
                    Some(PaneEntry::Terminal(managed)) => {
                        let surfaces: Vec<amux_session::SavedSurface> = managed
                            .surfaces
                            .iter()
                            .map(|sf| {
                                let working_dir = sf.metadata.cwd.clone().or_else(|| {
                                    sf.pane
                                        .working_dir()
                                        .and_then(|url| url.to_file_path().ok())
                                        .map(|p| p.to_string_lossy().to_string())
                                        .or_else(|| sf.pane.child_pid().and_then(get_cwd_from_pid))
                                });
                                let raw_scrollback = sf
                                    .pane
                                    .read_scrollback_text(amux_session::MAX_SCROLLBACK_LINES);
                                let truncated = amux_session::truncate_scrollback(
                                    &raw_scrollback,
                                    amux_session::MAX_SCROLLBACK_BYTES,
                                );
                                let scrollback = if truncated.len() == raw_scrollback.len() {
                                    raw_scrollback
                                } else {
                                    truncated.to_string()
                                };
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
                                browser: None,
                            },
                        );
                    }
                    Some(PaneEntry::Browser(browser)) => {
                        saved_panes.insert(
                            pane_id,
                            amux_session::SavedManagedPane {
                                panel_type: amux_session::PANEL_TYPE_BROWSER.to_string(),
                                surfaces: vec![],
                                active_surface_idx: 0,
                                browser: Some(amux_session::SavedBrowserPane {
                                    url: browser.url().unwrap_or_default(),
                                    zoom_level: 1.0,
                                    profile: browser.profile().to_string(),
                                }),
                            },
                        );
                    }
                    None => {}
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
                subtitle: n.subtitle.clone(),
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

/// ISO 8601 UTC timestamp for session metadata.
fn chrono_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
