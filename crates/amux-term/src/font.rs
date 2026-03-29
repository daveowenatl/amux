use cosmic_text::fontdb::Family;
use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping};

/// Bundled font bytes (IBM Plex, SIL OFL license). Single source of truth —
/// all crates reference these statics instead of their own `include_bytes!`.
pub static MONO_REGULAR: &[u8] = include_bytes!("../fonts/IBMPlexMono-Regular.ttf");
pub static MONO_BOLD: &[u8] = include_bytes!("../fonts/IBMPlexMono-Bold.ttf");
pub static MONO_ITALIC: &[u8] = include_bytes!("../fonts/IBMPlexMono-Italic.ttf");
pub static MONO_BOLD_ITALIC: &[u8] = include_bytes!("../fonts/IBMPlexMono-BoldItalic.ttf");
pub static SANS_REGULAR: &[u8] = include_bytes!("../fonts/IBMPlexSans-Regular.ttf");
pub static SANS_SEMIBOLD: &[u8] = include_bytes!("../fonts/IBMPlexSans-SemiBold.ttf");

pub const DEFAULT_FONT_FAMILY: &str = "IBM Plex Mono";
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

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

/// Create a `FontSystem` with bundled fonts loaded and the monospace family
/// set according to `config`. This is the single initialization point used
/// by both the soft and GPU renderers.
pub fn create_font_system(config: &FontConfig) -> FontSystem {
    let t0 = std::time::Instant::now();
    let mut font_system = FontSystem::new();
    tracing::info!(
        "FontSystem::new() loaded {} fonts in {:.0?}",
        font_system.db().len(),
        t0.elapsed()
    );

    font_system.db_mut().load_font_data(MONO_REGULAR.to_vec());
    font_system.db_mut().load_font_data(MONO_BOLD.to_vec());
    font_system.db_mut().load_font_data(MONO_ITALIC.to_vec());
    font_system
        .db_mut()
        .load_font_data(MONO_BOLD_ITALIC.to_vec());

    font_system
        .db_mut()
        .set_monospace_family(DEFAULT_FONT_FAMILY);

    // Override with the user-configured family if available.
    let family = &config.family;
    if family != DEFAULT_FONT_FAMILY {
        let has_family = font_system.db().faces().any(|f| {
            f.families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(family))
        });
        if has_family {
            font_system.db_mut().set_monospace_family(family);
        } else {
            tracing::warn!(
                "Font family '{}' not found, falling back to {}",
                family,
                DEFAULT_FONT_FAMILY,
            );
        }
    }

    font_system
}

/// Measure the width of a single monospace cell by laying out "M" with
/// cosmic-text and reading the glyph advance. Falls back to `font_size * 0.6`.
pub fn measure_cell_width(font_system: &mut FontSystem, metrics: Metrics) -> f32 {
    let mut buffer = Buffer::new_empty(metrics);
    {
        let mut borrowed = buffer.borrow_with(font_system);
        borrowed.set_size(Some(200.0), Some(metrics.line_height));
        borrowed.set_text(
            "M",
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
        borrowed.shape_until_scroll(true);
    }

    for run in buffer.layout_runs() {
        if let Some(glyph) = run.glyphs.iter().next() {
            return glyph.w;
        }
    }

    // Fallback: estimate from font size
    metrics.font_size * 0.6
}
