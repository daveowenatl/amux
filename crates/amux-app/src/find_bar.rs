//! In-pane scrollback search bar UI.
//!
//! Draws a small window anchored top-right that takes a query, runs it
//! against the focused pane's scrollback, and lets the user step through
//! matches with Enter / Shift+Enter / arrow buttons. Navigating scrolls
//! the pane to center the current match.

use crate::*;

impl AmuxApp {
    pub(crate) fn render_find_bar(&mut self, ctx: &egui::Context) {
        // Apply pending paste from menu bar (Cmd+V consumed by muda before egui).
        if let Some(paste_text) = self.pending_text_field_paste.take() {
            if let Some(fs) = self.find_state.as_mut() {
                fs.query.push_str(&paste_text);
            }
        }

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
                        if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&pane_id) {
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
                    if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&pane_id) {
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
}
