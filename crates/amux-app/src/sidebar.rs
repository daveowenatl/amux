use std::collections::HashMap;

use amux_notify::NotificationStore;
use egui::Color32;

use crate::{SidebarDragState, SidebarState, SurfaceMetadata, Workspace};

// ---------------------------------------------------------------------------
// Colors (cmux dark mode equivalents)
// ---------------------------------------------------------------------------

const ACCENT_BLUE: Color32 = Color32::from_rgb(0, 145, 255);
const SIDEBAR_BG: Color32 = Color32::from_rgba_premultiplied(20, 20, 20, 230);
const ROW_ACTIVE_BG: Color32 = Color32::from_rgb(0, 145, 255);
const ROW_HOVER_BG: Color32 = Color32::from_rgba_premultiplied(15, 15, 15, 15);
const TEXT_ACTIVE: Color32 = Color32::WHITE;
const TEXT_INACTIVE: Color32 = Color32::from_gray(180);
const TEXT_SECONDARY: Color32 = Color32::from_gray(140);
const BADGE_ACTIVE_BG: Color32 = Color32::from_rgba_premultiplied(64, 64, 64, 64);
const STATUS_GREEN: Color32 = Color32::from_rgb(50, 180, 80);
const STATUS_ORANGE: Color32 = Color32::from_rgb(230, 170, 40);
const STATUS_GRAY: Color32 = Color32::from_gray(100);
const NEW_BTN_TEXT: Color32 = Color32::from_gray(140);
const NEW_BTN_HOVER: Color32 = Color32::from_rgba_premultiplied(15, 15, 15, 15);
const CLOSE_BTN_COLOR: Color32 = Color32::from_rgba_premultiplied(140, 140, 140, 179);
const PROGRESS_TRACK: Color32 = Color32::from_rgba_premultiplied(20, 20, 20, 20);

// ---------------------------------------------------------------------------
// Layout constants (matching cmux points)
// ---------------------------------------------------------------------------

pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 180.0;
pub(crate) const SIDEBAR_MAX_WIDTH: f32 = 600.0;
const ROW_H_PAD: f32 = 10.0;
const ROW_V_PAD: f32 = 8.0;
const ROW_OUTER_H_PAD: f32 = 6.0;
const ROW_SPACING: f32 = 2.0;
const ROW_CORNER_RADIUS: f32 = 6.0;
const TITLE_FONT_SIZE: f32 = 12.5;
const BADGE_RADIUS: f32 = 8.0;
const BADGE_FONT_SIZE: f32 = 9.0;
const PILL_FONT_SIZE: f32 = 9.0;
const PILL_HEIGHT: f32 = 14.0;
const PILL_CORNER_RADIUS: f32 = 7.0;
const COUNT_FONT_SIZE: f32 = 10.0;
const NOTIF_FONT_SIZE: f32 = 10.0;
const NOTIF_PREVIEW_HEIGHT: f32 = 24.0;
const CLOSE_BTN_SIZE: f32 = 16.0;
const COLOR_CAPSULE_WIDTH: f32 = 3.0;
const PROGRESS_BAR_HEIGHT: f32 = 3.0;
const DROP_INDICATOR_HEIGHT: f32 = 2.0;
const METADATA_FONT_SIZE: f32 = 10.0;
const METADATA_LINE_HEIGHT: f32 = 16.0;
const PR_MERGED_COLOR: Color32 = Color32::from_rgb(130, 80, 223); // purple for merged
const PR_OPEN_COLOR: Color32 = Color32::from_rgb(0, 145, 255);
const PR_CLOSED_COLOR: Color32 = Color32::from_gray(100);
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_SPACER: f32 = 28.0;

// Preset workspace colors (8 options matching cmux)
const PRESET_COLORS: &[([u8; 4], &str)] = &[
    ([255, 59, 48, 255], "Red"),
    ([255, 149, 0, 255], "Orange"),
    ([255, 204, 0, 255], "Yellow"),
    ([52, 199, 89, 255], "Green"),
    ([0, 199, 190, 255], "Teal"),
    ([0, 122, 255, 255], "Blue"),
    ([88, 86, 214, 255], "Purple"),
    ([255, 45, 85, 255], "Pink"),
];

// ---------------------------------------------------------------------------
// Actions returned from sidebar rendering
// ---------------------------------------------------------------------------

pub(crate) enum SidebarAction {
    SwitchWorkspace(usize),
    CreateWorkspace,
    CloseWorkspace(usize),
    RenameWorkspace(usize, String),
    MarkWorkspaceRead(usize),
    ReorderWorkspace(usize, usize),
    SetWorkspaceColor(usize, Option<[u8; 4]>),
}

// ---------------------------------------------------------------------------
// Shared close-X painter
// ---------------------------------------------------------------------------

/// Paint an X icon (two diagonal lines) centered at `center` with the given `size` and `color`.
pub(crate) fn paint_close_x(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    color: Color32,
) {
    let half = size / 2.0;
    let stroke = egui::Stroke::new(1.2, color);
    painter.line_segment(
        [
            egui::pos2(center.x - half, center.y - half),
            egui::pos2(center.x + half, center.y + half),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(center.x + half, center.y - half),
            egui::pos2(center.x - half, center.y + half),
        ],
        stroke,
    );
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub(crate) fn render_sidebar(
    ctx: &egui::Context,
    state: &mut SidebarState,
    workspaces: &[Workspace],
    active_workspace_idx: usize,
    notifications: &NotificationStore,
    workspace_metadata: &HashMap<u64, SurfaceMetadata>,
) -> Vec<SidebarAction> {
    let mut actions = Vec::new();

    egui::SidePanel::left("sidebar")
        .resizable(true)
        .default_width(state.width)
        .min_width(SIDEBAR_MIN_WIDTH)
        .max_width(SIDEBAR_MAX_WIDTH)
        .frame(
            egui::Frame::new()
                .fill(SIDEBAR_BG)
                .inner_margin(egui::Margin::symmetric(ROW_OUTER_H_PAD as i8, 0)),
        )
        .show(ctx, |ui| {
            // Persist the actual panel width back to state for session save/restore
            state.width = ui.available_width() + ROW_OUTER_H_PAD * 2.0;

            #[cfg(target_os = "macos")]
            ui.add_space(TRAFFIC_LIGHT_SPACER);
            #[cfg(not(target_os = "macos"))]
            ui.add_space(8.0);

            ui.spacing_mut().item_spacing.y = ROW_SPACING;

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Collect row rects for drag indicator positioning
                    let mut row_rects: Vec<egui::Rect> = Vec::with_capacity(workspaces.len());

                    for (idx, ws) in workspaces.iter().enumerate() {
                        let is_active = idx == active_workspace_idx;
                        let is_being_dragged =
                            state.drag.as_ref().is_some_and(|d| d.source_idx == idx);
                        let meta = workspace_metadata.get(&ws.id);
                        let (row_actions, row_rect) = render_workspace_row(
                            ui,
                            ws,
                            idx,
                            is_active,
                            is_being_dragged,
                            notifications,
                            state,
                            meta,
                        );
                        actions.extend(row_actions);
                        row_rects.push(row_rect);
                    }

                    // Compute row midpoints from actual rects
                    let row_midpoints: Vec<f32> = row_rects.iter().map(|r| r.center().y).collect();

                    // --- Drag reorder logic ---
                    handle_drag_reorder(ui, ctx, state, &row_midpoints, &mut actions);

                    // --- Drop indicator ---
                    if let Some(drag) = &state.drag {
                        let avail_w = ui.available_width();
                        // Place indicator at the edge between rows
                        let drop_y = if drag.drop_target_idx == 0 {
                            // Before first row: at the top edge
                            row_rects.first().map(|r| r.min.y).unwrap_or_default()
                        } else if drag.drop_target_idx < row_rects.len() {
                            // Between rows: at the boundary
                            let above = row_rects[drag.drop_target_idx - 1].max.y;
                            let below = row_rects[drag.drop_target_idx].min.y;
                            (above + below) / 2.0
                        } else {
                            // After last row: at the bottom edge
                            row_rects.last().map(|r| r.max.y).unwrap_or_default()
                        };
                        let indicator_x = row_rects
                            .first()
                            .map(|r| r.min.x)
                            .unwrap_or_else(|| ui.min_rect().min.x);
                        let indicator_rect = egui::Rect::from_min_size(
                            egui::pos2(indicator_x, drop_y - DROP_INDICATOR_HEIGHT / 2.0),
                            egui::vec2(avail_w, DROP_INDICATOR_HEIGHT),
                        );
                        ui.painter().rect_filled(indicator_rect, 1.0, ACCENT_BLUE);
                    }

                    ui.add_space(8.0);

                    // "+ New Workspace" button
                    let avail_w = ui.available_width();
                    let (btn_rect, btn_response) =
                        ui.allocate_exact_size(egui::vec2(avail_w, 28.0), egui::Sense::click());
                    if ui.is_rect_visible(btn_rect) {
                        if btn_response.hovered() {
                            ui.painter()
                                .rect_filled(btn_rect, ROW_CORNER_RADIUS, NEW_BTN_HOVER);
                        }
                        ui.painter().text(
                            btn_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "+ New Workspace",
                            egui::FontId::proportional(11.0),
                            if btn_response.hovered() {
                                TEXT_INACTIVE
                            } else {
                                NEW_BTN_TEXT
                            },
                        );
                    }
                    if btn_response.clicked() {
                        actions.push(SidebarAction::CreateWorkspace);
                    }

                    // Fill remaining space (double-click to create workspace)
                    let remaining = ui.available_height().max(0.0);
                    if remaining > 0.0 {
                        let (_, empty_response) = ui.allocate_exact_size(
                            egui::vec2(avail_w, remaining),
                            egui::Sense::click(),
                        );
                        if empty_response.double_clicked() {
                            actions.push(SidebarAction::CreateWorkspace);
                        }
                    }
                });
        });

    actions
}

/// Handle drag reorder state machine.
fn handle_drag_reorder(
    ui: &egui::Ui,
    ctx: &egui::Context,
    state: &mut SidebarState,
    row_midpoints: &[f32],
    actions: &mut Vec<SidebarAction>,
) {
    let (hover_pos, any_released, primary_down) = ui.input(|i| {
        (
            i.pointer.hover_pos(),
            i.pointer.any_released(),
            i.pointer.primary_down(),
        )
    });

    if let Some(drag) = &mut state.drag {
        if any_released || !primary_down {
            let from = drag.source_idx;
            let to = drag.drop_target_idx;
            state.drag = None;
            if from != to {
                actions.push(SidebarAction::ReorderWorkspace(from, to));
            }
        } else if let Some(pos) = hover_pos {
            drag.current_y = pos.y;
            drag.row_midpoints = row_midpoints.to_vec();

            // Compute drop target from pointer Y vs row midpoints
            let mut target = row_midpoints.len();
            for (i, &mid) in row_midpoints.iter().enumerate() {
                if pos.y < mid {
                    target = i;
                    break;
                }
            }
            drag.drop_target_idx = target;
            ctx.request_repaint();
        }
    }
}

/// Renders a workspace row. Returns (actions, allocated_rect).
#[allow(clippy::too_many_arguments)]
fn render_workspace_row(
    ui: &mut egui::Ui,
    ws: &Workspace,
    idx: usize,
    is_active: bool,
    is_being_dragged: bool,
    notifications: &NotificationStore,
    state: &mut SidebarState,
    metadata: Option<&SurfaceMetadata>,
) -> (Vec<SidebarAction>, egui::Rect) {
    let mut actions = Vec::new();
    let pane_ids: Vec<u64> = ws.tree.iter_panes();
    let unread = notifications.workspace_unread_count(&pane_ids);
    let status = notifications.workspace_status(ws.id);
    let has_status = status.is_some();
    let has_progress = status.as_ref().and_then(|s| s.progress).is_some();
    let has_agent_message = status.as_ref().and_then(|s| s.message.as_ref()).is_some();
    let latest_notif = notifications.latest_for_workspace(ws.id);
    let has_notif_text = !has_agent_message && latest_notif.is_some_and(|n| !n.body.is_empty());
    let is_renaming = state.renaming == Some(idx);
    let has_color = ws.color.is_some();
    let has_git_or_cwd = metadata.is_some_and(|m| m.git_branch.is_some() || m.cwd.is_some());
    let has_pr = metadata.is_some_and(|m| m.pr_number.is_some());

    // Dynamic row height
    let title_line_h = TITLE_FONT_SIZE + 2.0;
    let mut row_h = ROW_V_PAD * 2.0 + title_line_h;
    if has_agent_message {
        row_h += METADATA_LINE_HEIGHT + 2.0;
    }
    if has_status {
        row_h += PILL_HEIGHT + 4.0;
    }
    if has_git_or_cwd {
        row_h += METADATA_LINE_HEIGHT + 2.0;
    }
    if has_pr {
        row_h += METADATA_LINE_HEIGHT + 2.0;
    }
    if has_notif_text {
        row_h += NOTIF_PREVIEW_HEIGHT + 2.0;
    }
    if has_progress {
        row_h += PROGRESS_BAR_HEIGHT + 4.0;
    }

    let avail_w = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(avail_w, row_h), egui::Sense::click_and_drag());

    if !ui.is_rect_visible(rect) {
        if response.clicked() && !is_active {
            actions.push(SidebarAction::SwitchWorkspace(idx));
        }
        return (actions, rect);
    }

    let hovered = response.hovered();

    // --- Drag initiation ---
    if response.drag_started() && state.drag.is_none() && !is_renaming {
        state.drag = Some(SidebarDragState {
            source_idx: idx,
            current_y: rect.center().y,
            drop_target_idx: idx,
            row_midpoints: Vec::new(),
        });
    }

    // --- Middle-click to close ---
    if response.middle_clicked() {
        actions.push(SidebarAction::CloseWorkspace(idx));
        return (actions, rect);
    }

    // --- Context menu ---
    response.context_menu(|ui| {
        if ui.button("Rename Workspace").clicked() {
            state.renaming = Some(idx);
            state.rename_buf = ws.title.clone();
            state.rename_just_opened = true;
            ui.close_menu();
        }
        if ui.button("Close Workspace").clicked() {
            actions.push(SidebarAction::CloseWorkspace(idx));
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Mark All Read").clicked() {
            actions.push(SidebarAction::MarkWorkspaceRead(idx));
            ui.close_menu();
        }
        ui.separator();
        ui.menu_button("Set Color", |ui| {
            for &(color, name) in PRESET_COLORS {
                let c = Color32::from_rgba_premultiplied(color[0], color[1], color[2], color[3]);
                ui.horizontal(|ui| {
                    let (swatch_rect, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().circle_filled(swatch_rect.center(), 5.0, c);
                    if ui.button(name).clicked() {
                        actions.push(SidebarAction::SetWorkspaceColor(idx, Some(color)));
                        ui.close_menu();
                    }
                });
            }
            ui.separator();
            if ui.button("Clear").clicked() {
                actions.push(SidebarAction::SetWorkspaceColor(idx, None));
                ui.close_menu();
            }
        });
    });

    // --- Background ---
    let opacity = if is_being_dragged { 0.6 } else { 1.0 };
    let bg = if is_active {
        with_opacity(ROW_ACTIVE_BG, opacity)
    } else if hovered {
        with_opacity(ROW_HOVER_BG, opacity)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, ROW_CORNER_RADIUS, bg);

    // --- Workspace color capsule (leading edge) ---
    let content_left = if has_color {
        if let Some(c) = ws.color {
            let capsule_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + 2.0, rect.min.y + ROW_V_PAD),
                egui::vec2(COLOR_CAPSULE_WIDTH, row_h - ROW_V_PAD * 2.0),
            );
            let color = Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3]);
            ui.painter()
                .rect_filled(capsule_rect, COLOR_CAPSULE_WIDTH / 2.0, color);
        }
        ROW_H_PAD + COLOR_CAPSULE_WIDTH + 4.0
    } else {
        ROW_H_PAD
    };

    // --- Title (or rename TextEdit) ---
    let title_color = if is_active {
        TEXT_ACTIVE
    } else {
        TEXT_INACTIVE
    };

    let close_btn_reserve = CLOSE_BTN_SIZE + 4.0;
    let badge_reserve = BADGE_RADIUS * 2.0 + 4.0;
    let right_reserve = if hovered && !is_renaming {
        close_btn_reserve
    } else {
        badge_reserve
    };
    let max_title_w = avail_w - content_left - ROW_H_PAD - right_reserve;

    if is_renaming {
        let title_rect = egui::Rect::from_min_size(
            rect.min + egui::vec2(content_left, ROW_V_PAD),
            egui::vec2(max_title_w, title_line_h),
        );
        let rename_id = ui.id().with("rename").with(idx);
        let mut text_edit = egui::TextEdit::singleline(&mut state.rename_buf)
            .id(rename_id)
            .font(egui::FontId::proportional(TITLE_FONT_SIZE))
            .text_color(title_color)
            .desired_width(max_title_w)
            .frame(false);
        text_edit = text_edit.background_color(Color32::from_rgba_premultiplied(0, 0, 0, 180));

        let te_response = ui.put(title_rect, text_edit);
        if !te_response.has_focus() {
            te_response.request_focus();
        }

        let confirmed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));

        let lost_focus = !te_response.has_focus() && !state.rename_just_opened;
        state.rename_just_opened = false;

        if confirmed || (lost_focus && !cancelled) {
            let new_name = state.rename_buf.trim().to_string();
            if !new_name.is_empty() && new_name != ws.title {
                actions.push(SidebarAction::RenameWorkspace(idx, new_name));
            }
            state.renaming = None;
            state.rename_buf.clear();
        } else if cancelled {
            state.renaming = None;
            state.rename_buf.clear();
        }
    } else {
        let title_pos = rect.min + egui::vec2(content_left, ROW_V_PAD);
        let title_font = egui::FontId::proportional(TITLE_FONT_SIZE);
        // Show agent task as title if available, with star prefix like cmux
        let display_title = if let Some(task) = status.as_ref().and_then(|s| s.task.as_ref()) {
            format!("\u{2731} {task}")
        } else {
            ws.title.clone()
        };
        let truncated_title = truncate_text(ui, &display_title, &title_font, max_title_w);
        ui.painter().text(
            title_pos,
            egui::Align2::LEFT_TOP,
            &truncated_title,
            title_font,
            title_color,
        );
    }

    // --- Close button on hover (replaces badge) or badge/count ---
    let badge_center_y = rect.min.y + ROW_V_PAD + title_line_h / 2.0;

    if hovered && !is_renaming {
        let btn_center = egui::pos2(
            rect.right() - ROW_H_PAD - CLOSE_BTN_SIZE / 2.0,
            badge_center_y,
        );
        let btn_rect =
            egui::Rect::from_center_size(btn_center, egui::vec2(CLOSE_BTN_SIZE, CLOSE_BTN_SIZE));
        let pointer_over_btn = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| btn_rect.contains(p));
        let btn_color = if pointer_over_btn {
            TEXT_ACTIVE
        } else {
            CLOSE_BTN_COLOR
        };
        paint_close_x(ui.painter(), btn_center, 4.0, btn_color);
        if response.clicked() && pointer_over_btn {
            actions.push(SidebarAction::CloseWorkspace(idx));
            return (actions, rect);
        }
    } else if unread > 0 {
        let badge_center = egui::pos2(rect.right() - ROW_H_PAD - BADGE_RADIUS, badge_center_y);
        let badge_color = if is_active {
            BADGE_ACTIVE_BG
        } else {
            ACCENT_BLUE
        };
        ui.painter()
            .circle_filled(badge_center, BADGE_RADIUS, badge_color);
        ui.painter().text(
            badge_center,
            egui::Align2::CENTER_CENTER,
            format!("{unread}"),
            egui::FontId::proportional(BADGE_FONT_SIZE),
            Color32::WHITE,
        );
    } else {
        let count = pane_ids.len();
        ui.painter().text(
            egui::pos2(rect.right() - ROW_H_PAD, rect.min.y + ROW_V_PAD),
            egui::Align2::RIGHT_TOP,
            format!("{count}"),
            egui::FontId::proportional(COUNT_FONT_SIZE),
            TEXT_SECONDARY,
        );
    }

    // --- Status pill ---
    let mut content_bottom = rect.min.y + ROW_V_PAD + title_line_h;
    if let Some(status) = &status {
        let (pill_color, default_text) = match status.state {
            amux_notify::AgentState::Active => (STATUS_GREEN, "active"),
            amux_notify::AgentState::Waiting => (STATUS_ORANGE, "waiting"),
            amux_notify::AgentState::Idle => (STATUS_GRAY, "idle"),
        };
        let label = status.label.as_deref().unwrap_or(default_text);
        content_bottom += 4.0;
        let pill_y = content_bottom;
        let pill_x = rect.min.x + content_left;

        let pill_font = egui::FontId::proportional(PILL_FONT_SIZE);
        let text_galley =
            ui.painter()
                .layout_no_wrap(label.to_string(), pill_font.clone(), Color32::WHITE);
        let text_w = text_galley.size().x;
        let pill_w = (text_w + 10.0).min(avail_w - content_left - ROW_H_PAD);

        let pill_rect =
            egui::Rect::from_min_size(egui::pos2(pill_x, pill_y), egui::vec2(pill_w, PILL_HEIGHT));
        ui.painter()
            .rect_filled(pill_rect, PILL_CORNER_RADIUS, pill_color);
        ui.painter().text(
            pill_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            pill_font,
            Color32::WHITE,
        );
        content_bottom += PILL_HEIGHT;
    }

    // --- Agent message (subtitle) ---
    if let Some(message) = status.as_ref().and_then(|s| s.message.as_ref()) {
        let msg_color = if is_active {
            Color32::from_rgba_premultiplied(204, 204, 204, 204)
        } else {
            TEXT_SECONDARY
        };
        content_bottom += 2.0;
        let msg_x = rect.min.x + content_left;
        let max_w = avail_w - content_left - ROW_H_PAD;
        let msg_font = egui::FontId::proportional(METADATA_FONT_SIZE);
        let galley = ui
            .painter()
            .layout(message.clone(), msg_font, msg_color, max_w);
        let clip_rect = egui::Rect::from_min_size(
            egui::pos2(msg_x, content_bottom),
            egui::vec2(max_w, METADATA_LINE_HEIGHT),
        );
        ui.painter().with_clip_rect(clip_rect).galley(
            egui::pos2(msg_x, content_bottom),
            galley,
            msg_color,
        );
        content_bottom += METADATA_LINE_HEIGHT;
    }

    // --- Git branch + CWD line ---
    if let Some(meta) = metadata {
        if meta.git_branch.is_some() || meta.cwd.is_some() {
            let line_color = if is_active {
                Color32::from_rgba_premultiplied(180, 180, 180, 204)
            } else {
                TEXT_SECONDARY
            };
            content_bottom += 2.0;
            let line_x = rect.min.x + content_left;
            let max_w = avail_w - content_left - ROW_H_PAD;
            let line_font = egui::FontId::monospace(METADATA_FONT_SIZE);

            let mut parts = Vec::new();
            if let Some(branch) = &meta.git_branch {
                let dirty = if meta.git_dirty { "*" } else { "" };
                parts.push(format!("{branch}{dirty}"));
            }
            if let Some(cwd) = &meta.cwd {
                parts.push(shorten_path(cwd));
            }
            let text = parts.join(" \u{2022} "); // bullet separator

            let truncated = truncate_text(ui, &text, &line_font, max_w);
            ui.painter().text(
                egui::pos2(line_x, content_bottom),
                egui::Align2::LEFT_TOP,
                &truncated,
                line_font,
                line_color,
            );
            content_bottom += METADATA_LINE_HEIGHT;
        }

        // --- PR badge ---
        if let Some(pr_num) = meta.pr_number {
            let pr_state = meta.pr_state.as_deref().unwrap_or("open");
            let pr_color = match pr_state {
                "merged" => PR_MERGED_COLOR,
                "open" => PR_OPEN_COLOR,
                _ => PR_CLOSED_COLOR,
            };
            content_bottom += 2.0;
            let pr_x = rect.min.x + content_left;
            let max_w = avail_w - content_left - ROW_H_PAD;
            let pr_font = egui::FontId::proportional(METADATA_FONT_SIZE);
            let pr_text = format!("\u{1F517} PR #{pr_num} {pr_state}");
            let truncated = truncate_text(ui, &pr_text, &pr_font, max_w);
            ui.painter().text(
                egui::pos2(pr_x, content_bottom),
                egui::Align2::LEFT_TOP,
                &truncated,
                pr_font,
                pr_color,
            );
            content_bottom += METADATA_LINE_HEIGHT;
        }
    }

    // --- Notification preview text (only when no agent message) ---
    if let Some(notif) = latest_notif
        .filter(|_| !has_agent_message)
        .filter(|n| !n.body.is_empty())
    {
        let notif_color = if is_active {
            Color32::from_rgba_premultiplied(204, 204, 204, 204)
        } else {
            TEXT_SECONDARY
        };
        content_bottom += 2.0;
        let notif_x = rect.min.x + content_left;
        let max_w = avail_w - content_left - ROW_H_PAD;
        let notif_font = egui::FontId::proportional(NOTIF_FONT_SIZE);

        let galley = ui
            .painter()
            .layout(notif.body.clone(), notif_font, notif_color, max_w);
        let clip_rect = egui::Rect::from_min_size(
            egui::pos2(notif_x, content_bottom),
            egui::vec2(max_w, NOTIF_PREVIEW_HEIGHT),
        );
        ui.painter().with_clip_rect(clip_rect).galley(
            egui::pos2(notif_x, content_bottom),
            galley,
            notif_color,
        );
        content_bottom += NOTIF_PREVIEW_HEIGHT;
    }

    // --- Progress bar ---
    if let Some(progress) = status.as_ref().and_then(|s| s.progress) {
        content_bottom += 4.0;
        let bar_x = rect.min.x + content_left;
        let bar_w = avail_w - content_left - ROW_H_PAD;
        let track_rect = egui::Rect::from_min_size(
            egui::pos2(bar_x, content_bottom),
            egui::vec2(bar_w, PROGRESS_BAR_HEIGHT),
        );
        ui.painter()
            .rect_filled(track_rect, PROGRESS_BAR_HEIGHT / 2.0, PROGRESS_TRACK);
        let fill_w = bar_w * progress.clamp(0.0, 1.0);
        if fill_w > 0.0 {
            let fill_rect = egui::Rect::from_min_size(
                egui::pos2(bar_x, content_bottom),
                egui::vec2(fill_w, PROGRESS_BAR_HEIGHT),
            );
            ui.painter()
                .rect_filled(fill_rect, PROGRESS_BAR_HEIGHT / 2.0, ACCENT_BLUE);
        }
    }

    // --- Click to switch workspace ---
    if response.clicked() && !is_active && !is_renaming {
        actions.push(SidebarAction::SwitchWorkspace(idx));
    }

    (actions, rect)
}

fn with_opacity(color: Color32, opacity: f32) -> Color32 {
    Color32::from_rgba_premultiplied(
        (color.r() as f32 * opacity) as u8,
        (color.g() as f32 * opacity) as u8,
        (color.b() as f32 * opacity) as u8,
        (color.a() as f32 * opacity) as u8,
    )
}

/// Truncate text to fit within `max_width`, appending "\u{2026}" if needed.
fn truncate_text(ui: &egui::Ui, text: &str, font: &egui::FontId, max_width: f32) -> String {
    let full_galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE);
    if full_galley.size().x <= max_width {
        return text.to_string();
    }

    let ellipsis = "\u{2026}";
    let ellipsis_w = ui
        .painter()
        .layout_no_wrap(ellipsis.to_string(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let target_w = max_width - ellipsis_w;

    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let prefix: String = chars[..mid].iter().collect();
        let w = ui
            .painter()
            .layout_no_wrap(prefix, font.clone(), Color32::WHITE)
            .size()
            .x;
        if w <= target_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    let prefix: String = chars[..lo].iter().collect();
    format!("{prefix}{ellipsis}")
}

/// Shorten a path for sidebar display: replace $HOME with ~.
fn shorten_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}
