use std::collections::HashMap;

use amux_notify::NotificationStore;
use egui::Color32;

use amux_core::model::{SidebarDragState, SidebarState, SurfaceMetadata, Workspace};

// ---------------------------------------------------------------------------
// Colors (cmux dark mode equivalents)
// ---------------------------------------------------------------------------

const TEXT_ACTIVE: Color32 = Color32::WHITE;
const TEXT_INACTIVE: Color32 = Color32::from_gray(180);
/// Title color for unread-but-not-active rows. Brighter than
/// `TEXT_INACTIVE` so an unread-but-idle row reads as "wants
/// attention" without stealing the selected row's full-white
/// treatment.
const TEXT_UNREAD: Color32 = Color32::from_gray(230);
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
/// Pill height. The badge is a capsule (rounded rect with corner
/// radius = height / 2) so multi-digit counts expand horizontally
/// without cramping, while single-digit counts still read as a
/// circle (minimum-width pill = height, which paints as a circle).
const BADGE_HEIGHT: f32 = 16.0;
/// Horizontal padding inside the pill, per side.
const BADGE_H_PAD: f32 = 5.0;
const BADGE_FONT_SIZE: f32 = 9.0;
/// Max count rendered inline. Anything higher shows as `99+`.
const BADGE_MAX_COUNT: usize = 99;
const NOTIF_FONT_SIZE: f32 = 10.0;
const NOTIF_PREVIEW_HEIGHT: f32 = 24.0;
const CLOSE_BTN_SIZE: f32 = 16.0;
const COLOR_CAPSULE_WIDTH: f32 = 3.0;
const PROGRESS_BAR_HEIGHT: f32 = 3.0;
const DROP_INDICATOR_HEIGHT: f32 = 2.0;
const METADATA_FONT_SIZE: f32 = 10.0;
const METADATA_LINE_HEIGHT: f32 = 16.0;
/// G13: cap on the number of PR rows rendered beneath a workspace.
/// More than this would start to dominate the sidebar for a workspace
/// that publishes a PR per tiny feature branch; we fall back to
/// "#N, #M, ... +K more" on the final visible row once this cap is hit.
const MAX_PR_ROWS: usize = 3;
/// G6: row-height animation duration. When a status entry appears or
/// expires, the row interpolates toward the new target height over
/// this window instead of popping instantly. egui's animation manager
/// eases internally and snaps once the delta is below its epsilon,
/// so the row stops requesting repaints at steady state.
const ROW_HEIGHT_ANIM_SECS: f32 = 0.2;
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
    TogglePinWorkspace(usize),
}

// ---------------------------------------------------------------------------
// Shared close-X painter
// ---------------------------------------------------------------------------

/// Paint an X icon (two diagonal lines) centered at `center` with the given `size` and `color`.
/// Stroke width scales with size so larger glyphs don't look hairline.
pub(crate) fn paint_close_x(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    color: Color32,
) {
    let half = size / 2.0;
    let stroke = egui::Stroke::new((size * 0.18).clamp(1.0, 2.0), color);
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

/// Build the priority-sorted list of keyed status rows to render for a
/// workspace. See the call site in `render_workspace_row` for the full
/// rationale (G20). Returned tuples are `(text, priority)` borrowing
/// from `status`; priority drives per-row color selection.
fn build_status_rows(status: &amux_notify::WorkspaceStatus) -> Vec<(&str, i32)> {
    // Only dedup against the legacy subtitle slots written by the
    // dual-write pattern (`agent.task` / `agent.message`). `agent.label`
    // is intentionally excluded: its text (e.g. "Running", "Needs input")
    // can legitimately collide with a user-published entry and must not
    // cause the user entry to vanish.
    let agent_texts: std::collections::HashSet<&str> =
        [amux_notify::KEY_AGENT_TASK, amux_notify::KEY_AGENT_MESSAGE]
            .iter()
            .filter_map(|k| status.displayed.get(*k).map(|e| e.text.as_str()))
            .collect();
    status
        .displayed_by_priority()
        .into_iter()
        .filter(|(k, _)| *k != amux_notify::KEY_AGENT_LABEL)
        .filter(|(_, e)| !e.text.is_empty())
        .filter(|(k, e)| {
            k.starts_with(amux_notify::AGENT_KEY_PREFIX) || !agent_texts.contains(e.text.as_str())
        })
        .map(|(_, e)| (e.text.as_str(), e.priority))
        .collect()
}

/// Format an unread count for the badge, capping at `BADGE_MAX_COUNT`.
fn badge_label(unread: usize) -> String {
    if unread > BADGE_MAX_COUNT {
        format!("{BADGE_MAX_COUNT}+")
    } else {
        format!("{unread}")
    }
}

/// Width the badge pill will occupy for a given unread count. Floors
/// at `BADGE_HEIGHT` so single-digit counts still render as a circle
/// (the minimum-width capsule is a circle).
fn badge_pill_width(ui: &egui::Ui, unread: usize) -> f32 {
    let label = badge_label(unread);
    let font = egui::FontId::proportional(BADGE_FONT_SIZE);
    let text_w = ui
        .fonts(|f| f.layout_no_wrap(label, font, Color32::WHITE))
        .size()
        .x;
    (text_w + BADGE_H_PAD * 2.0).max(BADGE_HEIGHT)
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

/// Four-way combination of active/hover states for a workspace row.
///
/// Prior to G19 we used nested `if is_active / else if hovered` which
/// meant hovering the active row produced no visual response (hover
/// was swallowed by the active branch). Keeping the combinations as
/// a single enum forces the render code to handle all four, and lets
/// the theme expose separate tokens for `Active` vs `ActiveHovered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowVisuals {
    Idle,
    Hovered,
    Active,
    ActiveHovered,
}

impl RowVisuals {
    fn resolve(is_active: bool, hovered: bool) -> Self {
        match (is_active, hovered) {
            (true, true) => RowVisuals::ActiveHovered,
            (true, false) => RowVisuals::Active,
            (false, true) => RowVisuals::Hovered,
            (false, false) => RowVisuals::Idle,
        }
    }

    fn is_active(self) -> bool {
        matches!(self, RowVisuals::Active | RowVisuals::ActiveHovered)
    }

    fn bg(self, chrome: &crate::theme::ChromeColors) -> Color32 {
        match self {
            RowVisuals::Idle => Color32::TRANSPARENT,
            RowVisuals::Hovered => chrome.sidebar_hover_bg,
            RowVisuals::Active => chrome.sidebar_active_bg,
            RowVisuals::ActiveHovered => chrome.sidebar_active_hover_bg,
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
    // Measured once per row and reused by the title-width-reserve pass
    // and the badge render pass, so we don't re-layout the pill label
    // twice per frame per workspace.
    let badge_pill_w = if unread > 0 {
        badge_pill_width(ui, unread)
    } else {
        0.0
    };
    let status = notifications.workspace_status(ws.id);
    let has_status = status.is_some();
    let has_progress = status.as_ref().and_then(|s| s.progress).is_some();
    let progress_label = status
        .as_ref()
        .filter(|s| s.progress.is_some())
        .and_then(|s| s.progress_label.as_deref())
        .filter(|l| !l.is_empty());
    let has_progress_label = progress_label.is_some();
    // G20: Build the priority-sorted list of keyed status entries that
    // render as per-workspace rows. Iterates the debounced `displayed`
    // snapshot so bursts of rapid hook writes don't flash intermediate
    // values. `agent.label` is rendered separately below with its
    // AgentState icon, so it's excluded here. Higher-priority entries
    // (task > notification > subagent > tool/message) sort first so the
    // row closest to the title is the most important.
    //
    // Dedup for the legacy dual-write: during the single-slot → keyed
    // transition, claude_hook writes both `agent.message = "X"` and
    // `claude.tool = "X"`. Rendering both would duplicate. We keep the
    // agent.* copy (it's the canonical legacy slot) and drop any non-
    // agent.* entry whose text matches any agent.* entry's text.
    let status_rows = status.map(build_status_rows).unwrap_or_default();
    let has_status_rows = !status_rows.is_empty();
    let latest_notif = notifications.latest_for_workspace(ws.id);
    let has_notif_text = !has_status_rows && latest_notif.is_some_and(|n| !n.body.is_empty());
    let has_color = ws.color.is_some();
    let has_git_or_cwd = metadata.is_some_and(|m| m.git_branch.is_some() || m.cwd.is_some());
    let pr_row_count = metadata.map(|m| m.prs.len().min(MAX_PR_ROWS)).unwrap_or(0);

    // Compute title text early so we can measure if it needs two lines.
    let title_font = crate::fonts::bold_font(TITLE_FONT_SIZE);

    // Title priority: user-set name (sticky) > surface title > default
    // workspace name. A user who explicitly renamed the workspace via
    // the rename modal should not have their choice overwritten by
    // auto-detected titles. The agent task previously lived here as a
    // `✱ task` prefix (G18); it now has its own row below the status
    // indicator so long task strings don't crowd the title and the
    // title reflects *workspace identity* rather than *agent activity*.
    let base_title = if let Some(ref ut) = ws.user_title {
        ut.clone()
    } else if let Some(st) = metadata.and_then(|m| m.surface_title.as_ref()) {
        st.clone()
    } else {
        ws.title.clone()
    };
    // Prepend a pin glyph for pinned workspaces. Included in the
    // measured `display_title` so wrap / ellipsis calculations below
    // reserve room for it.
    let display_title = if ws.pinned {
        format!("\u{1F4CC} {base_title}")
    } else {
        base_title
    };
    let content_left_est = if has_color {
        ROW_H_PAD + COLOR_CAPSULE_WIDTH + 4.0
    } else {
        ROW_H_PAD
    };
    let right_reserve_est = if unread > 0 {
        badge_pill_w + 4.0
    } else {
        BADGE_HEIGHT + 4.0
    };
    let max_title_w_est = ui.available_width() - content_left_est - ROW_H_PAD - right_reserve_est;
    let title_text_w = ui
        .fonts(|f| f.layout_no_wrap(display_title.clone(), title_font.clone(), Color32::WHITE))
        .size()
        .x;
    let title_needs_wrap = title_text_w > max_title_w_est;

    // Dynamic row height
    let title_line_h = TITLE_FONT_SIZE + 2.0;
    let title_lines = if title_needs_wrap { 2.0 } else { 1.0 };
    let mut row_h_live = ROW_V_PAD * 2.0 + title_line_h * title_lines;
    if has_status {
        row_h_live += METADATA_LINE_HEIGHT + 4.0;
    }
    row_h_live += status_rows.len() as f32 * (METADATA_LINE_HEIGHT + 2.0);
    if has_git_or_cwd {
        row_h_live += METADATA_LINE_HEIGHT + 2.0;
    }
    row_h_live += pr_row_count as f32 * (METADATA_LINE_HEIGHT + 2.0);
    if has_notif_text {
        row_h_live += NOTIF_PREVIEW_HEIGHT + 2.0;
    }
    if has_progress {
        row_h_live += PROGRESS_BAR_HEIGHT + 4.0;
    }
    if has_progress_label {
        row_h_live += METADATA_LINE_HEIGHT + 2.0;
    }

    // G4: if this row is mid-interaction (drag in progress or context
    // menu open, set via the freeze-update block below on a prior frame),
    // the target height is the frozen value rather than the live one so
    // the row can't shift under the pointer when a status entry arrives
    // or expires mid-interaction. Geometry-only freeze: text content
    // still reflects live state, just within a pinned rect.
    let row_h_target = state
        .frozen_row_heights
        .get(&ws.id)
        .copied()
        .unwrap_or(row_h_live);

    // G6: animate toward the target. When a status row appears or
    // expires the height lerps over `ROW_HEIGHT_ANIM_SECS` instead of
    // popping. The animation manager snaps once the delta is below its
    // epsilon, so steady-state rows stop requesting repaints. Interaction
    // freeze still wins: `row_h_target` is already pinned, so the animation
    // is a no-op unless the frozen height itself changed or interaction ended.
    let anim_id = egui::Id::new(("sidebar_row_h", ws.id));
    let row_h = ui
        .ctx()
        .animate_value_with_time(anim_id, row_h_target, ROW_HEIGHT_ANIM_SECS);

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

    // G6: clip row painting to `rect` so content doesn't spill into
    // neighboring rows while the height is mid-animation. Individual
    // block painters position themselves relative to the live content
    // layout, which exceeds the animated rect when the row is shrinking
    // (and the bottom padding is inside the animating rect when
    // growing). The clip keeps the row visually self-contained. Used
    // for every `row_painter` call below; the context-menu popup uses
    // its own closure-scoped `ui`, so it stays unclipped.
    let row_painter = ui.painter_at(rect);

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
    // `response.context_menu` builds its outer popup `Frame` from
    // `ctx.style()` (see egui's `menu_ui` → `Frame::menu`). Scope
    // the ctx-style mutation to just this call via
    // `with_menu_palette` so unrelated widgets painted later in the
    // frame don't inherit the menu-specific visuals. The
    // `.context_menu(...)` call is synchronous — the Frame has
    // already been built by the time `with_menu_palette`'s closure
    // returns, so the restore is safe.
    //
    // For the nested `Set Color` `ui.menu_button`, we apply the
    // palette to the *menu_ui* inside the closure because
    // `menu_button`'s popup reads the parent ui's style (not
    // `ctx.style()`).
    let palette = crate::popup_theme::MenuPalette::from_theme(theme);
    crate::popup_theme::with_menu_palette(ui.ctx(), palette, || {
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
            let pin_label = if ws.pinned {
                "Unpin Workspace"
            } else {
                "Pin Workspace"
            };
            if ui.button(pin_label).clicked() {
                actions.push(SidebarAction::TogglePinWorkspace(idx));
                ui.close_menu();
            }
            if ui.button("Mark All Read").clicked() {
                actions.push(SidebarAction::MarkWorkspaceRead(idx));
                ui.close_menu();
            }
            ui.separator();
            crate::popup_theme::apply_menu_palette(ui, palette);
            ui.menu_button("Set Color", |ui| {
                crate::popup_theme::apply_menu_palette(ui, palette);
                for &(color, name) in PRESET_COLORS {
                    let c =
                        Color32::from_rgba_premultiplied(color[0], color[1], color[2], color[3]);
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
    });

    // --- Background ---
    let visuals = RowVisuals::resolve(is_active, hovered);
    let opacity = if is_being_dragged { 0.6 } else { 1.0 };
    let bg = with_opacity(visuals.bg(&theme.chrome), opacity);
    row_painter.rect_filled(rect, ROW_CORNER_RADIUS, bg);

    // --- Workspace color capsule (leading edge) ---
    let content_left = if has_color {
        if let Some(c) = ws.color {
            let capsule_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + 2.0, rect.min.y + ROW_V_PAD),
                egui::vec2(COLOR_CAPSULE_WIDTH, row_h - ROW_V_PAD * 2.0),
            );
            let color = Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3]);
            row_painter.rect_filled(capsule_rect, COLOR_CAPSULE_WIDTH / 2.0, color);
        }
        ROW_H_PAD + COLOR_CAPSULE_WIDTH + 4.0
    } else {
        ROW_H_PAD
    };

    // --- Title (or rename TextEdit) ---
    // G19: active rows are full-white; non-active rows with unread
    // notifications get a brighter-than-idle grey so they read as
    // "wants attention" without matching the selected row.
    let title_color = if visuals.is_active() {
        TEXT_ACTIVE
    } else if unread > 0 {
        TEXT_UNREAD
    } else {
        TEXT_INACTIVE
    };

    let close_btn_reserve = CLOSE_BTN_SIZE + 4.0;
    let badge_reserve = if unread > 0 {
        badge_pill_w + 4.0
    } else {
        BADGE_HEIGHT + 4.0
    };
    // Take the max of both states so wrap/ellipsis width is stable
    // across hover — otherwise a wide unread pill can cause the title
    // to reflow (different line break, visible jitter) the instant
    // the user hovers the row.
    let right_reserve = badge_reserve.max(close_btn_reserve);
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
            row_painter.galley(title_pos, galley, title_color);
        } else {
            row_painter.text(
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
        paint_close_x(&row_painter, btn_center, 6.0, btn_color);
        if response.clicked() && pointer_over_btn {
            actions.push(SidebarAction::CloseWorkspace(idx));
            return (actions, rect);
        }
    } else if unread > 0 {
        let label = badge_label(unread);
        let pill_rect = egui::Rect::from_min_size(
            egui::pos2(
                rect.right() - ROW_H_PAD - badge_pill_w,
                badge_center_y - BADGE_HEIGHT / 2.0,
            ),
            egui::vec2(badge_pill_w, BADGE_HEIGHT),
        );
        let badge_color = if is_active {
            BADGE_ACTIVE_BG
        } else {
            theme.chrome.accent
        };
        // Capsule: corner radius = height / 2 makes the short sides
        // full semicircles. For the minimum-width case (single digit)
        // this degenerates to a circle.
        row_painter.rect_filled(pill_rect, BADGE_HEIGHT / 2.0, badge_color);
        row_painter.text(
            pill_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
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
            amux_notify::AgentState::Idle => ("\u{23F8}", "Idle"), // ⏸ (no variation selector — FE0E renders as box on Windows)
        };
        let label = status.displayed_label().unwrap_or(default_text);
        content_bottom += 4.0;
        let status_x = rect.min.x + content_left;
        let max_w = avail_w - content_left - ROW_H_PAD;
        let status_font = egui::FontId::proportional(METADATA_FONT_SIZE);
        let status_text = format!("{icon} {label}");
        let truncated = truncate_text(ui, &status_text, &status_font, max_w);
        row_painter.text(
            egui::pos2(status_x, content_bottom),
            egui::Align2::LEFT_TOP,
            &truncated,
            status_font,
            status_color,
        );
        content_bottom += METADATA_LINE_HEIGHT;
    }

    // --- Keyed status entry rows (G18 + G20) ---
    //
    // Renders the priority-sorted `status_rows` built earlier. agent.task
    // previously lived in the title as `✱ task` (G18); it now flows
    // through this generic loop alongside any other keyed entries
    // (claude.tool, claude.notification, user.*, …). Higher-priority
    // entries render closer to the title per G20's task > notification
    // > tool ordering. Task-tier entries (priority >= TASK) use
    // `status_color` so they're visually tied to the agent indicator
    // above; lower-priority entries use `meta_color` for a subtitle
    // treatment.
    for (text, priority) in &status_rows {
        content_bottom += 2.0;
        let row_x = rect.min.x + content_left;
        let max_w = avail_w - content_left - ROW_H_PAD;
        let font = egui::FontId::proportional(METADATA_FONT_SIZE);
        let color = if *priority >= amux_notify::priority::TASK {
            status_color
        } else {
            meta_color
        };
        let truncated = truncate_text(ui, text, &font, max_w);
        row_painter.text(
            egui::pos2(row_x, content_bottom),
            egui::Align2::LEFT_TOP,
            &truncated,
            font,
            color,
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
            row_painter.text(
                egui::pos2(line_x, content_bottom),
                egui::Align2::LEFT_TOP,
                &truncated,
                line_font,
                line_color,
            );
            content_bottom += METADATA_LINE_HEIGHT;
        }

        // --- PR rows ---
        // G13: one row per attached PR, capped at `MAX_PR_ROWS`. Surplus
        // PRs are collapsed into a `+N more` suffix on the final visible
        // row rather than silently dropped, so a workspace with many
        // dependent branches doesn't look like only a few are tracked.
        let total_prs = meta.prs.len();
        let visible_prs = total_prs.min(MAX_PR_ROWS);
        let overflow = total_prs.saturating_sub(visible_prs);
        for (i, pr) in meta.prs.iter().take(visible_prs).enumerate() {
            let pr_state = pr.state.as_deref().unwrap_or("open");
            let pr_color = meta_color;
            content_bottom += 2.0;
            let pr_x = rect.min.x + content_left;
            let max_w = avail_w - content_left - ROW_H_PAD;
            let pr_font = egui::FontId::proportional(METADATA_FONT_SIZE);
            let base = format!("\u{1F517} PR #{} {}", pr.number, pr_state);
            let pr_text = if i + 1 == visible_prs && overflow > 0 {
                format!("{base}  +{overflow} more")
            } else if let Some(title) = pr.title.as_deref().filter(|t| !t.is_empty()) {
                format!("{base}  {title}")
            } else {
                base
            };
            let truncated = truncate_text(ui, &pr_text, &pr_font, max_w);
            row_painter.text(
                egui::pos2(pr_x, content_bottom),
                egui::Align2::LEFT_TOP,
                &truncated,
                pr_font,
                pr_color,
            );
            content_bottom += METADATA_LINE_HEIGHT;
        }
    }

    // --- Notification preview text (only when no keyed status rows) ---
    if let Some(notif) = latest_notif
        .filter(|_| !has_status_rows)
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
        row_painter.with_clip_rect(clip_rect).galley(
            egui::pos2(notif_x, content_bottom),
            galley,
            notif_color,
        );
        content_bottom += NOTIF_PREVIEW_HEIGHT;
    }

    // --- Progress bar (optional label row + thin bar) ---
    if let Some(progress) = status.as_ref().and_then(|s| s.progress) {
        // Label sits above the bar so the bar stays aligned with the
        // same baseline whether or not there's a label; keeps the row
        // visual rhythm consistent across workspaces.
        if let Some(label) = progress_label {
            content_bottom += 2.0;
            let label_x = rect.min.x + content_left;
            let max_w = avail_w - content_left - ROW_H_PAD;
            let label_font = egui::FontId::proportional(METADATA_FONT_SIZE);
            let truncated = truncate_text(ui, label, &label_font, max_w);
            row_painter.text(
                egui::pos2(label_x, content_bottom),
                egui::Align2::LEFT_TOP,
                &truncated,
                label_font,
                meta_color,
            );
            content_bottom += METADATA_LINE_HEIGHT;
        }
        content_bottom += 4.0;
        let bar_x = rect.min.x + content_left;
        let bar_w = avail_w - content_left - ROW_H_PAD;
        let track_rect = egui::Rect::from_min_size(
            egui::pos2(bar_x, content_bottom),
            egui::vec2(bar_w, PROGRESS_BAR_HEIGHT),
        );
        row_painter.rect_filled(track_rect, PROGRESS_BAR_HEIGHT / 2.0, PROGRESS_TRACK);
        let fill_w = bar_w * progress.clamp(0.0, 1.0);
        if fill_w > 0.0 {
            let fill_rect = egui::Rect::from_min_size(
                egui::pos2(bar_x, content_bottom),
                egui::vec2(fill_w, PROGRESS_BAR_HEIGHT),
            );
            row_painter.rect_filled(fill_rect, PROGRESS_BAR_HEIGHT / 2.0, theme.chrome.accent);
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

    // G4: update the geometry freeze. While the context menu is open or
    // this row is being dragged, pin the row height to the value captured
    // on the first frame of interaction. Clear on the first frame the
    // interaction ends. The freeze takes effect on the frame *after*
    // interaction starts, since we need `response` to detect it — this
    // is fine in practice because any status change mid-interaction
    // arrives on a later frame.
    //
    // We capture the *animated* `row_h` (what's on screen this frame)
    // rather than `row_h_live` (the target). If interaction starts while
    // a G6 height animation is in-flight, this pins the freeze to the
    // currently-displayed height so the row can't continue growing or
    // shrinking under the pointer.
    let interaction_active =
        response.context_menu_opened() || state.drag.as_ref().is_some_and(|d| d.source_idx == idx);
    if interaction_active {
        state.frozen_row_heights.entry(ws.id).or_insert(row_h);
    } else {
        state.frozen_row_heights.remove(&ws.id);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_visuals_resolves_all_four_combinations() {
        use RowVisuals::*;
        assert_eq!(RowVisuals::resolve(false, false), Idle);
        assert_eq!(RowVisuals::resolve(false, true), Hovered);
        assert_eq!(RowVisuals::resolve(true, false), Active);
        assert_eq!(RowVisuals::resolve(true, true), ActiveHovered);
    }

    #[test]
    fn row_visuals_active_hover_distinct_from_active() {
        // Regression for G19: hovering the selected row must produce a
        // different background than the selected-but-not-hovered row,
        // so hover stays visible over active.
        let chrome = crate::theme::Theme::default().chrome;
        assert_ne!(
            RowVisuals::Active.bg(&chrome),
            RowVisuals::ActiveHovered.bg(&chrome),
        );
        assert!(RowVisuals::Active.is_active());
        assert!(RowVisuals::ActiveHovered.is_active());
        assert!(!RowVisuals::Hovered.is_active());
        assert!(!RowVisuals::Idle.is_active());
    }

    #[test]
    fn badge_label_passes_through_small_counts() {
        assert_eq!(badge_label(1), "1");
        assert_eq!(badge_label(42), "42");
        assert_eq!(badge_label(BADGE_MAX_COUNT), "99");
    }

    #[test]
    fn badge_label_caps_over_max() {
        assert_eq!(badge_label(100), "99+");
        assert_eq!(badge_label(12_345), "99+");
    }

    // Helper: populate the `displayed` snapshot via a real commit pass,
    // matching what the per-frame sidebar render sees.
    fn commit(store: &mut amux_notify::NotificationStore) {
        store.commit_displayed_at(
            std::time::Instant::now() + std::time::Duration::from_secs(1),
            amux_notify::NotificationStore::DEBOUNCE_WINDOW,
        );
    }

    #[test]
    fn build_status_rows_sorts_by_priority_descending() {
        let mut store = amux_notify::NotificationStore::new();
        store.set_status(
            1,
            amux_notify::AgentState::Active,
            Some("Running".into()),
            Some("Refactor foo".into()),
            None,
        );
        store.upsert_entry(
            1,
            "claude.notification",
            "Needs approval",
            amux_notify::priority::MESSAGE + 10, // 70
            None,
            None,
            None,
        );
        commit(&mut store);
        let status = store.workspace_status(1).unwrap();
        let rows = build_status_rows(status);
        // agent.label (priority 100) excluded — it renders in the status
        // row with its state icon. task (80) ranks above notification (70).
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "Refactor foo");
        assert_eq!(rows[0].1, amux_notify::priority::TASK);
        assert_eq!(rows[1].0, "Needs approval");
    }

    #[test]
    fn build_status_rows_dedups_non_agent_entries_matching_agent_text() {
        let mut store = amux_notify::NotificationStore::new();
        // Legacy dual-write pattern from claude_hook: agent.message and
        // claude.tool carry identical text during PreToolUse.
        store.set_status(
            1,
            amux_notify::AgentState::Active,
            Some("Running".into()),
            None,
            Some("Reading file.rs".into()),
        );
        store.upsert_entry(
            1,
            "claude.tool",
            "Reading file.rs",
            amux_notify::priority::MESSAGE,
            None,
            None,
            None,
        );
        commit(&mut store);
        let status = store.workspace_status(1).unwrap();
        let rows = build_status_rows(status);
        // Dedup keeps the agent.* copy and drops claude.tool.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "Reading file.rs");
    }

    #[test]
    fn build_status_rows_does_not_dedup_against_agent_label_text() {
        // Regression test: dedup must NOT strip a user entry whose text
        // coincides with `agent.label` ("Running", "Needs input", "Idle").
        // Only the agent.task / agent.message slots participate in dedup
        // because only they have the legacy dual-write problem.
        let mut store = amux_notify::NotificationStore::new();
        store.set_status(
            1,
            amux_notify::AgentState::Active,
            Some("Running".into()),
            None,
            None,
        );
        store.upsert_entry(
            1,
            "user.note",
            "Running",
            amux_notify::priority::USER_GENERIC,
            None,
            None,
            None,
        );
        commit(&mut store);
        let status = store.workspace_status(1).unwrap();
        let rows = build_status_rows(status);
        // user.note survives even though its text matches agent.label.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "Running");
    }

    #[test]
    fn build_status_rows_filters_empty_text() {
        let mut store = amux_notify::NotificationStore::new();
        store.upsert_entry(
            1,
            "user.generic",
            "",
            amux_notify::priority::USER_GENERIC,
            None,
            None,
            None,
        );
        commit(&mut store);
        let status = store.workspace_status(1).unwrap();
        assert!(build_status_rows(status).is_empty());
    }
}
