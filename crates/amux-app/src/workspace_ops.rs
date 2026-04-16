//! Workspace and pane lifecycle operations (split, close, navigate, scroll).

use crate::*;

impl AmuxApp {
    /// Check if any egui text field (omnibar, rename modal, find bar) has focus.
    pub(crate) fn has_focused_text_field(&self) -> bool {
        self.rename_modal.is_some()
            || self.settings_modal.is_some()
            || self.find_state.is_some()
            || self.omnibar_state.values().any(|s| s.focused)
    }

    /// Remove a pane and any browser tabs it owns from the panes map.
    fn remove_pane_and_browser_tabs(&mut self, pane_id: PaneId) {
        let browser_ids: Vec<PaneId> = self
            .panes
            .get(&pane_id)
            .and_then(|e| e.as_terminal())
            .map(|m| m.browser_pane_ids())
            .unwrap_or_default();
        for bid in browser_ids {
            self.panes.remove(&bid);
            self.omnibar_state.remove(&bid);
        }
        self.panes.remove(&pane_id);
    }

    /// Returns the CWD of the focused pane's active terminal surface, if available.
    /// When a browser tab is active, falls back to the last terminal surface's CWD.
    pub(crate) fn focused_cwd(&self) -> Option<String> {
        let focused = self.focused_pane_id();
        self.panes
            .get(&focused)
            .and_then(|e| e.as_terminal())
            .and_then(|m| m.active_surface())
            .and_then(|sf| sf.metadata.cwd.clone())
    }

    // --- Pane/Workspace management ---

    pub(crate) fn spawn_pane_with_surface(&mut self) -> Option<PaneId> {
        let ws_id = self.active_workspace().id;
        let sf_id = self.next_surface_id;
        let cwd = self.focused_cwd();

        match startup::spawn_surface(
            80,
            24,
            &self.socket_addr,
            &self.socket_token,
            &self.config,
            ws_id,
            sf_id,
            cwd.as_deref(),
            None,
            self.app_config.shell.as_deref(),
        ) {
            Ok(surface) => {
                let pane_id = self.next_pane_id;
                self.next_pane_id += 1;
                self.next_surface_id += 1;
                self.panes.insert(
                    pane_id,
                    PaneEntry::Terminal(ManagedPane {
                        tabs: vec![managed_pane::TabEntry::Terminal(Box::new(surface))],
                        active_tab_idx: 0,
                        selection: None,
                    }),
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
        let cwd = self.focused_cwd();

        match startup::spawn_surface(
            80,
            24,
            &self.socket_addr,
            &self.socket_token,
            &self.config,
            ws_id,
            sf_id,
            cwd.as_deref(),
            None,
            self.app_config.shell.as_deref(),
        ) {
            Ok(surface) => {
                self.next_workspace_id += 1;
                let pane_id = self.next_pane_id;
                self.next_pane_id += 1;
                self.next_surface_id += 1;
                self.panes.insert(
                    pane_id,
                    PaneEntry::Terminal(ManagedPane {
                        tabs: vec![managed_pane::TabEntry::Terminal(Box::new(surface))],
                        active_tab_idx: 0,
                        selection: None,
                    }),
                );

                let workspace = Workspace {
                    id: ws_id,
                    title,
                    user_title: None,
                    tree: PaneTree::new(pane_id),
                    focused_pane: pane_id,
                    zoomed: None,
                    dragging_divider: None,
                    last_pane_sizes: HashMap::new(),
                    color: None,
                    pinned: false,
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
        let cwd = self.focused_cwd();

        match startup::spawn_surface(
            80,
            24,
            &self.socket_addr,
            &self.socket_token,
            &self.config,
            ws_id,
            sf_id,
            cwd.as_deref(),
            None,
            self.app_config.shell.as_deref(),
        ) {
            Ok(surface) => {
                if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&focused) {
                    // Insert right after the active tab (cmux behavior).
                    let insert_at = (managed.active_tab_idx + 1).min(managed.tabs.len());
                    managed.tabs.insert(
                        insert_at,
                        managed_pane::TabEntry::Terminal(Box::new(surface)),
                    );
                    managed.active_tab_idx = insert_at;
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
                self.remove_pane_and_browser_tabs(*id);
                self.notifications.remove_pane(*id);
            }
            self.notifications.remove_workspace(ws_id);
            self.wants_exit = true;
            return;
        }
        let pane_ids: Vec<PaneId> = self.workspaces[ws_idx].tree.iter_panes();
        for id in &pane_ids {
            self.remove_pane_and_browser_tabs(*id);
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
                menu_bar::MenuAction::NewBrowserTab => {
                    let pane_id = self.focused_pane_id();
                    self.queue_browser_pane(pane_id, DEFAULT_BROWSER_URL.to_string());
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
                    if self.has_focused_text_field() {
                        // The native menu bar consumes Cmd+C before egui sees
                        // it, so egui's TextEdit never gets the key event and
                        // never copies. Set a flag so we can inject
                        // Event::Copy into egui's input before the render pass.
                        self.pending_text_field_copy = true;
                    } else {
                        self.copy_selection();
                    }
                }
                menu_bar::MenuAction::Paste => {
                    if self.has_focused_text_field() {
                        // Menu bar consumed Cmd+V before egui could generate
                        // Event::Paste. Store the clipboard text so the focused
                        // text field's render code can apply it.
                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                            if let Ok(text) = clipboard.get_text() {
                                if !text.is_empty() {
                                    self.pending_text_field_paste = Some(text);
                                }
                            }
                        }
                    } else {
                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                            if let Ok(text) = clipboard.get_text() {
                                if !text.is_empty() {
                                    self.do_paste(&text);
                                }
                            }
                        }
                    }
                }
                menu_bar::MenuAction::SelectAll => {
                    if self.has_focused_text_field() {
                        // Signal the omnibar render pass to select all text.
                        // The native menu bar consumes Cmd+A before egui sees
                        // it, so we defer the selection update to the next frame
                        // where the TextEdit state can be mutated.
                        self.pending_text_field_select_all = true;
                    } else {
                        self.select_all_visible();
                    }
                }
                menu_bar::MenuAction::Settings => {
                    if self.settings_modal.is_none() {
                        self.settings_modal =
                            Some(settings_modal::SettingsModal::from_config(&self.app_config));
                    }
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
        if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&focused_id) {
            if managed.tabs.len() > 1 {
                let active_idx = managed.active_tab_idx;
                let is_browser = managed.tabs[active_idx].browser_pane_id();
                let is_last_terminal = is_browser.is_none()
                    && managed.tabs.iter().filter(|t| !t.is_browser()).count() <= 1;

                if is_last_terminal {
                    // Don't remove the last terminal tab — close the whole pane
                    // to preserve the "at least one terminal surface" invariant.
                    self.close_pane(focused_id);
                    return true;
                }

                let bid = is_browser;
                let managed = self
                    .panes
                    .get_mut(&focused_id)
                    .unwrap()
                    .as_terminal_mut()
                    .unwrap();
                managed.tabs.remove(active_idx);
                if active_idx < managed.active_tab_idx {
                    managed.active_tab_idx -= 1;
                } else if managed.active_tab_idx >= managed.tabs.len() {
                    managed.active_tab_idx = managed.tabs.len() - 1;
                }

                if let Some(bid) = bid {
                    self.panes.remove(&bid);
                    self.omnibar_state.remove(&bid);
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
            Some(PaneEntry::Terminal(m)) => m,
            _ => return,
        };
        // Only restart if the active tab is a terminal surface
        if managed.active_is_browser() {
            return;
        }
        let (cwd, cols, rows) = match managed.active_surface_mut() {
            Some(sf) => (
                sf.metadata.cwd.clone(),
                sf.pane.dimensions().0,
                sf.pane.dimensions().1,
            ),
            None => return,
        };
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
            self.app_config.shell.as_deref(),
        ) {
            Ok(new_surface) => {
                let idx = managed.active_tab_idx;
                managed.tabs[idx] = managed_pane::TabEntry::Terminal(Box::new(new_surface));
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
                self.remove_pane_and_browser_tabs(pane_id);
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
        if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&focused_id) {
            if let Some(surface) = managed.active_surface_mut() {
                let (_, rows) = surface.pane.dimensions();
                let page_size = rows.saturating_sub(1).max(1);
                let lines = pages * page_size as isize;
                let total = surface.pane.scrollback_rows();
                let max_offset = total.saturating_sub(rows);

                if surface.pane.manages_own_scroll() {
                    surface.pane.scroll_viewport(lines);
                }

                // Always track scroll_offset + last_scroll_at so the
                // scrollbar overlay works (same as do_scroll_lines_for).
                let new_offset = surface.scroll_offset as isize - lines;
                surface.scroll_offset = (new_offset.max(0) as usize).min(max_offset);
                surface.last_scroll_at = Instant::now();
            }
        }
        true
    }

    pub(crate) fn do_scroll_lines_for(&mut self, pane_id: PaneId, lines: isize) {
        if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&pane_id) {
            if let Some(surface) = managed.active_surface_mut() {
                let (_, rows) = surface.pane.dimensions();
                let total = surface.pane.scrollback_rows();
                let max_offset = total.saturating_sub(rows);

                if surface.pane.manages_own_scroll() {
                    surface.pane.scroll_viewport(lines);
                }

                // Always track scroll_offset — even when the backend manages
                // scrolling internally — so the scrollbar overlay can read it.
                let new_offset = surface.scroll_offset as isize - lines;
                surface.scroll_offset = (new_offset.max(0) as usize).min(max_offset);
            }
        }
    }

    pub(crate) fn do_clear_scrollback(&mut self) {
        let focused_id = self.focused_pane_id();
        if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&focused_id) {
            if let Some(surface) = managed.active_surface_mut() {
                // 1. Clear visible screen and move cursor home via terminal state machine
                surface.pane.feed_bytes(b"\x1b[2J\x1b[H");
                // 2. Erase scrollback buffer
                surface.pane.erase_scrollback();
                surface.snap_scroll_to_bottom();
                // 3. Send Ctrl+L to the PTY so the shell redraws its prompt
                let _ = surface.pane.write_bytes(b"\x0c");
            }
        }
    }

    /// Poll the config file for changes and apply hot-reloadable fields.
    /// Called every frame but only actually checks the file every ~2 seconds.
    pub(crate) fn check_config_reload(&mut self) {
        const POLL_INTERVAL: Duration = Duration::from_secs(2);

        let Some(ref path) = self.config_file_path else {
            return;
        };

        if self.config_last_checked.elapsed() < POLL_INTERVAL {
            return;
        }
        self.config_last_checked = Instant::now();

        let current_mtime = match std::fs::metadata(path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return, // File deleted or unreadable — skip
        };

        if self.config_last_modified == Some(current_mtime) {
            return; // No change
        }

        // File changed — reload
        tracing::info!("Config file changed, reloading: {}", path.display());
        // Update mtime FIRST — even if read/parse fails, we don't
        // want to retry the same broken file every 2 seconds.
        self.config_last_modified = Some(current_mtime);

        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to read config on reload: {e}");
                return;
            }
        };
        let new_config = match toml::from_str::<config::AppConfig>(&contents) {
            Ok(mut c) => {
                c.font_size = config::validate_font_size(c.font_size);
                c.font_family = c.font_family.trim().to_owned();
                if c.font_family.is_empty() {
                    c.font_family = config::DEFAULT_FONT_FAMILY.to_owned();
                }
                c
            }
            Err(e) => {
                tracing::warn!("Failed to parse config on reload: {e}");
                return;
            }
        };

        // Apply hot-reloadable fields:

        // Font size
        // Font family
        if new_config.font_family != self.app_config.font_family {
            self.font_size = new_config.font_size;
            #[cfg(feature = "gpu-renderer")]
            if let Some(gpu) = &mut self.gpu_renderer {
                gpu.set_font_family(&new_config.font_family, new_config.font_size);
            }
            tracing::info!("Hot-reloaded font_family: {}", new_config.font_family);
        } else if (new_config.font_size - self.app_config.font_size).abs() > f32::EPSILON {
            // Font size (only if family didn't change — set_font_family handles both)
            self.font_size = new_config.font_size;
            #[cfg(feature = "gpu-renderer")]
            if let Some(gpu) = &mut self.gpu_renderer {
                gpu.set_font_size(self.font_size);
            }
            tracing::info!("Hot-reloaded font_size: {}", self.font_size);
        }

        // Theme / colors — rebuild from scratch
        let mut new_theme = match new_config.theme_source.as_str() {
            "ghostty" => {
                if let Some(ghostty_cfg) = amux_ghostty_config::GhosttyConfig::load() {
                    crate::theme::Theme::from_ghostty(&ghostty_cfg)
                } else {
                    crate::theme::Theme::default()
                }
            }
            _ => crate::theme::Theme::default(),
        };
        new_theme.apply_color_config(&new_config.colors);
        // Update the terminal palette so new/existing panes pick up colors
        let mut term_config = (*self.config).clone();
        new_theme.apply_to_palette(&mut term_config.color_palette);
        let new_palette = term_config.color_palette.clone();
        self.config = Arc::new(term_config);
        self.theme = new_theme;

        // Propagate the new palette to every existing terminal pane.
        // Without this, already-running panes keep their old palette
        // (set_palette is only called at spawn time in startup.rs).
        for entry in self.panes.values_mut() {
            if let PaneEntry::Terminal(managed) = entry {
                for surface in managed.surfaces_mut() {
                    surface.pane.set_palette(new_palette.clone());
                }
            }
        }
        tracing::info!("Hot-reloaded theme/colors");

        // Store the full new config but preserve non-hot-reloadable fields
        // (keybindings, shell, menu_bar_style require a restart).
        let keybindings_saved = self.app_config.keybindings.clone();
        let shell_saved = self.app_config.shell.clone();
        let menu_bar_style_saved = self.app_config.menu_bar_style;
        self.app_config = new_config;
        self.app_config.keybindings = keybindings_saved;
        self.app_config.shell = shell_saved;
        self.app_config.menu_bar_style = menu_bar_style_saved;
    }
}
