//! Notification delivery, workspace bubbling, and notification panel rendering.

use crate::*;

// --- Shared titlebar icon-row geometry -------------------------------------
// These constants describe the fixed-position icon row in the titlebar and
// are referenced by both the icon renderer and the notifications panel so
// that the panel always anchors on the actual bell-icon center.

const TITLEBAR_ICON_W: f32 = 28.0;
const TITLEBAR_ICON_H: f32 = 22.0;
const TITLEBAR_ICON_GAP: f32 = 2.0;
/// Index of the bell in the icon row: sidebar(0), bell(1), plus(2).
const TITLEBAR_BELL_INDEX: usize = 1;

/// Left inset before the first icon. On macOS we reserve space for the
/// native traffic-light buttons; on other platforms the row hugs the edge.
fn titlebar_left_inset() -> f32 {
    #[cfg(target_os = "macos")]
    {
        78.0
    }
    #[cfg(not(target_os = "macos"))]
    {
        8.0
    }
}

/// Horizontal center of the bell icon in screen-local x coordinates,
/// relative to the window's left edge.
fn titlebar_bell_center_x() -> f32 {
    titlebar_left_inset()
        + (TITLEBAR_BELL_INDEX as f32) * (TITLEBAR_ICON_W + TITLEBAR_ICON_GAP)
        + TITLEBAR_ICON_W / 2.0
}

impl AmuxApp {
    pub(crate) fn drain_notifications(&mut self) {
        // Collect events first to avoid borrow conflicts
        let mut events: Vec<(u64, u64, u64, NotificationEvent)> = Vec::new();

        for (&pane_id, entry) in &self.panes {
            let PaneEntry::Terminal(managed) = entry else {
                continue;
            };
            let ws_id = self.workspace_for_pane(pane_id).unwrap_or(0);
            for surface in managed.surfaces() {
                for event in surface.pane.drain_notifications() {
                    events.push((ws_id, pane_id, surface.id, event));
                }
            }
        }

        for (ws_id, pane_id, surface_id, event) in events {
            let (title, body, source) = match event {
                NotificationEvent::Toast { title, body } => {
                    (title.unwrap_or_default(), body, NotificationSource::Toast)
                }
                NotificationEvent::Bell => {
                    (String::new(), "Bell".to_string(), NotificationSource::Bell)
                }
                NotificationEvent::TitleChanged(_) => {
                    continue;
                }
                NotificationEvent::WorkingDirectoryChanged => {
                    // Store the CWD from OSC 7 into surface metadata
                    if let Some(PaneEntry::Terminal(managed)) = self.panes.get_mut(&pane_id) {
                        for surface in managed.surfaces_mut() {
                            if surface.id == surface_id {
                                let cwd = surface
                                    .pane
                                    .working_dir()
                                    .and_then(|url| url.to_file_path().ok())
                                    .map(|p| p.to_string_lossy().to_string());
                                if cwd.is_some() {
                                    surface.metadata.cwd = cwd;
                                }
                            }
                        }
                    }
                    continue;
                }
            };

            let skip_toast = matches!(source, NotificationSource::Bell);
            self.deliver_notification(
                ws_id,
                pane_id,
                surface_id,
                title,
                String::new(),
                body,
                source,
                skip_toast,
            );
        }
    }

    /// Three-tier notification delivery (matching cmux):
    /// 1. App unfocused → system toast + custom command + unread
    /// 2. App focused, different pane → in-app sound + custom command + unread
    /// 3. App focused, same pane → mark read (flash only, no ring)
    ///
    /// `skip_toast` suppresses the system toast (used for bell notifications).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn deliver_notification(
        &mut self,
        ws_id: u64,
        pane_id: PaneId,
        surface_id: u64,
        title: String,
        subtitle: String,
        body: String,
        source: NotificationSource,
        skip_toast: bool,
    ) -> u64 {
        let focused = self.focused_pane_id();
        let source_str = source.as_str();
        // Clone for the IPC broadcast after the notification is stored.
        let bc_title = title.clone();
        let bc_subtitle = subtitle.clone();
        let bc_body = body.clone();

        let nid = if !self.app_focused {
            // Tier 1: app is unfocused — always treat as background
            if self.app_config.notifications.system_notifications && !skip_toast {
                self.system_notifier.send(&title, &body, ws_id, pane_id);
            }
            if let Some(cmd) = &self.app_config.notifications.custom_command {
                self.system_notifier
                    .run_custom_command(cmd, &title, &body, source_str);
            }
            let nid = self
                .notifications
                .push(ws_id, pane_id, surface_id, title, subtitle, body, source);
            if self.app_config.notifications.auto_reorder_workspaces {
                self.bubble_workspace(ws_id);
            }
            nid
        } else if pane_id == focused {
            // Tier 3: app focused, same pane — mark read (flash only)
            self.notifications
                .push_read(ws_id, pane_id, surface_id, title, subtitle, body, source)
        } else {
            // Tier 2: app focused, different pane — in-app sound + command
            if self.app_config.notifications.sound.play_when_focused {
                if let Some(player) = &self.sound_player {
                    player.play();
                }
            }
            if let Some(cmd) = &self.app_config.notifications.custom_command {
                self.system_notifier
                    .run_custom_command(cmd, &title, &body, source_str);
            }
            let nid = self
                .notifications
                .push(ws_id, pane_id, surface_id, title, subtitle, body, source);
            if self.app_config.notifications.auto_reorder_workspaces {
                self.bubble_workspace(ws_id);
            }
            nid
        };

        // Broadcast to subscribed IPC clients
        self.event_broadcaster.send(amux_ipc::ServerEvent {
            event: "notification".to_string(),
            data: serde_json::json!({
                "notification_id": nid,
                "workspace_id": ws_id.to_string(),
                "pane_id": pane_id.to_string(),
                "title": bc_title,
                "subtitle": bc_subtitle,
                "body": bc_body,
                "source": source_str,
            }),
        });

        nid
    }

    /// Move a workspace to the top of the sidebar (just index 0 for now,
    /// since amux doesn't have pinned workspaces yet). Adjusts
    /// `active_workspace_idx` to keep the active workspace correct.
    pub(crate) fn bubble_workspace(&mut self, workspace_id: u64) {
        let active_ws_id = self.workspaces[self.active_workspace_idx].id;
        // Don't bubble the active workspace
        if workspace_id == active_ws_id {
            return;
        }
        let Some(from) = self.workspaces.iter().position(|ws| ws.id == workspace_id) else {
            return;
        };
        if from == 0 {
            return;
        }
        let ws = self.workspaces.remove(from);
        self.workspaces.insert(0, ws);
        // Fix active_workspace_idx: the active workspace shifted right by 1
        // if it was before the removed position.
        self.active_workspace_idx = self
            .workspaces
            .iter()
            .position(|ws| ws.id == active_ws_id)
            .unwrap_or(0);
    }

    pub(crate) fn workspace_for_pane(&self, pane_id: PaneId) -> Option<u64> {
        self.workspaces
            .iter()
            .find(|ws| ws.tree.iter_panes().contains(&pane_id))
            .map(|ws| ws.id)
    }

    /// Aggregate metadata from the focused surface of the focused pane in a workspace.
    pub(crate) fn workspace_metadata(&self, workspace: &Workspace) -> SurfaceMetadata {
        self.panes
            .get(&workspace.focused_pane)
            .and_then(|e| e.as_terminal())
            .map(|mp| {
                let sf = mp.active_surface();
                let mut meta = sf.metadata.clone();
                // Capture the surface's OSC title for sidebar display
                let title = sf.pane.title();
                if !title.is_empty() {
                    meta.surface_title = Some(title.to_string());
                }
                meta
            })
            .unwrap_or_default()
    }

    pub(crate) fn jump_to_latest_unread(&mut self) {
        if let Some(notif) = self.notifications.most_recent_unread() {
            let ws_id = notif.workspace_id;
            let pane_id = notif.pane_id as PaneId;

            // Switch to the notification's workspace
            if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) {
                self.active_workspace_idx = idx;
            }
            self.set_focus(pane_id);
        }
    }

    /// Render the top-left titlebar icon row: sidebar toggle, notifications bell,
    /// and new workspace — mirroring cmux's titlebar layout. The icons are
    /// anchored to the window's absolute top-left so they stay put regardless
    /// of sidebar visibility.
    pub(crate) fn render_titlebar_icons(&mut self, ctx: &egui::Context) {
        let icon_size = egui::vec2(TITLEBAR_ICON_W, TITLEBAR_ICON_H);
        let gap = TITLEBAR_ICON_GAP;
        let screen = ctx.screen_rect();
        let top_y = screen.min.y + 3.0;
        let origin_x = screen.min.x + titlebar_left_inset();

        egui::Area::new(egui::Id::new("amux_titlebar_icons"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(origin_x, top_y))
            .show(ctx, |ui| {
                self.render_titlebar_icons_inner(ui, icon_size, gap);
            });
    }

    fn render_titlebar_icons_inner(&mut self, ui: &mut egui::Ui, icon_size: egui::Vec2, gap: f32) {
        let origin = ui.min_rect().min;
        let top_y = origin.y;
        let mut x = origin.x;

        // Platform-specific shortcut labels.
        #[cfg(target_os = "macos")]
        let (sc_sidebar, sc_notif, sc_new) = ("⌘B", "⌘I", "⌘N");
        #[cfg(not(target_os = "macos"))]
        let (sc_sidebar, sc_notif, sc_new) = ("Ctrl+B", "Ctrl+I", "Ctrl+Shift+N");

        // --- Sidebar toggle ---
        let r_sidebar = egui::Rect::from_min_size(egui::pos2(x, top_y), icon_size);
        let resp_sidebar = draw_icon_button(
            ui,
            r_sidebar,
            ui.id().with("titlebar_sidebar_btn"),
            &format!("Toggle sidebar  ({sc_sidebar})"),
            draw_sidebar_glyph,
        );
        if resp_sidebar.clicked() {
            self.sidebar.visible = !self.sidebar.visible;
        }
        x += icon_size.x + gap;

        // --- Notifications bell ---
        let unread = self.notifications.total_unread();
        let r_bell = egui::Rect::from_min_size(egui::pos2(x, top_y), icon_size);
        let resp_bell = draw_icon_button(
            ui,
            r_bell,
            ui.id().with("titlebar_bell_btn"),
            &format!("Notifications  ({sc_notif})"),
            draw_bell_glyph,
        );
        // Unread badge (red dot + count).
        if unread > 0 {
            let painter = ui.painter();
            let badge_center = egui::pos2(r_bell.max.x - 5.0, r_bell.min.y + 5.0);
            let badge_radius = 6.5;
            painter.circle_filled(
                badge_center,
                badge_radius,
                egui::Color32::from_rgb(235, 75, 75),
            );
            painter.circle_stroke(
                badge_center,
                badge_radius,
                egui::Stroke::new(1.0, self.theme.titlebar_bg()),
            );
            let label = if unread > 99 {
                "99+".to_string()
            } else {
                unread.to_string()
            };
            let size = if unread > 9 { 8.0 } else { 9.0 };
            painter.text(
                badge_center,
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(size),
                egui::Color32::WHITE,
            );
        }
        if resp_bell.clicked() {
            self.show_notification_panel = !self.show_notification_panel;
        }
        x += icon_size.x + gap;

        // --- New workspace (+) ---
        let r_new = egui::Rect::from_min_size(egui::pos2(x, top_y), icon_size);
        let resp_new = draw_icon_button(
            ui,
            r_new,
            ui.id().with("titlebar_new_ws_btn"),
            &format!("New workspace  ({sc_new})"),
            draw_plus_glyph,
        );
        if resp_new.clicked() {
            self.create_workspace(None);
        }
    }

    pub(crate) fn render_notification_panel(&mut self, ctx: &egui::Context) {
        let mut close_panel = false;
        let mut clear_all = false;
        let mut jump_to: Option<(u64, u64)> = None; // (workspace_id, pane_id)
        let mut remove_id: Option<u64> = None;

        // Build a pane_id → (workspace title, tab title) lookup for context lines.
        let context_for: std::collections::HashMap<u64, (String, String)> = self
            .workspaces
            .iter()
            .flat_map(|ws| {
                ws.tree.iter_panes().into_iter().map(|pid| {
                    let tab_title = self
                        .panes
                        .get(&pid)
                        .map(|entry| entry.title())
                        .unwrap_or_else(|| "Tab".to_string());
                    (pid, (ws.title.clone(), tab_title))
                })
            })
            .collect();

        // Align the panel horizontally to the bell icon's center, clamping
        // BOTH the panel dimensions and its anchor to the current viewport so
        // the panel cannot overflow off-screen on small windows. Uses the
        // shared titlebar geometry constants so the panel stays aligned if
        // the icon row layout ever changes.
        let bell_center_x = titlebar_bell_center_x();
        let horizontal_margin = 10.0_f32;
        let top_margin = TERMINAL_TOP_PAD + 4.0;
        let bottom_margin = 10.0_f32;
        let screen = ctx.screen_rect();
        let panel_width = 400.0_f32.min((screen.width() - horizontal_margin * 2.0).max(0.0));
        let panel_height = 500.0_f32.min((screen.height() - top_margin - bottom_margin).max(0.0));
        let max_left = (screen.width() - panel_width - horizontal_margin).max(0.0);
        let min_left = horizontal_margin.min(max_left);
        let panel_left = (bell_center_x - panel_width / 2.0).clamp(min_left, max_left);

        egui::Window::new("Notifications")
            .title_bar(false)
            .movable(false)
            .collapsible(false)
            .resizable(false)
            .fixed_size([panel_width, panel_height])
            .anchor(egui::Align2::LEFT_TOP, [panel_left, top_margin])
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(self.theme.chrome.sidebar_bg)
                    .stroke(egui::Stroke::new(1.0, self.theme.chrome.divider))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| {
                // Header
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 36.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_space(14.0);
                        ui.label(
                            egui::RichText::new("Notifications")
                                .font(fonts::bold_font(14.0))
                                .color(egui::Color32::from_gray(230)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(10.0);
                            if ui
                                .add(egui::Button::new(
                                    egui::RichText::new("\u{2715}")
                                        .size(13.0)
                                        .color(egui::Color32::from_gray(160)),
                                ))
                                .clicked()
                            {
                                close_panel = true;
                            }
                            if ui
                                .add(egui::Button::new(
                                    egui::RichText::new("Clear all")
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(170)),
                                ))
                                .clicked()
                            {
                                clear_all = true;
                            }
                        });
                    },
                );
                // Divider under header
                let sep_y = ui.cursor().min.y;
                ui.painter().hline(
                    ui.min_rect().left()..=ui.min_rect().right(),
                    sep_y,
                    egui::Stroke::new(1.0, self.theme.chrome.divider),
                );
                ui.add_space(1.0);

                let notifications = self.notifications.all_notifications();
                if notifications.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(60.0);
                        ui.label(
                            egui::RichText::new("\u{1F515}") // 🔕
                                .size(34.0)
                                .color(egui::Color32::from_gray(90)),
                        );
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("No notifications")
                                .font(fonts::bold_font(13.0))
                                .color(egui::Color32::from_gray(180)),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Agent notifications will appear here")
                                .size(11.0)
                                .color(egui::Color32::from_gray(110)),
                        );
                        ui.add_space(60.0);
                    });
                } else {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_space(6.0);
                            // Flat chronological list (newest first), matching cmux.
                            for notif in notifications.iter().rev() {
                                let context = context_for.get(&notif.pane_id);
                                let row_action =
                                    render_notification_row(ui, notif, &self.theme, context);
                                match row_action {
                                    RowAction::Jump => {
                                        jump_to = Some((notif.workspace_id, notif.pane_id));
                                        close_panel = true;
                                    }
                                    RowAction::Dismiss => {
                                        remove_id = Some(notif.id);
                                    }
                                    RowAction::None => {}
                                }
                                ui.add_space(6.0);
                            }
                        });
                }
            });

        if clear_all {
            self.notifications.clear_all();
        }
        if let Some(id) = remove_id {
            self.notifications.remove_notification(id);
        }
        if let Some((ws_id, pane_id)) = jump_to {
            if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) {
                self.active_workspace_idx = idx;
            }
            self.set_focus(pane_id as PaneId);
        }
        if close_panel {
            self.show_notification_panel = false;
        }
    }
}

/// Shared chrome for a titlebar icon button: cursor, tooltip, and a
/// caller-supplied glyph drawer. Intentionally flat — no hover/active tint.
fn draw_icon_button<F>(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    tooltip: &str,
    draw_glyph: F,
) -> egui::Response
where
    F: FnOnce(&egui::Painter, egui::Rect, egui::Color32),
{
    // Expand the ui's min_rect to include this widget so egui's tooltip
    // system can position hover text correctly.
    ui.expand_to_include_rect(rect);
    let tooltip_text = egui::RichText::new(tooltip).color(egui::Color32::from_gray(220));
    let response = ui
        .interact(rect, id, egui::Sense::click())
        .on_hover_text(tooltip_text);
    let painter = ui.painter();

    let color = egui::Color32::from_gray(170);
    draw_glyph(painter, rect, color);

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Monochrome sidebar-toggle glyph: rounded rect with a filled left column.
fn draw_sidebar_glyph(p: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let gw = 14.0;
    let gh = 11.0;
    let origin = egui::pos2(rect.center().x - gw / 2.0, rect.center().y - gh / 2.0);
    let outer = egui::Rect::from_min_size(origin, egui::vec2(gw, gh));
    p.rect_stroke(
        outer,
        1.5,
        egui::Stroke::new(1.4, color),
        egui::StrokeKind::Inside,
    );
    // Left column separator + fill.
    let split_x = origin.x + 4.5;
    p.line_segment(
        [
            egui::pos2(split_x, origin.y),
            egui::pos2(split_x, origin.y + gh),
        ],
        egui::Stroke::new(1.2, color),
    );
    // Two tiny dots in the left column to mimic cmux's sidebar glyph.
    for dy in [3.0, 7.0] {
        p.circle_filled(egui::pos2(origin.x + 2.2, origin.y + dy), 0.6, color);
    }
}

/// Monochrome bell glyph approximating SF Symbol "bell".
fn draw_bell_glyph(p: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let c = rect.center();
    let stroke = egui::Stroke::new(1.3, color);
    // Bell body: a rounded trapezoid, represented as an open path.
    let top_y = c.y - 5.0;
    let bot_y = c.y + 4.0;
    let half_top = 3.0;
    let half_bot = 5.5;
    let path = vec![
        egui::pos2(c.x - half_bot, bot_y),
        egui::pos2(c.x - half_top, top_y + 1.0),
        egui::pos2(c.x - half_top + 0.5, top_y - 0.5),
        egui::pos2(c.x + half_top - 0.5, top_y - 0.5),
        egui::pos2(c.x + half_top, top_y + 1.0),
        egui::pos2(c.x + half_bot, bot_y),
    ];
    p.add(egui::Shape::line(path, stroke));
    // Bottom rim.
    p.line_segment(
        [
            egui::pos2(c.x - half_bot - 0.5, bot_y),
            egui::pos2(c.x + half_bot + 0.5, bot_y),
        ],
        stroke,
    );
    // Clapper.
    p.circle_filled(egui::pos2(c.x, bot_y + 2.2), 1.1, color);
    // Top cap.
    p.circle_filled(egui::pos2(c.x, top_y - 1.0), 0.9, color);
}

/// Monochrome plus glyph.
fn draw_plus_glyph(p: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let c = rect.center();
    let arm = 6.0;
    let stroke = egui::Stroke::new(1.5, color);
    p.line_segment(
        [egui::pos2(c.x - arm, c.y), egui::pos2(c.x + arm, c.y)],
        stroke,
    );
    p.line_segment(
        [egui::pos2(c.x, c.y - arm), egui::pos2(c.x, c.y + arm)],
        stroke,
    );
}

enum RowAction {
    None,
    Jump,
    Dismiss,
}

/// Render a single cmux-style card row for a notification.
fn render_notification_row(
    ui: &mut egui::Ui,
    notif: &amux_notify::Notification,
    theme: &theme::Theme,
    context: Option<&(String, String)>,
) -> RowAction {
    let row_padding = egui::Margin {
        left: 12,
        right: 10,
        top: 10,
        bottom: 10,
    };
    let outer_margin = egui::Margin::symmetric(8, 0);

    let frame = egui::Frame::new()
        .outer_margin(outer_margin)
        .inner_margin(row_padding)
        .fill(egui::Color32::from_gray(40))
        .corner_radius(8.0);

    let mut action = RowAction::None;
    let response = frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                // Unread dot (circle) on the left
                let dot_diam = 8.0;
                let (dot_rect, _) =
                    ui.allocate_exact_size(egui::vec2(dot_diam, 18.0), egui::Sense::hover());
                let dot_center = egui::pos2(dot_rect.left() + dot_diam / 2.0, dot_rect.top() + 7.0);
                if !notif.read {
                    ui.painter()
                        .circle_filled(dot_center, dot_diam / 2.0, theme.chrome.accent);
                } else {
                    ui.painter().circle_stroke(
                        dot_center,
                        dot_diam / 2.0,
                        egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
                    );
                }

                ui.add_space(8.0);

                ui.vertical(|ui| {
                    let avail = ui.available_width();
                    // Title + timestamp row
                    ui.horizontal(|ui| {
                        let title_text = if notif.title.is_empty() {
                            first_line(&notif.body)
                        } else {
                            notif.title.clone()
                        };
                        let age = render::format_duration(notif.created_at.elapsed());
                        ui.allocate_ui_with_layout(
                            egui::vec2((avail - 48.0).max(0.0), 18.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(title_text)
                                            .font(fonts::bold_font(13.0))
                                            .color(egui::Color32::from_gray(232)),
                                    )
                                    .truncate(),
                                );
                            },
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(age)
                                    .size(10.0)
                                    .color(egui::Color32::from_gray(120)),
                            );
                        });
                    });

                    // Subtitle (optional)
                    if !notif.subtitle.is_empty() {
                        ui.add_space(2.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&notif.subtitle)
                                    .size(11.5)
                                    .color(egui::Color32::from_gray(190)),
                            )
                            .truncate(),
                        );
                    }

                    // Body (3-line soft limit)
                    if !notif.body.is_empty()
                        && (!notif.title.is_empty() || !notif.subtitle.is_empty())
                    {
                        ui.add_space(3.0);
                        let body_display = clamp_lines(&notif.body, 3, 220);
                        ui.label(
                            egui::RichText::new(body_display)
                                .size(11.5)
                                .color(egui::Color32::from_gray(170)),
                        );
                    }

                    // Context caption (Workspace · Tab)
                    if let Some((ws_title, tab_title)) = context {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(format!("{ws_title} · {tab_title}"))
                                .size(10.5)
                                .color(egui::Color32::from_gray(115)),
                        );
                    }
                });
            });
        })
        .response;

    // Whole-card click to jump.
    let row_id = ui.id().with(("notif_row", notif.id));
    let click_response = ui.interact(response.rect, row_id, egui::Sense::click());
    if click_response.clicked() {
        action = RowAction::Jump;
    }
    if click_response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        // Hover tint
        ui.painter().rect_stroke(
            response.rect.shrink(0.5),
            8.0,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(18)),
            egui::StrokeKind::Inside,
        );
    }

    // Dismiss (×) button in the top-right corner of the card.
    let close_size = 18.0;
    let close_rect = egui::Rect::from_min_size(
        egui::pos2(
            response.rect.max.x - close_size - 6.0,
            response.rect.min.y + 6.0,
        ),
        egui::vec2(close_size, close_size),
    );
    let close_id = ui.id().with(("notif_close", notif.id));
    let close_resp = ui.interact(close_rect, close_id, egui::Sense::click());
    if close_resp.hovered() {
        ui.painter()
            .rect_filled(close_rect, 4.0, egui::Color32::from_white_alpha(22));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let close_color = if close_resp.hovered() {
        egui::Color32::from_gray(220)
    } else {
        egui::Color32::from_gray(130)
    };
    ui.painter().text(
        close_rect.center(),
        egui::Align2::CENTER_CENTER,
        "\u{2715}",
        egui::FontId::proportional(11.0),
        close_color,
    );
    if close_resp.clicked() {
        action = RowAction::Dismiss;
    }

    action
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}

/// Return at most `max_lines` lines from `s`, collapsing further content
/// with a trailing "…". Also hard-caps total character count.
fn clamp_lines(s: &str, max_lines: usize, max_chars: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let truncated_lines = if lines.len() > max_lines {
        let joined: String = lines[..max_lines].join("\n");
        format!("{joined}\u{2026}")
    } else {
        s.to_string()
    };
    if truncated_lines.chars().count() > max_chars {
        let clipped: String = truncated_lines.chars().take(max_chars - 1).collect();
        format!("{clipped}\u{2026}")
    } else {
        truncated_lines
    }
}
