mod find_bar;
mod fonts;
mod hyperlinks;
mod ime;
mod input;
mod ipc_dispatch;
mod key_encode;
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
            if sent_input {
                self.cursor_blink_since = Instant::now();
            }
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
                // Paint the titlebar strip across the FULL viewport, not just
                // the CentralPanel region. render_titlebar_icons draws in
                // window coordinates starting at screen.min.x + left_inset,
                // which on macOS (78px inset) lands over the sidebar when it
                // is visible. Using a background layer painter on the full
                // screen rect keeps the strip coherent regardless of sidebar
                // state and decouples it from sidebar_bg's color.
                let screen = ui.ctx().screen_rect();
                let strip_painter = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Background,
                    egui::Id::new("amux_titlebar_strip"),
                ));
                strip_painter.rect_filled(
                    egui::Rect::from_min_max(
                        screen.min,
                        egui::pos2(screen.max.x, screen.min.y + TERMINAL_TOP_PAD),
                    ),
                    0.0,
                    self.theme.titlebar_bg(),
                );
                // Top-left titlebar icons: sidebar toggle, notifications, new workspace.
                self.render_titlebar_icons(ui.ctx());
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
