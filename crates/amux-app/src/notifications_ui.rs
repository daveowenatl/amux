//! Notification delivery, workspace bubbling, and notification panel rendering.

use crate::*;

impl AmuxApp {
    pub(crate) fn drain_notifications(&mut self) {
        // Collect events first to avoid borrow conflicts
        let mut events: Vec<(u64, u64, u64, NotificationEvent)> = Vec::new();

        for (&pane_id, managed) in &self.panes {
            let ws_id = self.workspace_for_pane(pane_id).unwrap_or(0);
            for surface in &managed.surfaces {
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
                    if let Some(managed) = self.panes.get_mut(&pane_id) {
                        for surface in &mut managed.surfaces {
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

    pub(crate) fn render_notification_panel(&mut self, ctx: &egui::Context) {
        let mut close_panel = false;
        let mut mark_all = false;
        let mut jump_to: Option<(u64, u64)> = None; // (workspace_id, pane_id)
        let mut remove_id: Option<u64> = None;

        egui::Window::new("Notifications")
            .collapsible(false)
            .resizable(true)
            .default_size([380.0, 460.0])
            .anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(
                        egui::RichText::new("Notifications").color(egui::Color32::from_gray(220)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Clear All").clicked() {
                            mark_all = true;
                        }
                        if ui.small_button("Jump to Latest").clicked() {
                            jump_to = self
                                .notifications
                                .most_recent_unread()
                                .map(|n| (n.workspace_id, n.pane_id));
                            close_panel = true;
                        }
                    });
                });
                ui.separator();

                let notifications = self.notifications.all_notifications();
                if notifications.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.label(
                            egui::RichText::new("\u{1f515}") // 🔕
                                .size(32.0),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("No notifications")
                                .color(egui::Color32::from_gray(140))
                                .size(14.0),
                        );
                        ui.label(
                            egui::RichText::new("Agent notifications will appear here")
                                .color(egui::Color32::from_gray(80))
                                .size(11.0),
                        );
                    });
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        // Group by workspace, most recent notifications first within each
                        let mut grouped: Vec<_> = notifications.iter().rev().collect();
                        grouped.sort_by_key(|n| n.workspace_id);
                        let mut last_ws_id: Option<u64> = None;
                        for notif in &grouped {
                            // Workspace section header
                            if last_ws_id != Some(notif.workspace_id) {
                                last_ws_id = Some(notif.workspace_id);
                                if let Some(ws) =
                                    self.workspaces.iter().find(|w| w.id == notif.workspace_id)
                                {
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(&ws.title)
                                            .font(fonts::bold_font(11.0))
                                            .color(egui::Color32::from_gray(120)),
                                    );
                                    ui.add_space(2.0);
                                }
                            }

                            let response = ui.horizontal(|ui| {
                                // Source icon + unread dot
                                let source_icon = match notif.source {
                                    NotificationSource::Bell => "\u{1f514}",  // 🔔
                                    NotificationSource::Toast => "\u{1f4ac}", // 💬
                                    NotificationSource::Cli => "\u{2328}",    // ⌨
                                };
                                let dot_color = if notif.read {
                                    egui::Color32::from_gray(60)
                                } else {
                                    self.theme.chrome.accent
                                };
                                ui.label(
                                    egui::RichText::new(source_icon).size(10.0).color(dot_color),
                                );

                                ui.vertical(|ui| {
                                    let title = if notif.title.is_empty() {
                                        &notif.body
                                    } else {
                                        &notif.title
                                    };
                                    ui.label(
                                        egui::RichText::new(title)
                                            .color(egui::Color32::from_gray(200)),
                                    );
                                    if !notif.title.is_empty() && !notif.body.is_empty() {
                                        let body_display = if notif.body.len() > 100 {
                                            format!("{}...", &notif.body[..97])
                                        } else {
                                            notif.body.clone()
                                        };
                                        ui.label(
                                            egui::RichText::new(body_display)
                                                .small()
                                                .color(egui::Color32::from_gray(140)),
                                        );
                                    }
                                    let age = render::format_duration(notif.created_at.elapsed());
                                    ui.label(
                                        egui::RichText::new(age)
                                            .small()
                                            .color(egui::Color32::from_gray(80)),
                                    );
                                });

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("×").clicked() {
                                            remove_id = Some(notif.id);
                                        }
                                    },
                                );
                            });
                            if response.response.interact(egui::Sense::click()).clicked() {
                                jump_to = Some((notif.workspace_id, notif.pane_id));
                                close_panel = true;
                            }
                            ui.separator();
                        }
                    });
                }
            });

        if mark_all {
            self.notifications.mark_all_read();
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
