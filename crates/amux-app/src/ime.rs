//! IME (Input Method Editor) positioning and preedit rendering.
//!
//! Reports the focused pane's cursor position to egui as the IME rect so
//! CJK/emoji input methods can anchor their candidate window correctly,
//! and draws the preedit string as a floating overlay at the cursor.

use crate::*;

impl AmuxApp {
    pub(crate) fn update_ime_position(&self, ctx: &egui::Context) {
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

    pub(crate) fn render_ime_preedit(&self, ctx: &egui::Context, preedit: &str) {
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
}
