//! Pane layout operations: PTY resize when a pane's rect changes, and
//! interactive divider dragging to resize splits.
//!
//! Mirrors wezterm-gui's `termwindow/render/split.rs` and `resize.rs`
//! (grouped here because both operate on the PaneTree layout).

use crate::*;

impl AmuxApp {
    pub(crate) fn resize_pane_if_needed(&mut self, id: PaneId, rect: egui::Rect, ui: &egui::Ui) {
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
        if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&id) {
            if managed.active_is_browser() {
                return;
            }
            if let Some(surface) = managed.active_surface_mut() {
                let (cur_cols, cur_rows) = surface.pane.dimensions();
                if cur_cols != cols || cur_rows != rows {
                    let _ = surface.pane.resize(cols as u16, rows as u16);
                }
            }
        }
    }

    pub(crate) fn handle_divider_drag(&mut self, ui: &egui::Ui, panel_rect: egui::Rect) {
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
