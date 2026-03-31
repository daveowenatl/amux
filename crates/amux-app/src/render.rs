//! Pane rendering: GPU/CPU dispatch, exit overlay, and color conversion.

use std::time::Duration;

use amux_term::backend::{Color, CursorShape, TerminalBackend};
use amux_term::AnyBackend;

use crate::managed_pane::SelectionState;

#[cfg(feature = "gpu-renderer")]
use amux_render_gpu::GpuRenderer;

/// Render a terminal pane into the given rect, using GPU renderer when
/// available and falling back to the egui software path.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_pane(
    ui: &mut egui::Ui,
    pane: &mut AnyBackend,
    rect: egui::Rect,
    is_focused: bool,
    scroll_offset: usize,
    selection: Option<&SelectionState>,
    font_size: f32,
    find_highlights: &[(usize, usize, usize)],
    current_highlight: Option<usize>,
    #[cfg(feature = "gpu-renderer")] gpu_renderer: Option<&GpuRenderer>,
    #[cfg(feature = "gpu-renderer")] pane_id: u64,
) {
    // GPU renderer path: build snapshot, emit a paint callback, and return early.
    #[cfg(feature = "gpu-renderer")]
    if let Some(gpu) = gpu_renderer {
        let (actual_cols, actual_rows) = pane.dimensions();
        if actual_cols == 0 || actual_rows == 0 {
            return;
        }
        let gpu_selection = selection.map(|sel| {
            let (start, end) = sel.normalized();
            amux_render_gpu::snapshot::SelectionRange { start, end }
        });
        let seqno = pane.current_seqno();

        // Build snapshot: use wezterm-specific path (with Kitty image support)
        // when available, otherwise fall back to trait-based path.
        let snapshot = if let Some(wez) = pane.as_wezterm() {
            let palette = wez.color_palette();
            let cursor = pane.cursor();
            let screen = wez.screen();
            amux_render_gpu::TerminalSnapshot::from_screen(
                screen,
                &palette,
                &cursor,
                actual_cols,
                actual_rows,
                scroll_offset,
                is_focused,
                gpu_selection,
                pane_id,
                seqno,
                find_highlights.to_vec(),
                current_highlight,
            )
        } else {
            amux_render_gpu::TerminalSnapshot::from_backend(
                pane,
                actual_cols,
                actual_rows,
                scroll_offset,
                is_focused,
                gpu_selection,
                pane_id,
                seqno,
                find_highlights.to_vec(),
                current_highlight,
            )
        };
        let pixels_per_point = ui.ctx().pixels_per_point();
        let callback = gpu.paint_callback(rect, snapshot, pixels_per_point);
        ui.painter().add(egui::Shape::Callback(callback));
        return;
    }

    // --- Software renderer path (uses TerminalBackend trait) ---

    let font_id = egui::FontId::monospace(font_size);
    let cell_width = ui.fonts(|f| f.glyph_width(&font_id, 'M'));
    let cell_height = ui.fonts(|f| f.row_height(&font_id));

    let (actual_cols, actual_rows) = pane.dimensions();
    if actual_cols == 0 || actual_rows == 0 {
        return;
    }

    let palette = pane.palette();
    let cursor = pane.cursor();
    let bg_default = color_to_egui(palette.background);

    let painter = ui.painter();
    let origin = rect.min;

    // Fill the full allocated rect first to avoid artifacts when terminal is smaller
    painter.rect_filled(rect, 0.0, bg_default);

    // Dim unfocused panes with a semi-transparent overlay
    if !is_focused {
        let dim_overlay = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 100);
        painter.rect_filled(rect, 0.0, dim_overlay);
    }

    let total = pane.scrollback_rows();
    let end = total.saturating_sub(scroll_offset);
    let start = end.saturating_sub(actual_rows);
    let screen_rows = pane.read_cells_range(start, end);

    for (row_idx, screen_row) in screen_rows.iter().enumerate() {
        let y = origin.y + row_idx as f32 * cell_height;
        if y + cell_height < rect.min.y || y > rect.max.y {
            continue;
        }

        for (col_idx, cell) in screen_row.cells.iter().enumerate() {
            if col_idx >= actual_cols {
                break;
            }

            let x = origin.x + col_idx as f32 * cell_width;
            if x + cell_width < rect.min.x || x > rect.max.x {
                continue;
            }

            let mut fg = color_to_egui(cell.fg);
            let mut bg = color_to_egui(cell.bg);

            // Selection: swap fg/bg for selected cells (reverse video)
            let stable_row = start + row_idx;
            if let Some(sel) = selection {
                if sel.contains(col_idx, stable_row) {
                    std::mem::swap(&mut fg, &mut bg);
                    // Ensure selected empty cells have visible bg
                    if bg == bg_default {
                        bg = color_to_egui(palette.foreground);
                        fg = bg_default;
                    }
                }
            }

            // Find/search highlighting
            for (i, &(h_row, h_start, h_end)) in find_highlights.iter().enumerate() {
                if h_row == stable_row && col_idx >= h_start && col_idx < h_end {
                    if current_highlight == Some(i) {
                        bg = egui::Color32::from_rgb(255, 153, 0); // orange
                    } else {
                        bg = egui::Color32::from_rgba_unmultiplied(255, 255, 0, 180);
                        // yellow
                    }
                    fg = egui::Color32::BLACK;
                    break;
                }
            }

            if bg != bg_default {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, y),
                        egui::vec2(cell_width, cell_height),
                    ),
                    0.0,
                    bg,
                );
            }

            let text = &cell.text;
            if !text.is_empty() && text != " " {
                painter.text(
                    egui::pos2(x, y),
                    egui::Align2::LEFT_TOP,
                    text,
                    font_id.clone(),
                    fg,
                );
            }
        }
    }

    // Draw cursor
    if is_focused
        && scroll_offset == 0
        && cursor.visible
        && cursor.y >= 0
        && (cursor.y as usize) < actual_rows
        && cursor.x < actual_cols
    {
        let cx = origin.x + cursor.x as f32 * cell_width;
        let cy = origin.y + cursor.y as f32 * cell_height;
        let cursor_color = color_to_egui(palette.cursor_bg);

        match cursor.shape {
            CursorShape::BlinkingBar | CursorShape::SteadyBar => {
                let bar_rect =
                    egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(2.0, cell_height));
                painter.rect_filled(bar_rect, 0.0, cursor_color);
            }
            CursorShape::BlinkingUnderline | CursorShape::SteadyUnderline => {
                let underline_rect = egui::Rect::from_min_size(
                    egui::pos2(cx, cy + cell_height - 2.0),
                    egui::vec2(cell_width, 2.0),
                );
                painter.rect_filled(underline_rect, 0.0, cursor_color);
            }
            CursorShape::Default | CursorShape::BlinkingBlock | CursorShape::SteadyBlock => {
                let cursor_rect = egui::Rect::from_min_size(
                    egui::pos2(cx, cy),
                    egui::vec2(cell_width, cell_height),
                );
                let cursor_fg = color_to_egui(palette.cursor_fg);
                painter.rect_filled(cursor_rect, 0.0, cursor_color);

                // Draw text under the block cursor
                let cursor_line_idx = cursor.y as usize;
                if cursor_line_idx < screen_rows.len() {
                    if let Some(cell) = screen_rows[cursor_line_idx].cells.get(cursor.x) {
                        let text = &cell.text;
                        if !text.is_empty() && text != " " {
                            painter.text(
                                egui::pos2(cx, cy),
                                egui::Align2::LEFT_TOP,
                                text,
                                font_id.clone(),
                                cursor_fg,
                            );
                        }
                    }
                }
            }
        }
    }

    // Scroll indicator
    if scroll_offset > 0 {
        let indicator = format!("[+{}]", scroll_offset);
        let indicator_font = egui::FontId::monospace(font_size * 0.8);
        let text_color = egui::Color32::from_rgba_unmultiplied(255, 200, 50, 200);
        let bg_color = egui::Color32::from_rgba_unmultiplied(40, 40, 40, 180);

        let galley = painter.layout_no_wrap(indicator, indicator_font, text_color);
        let text_size = galley.size();
        let padding = 4.0;
        let indicator_rect = egui::Rect::from_min_size(
            egui::pos2(
                rect.right() - text_size.x - padding * 2.0,
                rect.top() + padding,
            ),
            egui::vec2(text_size.x + padding * 2.0, text_size.y + padding),
        );
        painter.rect_filled(indicator_rect, 3.0, bg_color);
        painter.galley(
            egui::pos2(
                indicator_rect.left() + padding,
                indicator_rect.top() + padding * 0.5,
            ),
            galley,
            text_color,
        );
    }
}

/// Render a semi-transparent overlay at the bottom of a pane showing exit status
/// and available actions (Enter to close, R to restart).
pub(crate) fn render_exit_overlay(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    message: &str,
    font_size: f32,
) {
    let painter = ui.painter_at(rect);
    let overlay_font = egui::FontId::monospace(font_size * 0.85);
    let small_font = egui::FontId::monospace(font_size * 0.75);

    let line1 = message;
    let line2 = "Press Enter to close  |  R to restart";

    let g1 = painter.layout_no_wrap(
        line1.to_string(),
        overlay_font.clone(),
        egui::Color32::WHITE,
    );
    let g2 = painter.layout_no_wrap(line2.to_string(), small_font, egui::Color32::from_gray(160));

    let text_width = g1.size().x.max(g2.size().x);
    let text_height = g1.size().y + g2.size().y + 4.0;
    let padding = 8.0;
    let box_width = text_width + padding * 2.0;
    let box_height = text_height + padding * 2.0;

    let box_rect = egui::Rect::from_min_size(
        egui::pos2(
            rect.center().x - box_width / 2.0,
            rect.bottom() - box_height - 12.0,
        ),
        egui::vec2(box_width, box_height),
    );

    // Semi-transparent background with border
    painter.rect_filled(
        box_rect,
        4.0,
        egui::Color32::from_rgba_unmultiplied(30, 30, 30, 220),
    );
    painter.rect_stroke(
        box_rect,
        4.0,
        egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
        egui::StrokeKind::Outside,
    );

    // Center text lines
    let x1 = box_rect.center().x - g1.size().x / 2.0;
    let y1 = box_rect.min.y + padding;
    painter.galley(egui::pos2(x1, y1), g1, egui::Color32::WHITE);

    let x2 = box_rect.center().x - g2.size().x / 2.0;
    let y2 = y1 + text_height - g2.size().y;
    painter.galley(egui::pos2(x2, y2), g2, egui::Color32::from_gray(160));
}

/// Convert an amux-native Color to egui Color32.
pub(crate) fn color_to_egui(c: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (c.0 * 255.0).round() as u8,
        (c.1 * 255.0).round() as u8,
        (c.2 * 255.0).round() as u8,
        (c.3 * 255.0).round() as u8,
    )
}

pub(crate) fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}
