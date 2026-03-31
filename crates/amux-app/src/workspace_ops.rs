//! Workspace and pane lifecycle operations (split, close, navigate, scroll).

use crate::*;

impl AmuxApp {
    // --- Pane/Workspace management ---

    pub(crate) fn spawn_pane_with_surface(&mut self) -> Option<PaneId> {
        let ws_id = self.active_workspace().id;
        let sf_id = self.next_surface_id;

        match startup::spawn_surface(
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

    pub(crate) fn create_workspace(&mut self, title: Option<String>) -> Option<u64> {
        let ws_id = self.next_workspace_id;
        let title = title.unwrap_or_else(|| format!("Terminal {}", self.workspaces.len() + 1));

        let sf_id = self.next_surface_id;

        match startup::spawn_surface(
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

    pub(crate) fn add_surface_to_focused_pane(&mut self) -> Option<u64> {
        let sf_id = self.next_surface_id;
        self.next_surface_id += 1;
        let ws_id = self.active_workspace().id;
        let focused = self.focused_pane_id();

        match startup::spawn_surface(
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

    pub(crate) fn close_workspace_at(&mut self, ws_idx: usize) {
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

    pub(crate) fn handle_menu_actions(&mut self) {
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
                menu_bar::MenuAction::Copy => {
                    self.copy_selection();
                }
                menu_bar::MenuAction::Paste => {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        if let Ok(text) = clipboard.get_text() {
                            if !text.is_empty() {
                                self.do_paste(&text);
                            }
                        }
                    }
                }
                menu_bar::MenuAction::SelectAll => {
                    self.select_all_visible();
                }
            }
        }
    }

    pub(crate) fn do_split(&mut self, direction: SplitDirection) -> bool {
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

    pub(crate) fn do_close_cascade(&mut self) -> bool {
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

    /// Restart the active surface in a pane by spawning a new shell in the
    /// same working directory and replacing the dead surface in-place.
    pub(crate) fn restart_surface(&mut self, pane_id: PaneId) {
        let ws_id = self
            .workspaces
            .iter()
            .find(|ws| ws.tree.iter_panes().contains(&pane_id))
            .map(|ws| ws.id)
            .unwrap_or(0);

        let managed = match self.panes.get_mut(&pane_id) {
            Some(m) => m,
            None => return,
        };
        let old_surface = managed.active_surface_mut();
        let cwd = old_surface.metadata.cwd.clone();
        let (cols, rows) = old_surface.pane.dimensions();
        let sf_id = self.next_surface_id;
        self.next_surface_id += 1;

        match startup::spawn_surface(
            cols as u16,
            rows as u16,
            &self.socket_addr,
            &self.socket_token,
            &self.config,
            ws_id,
            sf_id,
            cwd.as_deref(),
            None,
        ) {
            Ok(new_surface) => {
                let idx = managed.active_surface_idx;
                managed.surfaces[idx] = new_surface;
            }
            Err(e) => {
                tracing::warn!("Failed to restart surface: {e}");
            }
        }
    }

    /// Close a pane entirely. Finds the owning workspace (not necessarily the
    /// active one). If it's the last pane in that workspace, close the workspace.
    pub(crate) fn close_pane(&mut self, pane_id: PaneId) {
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

    pub(crate) fn do_navigate(&mut self, dir: NavDirection) -> bool {
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

    pub(crate) fn do_toggle_zoom(&mut self) -> bool {
        let ws = self.active_workspace_mut();
        if ws.zoomed.is_some() {
            ws.zoomed = None;
        } else {
            ws.zoomed = Some(ws.focused_pane);
        }
        true
    }

    pub(crate) fn do_scroll(&mut self, pages: isize) -> bool {
        let focused_id = self.focused_pane_id();
        if let Some(managed) = self.panes.get_mut(&focused_id) {
            let surface = managed.active_surface_mut();
            let (_, rows) = surface.pane.dimensions();
            let page_size = rows.saturating_sub(1).max(1);
            let lines = pages * page_size as isize;
            let total = surface.pane.scrollback_rows();
            let max_offset = total.saturating_sub(rows);
            let new_offset = surface.scroll_offset as isize - lines;
            surface.scroll_offset = (new_offset.max(0) as usize).min(max_offset);
        }
        true
    }

    pub(crate) fn do_scroll_lines_for(&mut self, pane_id: PaneId, lines: isize) {
        if let Some(managed) = self.panes.get_mut(&pane_id) {
            let surface = managed.active_surface_mut();
            let (_, rows) = surface.pane.dimensions();
            let total = surface.pane.scrollback_rows();
            let max_offset = total.saturating_sub(rows);
            let new_offset = surface.scroll_offset as isize - lines;
            surface.scroll_offset = (new_offset.max(0) as usize).min(max_offset);
        }
    }

    pub(crate) fn do_clear_scrollback(&mut self) {
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
}
