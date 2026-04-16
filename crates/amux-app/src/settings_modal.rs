//! Settings modal UI.
//!
//! Presents a centered window with configurable settings for appearance,
//! notifications, and colors. Font size and color changes are
//! live-previewed (applied immediately to running panes); other fields
//! (font family, theme source, shell, notification toggles, sound) take
//! effect on Save. Cancel reverts live-previewed changes.
//!
//! Save persists the full settings state to `config.toml` via `toml_edit`
//! so user comments and unknown fields are preserved.

use crate::*;

/// A color picker grid row. Shows the theme default color in the swatch when
/// no override is set; clicking the swatch promotes it to an explicit override.
/// "Reset" clears back to theme default.
fn color_picker_row(
    ui: &mut egui::Ui,
    label: &str,
    color: &mut Option<[u8; 3]>,
    theme_default: [u8; 3],
) {
    ui.label(label);
    let mut rgb = color.unwrap_or(theme_default);
    let response = ui.color_edit_button_srgb(&mut rgb);
    if response.changed() {
        *color = Some(rgb);
    }
    ui.end_row();
}

/// If the given text-field response has focus and a pending paste is
/// available, append the paste text to the field. Used to handle Cmd+V
/// on macOS where muda's native menu bar consumes the keystroke before
/// egui can dispatch Event::Paste.
fn apply_paste_if_focused(response: &egui::Response, field: &mut String, paste: Option<&str>) {
    if let Some(text) = paste {
        if response.has_focus() {
            field.push_str(text);
        }
    }
}

/// Build a ColorsConfig from the modal's current color state.
fn colors_from_modal(modal: &SettingsModal) -> config::ColorsConfig {
    config::ColorsConfig {
        foreground: modal.foreground.map(rgb_to_hex),
        background: modal.background.map(rgb_to_hex),
        cursor_fg: modal.cursor_fg.map(rgb_to_hex),
        cursor_bg: modal.cursor_bg.map(rgb_to_hex),
        selection_fg: modal.selection_fg.map(rgb_to_hex),
        selection_bg: modal.selection_bg.map(rgb_to_hex),
        accent: modal.accent.map(rgb_to_hex),
        sidebar_bg: modal.sidebar_bg.map(rgb_to_hex),
        notification_ring: modal.notification_ring.map(rgb_to_hex),
        pane_dim_alpha: Some(modal.pane_dim_alpha),
        palette: Vec::new(),
    }
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
    original_colors: config::ColorsConfig,
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
            original_colors: config.colors.clone(),
        }
    }
}

impl AmuxApp {
    pub(crate) fn render_settings_modal(&mut self, ctx: &egui::Context) {
        let mut save = false;
        let mut cancel = false;

        // The macOS native menu bar consumes Cmd+V before egui sees it.
        // workspace_ops stores the clipboard text in pending_text_field_paste;
        // here we route it to whichever text field is currently focused.
        let pending_paste = self.pending_text_field_paste.take();

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
                        if ui.small_button("−").clicked() {
                            modal.font_size = (modal.font_size - 1.0).max(4.0);
                        }
                        ui.label(format!("{}", modal.font_size as u32));
                        if ui.small_button("+").clicked() {
                            modal.font_size = (modal.font_size + 1.0).min(96.0);
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("Font family:");
                        let r = ui.text_edit_singleline(&mut modal.font_family);
                        apply_paste_if_focused(
                            &r,
                            &mut modal.font_family,
                            pending_paste.as_deref(),
                        );
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
                        let r = ui.text_edit_singleline(&mut modal.shell);
                        apply_paste_if_focused(&r, &mut modal.shell, pending_paste.as_deref());
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

                    let tc = &self.theme.terminal;
                    let cc = &self.theme.chrome;
                    let accent_default = [cc.accent.r(), cc.accent.g(), cc.accent.b()];
                    let sidebar_default = [cc.sidebar_bg.r(), cc.sidebar_bg.g(), cc.sidebar_bg.b()];
                    let ring_default = [
                        cc.notification_ring.r(),
                        cc.notification_ring.g(),
                        cc.notification_ring.b(),
                    ];

                    egui::Grid::new("colors_grid")
                        .num_columns(2)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            color_picker_row(
                                ui,
                                "Foreground",
                                &mut modal.foreground,
                                tc.foreground,
                            );
                            color_picker_row(
                                ui,
                                "Background",
                                &mut modal.background,
                                tc.background,
                            );
                            color_picker_row(ui, "Cursor fg", &mut modal.cursor_fg, tc.cursor_fg);
                            color_picker_row(ui, "Cursor bg", &mut modal.cursor_bg, tc.cursor_bg);
                            color_picker_row(
                                ui,
                                "Selection fg",
                                &mut modal.selection_fg,
                                tc.selection_fg,
                            );
                            color_picker_row(
                                ui,
                                "Selection bg",
                                &mut modal.selection_bg,
                                tc.selection_bg,
                            );
                            // Chrome
                            ui.separator();
                            ui.separator();
                            ui.end_row();
                            color_picker_row(ui, "Accent", &mut modal.accent, accent_default);
                            color_picker_row(
                                ui,
                                "Sidebar bg",
                                &mut modal.sidebar_bg,
                                sidebar_default,
                            );
                            color_picker_row(
                                ui,
                                "Notif ring",
                                &mut modal.notification_ring,
                                ring_default,
                            );
                        });

                    ui.horizontal(|ui| {
                        ui.label("Pane dim:");
                        ui.add(egui::Slider::new(&mut modal.pane_dim_alpha, 0..=255));
                    });

                    ui.add_space(4.0);
                    if ui.button("Reset colors to defaults").clicked() {
                        modal.foreground = None;
                        modal.background = None;
                        modal.cursor_fg = None;
                        modal.cursor_bg = None;
                        modal.selection_fg = None;
                        modal.selection_bg = None;
                        modal.accent = None;
                        modal.sidebar_bg = None;
                        modal.notification_ring = None;
                        modal.pane_dim_alpha = 100;
                    }

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

                    // Live preview color changes — rebuild theme + propagate
                    // to all panes whenever the modal's color state changes.
                    let live_colors = colors_from_modal(modal);
                    if live_colors != self.app_config.colors {
                        self.app_config.colors = live_colors;
                        self.rebuild_theme_and_propagate();
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
            // Revert live-previewed font + colors
            let modal = self.settings_modal.as_ref().unwrap();
            self.font_size = modal.original_font_size;
            #[cfg(feature = "gpu-renderer")]
            if let Some(gpu) = &mut self.gpu_renderer {
                gpu.set_font_size(self.font_size);
            }
            let original_colors = modal.original_colors.clone();
            if self.app_config.colors != original_colors {
                self.app_config.colors = original_colors;
                self.rebuild_theme_and_propagate();
            }
            self.settings_modal = None;
        }
    }

    /// Rebuild the theme from app_config.colors and propagate the new
    /// palette to all running panes. Used by both live color preview
    /// and the cancel-revert path.
    fn rebuild_theme_and_propagate(&mut self) {
        let mut new_theme = match self.app_config.theme_source.as_str() {
            "ghostty" => {
                if let Some(ghostty_cfg) = amux_ghostty_config::GhosttyConfig::load() {
                    crate::theme::Theme::from_ghostty(&ghostty_cfg)
                } else {
                    crate::theme::Theme::default()
                }
            }
            _ => crate::theme::Theme::default(),
        };
        new_theme.apply_color_config(&self.app_config.colors);
        let mut term_config = (*self.config).clone();
        new_theme.apply_to_palette(&mut term_config.color_palette);
        let new_palette = term_config.color_palette.clone();
        self.config = std::sync::Arc::new(term_config);
        self.theme = new_theme;
        for entry in self.panes.values_mut() {
            if let PaneEntry::Terminal(managed) = entry {
                for surface in managed.surfaces_mut() {
                    surface.pane.set_palette(new_palette.clone());
                }
            }
        }
    }

    fn apply_settings(&mut self) {
        let modal = self.settings_modal.as_ref().unwrap();

        // Normalize: clamp font size to validated range, trim font family
        // and fall back to default when empty.
        let validated_size = config::validate_font_size(modal.font_size);
        let trimmed_family = modal.font_family.trim();
        let normalized_family = if trimmed_family.is_empty() {
            config::DEFAULT_FONT_FAMILY.to_string()
        } else {
            trimmed_family.to_string()
        };

        self.font_size = validated_size;
        self.app_config.font_size = validated_size;
        self.app_config.font_family = normalized_family;
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
        // Reconfigure the runtime sound player so the new sound mode takes
        // effect immediately (without requiring a restart).
        if let Some(player) = &mut self.sound_player {
            player.configure(&modal.sound);
        }

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
        // Resolve path: prefer the loaded config path; otherwise fall back to
        // ~/.amux/config.toml so Save still works when the user's existing
        // config failed to parse at startup (load returned no path) or when
        // there's no config file yet.
        let path = self
            .config_file_path
            .clone()
            .or_else(|| dirs::home_dir().map(|h| h.join(".amux").join("config.toml")));
        let Some(path) = path else {
            tracing::warn!("Could not resolve config path — settings not saved");
            return;
        };

        // Ensure parent directory exists.
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("Failed to create config dir {}: {e}", parent.display());
            }
        }

        // Read existing file content to preserve comments and unknown fields.
        // Distinguish "missing/empty file" from "read error". On read error,
        // log it but proceed with an empty doc so Save still works.
        let existing = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                tracing::warn!("Failed to read existing config: {e} — starting fresh");
                String::new()
            }
        };
        let mut doc = if existing.trim().is_empty() {
            toml_edit::DocumentMut::new()
        } else {
            match existing.parse::<toml_edit::DocumentMut>() {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse existing config ({e}) — overwriting with fresh doc"
                    );
                    toml_edit::DocumentMut::new()
                }
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

        if let Err(e) = std::fs::write(&path, doc.to_string()) {
            tracing::error!("Failed to write config to {}: {e}", path.display());
        } else {
            tracing::info!("Settings saved to {}", path.display());
        }
    }
}
