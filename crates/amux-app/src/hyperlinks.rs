//! Terminal hyperlink (OSC 8) hover and click handling.
//!
//! Resolves the hovered cell to the pane beneath the mouse, reads back the
//! cell's hyperlink URL if any, and:
//!   - sets the hovered hyperlink for status/display purposes,
//!   - switches the cursor icon to a pointing hand,
//!   - opens the URL on Cmd/Ctrl + click (only for safe http/https/mailto schemes).

use crate::*;

impl AmuxApp {
    pub(crate) fn handle_hyperlinks(&mut self, ctx: &egui::Context) {
        self.hovered_hyperlink = None;

        let hover_pos = match ctx.input(|i| i.pointer.hover_pos()) {
            Some(pos) => pos,
            None => return,
        };

        let panel_rect = match self.last_panel_rect {
            Some(r) => r,
            None => return,
        };

        // Find which pane the mouse is over
        let ws = self.active_workspace();
        let pane_id = if let Some(zoomed_id) = ws.zoomed {
            if panel_rect.contains(hover_pos) {
                zoomed_id
            } else {
                return;
            }
        } else {
            let layout = ws.tree.layout(panel_rect);
            match layout
                .iter()
                .find(|(_, rect)| rect.contains(hover_pos))
                .map(|(id, _)| *id)
            {
                Some(id) => id,
                None => return,
            }
        };

        // Resolve cell coordinates from pixel position
        let (cell_w, cell_h) = ctx.fonts(|f| {
            let fid = egui::FontId::monospace(self.font_size);
            (f.glyph_width(&fid, 'M'), f.row_height(&fid))
        });

        #[cfg(feature = "gpu-renderer")]
        let (cell_w, cell_h) = if let Some(gpu) = &self.gpu_renderer {
            let cw = gpu.cell_width();
            let ch = gpu.cell_height();
            if cw > 0.0 && ch > 0.0 {
                (cw, ch)
            } else {
                (cell_w, cell_h)
            }
        } else {
            (cell_w, cell_h)
        };

        if cell_w <= 0.0 || cell_h <= 0.0 {
            return;
        }

        // Get the content rect (below tab bar) for this pane
        let pane_rect = if let Some(zoomed_id) = self.active_workspace().zoomed {
            if zoomed_id == pane_id {
                panel_rect
            } else {
                return;
            }
        } else {
            let layout = self.active_workspace().tree.layout(panel_rect);
            match layout.iter().find(|(id, _)| *id == pane_id) {
                Some((_, r)) => *r,
                None => return,
            }
        };
        let content_top = pane_rect.min.y + TAB_BAR_HEIGHT;
        if hover_pos.y < content_top || hover_pos.x < pane_rect.min.x {
            return;
        }
        let col = ((hover_pos.x - pane_rect.min.x) / cell_w) as usize;
        let row = ((hover_pos.y - content_top) / cell_h) as usize;

        // Check if cell has a hyperlink
        if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&pane_id) {
            if managed.active_is_browser() {
                return;
            }
            let surface = managed.active_surface();
            let (cols, rows) = surface.pane.dimensions();
            if col >= cols || row >= rows {
                return;
            }
            let total = surface.pane.scrollback_rows();
            let end = total.saturating_sub(surface.scroll_offset);
            let start = end.saturating_sub(rows);
            let phys_row = start + row;
            let screen_rows = surface.pane.read_cells_range(phys_row, phys_row + 1);
            if let Some(screen_row) = screen_rows.first() {
                if let Some(cell) = screen_row.cells.get(col) {
                    if let Some(ref url) = cell.hyperlink_url {
                        self.hovered_hyperlink = Some(url.clone());

                        // Set pointer cursor
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);

                        // Cmd+click opens URL
                        let cmd_held = ctx.input(|i| {
                            #[cfg(target_os = "macos")]
                            {
                                i.modifiers.mac_cmd || i.modifiers.command
                            }
                            #[cfg(not(target_os = "macos"))]
                            {
                                i.modifiers.ctrl
                            }
                        });
                        if cmd_held && ctx.input(|i| i.pointer.primary_clicked()) {
                            // Only open safe URL schemes (case-insensitive).
                            let lower = url.to_ascii_lowercase();
                            if lower.starts_with("http://")
                                || lower.starts_with("https://")
                                || lower.starts_with("mailto:")
                            {
                                if self.app_config.browser.open_terminal_links_in_app
                                    && !lower.starts_with("mailto:")
                                {
                                    self.open_url_in_browser_pane(url);
                                } else {
                                    let _ = open::that(url);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
