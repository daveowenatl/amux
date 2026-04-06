//! Single-pane rendering: tab bar + terminal content area.
//!
//! Draws the per-pane tab bar (surface tabs) and delegates the terminal
//! content rendering to either the GPU or software renderer. Handles
//! tab interactions: click to activate, middle-click to close, right-click
//! context menu, drag to reorder, and the trailing "+" to add a surface.
//!
//! Mirrors wezterm-gui's `termwindow/render/pane.rs` — the per-pane
//! renderer that emits tab chrome plus the terminal content call.

use crate::*;

impl AmuxApp {
    /// Render a single pane: tab bar (if >1 surface) + terminal content.
    pub(crate) fn render_single_pane(
        &mut self,
        ui: &mut egui::Ui,
        pane_id: PaneId,
        rect: egui::Rect,
        is_focused: bool,
    ) {
        let managed = match self.panes.get_mut(&pane_id) {
            Some(PaneEntry::Terminal(m)) => m,
            None => return,
        };

        // Always show tab bar
        let tab_rect =
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), TAB_BAR_HEIGHT));
        let content_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x, rect.min.y + TAB_CONTENT_TOP_INSET),
            egui::pos2(rect.max.x, rect.max.y - TERMINAL_BOTTOM_PAD),
        );
        // Paint bottom padding strip with terminal background color.
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.min.x, rect.max.y - TERMINAL_BOTTOM_PAD),
                rect.max,
            ),
            0.0,
            self.theme.terminal_bg(),
        );

        {
            let painter = ui.painter();
            painter.rect_filled(tab_rect, 0.0, self.theme.tab_bar_bg());
            let bar_stroke = egui::Stroke::new(1.0, self.theme.chrome.tab_bar_border);
            painter.hline(tab_rect.x_range(), tab_rect.min.y, bar_stroke);
            painter.hline(tab_rect.x_range(), tab_rect.max.y, bar_stroke);

            let active_idx = managed.active_surface_idx;
            let tab_font = egui::FontId::proportional(11.5);
            let tab_icon = ">_ ";
            let mut x = tab_rect.min.x + 2.0;

            // Get pointer state for hover detection and drag
            let hover_pos = ui.input(|i| i.pointer.hover_pos());
            let primary_pressed = ui.input(|i| i.pointer.primary_pressed());

            let any_released = ui.input(|i| i.pointer.any_released());
            let primary_down = ui.input(|i| i.pointer.primary_down());
            let interact_pos = ui.input(|i| i.pointer.interact_pos());

            // Track actions to apply after rendering
            let mut switch_to: Option<usize> = None;

            let mut close_tab: Option<usize> = None;
            let mut tab_rects: Vec<egui::Rect> = Vec::new();
            let mut drag_start: Option<usize> = None;
            let mut start_rename_tab: Option<usize> = None;

            for (idx, surface) in managed.surfaces.iter().enumerate() {
                let is_active = idx == active_idx;
                let raw_title = surface
                    .user_title
                    .as_deref()
                    .unwrap_or_else(|| surface.pane.title());
                let raw = if raw_title.is_empty() || raw_title == "?" {
                    // Fall back to working directory path.
                    surface
                        .metadata
                        .cwd
                        .as_deref()
                        .map(|p| {
                            // Replace home-dir prefix with ~ using path semantics
                            // (string strip_prefix would match partial components
                            // like "/home/dave" vs "/home/dave2").
                            if let Some(home) = dirs::home_dir() {
                                let path = std::path::Path::new(p);
                                if let Ok(rest) = path.strip_prefix(&home) {
                                    if rest.as_os_str().is_empty() {
                                        return "~".to_string();
                                    }
                                    return format!("~/{}", rest.to_string_lossy());
                                }
                            }
                            p.to_string()
                        })
                        .unwrap_or_else(|| "Tab".to_string())
                } else {
                    raw_title.to_string()
                };
                // Cap at 20 chars for any title source — long cwd paths would
                // otherwise overflow the TAB_MAX_WIDTH-clamped tab and overlap
                // neighbors. Ellipsize with "..." when truncated.
                let title = if raw.chars().count() > 20 {
                    let prefix: String = raw.chars().take(17).collect();
                    format!("{prefix}...")
                } else {
                    raw
                };
                let label = format!("{tab_icon}{title}");

                let text_galley =
                    painter.layout_no_wrap(label.clone(), tab_font.clone(), egui::Color32::WHITE);
                let text_width = text_galley.size().x;
                let tab_w = (text_width + 24.0).clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH);

                let this_tab = egui::Rect::from_min_size(
                    egui::pos2(x, tab_rect.min.y),
                    egui::vec2(tab_w, TAB_BAR_HEIGHT),
                );
                tab_rects.push(this_tab);

                let tab_hovered = hover_pos.is_some_and(|p| this_tab.contains(p));

                let is_dead = surface.exited.is_some();
                let is_leftmost = idx == 0;

                // Tab background + border
                let border_color = self.theme.chrome.tab_border;
                let side_stroke = egui::Stroke::new(1.0, border_color);
                if is_active {
                    painter.rect_filled(this_tab, 0.0, self.theme.chrome.tab_active_bg);
                    // Active highlight at the top
                    let topline = egui::Rect::from_min_size(
                        egui::pos2(x, tab_rect.min.y),
                        egui::vec2(tab_w, 2.0),
                    );
                    let accent = if is_dead {
                        egui::Color32::from_gray(60)
                    } else {
                        self.theme.chrome.accent
                    };
                    painter.rect_filled(topline, 0.0, accent);
                    // Side borders only (no bottom border — tab merges with terminal)
                    if !is_leftmost {
                        painter.vline(this_tab.min.x, this_tab.y_range(), side_stroke);
                    }
                    painter.vline(this_tab.max.x, this_tab.y_range(), side_stroke);
                    // Paint over the tab bar bottom border so active tab merges cleanly
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(this_tab.min.x + 1.0, tab_rect.max.y - 1.0),
                            egui::pos2(this_tab.max.x, tab_rect.max.y + 1.0),
                        ),
                        0.0,
                        self.theme.chrome.tab_active_bg,
                    );
                } else {
                    // Inactive tabs: top, right, bottom borders (skip left for leftmost)
                    painter.hline(this_tab.x_range(), this_tab.min.y, side_stroke);
                    painter.hline(this_tab.x_range(), this_tab.max.y, side_stroke);
                    painter.vline(this_tab.max.x, this_tab.y_range(), side_stroke);
                    if !is_leftmost {
                        painter.vline(this_tab.min.x, this_tab.y_range(), side_stroke);
                    }
                }
                let text_color = if is_dead {
                    egui::Color32::from_gray(80)
                } else {
                    egui::Color32::from_gray(180)
                };
                let text_font = tab_font.clone();
                painter.text(
                    egui::pos2(x + 6.0, tab_rect.min.y + 6.0),
                    egui::Align2::LEFT_TOP,
                    &label,
                    text_font,
                    text_color,
                );

                // Close button — only visible on hover
                let close_center = egui::pos2(x + tab_w - 10.0, tab_rect.center().y);
                let close_rect = egui::Rect::from_center_size(close_center, egui::vec2(12.0, 12.0));
                if tab_hovered {
                    let close_hovered = hover_pos.is_some_and(|p| close_rect.contains(p));
                    let close_color = if close_hovered {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_gray(90)
                    };
                    sidebar::paint_close_x(painter, close_center, 3.5, close_color);
                }

                // Right-click context menu
                let tab_id = ui.id().with("tab_ctx").with(pane_id).with(idx);
                let tab_response = ui.interact(this_tab, tab_id, egui::Sense::click());
                tab_response.context_menu(|ui| {
                    if ui.button("Rename Tab").clicked() {
                        start_rename_tab = Some(idx);
                        ui.close_menu();
                    }
                    if ui.button("Close Tab").clicked() {
                        close_tab = Some(idx);
                        ui.close_menu();
                    }
                });

                // Hit testing (primary button only)
                if primary_pressed {
                    if let Some(pos) = interact_pos {
                        if tab_hovered && close_rect.contains(pos) {
                            close_tab = Some(idx);
                        } else if this_tab.contains(pos) && !is_active {
                            switch_to = Some(idx);
                        }
                        // Start tab drag
                        if this_tab.contains(pos)
                            && !close_rect.contains(pos)
                            && self.tab_drag.is_none()
                        {
                            drag_start = Some(idx);
                        }
                    }
                }

                x += tab_w + 1.0;
            }

            // Tab drag reorder logic
            if let Some(src) = drag_start {
                self.tab_drag = Some(TabDragState {
                    pane_id,
                    source_idx: src,
                    drop_target_idx: src,
                });
            }

            if let Some(drag) = &mut self.tab_drag {
                if drag.pane_id == pane_id {
                    if any_released || !primary_down {
                        let from = drag.source_idx;
                        let to = drag.drop_target_idx;
                        self.tab_drag = None;
                        if from != to {
                            if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&pane_id) {
                                let surface = m.surfaces.remove(from);
                                let insert_idx = if from < to {
                                    (to - 1).min(m.surfaces.len())
                                } else {
                                    to.min(m.surfaces.len())
                                };
                                m.surfaces.insert(insert_idx, surface);
                                if m.active_surface_idx == from {
                                    m.active_surface_idx = insert_idx;
                                } else if from < m.active_surface_idx
                                    && insert_idx >= m.active_surface_idx
                                {
                                    m.active_surface_idx -= 1;
                                } else if from > m.active_surface_idx
                                    && insert_idx <= m.active_surface_idx
                                {
                                    m.active_surface_idx += 1;
                                }
                            }
                            return;
                        }
                    } else if let Some(pos) = hover_pos {
                        // Compute drop target from X position vs tab midpoints
                        let tab_midpoints: Vec<f32> =
                            tab_rects.iter().map(|r| r.center().x).collect();
                        let mut target = tab_midpoints.len();
                        for (i, &mid) in tab_midpoints.iter().enumerate() {
                            if pos.x < mid {
                                target = i;
                                break;
                            }
                        }
                        drag.drop_target_idx = target;

                        // Paint drop indicator
                        let drop_x = if target == 0 {
                            tab_rects.first().map(|r| r.min.x).unwrap_or(x)
                        } else if target < tab_rects.len() {
                            let left = tab_rects[target - 1].max.x;
                            let right = tab_rects[target].min.x;
                            (left + right) / 2.0
                        } else {
                            tab_rects.last().map(|r| r.max.x).unwrap_or(x)
                        };
                        let indicator_rect = egui::Rect::from_min_size(
                            egui::pos2(drop_x - 1.0, tab_rect.min.y + 2.0),
                            egui::vec2(2.0, TAB_BAR_HEIGHT - 4.0),
                        );
                        painter.rect_filled(indicator_rect, 1.0, self.theme.chrome.accent);
                    }
                }
            }

            // "+" button to add tab
            let plus_rect = egui::Rect::from_min_size(
                egui::pos2(x + 2.0, tab_rect.min.y),
                egui::vec2(20.0, TAB_BAR_HEIGHT),
            );
            painter.text(
                plus_rect.center(),
                egui::Align2::CENTER_CENTER,
                "+",
                egui::FontId::proportional(14.0),
                egui::Color32::from_gray(100),
            );
            if primary_pressed {
                if let Some(pos) = interact_pos {
                    if plus_rect.contains(pos) {
                        let ws_id = self.active_workspace().id;
                        let sf_id = self.next_surface_id;
                        self.next_surface_id += 1;
                        if let Ok(surface) = startup::spawn_surface(
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
                            // Re-borrow managed after spawn_surface
                            if let Some(PaneEntry::Terminal(m)) = self.panes.get_mut(&pane_id) {
                                m.surfaces.push(surface);
                                m.active_surface_idx = m.surfaces.len() - 1;
                            }
                        }
                        return; // skip further rendering this frame
                    }
                }
            }

            // Apply tab switch/close (need to re-borrow managed)
            if let Some(idx) = close_tab {
                let is_last = self.panes.get(&pane_id).is_some_and(|e| {
                    let PaneEntry::Terminal(m) = e;
                    m.surfaces.len() <= 1
                });
                if is_last {
                    self.close_pane(pane_id);
                    return;
                }
                let PaneEntry::Terminal(managed) = self.panes.get_mut(&pane_id).unwrap();
                managed.surfaces.remove(idx);
                if idx < managed.active_surface_idx {
                    managed.active_surface_idx -= 1;
                } else if managed.active_surface_idx >= managed.surfaces.len() {
                    managed.active_surface_idx = managed.surfaces.len() - 1;
                }
            } else if let Some(idx) = switch_to {
                let PaneEntry::Terminal(managed) = self.panes.get_mut(&pane_id).unwrap();
                managed.active_surface_idx = idx;
            }

            // Open rename modal for tab
            if let Some(idx) = start_rename_tab {
                if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&pane_id) {
                    if idx < managed.surfaces.len() {
                        let surface = &managed.surfaces[idx];
                        let current_title = surface
                            .user_title
                            .clone()
                            .unwrap_or_else(|| surface.pane.title().to_string());
                        self.rename_modal = Some(RenameModal {
                            target: RenameTarget::Tab {
                                pane_id,
                                surface_id: surface.id,
                            },
                            buf: current_title,
                            just_opened: true,
                        });
                    }
                }
            }
        }

        // Collect find highlights for this pane
        let (find_highlights, current_highlight) = self
            .find_state
            .as_ref()
            .filter(|f| f.pane_id == pane_id)
            .map(|f| (f.matches.clone(), Some(f.current_match)))
            .unwrap_or_default();

        // Build selection: use copy mode visual selection if active, else normal selection
        let copy_mode_sel = self
            .copy_mode
            .as_ref()
            .filter(|cm| cm.pane_id == pane_id && cm.visual_anchor.is_some())
            .map(|cm| {
                let anchor = cm.visual_anchor.unwrap();
                let cursor = cm.cursor;
                let (start, end) =
                    if anchor.1 < cursor.1 || (anchor.1 == cursor.1 && anchor.0 <= cursor.0) {
                        (anchor, cursor)
                    } else {
                        (cursor, anchor)
                    };
                SelectionState {
                    anchor: start,
                    end,
                    mode: if cm.line_visual {
                        SelectionMode::Line
                    } else {
                        SelectionMode::Cell
                    },
                    active: false,
                }
            });

        // Render terminal content for the active surface
        let managed = match self.panes.get_mut(&pane_id) {
            Some(PaneEntry::Terminal(m)) => m,
            None => return,
        };
        let selection = copy_mode_sel
            .as_ref()
            .or(managed.selection.as_ref())
            .cloned();
        let surface = managed.active_surface_mut();
        // Cursor blink: 500ms on, 500ms off cycle, reset on input.
        let blink_elapsed_ms = self.cursor_blink_since.elapsed().as_millis();
        let cursor_blink_on = (blink_elapsed_ms % 1000) < 500;
        render::render_pane(
            ui,
            &mut surface.pane,
            content_rect,
            is_focused,
            surface.scroll_offset,
            selection.as_ref(),
            self.font_size,
            &find_highlights,
            current_highlight,
            cursor_blink_on,
            #[cfg(feature = "gpu-renderer")]
            self.gpu_renderer.as_ref(),
            #[cfg(feature = "gpu-renderer")]
            pane_id,
        );

        // Render exit overlay when process has exited
        if let Some(exit_info) = &surface.exited {
            render::render_exit_overlay(ui, content_rect, &exit_info.message, self.font_size);
        }

        // Render copy mode cursor overlay
        if let Some(cm) = self.copy_mode.as_ref().filter(|cm| cm.pane_id == pane_id) {
            let (cell_w, cell_h) = self.cell_dimensions(ui);
            if let Some(PaneEntry::Terminal(managed)) = self.panes.get(&pane_id) {
                let surface = managed.active_surface();
                let (_, rows) = surface.pane.dimensions();
                let total = surface.pane.scrollback_rows();
                let end = total.saturating_sub(surface.scroll_offset);
                let start = end.saturating_sub(rows);
                if cm.cursor.1 >= start && cm.cursor.1 < end {
                    let row_in_view = cm.cursor.1 - start;
                    let x = content_rect.min.x + cm.cursor.0 as f32 * cell_w;
                    let y = content_rect.min.y + row_in_view as f32 * cell_h;
                    let cursor_rect =
                        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell_w, cell_h));
                    ui.painter().rect_stroke(
                        cursor_rect,
                        0.0,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 255, 0)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }

        // Notification ring + flash animation (matching cmux)
        let pane_u64 = pane_id;
        let ring_rect = rect.shrink(2.0);

        // Fetch pane state once; used for both flash detection and animation.
        let pane_state = self.notifications.pane_state(pane_u64);
        // Is a flash animation currently running? (used to suppress the
        // persistent ring so the two don't double-stroke the same path)
        let flash_active = pane_state
            .and_then(|s| s.flash_started_at)
            .is_some_and(|started| started.elapsed().as_secs_f32() < FLASH_DURATION);

        // 1. Persistent unread ring (NOT on focused pane, NOT during flash)
        if !is_focused && !flash_active && self.notifications.pane_unread(pane_u64) > 0 {
            // Steady blue ring with glow
            let ring_color = egui::Color32::from_rgba_unmultiplied(40, 120, 255, 89); // 0.35 * 255
            ui.painter().rect_stroke(
                ring_rect,
                6.0,
                egui::Stroke::new(2.5, ring_color),
                egui::StrokeKind::Inside,
            );
            ui.ctx().request_repaint();
        }

        // 2. Flash animation (on ANY pane including focused)
        if let Some(state) = pane_state {
            if let Some(started) = state.flash_started_at {
                let elapsed = started.elapsed().as_secs_f32();
                if elapsed < FLASH_DURATION {
                    let alpha = flash_alpha(elapsed);
                    let base_color = match state.flash_reason {
                        Some(FlashReason::Navigation) => [0u8, 128, 128], // teal
                        _ => [40, 120, 255],                              // blue
                    };
                    let glow_alpha = (alpha * 0.6 * 255.0) as u8;
                    let ring_alpha = (alpha * 255.0) as u8;
                    // Glow (wider, more transparent). Anchor on ring_rect with
                    // Outside kind so it butts up directly against the inner ring
                    // at the ring_rect boundary — any gap or expansion leaves a
                    // visible 1px dark line where the terminal bg shows through.
                    ui.painter().rect_stroke(
                        ring_rect,
                        6.0,
                        egui::Stroke::new(
                            4.0,
                            egui::Color32::from_rgba_unmultiplied(
                                base_color[0],
                                base_color[1],
                                base_color[2],
                                glow_alpha,
                            ),
                        ),
                        egui::StrokeKind::Outside,
                    );
                    // Ring
                    ui.painter().rect_stroke(
                        ring_rect,
                        6.0,
                        egui::Stroke::new(
                            2.5,
                            egui::Color32::from_rgba_unmultiplied(
                                base_color[0],
                                base_color[1],
                                base_color[2],
                                ring_alpha,
                            ),
                        ),
                        egui::StrokeKind::Inside,
                    );
                    ui.ctx().request_repaint();
                }
            }
        }
    }
}
