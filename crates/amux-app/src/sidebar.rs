use amux_notify::NotificationStore;
use egui::Color32;

use crate::{SidebarState, Workspace};

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
const NOTIF_PREVIEW_HEIGHT: f32 = 24.0; // ~2 lines at 10pt
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_SPACER: f32 = 28.0;

// ---------------------------------------------------------------------------
// Actions returned from sidebar rendering
// ---------------------------------------------------------------------------

pub(crate) enum SidebarAction {
    SwitchWorkspace(usize),
    CreateWorkspace,
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
            // Traffic light spacer (macOS) or small top margin (other)
            #[cfg(target_os = "macos")]
            ui.add_space(TRAFFIC_LIGHT_SPACER);
            #[cfg(not(target_os = "macos"))]
            ui.add_space(8.0);

            ui.spacing_mut().item_spacing.y = ROW_SPACING;

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (idx, ws) in workspaces.iter().enumerate() {
                        let is_active = idx == active_workspace_idx;
                        let action =
                            render_workspace_row(ui, ws, idx, is_active, notifications, state);
                        if let Some(a) = action {
                            actions.push(a);
                        }
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

fn render_workspace_row(
    ui: &mut egui::Ui,
    ws: &Workspace,
    idx: usize,
    is_active: bool,
    notifications: &NotificationStore,
    state: &SidebarState,
) -> Option<SidebarAction> {
    let pane_ids: Vec<u64> = ws.tree.iter_panes();
    let unread = notifications.workspace_unread_count(&pane_ids);
    let status = notifications.workspace_status(ws.id);
    let has_status = status.is_some();
    let latest_notif = notifications.latest_for_workspace(ws.id);
    let has_notif_text = latest_notif.is_some_and(|n| !n.body.is_empty());

    // Dynamic row height
    let title_line_h = TITLE_FONT_SIZE + 2.0; // text + small buffer
    let mut row_h = ROW_V_PAD * 2.0 + title_line_h;
    if has_status {
        row_h += PILL_HEIGHT + 4.0;
    }
    if has_notif_text {
        row_h += NOTIF_PREVIEW_HEIGHT + 2.0;
    }

    let avail_w = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(avail_w, row_h), egui::Sense::click());

    if !ui.is_rect_visible(rect) {
        return if response.clicked() && !is_active {
            Some(SidebarAction::SwitchWorkspace(idx))
        } else {
            None
        };
    }

    let hovered = response.hovered();
    let _ = state; // will use in later PRs

    // --- Background ---
    let bg = if is_active {
        ROW_ACTIVE_BG
    } else if hovered {
        ROW_HOVER_BG
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, ROW_CORNER_RADIUS, bg);

    // --- Title ---
    let title_color = if is_active {
        TEXT_ACTIVE
    } else {
        TEXT_INACTIVE
    };
    let title_pos = rect.min + egui::vec2(ROW_H_PAD, ROW_V_PAD);

    // Measure and truncate title with ellipsis
    let badge_reserve = BADGE_RADIUS * 2.0 + 4.0; // space for badge/count on right
    let max_title_w = avail_w - ROW_H_PAD * 2.0 - badge_reserve;
    let title_font = egui::FontId::proportional(TITLE_FONT_SIZE);

    let truncated_title = truncate_text(ui, &ws.title, &title_font, max_title_w);
    ui.painter().text(
        title_pos,
        egui::Align2::LEFT_TOP,
        &truncated_title,
        title_font.clone(),
        title_color,
    );

    // --- Unread badge or pane count (right-aligned) ---
    let badge_center_y = rect.min.y + ROW_V_PAD + title_line_h / 2.0;
    if unread > 0 {
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
    if let Some(status) = status {
        let (pill_color, default_text) = match status.state {
            amux_notify::AgentState::Active => (STATUS_GREEN, "active"),
            amux_notify::AgentState::Waiting => (STATUS_ORANGE, "waiting"),
            amux_notify::AgentState::Idle => (STATUS_GRAY, "idle"),
        };
        let label = status.label.as_deref().unwrap_or(default_text);
        let pill_y = rect.min.y + ROW_V_PAD + title_line_h + 4.0;
        let pill_x = rect.min.x + ROW_H_PAD;

        // Measure pill text width
        let pill_font = egui::FontId::proportional(PILL_FONT_SIZE);
        let text_galley =
            ui.painter()
                .layout_no_wrap(label.to_string(), pill_font.clone(), Color32::WHITE);
        let text_w = text_galley.size().x;
        let pill_w = (text_w + 10.0).min(avail_w - ROW_H_PAD * 2.0);

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
    }

    // --- Notification preview text ---
    if let Some(notif) = latest_notif.filter(|n| !n.body.is_empty()) {
        let notif_color = if is_active {
            Color32::from_rgba_premultiplied(204, 204, 204, 204) // white@0.8
        } else {
            TEXT_SECONDARY
        };
        // Position below pill or title
        let notif_y = if has_status {
            rect.min.y + ROW_V_PAD + title_line_h + 4.0 + PILL_HEIGHT + 2.0
        } else {
            rect.min.y + ROW_V_PAD + title_line_h + 2.0
        };
        let notif_x = rect.min.x + ROW_H_PAD;
        let max_w = avail_w - ROW_H_PAD * 2.0;
        let notif_font = egui::FontId::proportional(NOTIF_FONT_SIZE);

        // Use galley for word-wrap, clip to 2 lines
        let galley = ui
            .painter()
            .layout(notif.body.clone(), notif_font, notif_color, max_w);
        let clip_rect = egui::Rect::from_min_size(
            egui::pos2(notif_x, notif_y),
            egui::vec2(max_w, NOTIF_PREVIEW_HEIGHT),
        );
        ui.painter().with_clip_rect(clip_rect).galley(
            egui::pos2(notif_x, notif_y),
            galley,
            notif_color,
        );
    }

    if response.clicked() && !is_active {
        Some(SidebarAction::SwitchWorkspace(idx))
    } else {
        None
    }
}

/// Truncate text to fit within `max_width`, appending "…" if needed.
fn truncate_text(ui: &egui::Ui, text: &str, font: &egui::FontId, max_width: f32) -> String {
    let full_galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE);
    if full_galley.size().x <= max_width {
        return text.to_string();
    }

    // Binary search for the longest prefix that fits with "…"
    let ellipsis = "…";
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
