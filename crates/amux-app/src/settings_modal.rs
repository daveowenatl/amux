//! Settings modal UI.
//!
//! Presents a centered window with configurable settings for appearance
//! and notifications. Changes are applied immediately (live preview) and
//! persisted to config.toml on Save.

use crate::*;

/// Editable settings state — populated from AppConfig on open,
/// applied back on Save.
pub(crate) struct SettingsModal {
    pub(crate) font_size: f32,
    pub(crate) font_family: String,
    pub(crate) theme_source: String,
    pub(crate) shell: String,
    pub(crate) system_notifications: bool,
    pub(crate) dock_badge: bool,
    pub(crate) auto_reorder_workspaces: bool,
    pub(crate) sound: String,
    pub(crate) play_when_focused: bool,
    /// Snapshot of original values for cancel/revert.
    original_font_size: f32,
    original_font_family: String,
}

impl SettingsModal {
    pub(crate) fn from_config(config: &config::AppConfig) -> Self {
        Self {
            font_size: config.font_size,
            font_family: config.font_family.clone(),
            theme_source: config.theme_source.clone(),
            shell: config.shell.clone().unwrap_or_default(),
            system_notifications: config.notifications.system_notifications,
            dock_badge: config.notifications.dock_badge,
            auto_reorder_workspaces: config.notifications.auto_reorder_workspaces,
            sound: config.notifications.sound.sound.clone(),
            play_when_focused: config.notifications.sound.play_when_focused,
            original_font_size: config.font_size,
            original_font_family: config.font_family.clone(),
        }
    }
}

impl AmuxApp {
    pub(crate) fn render_settings_modal(&mut self, ctx: &egui::Context) {
        let mut save = false;
        let mut cancel = false;

        let palette = crate::popup_theme::MenuPalette::from_theme(&self.theme);
        crate::popup_theme::with_modal_palette(ctx, palette, || {
            egui::Window::new("Settings")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .fixed_size([360.0, 0.0])
                .show(ctx, |ui| {
                    let modal = self.settings_modal.as_mut().unwrap();

                    // --- Appearance ---
                    ui.heading("Appearance");
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label("Font size:");
                        ui.add(egui::Slider::new(&mut modal.font_size, 6.0..=48.0).step_by(1.0));
                    });

                    ui.horizontal(|ui| {
                        ui.label("Font family:");
                        ui.text_edit_singleline(&mut modal.font_family);
                    });

                    ui.horizontal(|ui| {
                        ui.label("Theme:");
                        egui::ComboBox::from_id_salt("theme_source")
                            .selected_text(&modal.theme_source)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut modal.theme_source,
                                    "default".to_string(),
                                    "Default (Monokai Classic)",
                                );
                                ui.selectable_value(
                                    &mut modal.theme_source,
                                    "ghostty".to_string(),
                                    "Ghostty",
                                );
                            });
                    });

                    ui.horizontal(|ui| {
                        ui.label("Shell:");
                        ui.text_edit_singleline(&mut modal.shell);
                    });

                    ui.add_space(8.0);

                    // --- Notifications ---
                    ui.heading("Notifications");
                    ui.add_space(4.0);

                    ui.checkbox(&mut modal.system_notifications, "System notifications");
                    ui.checkbox(&mut modal.dock_badge, "Dock / taskbar badge");
                    ui.checkbox(
                        &mut modal.auto_reorder_workspaces,
                        "Auto-reorder workspaces on notification",
                    );

                    ui.horizontal(|ui| {
                        ui.label("Sound:");
                        egui::ComboBox::from_id_salt("sound")
                            .selected_text(&modal.sound)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut modal.sound,
                                    "system".to_string(),
                                    "System",
                                );
                                ui.selectable_value(&mut modal.sound, "none".to_string(), "None");
                            });
                    });

                    ui.checkbox(&mut modal.play_when_focused, "Play sound when focused");

                    ui.add_space(8.0);

                    // --- Buttons ---
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });

                    // Live preview font size changes
                    if modal.font_size != self.font_size {
                        self.font_size = modal.font_size;
                        #[cfg(feature = "gpu-renderer")]
                        if let Some(gpu) = &mut self.gpu_renderer {
                            gpu.set_font_size(self.font_size);
                        }
                    }
                });
        });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }

        if save {
            self.apply_settings();
            self.save_config_to_disk();
            self.settings_modal = None;
        } else if cancel {
            // Revert live-previewed changes
            let modal = self.settings_modal.as_ref().unwrap();
            self.font_size = modal.original_font_size;
            #[cfg(feature = "gpu-renderer")]
            if let Some(gpu) = &mut self.gpu_renderer {
                gpu.set_font_size(self.font_size);
            }
            self.settings_modal = None;
        }
    }

    fn apply_settings(&mut self) {
        let modal = self.settings_modal.as_ref().unwrap();

        self.font_size = modal.font_size;
        self.app_config.font_size = modal.font_size;
        self.app_config.font_family = modal.font_family.clone();
        self.app_config.theme_source = modal.theme_source.clone();
        self.app_config.shell = if modal.shell.is_empty() {
            None
        } else {
            Some(modal.shell.clone())
        };
        self.app_config.notifications.system_notifications = modal.system_notifications;
        self.app_config.notifications.dock_badge = modal.dock_badge;
        self.app_config.notifications.auto_reorder_workspaces = modal.auto_reorder_workspaces;
        self.app_config.notifications.sound.sound = modal.sound.clone();
        self.app_config.notifications.sound.play_when_focused = modal.play_when_focused;

        #[cfg(feature = "gpu-renderer")]
        if let Some(gpu) = &mut self.gpu_renderer {
            gpu.set_font_size(self.font_size);
            if modal.font_family != modal.original_font_family {
                gpu.set_font_family(&modal.font_family, self.font_size);
            }
        }
    }

    fn save_config_to_disk(&self) {
        let Some(path) = &self.config_file_path else {
            tracing::warn!("No config file path — settings not saved to disk");
            return;
        };

        // Read existing file content to preserve comments and unknown fields.
        let existing = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc = match existing.parse::<toml_edit::DocumentMut>() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("Failed to parse config for update: {e}");
                return;
            }
        };

        // Update known fields
        doc["font_size"] = toml_edit::value(self.app_config.font_size as f64);
        doc["font_family"] = toml_edit::value(&self.app_config.font_family);
        doc["theme_source"] = toml_edit::value(&self.app_config.theme_source);
        if let Some(shell) = &self.app_config.shell {
            doc["shell"] = toml_edit::value(shell);
        } else if doc.contains_key("shell") {
            doc.remove("shell");
        }

        // Notifications
        let notif = doc["notifications"]
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut();
        if let Some(t) = notif {
            t["system_notifications"] =
                toml_edit::value(self.app_config.notifications.system_notifications);
            t["dock_badge"] = toml_edit::value(self.app_config.notifications.dock_badge);
            t["auto_reorder_workspaces"] =
                toml_edit::value(self.app_config.notifications.auto_reorder_workspaces);

            let sound = t["sound"]
                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_mut();
            if let Some(s) = sound {
                s["sound"] = toml_edit::value(&self.app_config.notifications.sound.sound);
                s["play_when_focused"] =
                    toml_edit::value(self.app_config.notifications.sound.play_when_focused);
            }
        }

        if let Err(e) = std::fs::write(path, doc.to_string()) {
            tracing::error!("Failed to write config: {e}");
        } else {
            tracing::info!("Settings saved to {}", path.display());
        }
    }
}
