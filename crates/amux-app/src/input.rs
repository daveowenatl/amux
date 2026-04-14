//! Keyboard, mouse, clipboard, copy-mode, and selection input handling.

use std::collections::HashMap;

use amux_core::config::{self, Action};
use amux_term::TerminalBackend;

use crate::*;

/// Check if a [`KeyCombo`] matches the current egui key event state.
fn combo_matches(combo: &config::KeyCombo, modifiers: &egui::Modifiers, key: &egui::Key) -> bool {
    // Check modifiers.
    // On macOS, "cmd" maps to mac_cmd/command and "ctrl" maps to ctrl independently.
    // On other platforms, both "cmd" and "ctrl" map to the Ctrl key (the platform
    // command key), so we merge them: either combo.cmd or combo.ctrl must match.
    #[cfg(target_os = "macos")]
    let cmd_ok = combo.cmd == (modifiers.mac_cmd || modifiers.command);
    #[cfg(not(target_os = "macos"))]
    let cmd_ok = (combo.cmd || combo.ctrl) == modifiers.ctrl;

    let shift_ok = combo.shift == modifiers.shift;
    let alt_ok = combo.alt == modifiers.alt;

    #[cfg(target_os = "macos")]
    let ctrl_ok = combo.ctrl == modifiers.ctrl;
    #[cfg(not(target_os = "macos"))]
    let ctrl_ok = true; // ctrl is consumed by cmd_ok on non-macOS

    if !(cmd_ok && shift_ok && alt_ok && ctrl_ok) {
        return false;
    }

    // Match key name
    let key_name = &combo.key;
    match key {
        egui::Key::A => key_name == "a",
        egui::Key::B => key_name == "b",
        egui::Key::C => key_name == "c",
        egui::Key::D => key_name == "d",
        egui::Key::E => key_name == "e",
        egui::Key::F => key_name == "f",
        egui::Key::G => key_name == "g",
        egui::Key::H => key_name == "h",
        egui::Key::I => key_name == "i",
        egui::Key::J => key_name == "j",
        egui::Key::K => key_name == "k",
        egui::Key::L => key_name == "l",
        egui::Key::M => key_name == "m",
        egui::Key::N => key_name == "n",
        egui::Key::O => key_name == "o",
        egui::Key::P => key_name == "p",
        egui::Key::Q => key_name == "q",
        egui::Key::R => key_name == "r",
        egui::Key::S => key_name == "s",
        egui::Key::T => key_name == "t",
        egui::Key::U => key_name == "u",
        egui::Key::V => key_name == "v",
        egui::Key::W => key_name == "w",
        egui::Key::X => key_name == "x",
        egui::Key::Y => key_name == "y",
        egui::Key::Z => key_name == "z",
        egui::Key::Num0 => key_name == "0",
        egui::Key::Num1 => key_name == "1",
        egui::Key::Num2 => key_name == "2",
        egui::Key::Num3 => key_name == "3",
        egui::Key::Num4 => key_name == "4",
        egui::Key::Num5 => key_name == "5",
        egui::Key::Num6 => key_name == "6",
        egui::Key::Num7 => key_name == "7",
        egui::Key::Num8 => key_name == "8",
        egui::Key::Num9 => key_name == "9",
        egui::Key::Tab => key_name == "tab",
        egui::Key::Enter => key_name == "enter",
        egui::Key::Escape => key_name == "escape",
        egui::Key::ArrowLeft => key_name == "left",
        egui::Key::ArrowRight => key_name == "right",
        egui::Key::ArrowUp => key_name == "up",
        egui::Key::ArrowDown => key_name == "down",
        egui::Key::PageUp => key_name == "pageup",
        egui::Key::PageDown => key_name == "pagedown",
        egui::Key::OpenBracket => key_name == "[",
        egui::Key::CloseBracket => key_name == "]",
        egui::Key::Space => key_name == "space",
        egui::Key::Backspace => key_name == "backspace",
        egui::Key::Delete => key_name == "delete",
        _ => false,
    }
}

/// Check if a specific [`Action`] in the resolved keybindings matches the current event.
fn action_matches(
    keybindings: &HashMap<Action, config::KeyCombo>,
    action: Action,
    modifiers: &egui::Modifiers,
    key: &egui::Key,
) -> bool {
    keybindings
        .get(&action)
        .is_some_and(|combo| combo_matches(combo, modifiers, key))
}

impl AmuxApp {
    // --- Shortcuts ---

    pub(crate) fn handle_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        // Skip terminal shortcuts when a modal text field has focus — let egui
        // handle Cmd+V, Cmd+C, etc. for the text widget instead.
        if self.rename_modal.is_some() || self.find_state.is_some() {
            return false;
        }
        let omnibar_focused = self.omnibar_state.values().any(|s| s.focused);
        if omnibar_focused {
            return false;
        }

        // When a browser tab is active, skip terminal-specific shortcuts
        // (copy/paste/select-all/find/copy-mode/clear) so the webview gets them.
        // Amux chrome shortcuts (tabs, workspaces, splits, nav) still apply.
        let browser_active = {
            let focused = self.focused_pane_id();
            self.panes
                .get(&focused)
                .and_then(|e| e.as_terminal())
                .is_some_and(|m| m.active_is_browser())
        };
        let events = ctx.input(|i| i.events.clone());

        for event in &events {
            // Handle platform copy/cut events (terminal only — browser handles
            // its own copy/cut via the webview).
            if !browser_active {
                match event {
                    egui::Event::Copy => {
                        if self.copy_selection() {
                            return true;
                        }
                        continue;
                    }
                    egui::Event::Cut => {
                        if self.copy_selection() {
                            return true;
                        }
                        continue;
                    }
                    _ => {}
                }
            }

            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                // Pre-compute all action matches so the immutable borrow of
                // self.keybindings is released before any &mut self calls.
                let is_copy = action_matches(&self.keybindings, Action::Copy, modifiers, key);
                let is_paste = action_matches(&self.keybindings, Action::Paste, modifiers, key);
                let is_find = action_matches(&self.keybindings, Action::Find, modifiers, key);
                let is_select_all =
                    action_matches(&self.keybindings, Action::SelectAll, modifiers, key);
                let is_copy_mode =
                    action_matches(&self.keybindings, Action::CopyMode, modifiers, key);
                let is_toggle_sidebar =
                    action_matches(&self.keybindings, Action::ToggleSidebar, modifiers, key);
                let is_new_browser_tab =
                    action_matches(&self.keybindings, Action::NewBrowserTab, modifiers, key);
                let is_new_workspace =
                    action_matches(&self.keybindings, Action::NewWorkspace, modifiers, key);
                let is_new_tab = action_matches(&self.keybindings, Action::NewTab, modifiers, key);
                let is_next_workspace =
                    action_matches(&self.keybindings, Action::NextWorkspace, modifiers, key);
                let is_prev_workspace =
                    action_matches(&self.keybindings, Action::PrevWorkspace, modifiers, key);
                let is_next_tab =
                    action_matches(&self.keybindings, Action::NextTab, modifiers, key);
                let is_prev_tab =
                    action_matches(&self.keybindings, Action::PrevTab, modifiers, key);
                let is_split_right =
                    action_matches(&self.keybindings, Action::SplitRight, modifiers, key);
                let is_split_down =
                    action_matches(&self.keybindings, Action::SplitDown, modifiers, key);
                let is_close_pane =
                    action_matches(&self.keybindings, Action::ClosePane, modifiers, key);
                let is_nav_left =
                    action_matches(&self.keybindings, Action::NavigateLeft, modifiers, key);
                let is_nav_right =
                    action_matches(&self.keybindings, Action::NavigateRight, modifiers, key);
                let is_nav_up =
                    action_matches(&self.keybindings, Action::NavigateUp, modifiers, key);
                let is_nav_down =
                    action_matches(&self.keybindings, Action::NavigateDown, modifiers, key);
                let is_zoom_toggle =
                    action_matches(&self.keybindings, Action::ZoomToggle, modifiers, key);
                let is_devtools =
                    action_matches(&self.keybindings, Action::DevTools, modifiers, key);
                let is_notification_panel =
                    action_matches(&self.keybindings, Action::NotificationPanel, modifiers, key);
                let is_jump_to_unread =
                    action_matches(&self.keybindings, Action::JumpToUnread, modifiers, key);
                let is_clear_scrollback =
                    action_matches(&self.keybindings, Action::ClearScrollback, modifiers, key);

                // --- Terminal-specific shortcuts (skipped when browser tab active) ---
                if !browser_active {
                    // Copy
                    if is_copy && self.copy_selection() {
                        return true;
                    }

                    // Paste
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
                        if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&focused) {
                            if m.selection.is_some() {
                                m.selection = None;
                                return true;
                            }
                        }
                    }

                    // Find
                    if is_find {
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

                    // Select all
                    if is_select_all {
                        self.select_all_visible();
                        return true;
                    }

                    // Enter copy mode
                    if is_copy_mode {
                        self.enter_copy_mode();
                        return true;
                    }
                }

                // --- Browser-specific shortcuts (proxy to webview via JS) ---
                if browser_active {
                    let focused = self.focused_pane_id();
                    let browser_id = self
                        .panes
                        .get(&focused)
                        .and_then(|e| e.as_terminal())
                        .and_then(|m| match m.active_tab() {
                            managed_pane::ActiveTab::Browser(bid) => Some(bid),
                            _ => None,
                        });

                    if let Some(bid) = browser_id {
                        // Paste
                        if is_paste {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                if let Ok(text) = clipboard.get_text() {
                                    if !text.is_empty() {
                                        if let Some(PaneEntry::Browser(b)) = self.panes.get(&bid) {
                                            b.type_text(&text);
                                        }
                                    }
                                }
                            }
                            return true;
                        }

                        // Select all
                        if is_select_all {
                            if let Some(PaneEntry::Browser(b)) = self.panes.get(&bid) {
                                b.evaluate_script("document.execCommand('selectAll')");
                            }
                            return true;
                        }

                        // Copy
                        if is_copy {
                            if let Some(PaneEntry::Browser(b)) = self.panes.get(&bid) {
                                b.evaluate_script("document.execCommand('copy')");
                            }
                            return true;
                        }

                        // Cut: uses the Copy combo key with no shift distinction;
                        // browser cut is Cmd+X / Ctrl+X — hardcoded since there is
                        // no CopyMode-style action for Cut.
                        #[cfg(target_os = "macos")]
                        let is_cut =
                            (modifiers.mac_cmd || modifiers.command) && *key == egui::Key::X;
                        #[cfg(not(target_os = "macos"))]
                        let is_cut = modifiers.ctrl && *key == egui::Key::X;

                        if is_cut {
                            if let Some(PaneEntry::Browser(b)) = self.panes.get(&bid) {
                                b.evaluate_script("document.execCommand('cut')");
                            }
                            return true;
                        }
                    }
                }

                // --- Amux chrome shortcuts (always active) ---

                // Toggle sidebar
                if is_toggle_sidebar {
                    self.sidebar.visible = !self.sidebar.visible;
                    return true;
                }

                // New browser tab
                if is_new_browser_tab {
                    let pane_id = self.focused_pane_id();
                    self.queue_browser_pane(pane_id, DEFAULT_BROWSER_URL.to_string());
                    return true;
                }

                // New workspace
                if is_new_workspace {
                    self.create_workspace(None);
                    return true;
                }

                // New tab in focused pane
                if is_new_tab {
                    self.add_surface_to_focused_pane();
                    return true;
                }

                // Next workspace
                if is_next_workspace {
                    if self.workspaces.len() > 1 {
                        self.active_workspace_idx =
                            (self.active_workspace_idx + 1) % self.workspaces.len();
                        let pane_ids: Vec<u64> = self.active_workspace().tree.iter_panes();
                        self.notifications.mark_workspace_read(&pane_ids);
                    }
                    return true;
                }

                // Prev workspace
                if is_prev_workspace {
                    if self.workspaces.len() > 1 {
                        self.active_workspace_idx = if self.active_workspace_idx == 0 {
                            self.workspaces.len() - 1
                        } else {
                            self.active_workspace_idx - 1
                        };
                        let pane_ids: Vec<u64> = self.active_workspace().tree.iter_panes();
                        self.notifications.mark_workspace_read(&pane_ids);
                    }
                    return true;
                }

                // Jump to workspace 1-9 (Cmd+9 = last workspace) — always fixed
                #[cfg(target_os = "macos")]
                let is_jump_mod = (modifiers.mac_cmd || modifiers.command) && !modifiers.shift;
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
                            let pane_ids: Vec<u64> = self.workspaces[idx].tree.iter_panes();
                            self.notifications.mark_workspace_read(&pane_ids);
                            return true;
                        }
                    }
                }

                // Next tab in focused pane
                if is_next_tab {
                    if let Some(PaneEntry::Terminal(managed)) =
                        self.panes.get_mut(&self.focused_pane_id())
                    {
                        let total = managed.tab_count();
                        if total > 1 {
                            managed.active_tab_idx = (managed.active_tab_idx + 1) % total;
                        }
                    }
                    return true;
                }

                // Prev tab in focused pane
                if is_prev_tab {
                    if let Some(PaneEntry::Terminal(managed)) =
                        self.panes.get_mut(&self.focused_pane_id())
                    {
                        let total = managed.tab_count();
                        if total > 1 {
                            managed.active_tab_idx = if managed.active_tab_idx == 0 {
                                total - 1
                            } else {
                                managed.active_tab_idx - 1
                            };
                        }
                    }
                    return true;
                }

                // --- Pane shortcuts ---

                // Split right
                if is_split_right {
                    return self.do_split(SplitDirection::Horizontal);
                }

                // Split down
                if is_split_down {
                    return self.do_split(SplitDirection::Vertical);
                }

                // Close pane (cascade: tab -> pane -> workspace)
                if is_close_pane {
                    return self.do_close_cascade();
                }

                // Navigate
                if is_nav_left {
                    return self.do_navigate(NavDirection::Left);
                }
                if is_nav_right {
                    return self.do_navigate(NavDirection::Right);
                }
                if is_nav_up {
                    return self.do_navigate(NavDirection::Up);
                }
                if is_nav_down {
                    return self.do_navigate(NavDirection::Down);
                }

                // Zoom toggle
                if is_zoom_toggle {
                    return self.do_toggle_zoom();
                }

                // DevTools
                if is_devtools {
                    let pane_id = self.focused_pane_id();
                    if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&pane_id) {
                        if let managed_pane::ActiveTab::Browser(bid) = managed.active_tab() {
                            if let Some(PaneEntry::Browser(browser)) = self.panes.get(&bid) {
                                browser.open_devtools();
                                return true;
                            }
                        }
                    }
                }

                // Notification panel
                if is_notification_panel {
                    self.show_notification_panel = !self.show_notification_panel;
                    return true;
                }

                // Jump to latest unread
                if is_jump_to_unread {
                    self.jump_to_latest_unread();
                    return true;
                }

                if !browser_active {
                    // Clear scrollback
                    if is_clear_scrollback {
                        self.do_clear_scrollback();
                        return true;
                    }

                    // Scroll — always fixed (Shift+PageUp/Down)
                    if modifiers.shift && *key == egui::Key::PageUp {
                        return self.do_scroll(-1);
                    }
                    if modifiers.shift && *key == egui::Key::PageDown {
                        return self.do_scroll(1);
                    }
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
                if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&pane_id) {
                    if let Some(surface) = managed.active_surface_mut() {
                        let font_id = egui::FontId::monospace(self.font_size);
                        let cell_height = ctx.fonts(|f| f.row_height(&font_id));

                        surface.scroll_accum += -scroll_delta / cell_height;
                        let whole_lines = surface.scroll_accum.trunc() as isize;
                        if whole_lines != 0 {
                            surface.scroll_accum -= whole_lines as f32;
                            surface.last_scroll_at = Instant::now();
                            self.do_scroll_lines_for(pane_id, whole_lines);
                        }
                    }
                }
            }
        }

        false
    }

    pub(crate) fn enter_copy_mode(&mut self) {
        let pane_id = self.focused_pane_id();
        if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&pane_id) {
            if let Some(surface) = managed.active_surface() {
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
            Some(PaneEntry::Terminal(m)) => match m.active_surface() {
                Some(s) => {
                    let (c, r) = s.pane.dimensions();
                    let t = s.pane.scrollback_rows();
                    (c, r, t)
                }
                None => {
                    self.copy_mode = None;
                    return true;
                }
            },
            _ => {
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
        if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&pane_id) {
            let cm = self.copy_mode.as_ref().unwrap();
            if let Some(surface) = managed.active_surface_mut() {
                let end = total_rows.saturating_sub(surface.scroll_offset);
                let start = end.saturating_sub(rows);
                if cm.cursor.1 < start {
                    surface.scroll_offset = total_rows.saturating_sub(cm.cursor.1 + rows);
                } else if cm.cursor.1 >= end {
                    surface.scroll_offset = total_rows.saturating_sub(cm.cursor.1 + 1);
                }
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
        let managed = self.panes.get(&pane_id)?.as_terminal()?;
        let surface = managed.active_surface()?;
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
            Some(PaneEntry::Terminal(m)) => m,
            _ => return false,
        };
        let sel = match &managed.selection {
            Some(s) => s.clone(),
            None => return false,
        };

        let surface = match managed.active_surface() {
            Some(s) => s,
            None => return false,
        };
        let (cols, _) = surface.pane.dimensions();
        let (start, end) = sel.normalized();
        let text = selection::extract_selection_text(&surface.pane, start, end, cols);

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
        if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&focused_id) {
            if let Some(surface) = managed.active_surface_mut() {
                surface.snap_scroll_to_bottom();
                if surface.pane.bracketed_paste_enabled() {
                    let _ = surface.pane.write_bytes(b"\x1b[200~");
                    let _ = surface.pane.write_bytes(text.as_bytes());
                    let _ = surface.pane.write_bytes(b"\x1b[201~");
                } else {
                    let _ = surface.pane.write_bytes(text.as_bytes());
                }
            }
        }
    }

    pub(crate) fn select_all_visible(&mut self) {
        let focused = self.focused_pane_id();
        let managed = match self.panes.get_mut(&focused) {
            Some(PaneEntry::Terminal(m)) => m,
            _ => return,
        };
        if let Some(surface) = managed.active_surface() {
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
    }

    pub(crate) fn clear_selection_on_focused(&mut self) {
        let focused = self.focused_pane_id();
        if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&focused) {
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
            Some(PaneEntry::Terminal(m)) => m,
            _ => return false,
        };
        let surface = match managed.active_surface() {
            Some(s) => s,
            None => return false,
        };
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

        // Skip selection when the pointer is in the scrollbar hit zone
        // (rightmost 16px of the content area) so the scrollbar drag
        // interaction takes priority over text selection.
        if let Some(pos) = pointer_pos {
            let scrollbar_zone_left = content_rect.max.x - 16.0;
            if pos.x >= scrollbar_zone_left && content_rect.contains(pos) {
                return false;
            }
        }

        if primary_pressed {
            if let Some(pos) = pointer_pos {
                if !content_rect.contains(pos) {
                    // Click outside content — clear any existing selection
                    if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&pane_id) {
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
                        // `surface` borrow ends at the match guard so this borrow is safe
                        let Some(surface) = managed.active_surface() else {
                            return false;
                        };
                        let text = selection::line_text_string(&surface.pane, stable_row, cols);
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

                if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&pane_id) {
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
                .and_then(|e| e.as_terminal())
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

                    if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&pane_id) {
                        // Pre-fetch line text before mutably borrowing selection
                        let line_text = m
                            .active_surface()
                            .map(|sf| selection::line_text_string(&sf.pane, stable_row, cols))
                            .unwrap_or_default();
                        if let Some(ref mut sel) = m.selection {
                            match sel.mode {
                                SelectionMode::Cell => {
                                    sel.end = (col, stable_row);
                                }
                                SelectionMode::Word => {
                                    let (_, wend) = selection::word_bounds_in_line(&line_text, col);
                                    // Extend: keep anchor word start, update end word boundary
                                    if stable_row > sel.anchor.1
                                        || (stable_row == sel.anchor.1 && col >= sel.anchor.0)
                                    {
                                        sel.end = (wend, stable_row);
                                    } else {
                                        let (wstart, _) =
                                            selection::word_bounds_in_line(&line_text, col);
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
            if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&pane_id) {
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
            Some(PaneEntry::Terminal(m)) => m,
            _ => return has_input,
        };
        // Skip terminal input when the active tab is a browser
        if managed.active_is_browser() {
            return has_input;
        }
        let surface = match managed.active_surface_mut() {
            Some(s) => s,
            None => return has_input,
        };

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
                    surface.snap_scroll_to_bottom();
                    let _ = surface.pane.write_bytes(text.as_bytes());
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if let Some(bytes) = key_encode::encode_egui_key(key, modifiers) {
                        surface.snap_scroll_to_bottom();
                        let _ = surface.pane.write_bytes(&bytes);
                    }
                }
                egui::Event::Paste(text) => {
                    surface.snap_scroll_to_bottom();
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
                        surface.snap_scroll_to_bottom();
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
