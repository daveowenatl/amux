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
mod tab_icons;
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

const DEFAULT_BROWSER_URL: &str = "https://www.google.com";
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
    /// Pending browser pane creation requests (originating_pane_id, URL).
    /// Processed in update() where the window handle is available.
    pending_browser_panes: Vec<(PaneId, String)>,
    /// Browser panes being restored: (parent_pane_id, browser_pane_id, url).
    pending_browser_restores: Vec<(PaneId, PaneId, String)>,
    /// Per-browser-pane omnibar editing state.
    omnibar_state: HashMap<PaneId, OmnibarState>,
    /// Browser URL history for omnibar autocomplete.
    browser_history: amux_browser::history::BrowserHistory,
    /// Favicon texture cache: favicon URL → egui texture.
    favicon_cache: HashMap<String, egui::TextureHandle>,
    /// In-flight favicon fetches (URL). Prevents duplicate JS fetch requests.
    favicon_pending: std::collections::HashSet<String>,
    /// Clipboard text from a menu-bar Paste action when an egui text field has
    /// focus. The native menu bar consumes Cmd+V before egui sees it, so we
    /// stash the text here and apply it during the text field's render pass.
    pending_text_field_paste: Option<String>,
    /// Whether a menu-bar Select All action is pending for the focused text
    /// field. Applied during the omnibar render pass so that egui can update
    /// the TextEdit cursor selection state.
    pending_text_field_select_all: bool,
}

/// Editing state for a browser pane's omnibar.
struct OmnibarState {
    /// Text currently in the omnibar input.
    text: String,
    /// Whether the omnibar input is focused (editing mode).
    focused: bool,
    /// Last URL recorded in history (to avoid duplicate recordings per frame).
    last_recorded_url: String,
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

            // Send DECSET 1004 focus-out / unfocus browser webview
            if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&old_id) {
                match managed.active_tab() {
                    managed_pane::ActiveTab::Terminal(_) => {
                        if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&old_id) {
                            if let Some(sf) = m.active_surface_mut() {
                                sf.pane.focus_changed(false);
                            }
                        }
                    }
                    managed_pane::ActiveTab::Browser(bid) => {
                        if let Some(PaneEntry::Browser(b)) = self.panes.get(&bid) {
                            b.focus_parent();
                        }
                    }
                }
            }
            // Send DECSET 1004 focus-in / focus browser webview
            if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&pane_id) {
                match managed.active_tab() {
                    managed_pane::ActiveTab::Terminal(_) => {
                        if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&pane_id) {
                            if let Some(sf) = m.active_surface_mut() {
                                sf.pane.focus_changed(true);
                            }
                        }
                    }
                    managed_pane::ActiveTab::Browser(bid) => {
                        if let Some(PaneEntry::Browser(b)) = self.panes.get(&bid) {
                            b.focus();
                        }
                    }
                }
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
    /// is available. `pane_id` is the originating pane — captured now so that
    /// deferred processing attaches the new browser tab to the correct pane even
    /// if focus changes before `create_pending_browser_panes` runs.
    fn queue_browser_pane(&mut self, pane_id: PaneId, url: String) {
        self.pending_browser_panes.push((pane_id, url));
    }

    /// Open a URL in an existing browser tab, or queue creation of a new one.
    fn open_url_in_browser_pane(&mut self, url: &str) {
        // Find an existing browser tab in the current workspace's panes
        let ws = self.active_workspace();
        let pane_ids = ws.tree.iter_panes();
        let existing = pane_ids.iter().find_map(|&tree_pane_id| {
            let managed = self.panes.get(&tree_pane_id)?.as_terminal()?;
            let browser_id = managed.browser_pane_ids().into_iter().next()?;
            Some((tree_pane_id, browser_id))
        });

        if let Some((tree_pane_id, browser_id)) = existing {
            if let Some(PaneEntry::Browser(browser)) = self.panes.get(&browser_id) {
                browser.navigate(url);
            }
            // Switch to the browser tab
            if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&tree_pane_id) {
                if let Some(idx) = managed
                    .tabs
                    .iter()
                    .position(|t| t.browser_pane_id() == Some(browser_id))
                {
                    managed.active_tab_idx = idx;
                }
            }
            self.set_focus(tree_pane_id);
        } else {
            let pane_id = self.focused_pane_id();
            self.queue_browser_pane(pane_id, url.to_string());
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

        let ua = self.app_config.browser.user_agent.clone();
        let dl_dir = self.app_config.browser.download_dir.clone();
        let options = amux_browser::BrowserOptions {
            user_agent: ua.as_deref(),
            download_dir: dl_dir.as_deref(),
        };

        // New browser panes — added as tabs in their originating pane
        let pending: Vec<(PaneId, String)> = self.pending_browser_panes.drain(..).collect();
        for (originating_pane_id, url) in pending {
            let browser_pane_id = self.next_pane_id;
            self.next_pane_id += 1;

            match amux_browser::BrowserPane::new(frame, bounds, &url, None, Some(&options)) {
                Ok(browser) => {
                    browser.focus();
                    self.panes
                        .insert(browser_pane_id, PaneEntry::Browser(browser));
                    // Add as a tab in the originating ManagedPane (not a tree split).
                    // Insert right after the active tab (cmux behavior).
                    if let Some(PaneEntry::Terminal(managed)) =
                        self.panes.get_mut(&originating_pane_id)
                    {
                        let insert_at = (managed.active_tab_idx + 1).min(managed.tabs.len());
                        managed
                            .tabs
                            .insert(insert_at, managed_pane::TabEntry::Browser(browser_pane_id));
                        managed.active_tab_idx = insert_at;
                    }
                    tracing::info!(
                        "Created browser tab {} in pane {} with URL: {}",
                        browser_pane_id,
                        originating_pane_id,
                        url
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to create browser pane: {}", e);
                }
            }
        }

        // Restored browser panes — added as tabs in their parent pane
        let restores: Vec<(PaneId, PaneId, String)> =
            self.pending_browser_restores.drain(..).collect();
        for (parent_pane_id, browser_pane_id, url) in restores {
            match amux_browser::BrowserPane::new(frame, bounds, &url, None, Some(&options)) {
                Ok(browser) => {
                    self.panes
                        .insert(browser_pane_id, PaneEntry::Browser(browser));
                    // tabs list was already populated with Browser entries during
                    // session restore in startup.rs — no need to push again here.
                    tracing::info!(
                        "Restored browser tab {} in pane {} with URL: {}",
                        browser_pane_id,
                        parent_pane_id,
                        url
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to restore browser pane {}: {}", browser_pane_id, e);
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
            for surface in managed.surfaces_mut() {
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
                            .surfaces()
                            .map(|sf| {
                                let working_dir = sf.metadata.cwd.clone().or_else(|| {
                                    sf.pane
                                        .working_dir()
                                        .and_then(|url| url.to_file_path().ok())
                                        .map(|p| p.to_string_lossy().to_string())
                                        .or_else(|| sf.pane.child_pid().and_then(get_cwd_from_pid))
                                });
                                // Prefer VT state snapshot (exact terminal state
                                // reconstruction) over text-based scrollback.
                                let scrollback_vt = sf
                                    .pane
                                    .vt_state_snapshot()
                                    .filter(|bytes| {
                                        bytes.len() <= amux_session::MAX_SCROLLBACK_BYTES
                                    })
                                    .map(|bytes| {
                                        use base64::Engine;
                                        base64::engine::general_purpose::STANDARD.encode(&bytes)
                                    });
                                let scrollback = if scrollback_vt.is_none() {
                                    let raw = sf
                                        .pane
                                        .read_scrollback_text(amux_session::MAX_SCROLLBACK_LINES);
                                    let truncated = amux_session::truncate_scrollback(
                                        &raw,
                                        amux_session::MAX_SCROLLBACK_BYTES,
                                    );
                                    if truncated.len() == raw.len() {
                                        raw
                                    } else {
                                        truncated.to_string()
                                    }
                                } else {
                                    String::new()
                                };
                                let (cols, rows) = sf.pane.dimensions();
                                amux_session::SavedSurface {
                                    id: sf.id,
                                    title: sf.pane.title().to_string(),
                                    working_dir,
                                    scrollback,
                                    scrollback_vt,
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
                        // Save browser tabs within this pane
                        let browser_tabs: Vec<amux_session::SavedBrowserTab> = managed
                            .browser_pane_ids()
                            .iter()
                            .filter_map(|&bid| {
                                let b = self.panes.get(&bid)?.as_browser()?;
                                Some(amux_session::SavedBrowserTab {
                                    pane_id: bid,
                                    url: b.url().unwrap_or_default(),
                                    zoom_level: 1.0,
                                    profile: b.profile().to_string(),
                                })
                            })
                            .collect();
                        saved_panes.insert(
                            pane_id,
                            amux_session::SavedManagedPane {
                                panel_type: managed.panel_type().to_string(),
                                surfaces,
                                active_surface_idx: managed.active_tab_idx,
                                browser: None,
                                browser_tabs,
                            },
                        );
                    }
                    Some(PaneEntry::Browser(_)) => {
                        // Standalone browser entries are saved via their parent
                        // ManagedPane's browser_tabs list. Skip here.
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
