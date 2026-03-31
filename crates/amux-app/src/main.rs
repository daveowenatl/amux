mod fonts;
mod input;
mod ipc_dispatch;
mod key_encode;
mod managed_pane;
mod menu_bar;
mod notifications_ui;
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

fn main() -> anyhow::Result<()> {
    startup::run()
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

        // Drain PTY output from all surfaces, with a per-surface byte budget
        // to prevent high-throughput output (e.g. `cat large_file`) from
        // blocking input handling and causing frame drops.
        const MAX_BYTES_PER_SURFACE_PER_FRAME: usize = 64 * 1024;
        let mut got_data = false;
        let mut pending_data = false;
        for managed in self.panes.values_mut() {
            for surface in &mut managed.surfaces {
                let mut bytes_this_frame = 0;
                while bytes_this_frame < MAX_BYTES_PER_SURFACE_PER_FRAME {
                    match surface.byte_rx.try_recv() {
                        Ok(bytes) => {
                            bytes_this_frame += bytes.len();
                            got_data = true;
                            surface.pane.feed_bytes(&bytes);
                        }
                        Err(_) => break,
                    }
                }
                if bytes_this_frame >= MAX_BYTES_PER_SURFACE_PER_FRAME {
                    pending_data = true;
                }
                // Detect process exit once the channel is drained
                if surface.exited.is_none() && !surface.pane.is_alive() {
                    let message = match surface.pane.exit_status() {
                        Some(status) => {
                            if let Some(signal) = status.signal() {
                                format!("Process killed ({signal})")
                            } else if status.success() {
                                "Process exited (code 0)".to_string()
                            } else {
                                format!("Process exited (code {})", status.exit_code())
                            }
                        }
                        None => "Process exited".to_string(),
                    };
                    surface.exited = Some(ExitInfo { message });
                }
            }
        }
        if pending_data {
            ctx.request_repaint();
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
                        egui::pos2(panel_rect.min.x, panel_rect.min.y + TAB_CONTENT_TOP_INSET),
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
                                egui::pos2(rect.min.x, rect.min.y + TAB_CONTENT_TOP_INSET),
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
            let tab_font_bold = fonts::bold_font(11.0);
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

                let is_dead = surface.exited.is_some();

                // Tab background + border
                let border_color = self.theme.chrome.tab_border;
                if is_active {
                    painter.rect_filled(this_tab, 0.0, self.theme.chrome.tab_active_bg);
                    // Active highlight at the top
                    let topline = egui::Rect::from_min_size(
                        egui::pos2(x, tab_rect.min.y),
                        egui::vec2(tab_w, 2.0),
                    );
                    let accent = if is_dead {
                        egui::Color32::from_gray(60)
                    } else {
                        self.theme.chrome.accent
                    };
                    painter.rect_filled(topline, 0.0, accent);
                }
                // 1px border around each tab
                painter.rect_stroke(
                    this_tab,
                    0.0,
                    egui::Stroke::new(1.0, border_color),
                    egui::StrokeKind::Outside,
                );
                let (text_color, text_font) = if is_dead {
                    (egui::Color32::from_gray(80), tab_font.clone())
                } else if is_active {
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
                        if let Ok(surface) = startup::spawn_surface(
                            80,
                            24,
                            &self.socket_addr,
                            &self.socket_token,
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
        render::render_pane(
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

        // Render exit overlay when process has exited
        if let Some(exit_info) = &surface.exited {
            render::render_exit_overlay(ui, content_rect, &exit_info.message, self.font_size);
        }

        // Render copy mode cursor overlay
        if let Some(cm) = self.copy_mode.as_ref().filter(|cm| cm.pane_id == pane_id) {
            let (cell_w, cell_h) = self.cell_dimensions(ui);
            if let Some(managed) = self.panes.get(&pane_id) {
                let surface = managed.active_surface();
                let (_, rows) = surface.pane.dimensions();
                let total = surface.pane.scrollback_rows();
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
            let total = surface.pane.scrollback_rows();
            let end = total.saturating_sub(surface.scroll_offset);
            let start = end.saturating_sub(rows);
            let phys_row = start + row;
            let screen_rows = surface.pane.read_cells_range(phys_row, phys_row + 1);
            if let Some(screen_row) = screen_rows.first() {
                if let Some(cell) = screen_row.cells.get(col) {
                    if let Some(ref url) = cell.hyperlink_url {
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
                                let _ = open::that(url);
                            }
                        }
                    }
                }
            }
        }
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
                        let total_rows = surface.pane.scrollback_rows();
                        // Calculate scroll offset to center the match
                        let target_end = phys_row + rows / 2;
                        let actual_end = target_end.min(total_rows);
                        surface.scroll_offset = total_rows.saturating_sub(actual_end);
                    }
                }
            }
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
}
