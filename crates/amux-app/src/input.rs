//! Keyboard, mouse, clipboard, copy-mode, and selection input handling.

use amux_term::TerminalBackend;

use crate::*;

impl AmuxApp {
    // --- Shortcuts ---

    pub(crate) fn handle_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        // Skip terminal shortcuts when a modal text field has focus — let egui
        // handle Cmd+V, Cmd+C, etc. for the text widget instead.
        if self.rename_modal.is_some() || self.find_state.is_some() {
            return false;
        }
        let events = ctx.input(|i| i.events.clone());

        for event in &events {
            // Handle platform copy/cut events (egui may fire these instead of
            // Key events for Cmd+C / Cmd+X on macOS).
            match event {
                egui::Event::Copy => {
                    if self.copy_selection() {
                        return true;
                    }
                    // No selection to copy — leave this Copy event unhandled so any
                    // terminal Ctrl+C behavior (e.g. SIGINT) remains unaffected.
                    continue;
                }
                egui::Event::Cut => {
                    // Terminal text is read-only; treat cut as copy.
                    if self.copy_selection() {
                        return true;
                    }
                    continue;
                }
                _ => {}
            }

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

    pub(crate) fn enter_copy_mode(&mut self) {
        let pane_id = self.focused_pane_id();
        if let Some(managed) = self.panes.get(&pane_id) {
            let surface = managed.active_surface();
            let cursor = surface.pane.cursor();
            let (_, rows) = surface.pane.dimensions();
            let total = surface.pane.scrollback_rows();
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

    pub(crate) fn handle_copy_mode_key(
        &mut self,
        key: &egui::Key,
        modifiers: &egui::Modifiers,
    ) -> bool {
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
                let t = s.pane.scrollback_rows();
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

    pub(crate) fn extract_copy_mode_text(
        &self,
        pane_id: PaneId,
        anchor: (usize, usize),
        _cols: usize,
        line_visual: bool,
    ) -> Option<String> {
        let cm = self.copy_mode.as_ref()?;
        let managed = self.panes.get(&pane_id)?;
        let surface = managed.active_surface();
        let (start, end) =
            if anchor.1 < cm.cursor.1 || (anchor.1 == cm.cursor.1 && anchor.0 <= cm.cursor.0) {
                (anchor, cm.cursor)
            } else {
                (cm.cursor, anchor)
            };

        let rows = surface.pane.read_cells_range(start.1, end.1 + 1);
        let mut result = String::new();

        for (i, screen_row) in rows.iter().enumerate() {
            if i > 0 {
                result.push('\n');
            }
            let phys_row = start.1 + i;
            for (col, cell) in screen_row.cells.iter().enumerate() {
                if line_visual {
                    result.push_str(&cell.text);
                } else {
                    // Character visual: clip to selection bounds
                    if phys_row == start.1 && col < start.0 {
                        continue;
                    }
                    if phys_row == end.1 && col > end.0 {
                        break;
                    }
                    result.push_str(&cell.text);
                }
            }
        }

        // Trim trailing whitespace per line
        let trimmed: Vec<&str> = result.lines().map(|l| l.trim_end()).collect();
        Some(trimmed.join("\n"))
    }

    // --- Selection ---

    pub(crate) fn copy_selection(&mut self) -> bool {
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
        let text =
            selection::extract_selection_text(&managed.active_surface().pane, start, end, cols);

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

    pub(crate) fn do_paste(&mut self, text: &str) {
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

    pub(crate) fn select_all_visible(&mut self) {
        let focused = self.focused_pane_id();
        let managed = match self.panes.get_mut(&focused) {
            Some(m) => m,
            None => return,
        };
        let surface = managed.active_surface();
        let (cols, visible_rows) = surface.pane.dimensions();
        let total = surface.pane.scrollback_rows();
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

    pub(crate) fn clear_selection_on_focused(&mut self) {
        let focused = self.focused_pane_id();
        if let Some(m) = self.panes.get_mut(&focused) {
            m.selection = None;
        }
    }

    // --- Selection Mouse ---

    pub(crate) fn handle_selection_mouse(
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
        let total_rows = surface.pane.scrollback_rows();
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

                let (col, stable_row) = selection::pointer_to_cell(
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
                        let text = selection::line_text_string(
                            &managed.active_surface().pane,
                            stable_row,
                            cols,
                        );
                        let (wstart, wend) = selection::word_bounds_in_line(&text, col);
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
                    let (col, stable_row) = selection::pointer_to_cell(
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
                                    let text = selection::line_text_string(
                                        &m.surfaces[m.active_surface_idx].pane,
                                        stable_row,
                                        cols,
                                    );
                                    let (_, wend) = selection::word_bounds_in_line(&text, col);
                                    // Extend: keep anchor word start, update end word boundary
                                    if stable_row > sel.anchor.1
                                        || (stable_row == sel.anchor.1 && col >= sel.anchor.0)
                                    {
                                        sel.end = (wend, stable_row);
                                    } else {
                                        let (wstart, _) =
                                            selection::word_bounds_in_line(&text, col);
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

    pub(crate) fn handle_input(&mut self, ctx: &egui::Context) -> bool {
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

        // When the process has exited, intercept Enter (close) and R (restart)
        if surface.exited.is_some() {
            let mut action = DeadPaneAction::None;
            for event in &events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                {
                    match key {
                        egui::Key::Enter => action = DeadPaneAction::Close,
                        egui::Key::R if modifiers.is_none() => {
                            action = DeadPaneAction::Restart;
                        }
                        _ => {}
                    }
                }
            }
            match action {
                DeadPaneAction::Close => self.close_pane(focused_id),
                DeadPaneAction::Restart => {
                    self.restart_surface(focused_id);
                }
                DeadPaneAction::None => {}
            }
            return has_input;
        }

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
                    if let Some(bytes) = key_encode::encode_egui_key(key, modifiers) {
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
}
