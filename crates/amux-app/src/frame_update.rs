//! Per-frame eframe::App loop and session save on exit.
//!
//! Contains the `impl eframe::App for AmuxApp` block: the `update()`
//! method that orchestrates the full per-frame work (drain IPC,
//! input handling, sidebar, tab-bar, panes, modals, notifications,
//! and repaint scheduling) and `on_exit()` which saves or clears the
//! session.

use crate::*;

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

        // Create any pending browser panes (needs window handle from frame)
        if !self.pending_browser_panes.is_empty() || !self.pending_browser_restores.is_empty() {
            self.create_pending_browser_panes(_frame);
        }

        // Drain popup requests from browser panes → queue as new browser panes.
        // Build a map of browser_pane_id → parent managed pane_id so the new tab
        // is inserted into the same managed pane as the source browser tab.
        let browser_to_parent: HashMap<PaneId, PaneId> = self
            .panes
            .iter()
            .filter_map(|(&managed_id, e)| {
                e.as_terminal().map(|m| {
                    m.browser_pane_ids()
                        .into_iter()
                        .map(move |bid| (bid, managed_id))
                })
            })
            .flatten()
            .collect();
        let popup_requests: Vec<(PaneId, Vec<String>)> = self
            .panes
            .iter()
            .filter_map(|(&browser_id, e)| {
                let urls = e.as_browser()?.drain_popup_requests();
                if urls.is_empty() {
                    return None;
                }
                let parent_id = browser_to_parent
                    .get(&browser_id)
                    .copied()
                    .unwrap_or_else(|| self.focused_pane_id());
                Some((parent_id, urls))
            })
            .collect();
        for (parent_pane_id, urls) in popup_requests {
            for url in urls {
                self.queue_browser_pane(parent_pane_id, url);
            }
        }

        // Record browser page visits for history/autocomplete (only when URL changes)
        let visits: Vec<(PaneId, String, String)> = self
            .panes
            .iter()
            .filter_map(|(&id, e)| {
                let b = e.as_browser()?;
                if b.is_loading() {
                    return None;
                }
                let url = b.url()?;
                if url == "about:blank" || url.is_empty() {
                    return None;
                }
                let last = self
                    .omnibar_state
                    .get(&id)
                    .map(|s| s.last_recorded_url.as_str())
                    .unwrap_or("");
                if url == last {
                    return None;
                }
                Some((id, url, b.title()))
            })
            .collect();
        for (pane_id, url, title) in visits {
            self.browser_history.record_visit(&url, &title);
            if let Some(state) = self.omnibar_state.get_mut(&pane_id) {
                state.last_recorded_url = url;
            }
        }

        self.selection_changed = false;
        self.app_focused = ctx.input(|i| i.focused);

        // Drain PTY output from all surfaces, with a per-surface byte budget
        // to prevent high-throughput output (e.g. `cat large_file`) from
        // blocking input handling and causing frame drops.
        const MAX_BYTES_PER_SURFACE_PER_FRAME: usize = 64 * 1024;
        let mut got_data = false;
        let mut pending_data = false;
        for entry in self.panes.values_mut() {
            let PaneEntry::Terminal(managed) = entry else {
                continue;
            };
            for surface in managed.surfaces_mut() {
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

        // Process favicon data from browser panes
        self.process_favicon_data(ctx);

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

                // Hide browser webviews that don't belong to the active workspace.
                // Native webviews sit above egui, so they must be explicitly hidden
                // when switching workspaces — render_single_pane only manages
                // visibility for the pane it's rendering, not cross-workspace.
                let active_pane_ids: std::collections::HashSet<PaneId> = self
                    .active_workspace()
                    .tree
                    .iter_panes()
                    .into_iter()
                    .collect();
                let active_browser_ids: std::collections::HashSet<PaneId> = active_pane_ids
                    .iter()
                    .filter_map(|&aid| self.panes.get(&aid).and_then(|e| e.as_terminal()))
                    .flat_map(|m| m.browser_pane_ids())
                    .collect();
                for (&pid, entry) in &self.panes {
                    if let PaneEntry::Browser(b) = entry {
                        if !active_pane_ids.contains(&pid) && !active_browser_ids.contains(&pid) {
                            b.set_visible(false);
                        }
                    }
                }

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
                                    if let Some(PaneEntry::Terminal(m)) =
                                        self.panes.get_mut(&old_focused)
                                    {
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

        // Update window title from focused pane's active tab
        let focused_id = self.focused_pane_id();
        if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&focused_id) {
            let title = match managed.active_tab() {
                managed_pane::ActiveTab::Terminal(_) => managed
                    .active_surface()
                    .map(|sf| sf.pane.title().to_string())
                    .unwrap_or_default(),
                managed_pane::ActiveTab::Browser(bid) => {
                    self.panes.get(&bid).map(|e| e.title()).unwrap_or_default()
                }
            };
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
        self.browser_history.save();

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
