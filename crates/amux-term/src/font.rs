/// Bundled font bytes (IBM Plex, SIL OFL license). Single source of truth —
/// all crates reference these statics instead of their own `include_bytes!`.
pub static MONO_REGULAR: &[u8] = include_bytes!("../fonts/IBMPlexMono-Regular.ttf");
pub static MONO_BOLD: &[u8] = include_bytes!("../fonts/IBMPlexMono-Bold.ttf");
pub static SANS_REGULAR: &[u8] = include_bytes!("../fonts/IBMPlexSans-Regular.ttf");
pub static SANS_SEMIBOLD: &[u8] = include_bytes!("../fonts/IBMPlexSans-SemiBold.ttf");

pub const DEFAULT_FONT_FAMILY: &str = "IBM Plex Mono";
pub const DEFAULT_FONT_SIZE: f32 = 13.0;

/// Font configuration — just a name and a size (cmux/Ghostty pattern).
/// Renderers resolve the family name against their own font database.
#[derive(Clone, Debug)]
pub struct FontConfig {
    /// Font family name (e.g. "IBM Plex Mono", "JetBrains Mono").
    /// Resolved by cosmic-text's fontdb at shaping time.
    pub family: String,
    /// Font size in logical points.
    pub size: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: DEFAULT_FONT_FAMILY.to_owned(),
            size: DEFAULT_FONT_SIZE,
        }
    }
}
