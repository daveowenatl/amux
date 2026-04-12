//! Application configuration loaded from config.toml.

use std::collections::HashMap;

/// Default font family — must match `amux_term::DEFAULT_FONT_FAMILY`.
pub const DEFAULT_FONT_FAMILY: &str = "IBM Plex Mono";
/// Default font size — must match `amux_term::DEFAULT_FONT_SIZE`.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// Custom color palette configuration.
/// Colors are specified as hex strings: "#rrggbb" or "rrggbb".
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct ColorsConfig {
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub cursor_fg: Option<String>,
    pub cursor_bg: Option<String>,
    pub selection_fg: Option<String>,
    pub selection_bg: Option<String>,
    /// ANSI colors 0-15. Index maps to color number.
    /// e.g. palette = ["#000000", "#cc0000", ...] for colors 0, 1, etc.
    #[serde(default)]
    pub palette: Vec<String>,
}

impl ColorsConfig {
    /// Parse a hex color string like "#rrggbb" or "rrggbb" into [u8; 3].
    pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some([r, g, b])
    }
}

/// Bindable keyboard actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Copy,
    Paste,
    Find,
    SelectAll,
    CopyMode,
    ToggleSidebar,
    NewBrowserTab,
    NewWorkspace,
    NewTab,
    NextWorkspace,
    PrevWorkspace,
    NextTab,
    PrevTab,
    SplitRight,
    SplitDown,
    ClosePane,
    NavigateLeft,
    NavigateRight,
    NavigateUp,
    NavigateDown,
    ZoomToggle,
    DevTools,
    NotificationPanel,
    JumpToUnread,
    ClearScrollback,
}

/// A keyboard shortcut: modifier flags + key name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    /// Cmd on macOS, Ctrl on others.
    pub cmd: bool,
    pub shift: bool,
    pub alt: bool,
    /// Always Ctrl (even on macOS).
    pub ctrl: bool,
    /// Lowercase key name: "c", "v", "f", "tab", "enter", etc.
    pub key: String,
}

impl KeyCombo {
    /// Parse a key combo string like `"cmd+shift+t"`, `"ctrl+c"`, `"cmd+alt+left"`.
    /// Returns `None` if the string is empty or has no key after modifiers.
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('+').collect();
        if parts.is_empty() {
            return None;
        }

        let mut cmd = false;
        let mut shift = false;
        let mut alt = false;
        let mut ctrl = false;

        // All segments except the last are modifier candidates.
        // The last segment is always the key.
        let (modifier_parts, key_parts) = parts.split_at(parts.len() - 1);
        let key = key_parts[0].trim().to_lowercase();
        if key.is_empty()
            || matches!(
                key.as_str(),
                "cmd" | "super" | "meta" | "shift" | "alt" | "option" | "ctrl" | "control"
            )
        {
            return None;
        }

        for part in modifier_parts {
            match part.trim().to_lowercase().as_str() {
                "cmd" | "super" | "meta" => cmd = true,
                "shift" => shift = true,
                "alt" | "option" => alt = true,
                "ctrl" | "control" => ctrl = true,
                _ => {}
            }
        }

        Some(KeyCombo {
            cmd,
            shift,
            alt,
            ctrl,
            key,
        })
    }
}

impl<'de> serde::Deserialize<'de> for KeyCombo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        KeyCombo::parse(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid key combo: {s:?}")))
    }
}

/// User-customizable keybindings. Each field is an optional override;
/// if None, the platform default is used.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct KeybindingsConfig {
    pub copy: Option<KeyCombo>,
    pub paste: Option<KeyCombo>,
    pub find: Option<KeyCombo>,
    pub select_all: Option<KeyCombo>,
    pub copy_mode: Option<KeyCombo>,
    pub toggle_sidebar: Option<KeyCombo>,
    pub new_browser_tab: Option<KeyCombo>,
    pub new_workspace: Option<KeyCombo>,
    pub new_tab: Option<KeyCombo>,
    pub next_workspace: Option<KeyCombo>,
    pub prev_workspace: Option<KeyCombo>,
    pub next_tab: Option<KeyCombo>,
    pub prev_tab: Option<KeyCombo>,
    pub split_right: Option<KeyCombo>,
    pub split_down: Option<KeyCombo>,
    pub close_pane: Option<KeyCombo>,
    pub navigate_left: Option<KeyCombo>,
    pub navigate_right: Option<KeyCombo>,
    pub navigate_up: Option<KeyCombo>,
    pub navigate_down: Option<KeyCombo>,
    pub zoom_toggle: Option<KeyCombo>,
    pub devtools: Option<KeyCombo>,
    pub notification_panel: Option<KeyCombo>,
    pub jump_to_unread: Option<KeyCombo>,
    pub clear_scrollback: Option<KeyCombo>,
}

impl KeybindingsConfig {
    /// Return resolved keybindings: user overrides merged with platform defaults.
    pub fn resolved(&self) -> HashMap<Action, KeyCombo> {
        let mut map = Self::platform_defaults();
        if let Some(k) = &self.copy {
            map.insert(Action::Copy, k.clone());
        }
        if let Some(k) = &self.paste {
            map.insert(Action::Paste, k.clone());
        }
        if let Some(k) = &self.find {
            map.insert(Action::Find, k.clone());
        }
        if let Some(k) = &self.select_all {
            map.insert(Action::SelectAll, k.clone());
        }
        if let Some(k) = &self.copy_mode {
            map.insert(Action::CopyMode, k.clone());
        }
        if let Some(k) = &self.toggle_sidebar {
            map.insert(Action::ToggleSidebar, k.clone());
        }
        if let Some(k) = &self.new_browser_tab {
            map.insert(Action::NewBrowserTab, k.clone());
        }
        if let Some(k) = &self.new_workspace {
            map.insert(Action::NewWorkspace, k.clone());
        }
        if let Some(k) = &self.new_tab {
            map.insert(Action::NewTab, k.clone());
        }
        if let Some(k) = &self.next_workspace {
            map.insert(Action::NextWorkspace, k.clone());
        }
        if let Some(k) = &self.prev_workspace {
            map.insert(Action::PrevWorkspace, k.clone());
        }
        if let Some(k) = &self.next_tab {
            map.insert(Action::NextTab, k.clone());
        }
        if let Some(k) = &self.prev_tab {
            map.insert(Action::PrevTab, k.clone());
        }
        if let Some(k) = &self.split_right {
            map.insert(Action::SplitRight, k.clone());
        }
        if let Some(k) = &self.split_down {
            map.insert(Action::SplitDown, k.clone());
        }
        if let Some(k) = &self.close_pane {
            map.insert(Action::ClosePane, k.clone());
        }
        if let Some(k) = &self.navigate_left {
            map.insert(Action::NavigateLeft, k.clone());
        }
        if let Some(k) = &self.navigate_right {
            map.insert(Action::NavigateRight, k.clone());
        }
        if let Some(k) = &self.navigate_up {
            map.insert(Action::NavigateUp, k.clone());
        }
        if let Some(k) = &self.navigate_down {
            map.insert(Action::NavigateDown, k.clone());
        }
        if let Some(k) = &self.zoom_toggle {
            map.insert(Action::ZoomToggle, k.clone());
        }
        if let Some(k) = &self.devtools {
            map.insert(Action::DevTools, k.clone());
        }
        if let Some(k) = &self.notification_panel {
            map.insert(Action::NotificationPanel, k.clone());
        }
        if let Some(k) = &self.jump_to_unread {
            map.insert(Action::JumpToUnread, k.clone());
        }
        if let Some(k) = &self.clear_scrollback {
            map.insert(Action::ClearScrollback, k.clone());
        }
        map
    }

    fn platform_defaults() -> HashMap<Action, KeyCombo> {
        let mut m = HashMap::new();
        #[cfg(target_os = "macos")]
        {
            m.insert(Action::Copy, KeyCombo::parse("cmd+c").unwrap());
            m.insert(Action::Paste, KeyCombo::parse("cmd+v").unwrap());
            m.insert(Action::Find, KeyCombo::parse("cmd+f").unwrap());
            m.insert(Action::SelectAll, KeyCombo::parse("cmd+a").unwrap());
            m.insert(Action::CopyMode, KeyCombo::parse("cmd+shift+x").unwrap());
            m.insert(Action::ToggleSidebar, KeyCombo::parse("cmd+b").unwrap());
            m.insert(
                Action::NewBrowserTab,
                KeyCombo::parse("cmd+shift+l").unwrap(),
            );
            m.insert(Action::NewWorkspace, KeyCombo::parse("cmd+n").unwrap());
            m.insert(Action::NewTab, KeyCombo::parse("cmd+t").unwrap());
            m.insert(
                Action::NextWorkspace,
                KeyCombo::parse("cmd+shift+]").unwrap(),
            );
            m.insert(
                Action::PrevWorkspace,
                KeyCombo::parse("cmd+shift+[").unwrap(),
            );
            m.insert(Action::NextTab, KeyCombo::parse("ctrl+tab").unwrap());
            m.insert(Action::PrevTab, KeyCombo::parse("ctrl+shift+tab").unwrap());
            m.insert(Action::SplitRight, KeyCombo::parse("cmd+d").unwrap());
            m.insert(Action::SplitDown, KeyCombo::parse("cmd+shift+d").unwrap());
            m.insert(Action::ClosePane, KeyCombo::parse("cmd+w").unwrap());
            m.insert(
                Action::NavigateLeft,
                KeyCombo::parse("cmd+alt+left").unwrap(),
            );
            m.insert(
                Action::NavigateRight,
                KeyCombo::parse("cmd+alt+right").unwrap(),
            );
            m.insert(Action::NavigateUp, KeyCombo::parse("cmd+alt+up").unwrap());
            m.insert(
                Action::NavigateDown,
                KeyCombo::parse("cmd+alt+down").unwrap(),
            );
            m.insert(
                Action::ZoomToggle,
                KeyCombo::parse("cmd+shift+enter").unwrap(),
            );
            m.insert(Action::DevTools, KeyCombo::parse("cmd+alt+i").unwrap());
            m.insert(Action::NotificationPanel, KeyCombo::parse("cmd+i").unwrap());
            m.insert(
                Action::JumpToUnread,
                KeyCombo::parse("cmd+shift+u").unwrap(),
            );
            m.insert(Action::ClearScrollback, KeyCombo::parse("cmd+k").unwrap());
        }
        #[cfg(not(target_os = "macos"))]
        {
            m.insert(Action::Copy, KeyCombo::parse("ctrl+shift+c").unwrap());
            m.insert(Action::Paste, KeyCombo::parse("ctrl+shift+v").unwrap());
            m.insert(Action::Find, KeyCombo::parse("ctrl+f").unwrap());
            m.insert(Action::SelectAll, KeyCombo::parse("ctrl+shift+a").unwrap());
            m.insert(Action::CopyMode, KeyCombo::parse("ctrl+shift+x").unwrap());
            m.insert(Action::ToggleSidebar, KeyCombo::parse("ctrl+b").unwrap());
            m.insert(
                Action::NewBrowserTab,
                KeyCombo::parse("ctrl+shift+l").unwrap(),
            );
            m.insert(
                Action::NewWorkspace,
                KeyCombo::parse("ctrl+shift+n").unwrap(),
            );
            m.insert(Action::NewTab, KeyCombo::parse("ctrl+shift+t").unwrap());
            m.insert(
                Action::NextWorkspace,
                KeyCombo::parse("ctrl+shift+]").unwrap(),
            );
            m.insert(
                Action::PrevWorkspace,
                KeyCombo::parse("ctrl+shift+[").unwrap(),
            );
            m.insert(Action::NextTab, KeyCombo::parse("ctrl+tab").unwrap());
            m.insert(Action::PrevTab, KeyCombo::parse("ctrl+shift+tab").unwrap());
            m.insert(Action::SplitRight, KeyCombo::parse("ctrl+d").unwrap());
            m.insert(Action::SplitDown, KeyCombo::parse("ctrl+shift+d").unwrap());
            m.insert(Action::ClosePane, KeyCombo::parse("ctrl+w").unwrap());
            m.insert(
                Action::NavigateLeft,
                KeyCombo::parse("ctrl+alt+left").unwrap(),
            );
            m.insert(
                Action::NavigateRight,
                KeyCombo::parse("ctrl+alt+right").unwrap(),
            );
            m.insert(Action::NavigateUp, KeyCombo::parse("ctrl+alt+up").unwrap());
            m.insert(
                Action::NavigateDown,
                KeyCombo::parse("ctrl+alt+down").unwrap(),
            );
            m.insert(
                Action::ZoomToggle,
                KeyCombo::parse("ctrl+shift+enter").unwrap(),
            );
            m.insert(Action::DevTools, KeyCombo::parse("ctrl+shift+i").unwrap());
            m.insert(
                Action::NotificationPanel,
                KeyCombo::parse("ctrl+i").unwrap(),
            );
            m.insert(
                Action::JumpToUnread,
                KeyCombo::parse("ctrl+shift+u").unwrap(),
            );
            m.insert(
                Action::ClearScrollback,
                KeyCombo::parse("ctrl+shift+k").unwrap(),
            );
        }
        m
    }
}

/// Style of the in-window menu bar on Windows/Linux, and of the
/// (optional) in-window menu chrome on macOS when overriding defaults.
///
/// macOS always retains the NSApp native menu bar at the top of the
/// screen regardless of this setting — it's the idiomatic macOS
/// pattern and removing it would be actively hostile to Mac users.
/// This setting only controls the *in-window* menu presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuBarStyle {
    /// Traditional two-strip layout: a dedicated `File Edit View`
    /// strip above the icon/tab row. Costs ~24px of vertical chrome.
    /// Default on Windows and Linux.
    Menubar,
    /// Single `≡` button collapsed into the existing icon row.
    /// Clicking it opens a nested popup with every submenu. Zero
    /// extra vertical chrome — the most space-efficient option.
    Hamburger,
    /// No in-window menu chrome at all. Users reach menu actions via
    /// keyboard shortcuts only (and the native NSApp menu bar on
    /// macOS). Default on macOS because the NSApp menu bar already
    /// provides full menu access.
    None,
}

// Clippy can't see past the `cfg` — it thinks this impl is just
// "default to None" and suggests `#[derive(Default)]` on the enum.
// Keep the manual impl because the default is genuinely platform-
// specific.
#[allow(clippy::derivable_impls)]
impl Default for MenuBarStyle {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::None
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::Menubar
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub font_size: f32,
    /// Font family for terminal text (e.g. "JetBrains Mono", "Menlo").
    /// Resolved against system-installed fonts by cosmic-text.
    pub font_family: String,
    /// Theme source. `"default"` uses amux's built-in dark palette
    /// (Monokai Classic — the same palette cmux ships as its
    /// default; see `crates/amux-app/src/theme.rs` for the exact
    /// values); `"ghostty"` loads colors and fonts from Ghostty's
    /// config file at `~/.config/ghostty/config` (or the
    /// platform-appropriate equivalent). Individual colors can
    /// still be overridden in the `[colors]` section on top of
    /// either source.
    pub theme_source: String,
    /// Shell to spawn in new panes. Accepts either a plain binary name
    /// (`"pwsh"`, `"bash"`) that amux resolves against `PATH`, or an
    /// absolute path (`"/opt/homebrew/bin/fish"`, `"C:\\Program Files\\PowerShell\\7\\pwsh.exe"`).
    /// When unset (the default), amux uses `$SHELL` on Unix and prefers
    /// `pwsh.exe` on Windows if it's on `PATH`, otherwise `$COMSPEC`.
    pub shell: Option<String>,
    /// In-window menu bar style. See [`MenuBarStyle`] for the full
    /// list of values and per-platform behavior. Changing this
    /// requires a restart to take effect.
    #[serde(default)]
    pub menu_bar_style: MenuBarStyle,
    pub notifications: NotificationConfig,
    pub browser: BrowserConfig,
    #[serde(default)]
    pub colors: ColorsConfig,
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            font_size: DEFAULT_FONT_SIZE,
            font_family: DEFAULT_FONT_FAMILY.to_owned(),
            theme_source: "default".to_owned(),
            shell: None,
            menu_bar_style: MenuBarStyle::default(),
            notifications: NotificationConfig::default(),
            browser: BrowserConfig::default(),
            colors: ColorsConfig::default(),
            keybindings: KeybindingsConfig::default(),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(default)]
pub struct BrowserConfig {
    /// Search engine: "google", "duckduckgo", "bing", "kagi", "startpage"
    pub search_engine: String,
    /// Open terminal hyperlinks in an in-app browser pane instead of system browser.
    pub open_terminal_links_in_app: bool,
    /// Custom user agent string. When set, overrides the default webview UA.
    pub user_agent: Option<String>,
    /// Download directory. Defaults to the system Downloads folder.
    pub download_dir: Option<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            search_engine: "google".to_string(),
            open_terminal_links_in_app: true,
            user_agent: None,
            download_dir: None,
        }
    }
}

/// Build a search URL from a query string and search engine name.
pub fn search_url(query: &str, engine: &str) -> String {
    let encoded = urlencoding::encode(query);
    match engine {
        "duckduckgo" => format!("https://duckduckgo.com/?q={encoded}"),
        "bing" => format!("https://www.bing.com/search?q={encoded}"),
        "kagi" => format!("https://kagi.com/search?q={encoded}"),
        "startpage" => format!("https://www.startpage.com/sp/search?query={encoded}"),
        _ => format!("https://www.google.com/search?q={encoded}"),
    }
}

/// Determine if input looks like a URL (has a dot, no spaces) or a search query.
pub fn is_url_like(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Reject any whitespace (spaces, tabs, newlines)
    if trimmed.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    // Already has a scheme
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("file://")
    {
        return true;
    }
    // localhost or 127.0.0.1 with optional port/path
    if let Some(rest) = trimmed
        .strip_prefix("localhost")
        .or_else(|| trimmed.strip_prefix("127.0.0.1"))
    {
        return rest.is_empty() || rest.starts_with(':') || rest.starts_with('/');
    }
    // Has a dot → likely a domain
    trimmed.contains('.')
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_bar_style_deserializes_all_variants() {
        // Round-trip each snake_case variant name through TOML so
        // a future rename of one of the enum variants doesn't
        // silently break config files in the wild.
        for (snake, expected) in [
            ("menubar", MenuBarStyle::Menubar),
            ("hamburger", MenuBarStyle::Hamburger),
            ("none", MenuBarStyle::None),
        ] {
            let toml_src = format!("menu_bar_style = \"{snake}\"");
            let parsed: AppConfig =
                toml::from_str(&toml_src).unwrap_or_else(|e| panic!("{snake}: {e}"));
            assert_eq!(parsed.menu_bar_style, expected, "variant {snake}");
        }
    }

    #[test]
    fn menu_bar_style_default_is_platform_appropriate() {
        let default = MenuBarStyle::default();
        #[cfg(target_os = "macos")]
        assert_eq!(default, MenuBarStyle::None);
        #[cfg(not(target_os = "macos"))]
        assert_eq!(default, MenuBarStyle::Menubar);
    }

    #[test]
    fn menu_bar_style_absent_uses_default() {
        // `#[serde(default)]` on the field means an AppConfig
        // parsed from an empty TOML document still round-trips to
        // the platform default.
        let parsed: AppConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.menu_bar_style, MenuBarStyle::default());
    }

    #[test]
    fn is_url_like_schemes() {
        assert!(is_url_like("http://example.com"));
        assert!(is_url_like("https://example.com"));
        assert!(is_url_like("file:///tmp/test.html"));
    }

    #[test]
    fn is_url_like_domains() {
        assert!(is_url_like("example.com"));
        assert!(is_url_like("docs.rs"));
    }

    #[test]
    fn is_url_like_localhost() {
        assert!(is_url_like("localhost:3000"));
        assert!(is_url_like("localhost:8080/api"));
        assert!(is_url_like("127.0.0.1:9090"));
    }

    #[test]
    fn is_url_like_rejects_search() {
        assert!(!is_url_like("how to write rust"));
        assert!(!is_url_like("hello world"));
        assert!(!is_url_like(""));
        assert!(!is_url_like("   "));
    }

    #[test]
    fn is_url_like_rejects_whitespace() {
        assert!(!is_url_like("example\t.com"));
        assert!(!is_url_like("example\n.com"));
        assert!(!is_url_like("hello\tworld"));
    }

    #[test]
    fn parse_hex_with_hash() {
        assert_eq!(ColorsConfig::parse_hex("#ff0000"), Some([255, 0, 0]));
    }

    #[test]
    fn parse_hex_without_hash() {
        assert_eq!(ColorsConfig::parse_hex("00ff00"), Some([0, 255, 0]));
    }

    #[test]
    fn parse_hex_invalid() {
        assert_eq!(ColorsConfig::parse_hex("xyz"), None);
        assert_eq!(ColorsConfig::parse_hex("#ff"), None);
    }

    #[test]
    fn key_combo_parse_simple() {
        let combo = KeyCombo::parse("cmd+c").unwrap();
        assert!(combo.cmd);
        assert!(!combo.shift);
        assert!(!combo.alt);
        assert!(!combo.ctrl);
        assert_eq!(combo.key, "c");
    }

    #[test]
    fn key_combo_parse_multi_modifier() {
        let combo = KeyCombo::parse("cmd+shift+t").unwrap();
        assert!(combo.cmd);
        assert!(combo.shift);
        assert!(!combo.alt);
        assert!(!combo.ctrl);
        assert_eq!(combo.key, "t");
    }

    #[test]
    fn key_combo_parse_ctrl_only() {
        let combo = KeyCombo::parse("ctrl+tab").unwrap();
        assert!(!combo.cmd);
        assert!(!combo.shift);
        assert!(!combo.alt);
        assert!(combo.ctrl);
        assert_eq!(combo.key, "tab");
    }

    #[test]
    fn key_combo_parse_special_keys() {
        let combo = KeyCombo::parse("cmd+alt+left").unwrap();
        assert!(combo.cmd);
        assert!(!combo.shift);
        assert!(combo.alt);
        assert!(!combo.ctrl);
        assert_eq!(combo.key, "left");
    }

    #[test]
    fn key_combo_parse_invalid_empty() {
        assert!(KeyCombo::parse("").is_none());
    }

    #[test]
    fn key_combo_parse_bracket_keys() {
        let combo = KeyCombo::parse("cmd+shift+]").unwrap();
        assert!(combo.cmd);
        assert!(combo.shift);
        assert_eq!(combo.key, "]");
    }

    #[test]
    fn key_combo_serde_roundtrip() {
        // Verify deserialization via serde works
        let toml_str = r#"copy = "cmd+c""#;
        let cfg: KeybindingsConfig = toml::from_str(toml_str).unwrap();
        let combo = cfg.copy.unwrap();
        assert!(combo.cmd);
        assert_eq!(combo.key, "c");
    }

    #[test]
    fn keybindings_resolved_has_all_defaults() {
        let cfg = KeybindingsConfig::default();
        let resolved = cfg.resolved();
        // On any platform, Copy should have a default.
        assert!(resolved.contains_key(&Action::Copy));
        assert!(resolved.contains_key(&Action::Paste));
        assert!(resolved.contains_key(&Action::NewTab));
    }

    #[test]
    fn keybindings_resolved_user_override() {
        let toml_str = r#"copy = "ctrl+shift+y""#;
        let cfg: KeybindingsConfig = toml::from_str(toml_str).unwrap();
        let resolved = cfg.resolved();
        let copy = resolved.get(&Action::Copy).unwrap();
        assert!(copy.ctrl);
        assert!(copy.shift);
        assert_eq!(copy.key, "y");
    }
}
