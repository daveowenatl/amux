//! Rename modal UI for workspaces and tabs.
//!
//! Presents a small centered window with a text field that applies a new
//! name to the targeted workspace or tab on Enter/OK, or closes on
//! Escape/Cancel. Uses stable IDs (rather than indices) so background
//! reorder/close can't cause the modal to rename the wrong item.

use crate::*;

/// Identifies which item the rename modal is editing.
///
/// Uses stable IDs rather than indices so background reorder/close can't
/// cause the modal to rename the wrong item.
#[derive(Copy, Clone)]
pub(crate) enum RenameTarget {
    Workspace(u64),
    Tab { pane_id: PaneId, surface_id: u64 },
}

pub(crate) struct RenameModal {
    pub(crate) target: RenameTarget,
    pub(crate) buf: String,
    pub(crate) just_opened: bool,
}

impl AmuxApp {
    pub(crate) fn render_rename_modal(&mut self, ctx: &egui::Context) {
        let mut apply: Option<String> = None;
        let mut cancel = false;

        let title = match &self.rename_modal.as_ref().unwrap().target {
            RenameTarget::Workspace(_) => "Rename Workspace",
            RenameTarget::Tab { .. } => "Rename Tab",
        };

        // Apply pending paste from menu bar (Cmd+V consumed by muda before egui).
        if let Some(paste_text) = self.pending_text_field_paste.take() {
            let modal = self.rename_modal.as_mut().unwrap();
            modal.buf.push_str(&paste_text);
        }

        // Apply pending select-all from menu bar. The rename modal text field
        // manages its own cursor state; clearing the flag is sufficient since
        // egui's TextEdit handles Cmd+A internally when the field is focused.
        self.pending_text_field_select_all = false;

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
                match self.rename_modal.as_ref().unwrap().target {
                    RenameTarget::Workspace(ws_id) => {
                        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id) {
                            ws.title = new_name;
                        }
                    }
                    RenameTarget::Tab {
                        pane_id,
                        surface_id,
                    } => {
                        if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&pane_id) {
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
}
