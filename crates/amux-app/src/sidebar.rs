use std::collections::HashMap;

use amux_notify::NotificationStore;
use egui::Color32;

use amux_core::model::{SidebarDragState, SidebarState, SurfaceMetadata, Workspace};

// ---------------------------------------------------------------------------
// Colors (cmux dark mode equivalents)
// ---------------------------------------------------------------------------

const ROW_HOVER_BG: Color32 = Color32::from_rgba_premultiplied(15, 15, 15, 15);
const TEXT_ACTIVE: Color32 = Color32::WHITE;
const TEXT_INACTIVE: Color32 = Color32::from_gray(180);
const BADGE_ACTIVE_BG: Color32 = Color32::from_rgba_premultiplied(64, 64, 64, 64);
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
const NOTIF_FONT_SIZE: f32 = 10.0;
const NOTIF_PREVIEW_HEIGHT: f32 = 24.0;
const CLOSE_BTN_SIZE: f32 = 16.0;
const COLOR_CAPSULE_WIDTH: f32 = 3.0;
const PROGRESS_BAR_HEIGHT: f32 = 3.0;
const DROP_INDICATOR_HEIGHT: f32 = 2.0;
const METADATA_FONT_SIZE: f32 = 10.0;
const METADATA_LINE_HEIGHT: f32 = 16.0;
// `TRAFFIC_LIGHT_SPACER` was removed — the sidebar's top padding is
// now computed by the caller as `AmuxApp::top_pad()` and passed in,
// so macOS traffic lights, the Windows/Linux titlebar strip, and
// the optional menubar strip all share one single source of truth.

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
    StartRenameWorkspace(usize),
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

/// Render the sidebar panel.
///
/// `top_pad` is the total top chrome height in logical pixels —
/// the amount of vertical space the sidebar's first row must skip
/// past to avoid rendering underneath the titlebar strip (which is
/// drawn in a background layer across the full screen width and
/// contains the sidebar-toggle / bell / + icons in a foreground
/// layer). On macOS this also covers the traffic-light buttons.
/// Passed in from `frame_update.rs` rather than read from a
/// constant because the value depends on `menu_bar_style` — Menubar
/// mode adds a menu strip above the icon strip.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_sidebar(
    ctx: &egui::Context,
    state: &mut SidebarState,
    workspaces: &[Workspace],
    active_workspace_idx: usize,
    notifications: &NotificationStore,
    workspace_metadata: &HashMap<u64, SurfaceMetadata>,
    theme: &crate::theme::Theme,
    top_pad: f32,
) -> Vec<SidebarAction> {
    let mut actions = Vec::new();

    // Hide the sidebar resize indicator (including hover/active states) by
    // temporarily matching stroke colors to the sidebar background. egui draws
    // the resize line using hovered.fg_stroke and active.fg_stroke even when
    // show_separator_line(false) is set. Restored after the panel renders.
    let saved_styles = ctx.style().visuals.widgets.clone();
    let hide_color = theme.chrome.sidebar_bg;
    ctx.style_mut(|style| {
        style.visuals.widgets.noninteractive.bg_stroke.color = hide_color;
        style.visuals.widgets.hovered.fg_stroke.color = hide_color;
        style.visuals.widgets.active.fg_stroke.color = hide_color;
    });

    egui::SidePanel::left("sidebar")
        .resizable(true)
        .show_separator_line(false)
        .default_width(state.width)
        .min_width(SIDEBAR_MIN_WIDTH)
        .max_width(SIDEBAR_MAX_WIDTH)
        .frame(
            egui::Frame::new()
                .fill(theme.chrome.sidebar_bg)
                .inner_margin(egui::Margin::symmetric(ROW_OUTER_H_PAD as i8, 0)),
        )
        .show(ctx, |ui| {
            // Persist the actual panel width back to state for session save/restore
            state.width = ui.available_width() + ROW_OUTER_H_PAD * 2.0;

            // Shift content down past the full top chrome (titlebar
            // strip + optional menu strip in Menubar mode). Using the
            // same `top_pad` value `frame_update.rs` uses for the
            // CentralPanel offset keeps the sidebar's first row and
            // the central panel's first content aligned on a single
            // horizontal baseline.
            //
            // Note that the egui SidePanel itself starts at the top
            // of the screen (y=0) regardless — it doesn't know about
            // the titlebar strip. The padding we add here is inside
            // the panel, so the workspace rows land below the chrome
            // and don't get painted over by the foreground titlebar
            // icon Area.
            ui.add_space(top_pad);

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
                            theme,
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
                        ui.painter()
                            .rect_filled(indicator_rect, 1.0, theme.chrome.accent);
                    }

                    // Fill remaining space (double-click to create workspace)
                    let avail_w = ui.available_width();
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

    // Restore widget styles so UI rendered after the sidebar uses normal styling.
    ctx.style_mut(|style| {
        style.visuals.widgets = saved_styles;
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
    theme: &crate::theme::Theme,
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
    let has_color = ws.color.is_some();
    let has_git_or_cwd = metadata.is_some_and(|m| m.git_branch.is_some() || m.cwd.is_some());
    let has_pr = metadata.is_some_and(|m| m.pr_number.is_some());

    // Compute title text early so we can measure if it needs two lines.
    let title_font = crate::fonts::bold_font(TITLE_FONT_SIZE);
    let display_title = if let Some(task) = status.as_ref().and_then(|s| s.task.as_ref()) {
        format!("\u{2731} {task}")
    } else if let Some(st) = metadata.and_then(|m| m.surface_title.as_ref()) {
        st.clone()
    } else {
        ws.title.clone()
    };
    let content_left_est = if has_color {
        ROW_H_PAD + COLOR_CAPSULE_WIDTH + 4.0
    } else {
        ROW_H_PAD
    };
    let right_reserve_est = BADGE_RADIUS * 2.0 + 4.0;
    let max_title_w_est = ui.available_width() - content_left_est - ROW_H_PAD - right_reserve_est;
    let title_text_w = ui
        .fonts(|f| f.layout_no_wrap(display_title.clone(), title_font.clone(), Color32::WHITE))
        .size()
        .x;
    let title_needs_wrap = title_text_w > max_title_w_est;

    // Dynamic row height
    let title_line_h = TITLE_FONT_SIZE + 2.0;
    let title_lines = if title_needs_wrap { 2.0 } else { 1.0 };
    let mut row_h = ROW_V_PAD * 2.0 + title_line_h * title_lines;
    if has_agent_message {
        row_h += METADATA_LINE_HEIGHT + 2.0;
    }
    if has_status {
        row_h += METADATA_LINE_HEIGHT + 4.0;
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
    if response.drag_started() && state.drag.is_none() {
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
    // Apply amux's popup palette to the parent `ui` BEFORE calling
    // `response.context_menu`. Egui's context menu builds its Frame
    // from the parent's style at construction time, so any palette
    // applied inside the closure would be too late to affect the
    // popup's background / border. Applying to the parent also
    // propagates into the popup's child UI via inheritance so
    // buttons and separators inside the menu are themed too.
    //
    // We also re-apply inside the nested `Set Color` submenu
    // (which builds its own fresh Area + Frame from its local ui
    // style) so that nested popup is themed as well.
    let palette = crate::popup_theme::MenuPalette::from_theme(theme);
    crate::popup_theme::apply_menu_palette(ui, palette);
    response.context_menu(|ui| {
        if ui.button("Rename Workspace").clicked() {
            actions.push(SidebarAction::StartRenameWorkspace(idx));
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
        // Apply palette on the context-menu ui before opening the
        // nested submenu so the nested menu's Frame also inherits
        // amux colors.
        crate::popup_theme::apply_menu_palette(ui, palette);
        ui.menu_button("Set Color", |ui| {
            crate::popup_theme::apply_menu_palette(ui, palette);
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
        with_opacity(theme.chrome.sidebar_active_bg, opacity)
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
    let right_reserve = if hovered {
        close_btn_reserve
    } else {
        badge_reserve
    };
    let max_title_w = avail_w - content_left - ROW_H_PAD - right_reserve;

    {
        let title_pos = rect.min + egui::vec2(content_left, ROW_V_PAD);
        if title_needs_wrap {
            // Wrap to two lines with ellipsis on overflow.
            let mut job = egui::text::LayoutJob::single_section(
                display_title.clone(),
                egui::TextFormat {
                    font_id: title_font.clone(),
                    color: title_color,
                    ..Default::default()
                },
            );
            job.wrap = egui::text::TextWrapping {
                max_width: max_title_w,
                max_rows: 2,
                break_anywhere: false,
                overflow_character: Some('\u{2026}'),
            };
            let galley = ui.fonts(|f| f.layout_job(job));
            ui.painter().galley(title_pos, galley, title_color);
        } else {
            ui.painter().text(
                title_pos,
                egui::Align2::LEFT_TOP,
                &display_title,
                title_font.clone(),
                title_color,
            );
        }
    }

    // --- Close button on hover (replaces badge) or badge/count ---
    let badge_center_y = rect.min.y + ROW_V_PAD + title_line_h / 2.0;

    if hovered {
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
            theme.chrome.accent
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
    }

    // --- Status indicator (icon + text, matching cmux) ---
    let mut content_bottom = rect.min.y + ROW_V_PAD + title_line_h * title_lines;

    // Metadata text color: light grey
    let meta_color = Color32::from_gray(190);
    // Status indicator color: blue when unselected, white when selected
    let status_color = if is_active {
        Color32::WHITE
    } else {
        theme.chrome.accent
    };

    if let Some(status) = &status {
        let (icon, default_text) = match status.state {
            amux_notify::AgentState::Active => ("\u{26A1}", "Running"), // ⚡
            amux_notify::AgentState::Waiting => ("\u{1F514}", "Needs input"), // 🔔
            amux_notify::AgentState::Idle => ("\u{23F8}\u{FE0E}", "Idle"), // ⏸︎
        };
        let label = status.label.as_deref().unwrap_or(default_text);
        content_bottom += 4.0;
        let status_x = rect.min.x + content_left;
        let max_w = avail_w - content_left - ROW_H_PAD;
        let status_font = egui::FontId::proportional(METADATA_FONT_SIZE);
        let status_text = format!("{icon} {label}");
        let truncated = truncate_text(ui, &status_text, &status_font, max_w);
        ui.painter().text(
            egui::pos2(status_x, content_bottom),
            egui::Align2::LEFT_TOP,
            &truncated,
            status_font,
            status_color,
        );
        content_bottom += METADATA_LINE_HEIGHT;
    }

    // --- Agent message (subtitle) ---
    if let Some(message) = status.as_ref().and_then(|s| s.message.as_ref()) {
        let msg_color = meta_color;
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
            let line_color = meta_color;
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
            let pr_color = meta_color;
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
        let notif_color = meta_color;
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
                .rect_filled(fill_rect, PROGRESS_BAR_HEIGHT / 2.0, theme.chrome.accent);
        }
    }

    // --- Click to switch workspace or mark read ---
    if response.clicked() {
        if is_active {
            actions.push(SidebarAction::MarkWorkspaceRead(idx));
        } else {
            actions.push(SidebarAction::SwitchWorkspace(idx));
        }
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
        let p = std::path::Path::new(path);
        if let Ok(rest) = p.strip_prefix(&home) {
            let rest_str = rest.to_string_lossy();
            return if rest_str.is_empty() {
                "~".to_string()
            } else {
                format!("~/{rest_str}")
            };
        }
    }
    path.to_string()
}
