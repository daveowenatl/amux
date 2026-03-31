//! Application configuration loaded from config.toml.

/// Default font family — must match `amux_term::DEFAULT_FONT_FAMILY`.
pub const DEFAULT_FONT_FAMILY: &str = "IBM Plex Mono";
/// Default font size — must match `amux_term::DEFAULT_FONT_SIZE`.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

#[derive(Debug, serde::Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub font_size: f32,
    /// Font family for terminal text (e.g. "JetBrains Mono", "Menlo").
    /// Resolved against system-installed fonts by cosmic-text.
    pub font_family: String,
    /// Terminal backend engine: "wezterm" (default) or "ghostty".
    /// The "ghostty" backend requires the `libghostty` feature.
    pub backend: String,
    pub notifications: NotificationConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            font_size: DEFAULT_FONT_SIZE,
            font_family: DEFAULT_FONT_FAMILY.to_owned(),
            backend: "wezterm".to_owned(),
            notifications: NotificationConfig::default(),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    /// Deliver OS-native toast notifications when the app is unfocused.
    pub system_notifications: bool,
    /// Automatically move workspaces to the top of the sidebar on notification.
    pub auto_reorder_workspaces: bool,
    /// Show unread count on macOS dock icon / Windows taskbar.
    pub dock_badge: bool,
    /// Shell command to run on each notification (receives AMUX_NOTIFICATION_* env vars).
    pub custom_command: Option<String>,
    /// Notification sound settings.
    pub sound: NotificationSoundConfig,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            system_notifications: true,
            auto_reorder_workspaces: true,
            dock_badge: true,
            custom_command: None,
            sound: NotificationSoundConfig::default(),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(default)]
pub struct NotificationSoundConfig {
    /// "system", "none", or path to a .wav/.ogg/.mp3 file.
    pub sound: String,
    /// Play sound even when app is focused (suppressed notification feedback).
    pub play_when_focused: bool,
}

impl Default for NotificationSoundConfig {
    fn default() -> Self {
        Self {
            sound: "system".to_string(),
            play_when_focused: true,
        }
    }
}

pub fn load_app_config() -> AppConfig {
    let config_path = if cfg!(target_os = "windows") {
        dirs::config_dir().map(|d| d.join("amux").join("config.toml"))
    } else {
        dirs::home_dir().map(|d| d.join(".config").join("amux").join("config.toml"))
    };

    if let Some(path) = config_path {
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str::<AppConfig>(&contents) {
                Ok(mut config) => {
                    tracing::info!("Loaded config from {}", path.display());
                    config.font_size = validate_font_size(config.font_size);
                    // Trim whitespace; treat empty as default.
                    config.font_family = config.font_family.trim().to_owned();
                    if config.font_family.is_empty() {
                        config.font_family = DEFAULT_FONT_FAMILY.to_owned();
                    }
                    return config;
                }
                Err(e) => {
                    tracing::warn!("Failed to parse {}: {}", path.display(), e);
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!("No config file at {}", path.display());
            }
            Err(e) => {
                tracing::warn!("Failed to read {}: {}", path.display(), e);
            }
        }
    }

    AppConfig::default()
}

pub fn validate_font_size(size: f32) -> f32 {
    const MIN_FONT_SIZE: f32 = 4.0;
    const MAX_FONT_SIZE: f32 = 96.0;
    if !size.is_finite() || size <= 0.0 {
        DEFAULT_FONT_SIZE
    } else {
        size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    }
}
