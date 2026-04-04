//! Parser for Ghostty's `key = value` configuration format.
//!
//! Loads `~/.config/ghostty/config` (or the macOS-specific path) and extracts
//! color palette, font family, and font size settings that amux can use to
//! match the user's Ghostty theme.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Parsed Ghostty configuration relevant to amux theming.
#[derive(Debug, Clone)]
pub struct GhosttyConfig {
    /// Raw key-value pairs from the config file (last value wins).
    entries: HashMap<String, String>,
    /// Palette overrides: index (0-255) → `[r, g, b]`.
    palette: HashMap<u8, [u8; 3]>,
}

impl GhosttyConfig {
    /// Load Ghostty config from the standard search paths.
    ///
    /// Search order:
    /// 1. `~/.config/ghostty/config`
    /// 2. macOS: `~/Library/Application Support/com.mitchellh.ghostty/config`
    pub fn load() -> Option<Self> {
        for path in config_search_paths() {
            if let Some(cfg) = Self::load_from(&path) {
                return Some(cfg);
            }
        }
        tracing::debug!("No Ghostty config found");
        None
    }

    /// Load from a specific file path.
    pub fn load_from(path: &Path) -> Option<Self> {
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("Failed to read Ghostty config at {}: {}", path.display(), e);
                }
                return None;
            }
        };
        tracing::info!("Loading Ghostty config from {}", path.display());
        let mut cfg = Self::parse(&contents);

        // If a theme is referenced, load it and let the main config override.
        if let Some(theme_name) = cfg.entries.get("theme").cloned() {
            if let Some(theme_cfg) = load_theme(&theme_name, path.parent()) {
                // Merge: theme provides defaults, main config overrides.
                let mut merged = theme_cfg;
                for (k, v) in cfg.entries {
                    merged.entries.insert(k, v);
                }
                for (idx, color) in cfg.palette {
                    merged.palette.insert(idx, color);
                }
                cfg = merged;
            }
        }

        Some(cfg)
    }

    /// Parse config text in Ghostty's `key = value` format.
    pub fn parse(text: &str) -> Self {
        let mut entries = HashMap::new();
        let mut palette = HashMap::new();

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Split on first '=' only.
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            // Strip surrounding quotes (single or double).
            let value = if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };

            if key == "palette" {
                // palette = INDEX=COLOR  (e.g., "palette = 0=#1a1b26")
                if let Some((idx_str, color_str)) = value.split_once('=') {
                    if let Ok(idx) = idx_str.trim().parse::<u8>() {
                        if let Some(rgb) = parse_hex_color(color_str.trim()) {
                            palette.insert(idx, rgb);
                        }
                    }
                }
            } else {
                entries.insert(key.to_string(), value.to_string());
            }
        }

        Self { entries, palette }
    }

    // ── Accessors ──

    pub fn background(&self) -> Option<[u8; 3]> {
        self.get_color("background")
    }

    pub fn foreground(&self) -> Option<[u8; 3]> {
        self.get_color("foreground")
    }

    pub fn cursor_color(&self) -> Option<[u8; 3]> {
        self.get_color("cursor-color")
    }

    pub fn cursor_text(&self) -> Option<[u8; 3]> {
        self.get_color("cursor-text")
    }

    pub fn selection_background(&self) -> Option<[u8; 3]> {
        self.get_color("selection-background")
    }

    pub fn selection_foreground(&self) -> Option<[u8; 3]> {
        self.get_color("selection-foreground")
    }

    /// Get an ANSI palette color (0-15 from config, or overridden by `palette = N=COLOR`).
    pub fn ansi_color(&self, index: u8) -> Option<[u8; 3]> {
        // Explicit palette override takes priority.
        if let Some(&color) = self.palette.get(&index) {
            return Some(color);
        }
        // Ghostty uses palette = N=COLOR for all 256 colors, but also has
        // shorthand keys for the first 16: palette = 0=#... through palette = 15=#...
        // Those are already handled above. No separate "color0" key in Ghostty.
        None
    }

    /// All palette overrides (0-255).
    pub fn palette_overrides(&self) -> &HashMap<u8, [u8; 3]> {
        &self.palette
    }

    pub fn font_family(&self) -> Option<&str> {
        self.entries.get("font-family").map(|s| s.as_str())
    }

    pub fn font_size(&self) -> Option<f32> {
        self.entries
            .get("font-size")
            .and_then(|s| s.parse::<f32>().ok())
    }

    /// Get a raw config value by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|s| s.as_str())
    }

    // ── Helpers ──

    fn get_color(&self, key: &str) -> Option<[u8; 3]> {
        self.entries.get(key).and_then(|v| parse_hex_color(v))
    }
}

/// Parse a hex color string: `#RRGGBB`, `RRGGBB`, or `#RGB`.
fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    match hex.len() {
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some([r, g, b])
        }
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            Some([r, g, b])
        }
        _ => None,
    }
}

/// Standard config file search paths.
fn config_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // XDG_CONFIG_HOME / Linux / cross-platform (respects $XDG_CONFIG_HOME).
    if let Some(config_dir) = dirs::config_dir() {
        paths.push(config_dir.join("ghostty").join("config"));
    }

    // macOS: ~/Library/Application Support/com.mitchellh.ghostty/config
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        paths.push(
            home.join("Library")
                .join("Application Support")
                .join("com.mitchellh.ghostty")
                .join("config"),
        );
    }

    // Windows: %APPDATA%\ghostty\config
    #[cfg(target_os = "windows")]
    if let Some(appdata) = dirs::config_dir() {
        paths.push(appdata.join("ghostty").join("config"));
    }

    paths
}

/// Load a named theme file. Search order:
/// 1. `~/.config/ghostty/themes/<name>`
/// 2. Next to the config file: `<config_dir>/themes/<name>`
fn load_theme(name: &str, config_dir: Option<&Path>) -> Option<GhosttyConfig> {
    // Sanitize theme name (no path traversal).
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        tracing::warn!("Ignoring suspicious theme name: {}", name);
        return None;
    }

    let mut search = Vec::new();

    if let Some(config_dir) = dirs::config_dir() {
        search.push(config_dir.join("ghostty").join("themes").join(name));
    }
    if let Some(dir) = config_dir {
        search.push(dir.join("themes").join(name));
    }

    // macOS themes directory
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        search.push(
            home.join("Library")
                .join("Application Support")
                .join("com.mitchellh.ghostty")
                .join("themes")
                .join(name),
        );
    }

    for path in search {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            tracing::info!("Loaded Ghostty theme '{}' from {}", name, path.display());
            return Some(GhosttyConfig::parse(&contents));
        }
    }

    tracing::warn!("Ghostty theme '{}' not found", name);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_config() {
        let text = r#"
# Tokyo Night theme
background = #1a1b26
foreground = #c0caf5
font-family = JetBrains Mono
font-size = 14

# Palette overrides
palette = 0=#15161e
palette = 1=#f7768e
"#;
        let cfg = GhosttyConfig::parse(text);
        assert_eq!(cfg.background(), Some([0x1a, 0x1b, 0x26]));
        assert_eq!(cfg.foreground(), Some([0xc0, 0xca, 0xf5]));
        assert_eq!(cfg.font_family(), Some("JetBrains Mono"));
        assert_eq!(cfg.font_size(), Some(14.0));
        assert_eq!(cfg.ansi_color(0), Some([0x15, 0x16, 0x1e]));
        assert_eq!(cfg.ansi_color(1), Some([0xf7, 0x76, 0x8e]));
        assert_eq!(cfg.ansi_color(2), None);
    }

    #[test]
    fn parse_hex_variants() {
        assert_eq!(parse_hex_color("#1a1b26"), Some([0x1a, 0x1b, 0x26]));
        assert_eq!(parse_hex_color("1a1b26"), Some([0x1a, 0x1b, 0x26]));
        assert_eq!(parse_hex_color("#abc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(parse_hex_color("xyz"), None);
        assert_eq!(parse_hex_color(""), None);
    }

    #[test]
    fn parse_empty_and_comments() {
        let cfg = GhosttyConfig::parse("# just a comment\n\n");
        assert_eq!(cfg.background(), None);
        assert_eq!(cfg.font_family(), None);
    }

    #[test]
    fn last_value_wins() {
        let text = "background = #000000\nbackground = #ffffff\n";
        let cfg = GhosttyConfig::parse(text);
        assert_eq!(cfg.background(), Some([0xff, 0xff, 0xff]));
    }
}
