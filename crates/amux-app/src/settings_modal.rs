//! Settings modal UI.
//!
//! Presents a centered window with configurable settings for appearance
//! and notifications. Changes are applied immediately (live preview) and
//! persisted to config.toml on Save.

use crate::*;

/// A labeled color picker with optional "clear" button.
/// `color` is None when using theme default, Some([r,g,b]) when overridden.
fn color_picker_field(ui: &mut egui::Ui, label: &str, color: &mut Option<[u8; 3]>) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut rgb = color.unwrap_or([128, 128, 128]);
        let response = ui.color_edit_button_srgb(&mut rgb);
        if response.changed() {
            *color = Some(rgb);
        }
        if color.is_some() {
            if ui.small_button("Clear").clicked() {
                *color = None;
            }
        } else {
            ui.weak("(default)");
        }
    });
}

fn hex_to_rgb(s: &str) -> Option<[u8; 3]> {
    config::ColorsConfig::parse_hex(s)
}

fn rgb_to_hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

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
    // Colors (None = use theme default)
    pub(crate) foreground: Option<[u8; 3]>,
    pub(crate) background: Option<[u8; 3]>,
    pub(crate) cursor_fg: Option<[u8; 3]>,
    pub(crate) cursor_bg: Option<[u8; 3]>,
    pub(crate) selection_fg: Option<[u8; 3]>,
    pub(crate) selection_bg: Option<[u8; 3]>,
    pub(crate) accent: Option<[u8; 3]>,
    pub(crate) sidebar_bg: Option<[u8; 3]>,
    pub(crate) notification_ring: Option<[u8; 3]>,
    pub(crate) pane_dim_alpha: u8,
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
            foreground: config.colors.foreground.as_deref().and_then(hex_to_rgb),
            background: config.colors.background.as_deref().and_then(hex_to_rgb),
            cursor_fg: config.colors.cursor_fg.as_deref().and_then(hex_to_rgb),
            cursor_bg: config.colors.cursor_bg.as_deref().and_then(hex_to_rgb),
            selection_fg: config.colors.selection_fg.as_deref().and_then(hex_to_rgb),
            selection_bg: config.colors.selection_bg.as_deref().and_then(hex_to_rgb),
            accent: config.colors.accent.as_deref().and_then(hex_to_rgb),
            sidebar_bg: config.colors.sidebar_bg.as_deref().and_then(hex_to_rgb),
            notification_ring: config
                .colors
                .notification_ring
                .as_deref()
                .and_then(hex_to_rgb),
            pane_dim_alpha: config.colors.pane_dim_alpha.unwrap_or(100),
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

                    // --- Colors ---
                    ui.heading("Colors");
                    ui.add_space(4.0);
                    ui.label("Leave blank for theme default. Use #rrggbb hex.");
                    ui.add_space(2.0);

                    color_picker_field(ui, "Foreground:", &mut modal.foreground);
                    color_picker_field(ui, "Background:", &mut modal.background);
                    color_picker_field(ui, "Cursor fg:", &mut modal.cursor_fg);
                    color_picker_field(ui, "Cursor bg:", &mut modal.cursor_bg);
                    color_picker_field(ui, "Selection fg:", &mut modal.selection_fg);
                    color_picker_field(ui, "Selection bg:", &mut modal.selection_bg);

                    ui.add_space(4.0);
                    ui.label("Chrome");
                    color_picker_field(ui, "Accent:", &mut modal.accent);
                    color_picker_field(ui, "Sidebar bg:", &mut modal.sidebar_bg);
                    color_picker_field(ui, "Notif ring:", &mut modal.notification_ring);
                    ui.horizontal(|ui| {
                        ui.label("Pane dim:");
                        ui.add(egui::Slider::new(&mut modal.pane_dim_alpha, 0..=255));
                    });

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

        // Colors
        self.app_config.colors.foreground = modal.foreground.map(rgb_to_hex);
        self.app_config.colors.background = modal.background.map(rgb_to_hex);
        self.app_config.colors.cursor_fg = modal.cursor_fg.map(rgb_to_hex);
        self.app_config.colors.cursor_bg = modal.cursor_bg.map(rgb_to_hex);
        self.app_config.colors.selection_fg = modal.selection_fg.map(rgb_to_hex);
        self.app_config.colors.selection_bg = modal.selection_bg.map(rgb_to_hex);
        self.app_config.colors.accent = modal.accent.map(rgb_to_hex);
        self.app_config.colors.sidebar_bg = modal.sidebar_bg.map(rgb_to_hex);
        self.app_config.colors.notification_ring = modal.notification_ring.map(rgb_to_hex);
        self.app_config.colors.pane_dim_alpha = Some(modal.pane_dim_alpha);

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

        // Colors
        let colors = doc["colors"]
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut();
        if let Some(c) = colors {
            fn set_opt_color(table: &mut toml_edit::Table, key: &str, val: &Option<String>) {
                match val {
                    Some(v) => table[key] = toml_edit::value(v),
                    None => {
                        table.remove(key);
                    }
                }
            }
            set_opt_color(c, "foreground", &self.app_config.colors.foreground);
            set_opt_color(c, "background", &self.app_config.colors.background);
            set_opt_color(c, "cursor_fg", &self.app_config.colors.cursor_fg);
            set_opt_color(c, "cursor_bg", &self.app_config.colors.cursor_bg);
            set_opt_color(c, "selection_fg", &self.app_config.colors.selection_fg);
            set_opt_color(c, "selection_bg", &self.app_config.colors.selection_bg);
            set_opt_color(c, "accent", &self.app_config.colors.accent);
            set_opt_color(c, "sidebar_bg", &self.app_config.colors.sidebar_bg);
            set_opt_color(
                c,
                "notification_ring",
                &self.app_config.colors.notification_ring,
            );
            if let Some(alpha) = self.app_config.colors.pane_dim_alpha {
                c["pane_dim_alpha"] = toml_edit::value(alpha as i64);
            }
        }

        if let Err(e) = std::fs::write(path, doc.to_string()) {
            tracing::error!("Failed to write config: {e}");
        } else {
            tracing::info!("Settings saved to {}", path.display());
        }
    }
}
