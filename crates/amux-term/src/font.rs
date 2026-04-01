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

/// Font decoration metrics extracted from OpenType tables (POST/OS2).
/// All values are in logical points at the current font size. Callers typically
/// convert these to physical pixels by multiplying with `pixels_per_point`.
#[derive(Clone, Copy, Debug)]
pub struct DecorationMetrics {
    /// Distance from baseline to top of underline stroke (positive = below baseline),
    /// in logical points.
    pub underline_offset: f32,
    /// Distance from baseline to top of strikethrough stroke (positive = above baseline),
    /// in logical points.
    pub strikeout_offset: f32,
    /// Recommended stroke thickness for underline/strikethrough, in logical points.
    pub stroke_size: f32,
}

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

/// Extract decoration metrics (underline/strikethrough position and thickness)
/// from the configured monospace font via ttf_parser's OpenType table reader.
/// Returns values in logical points at the given font size.
pub fn measure_decoration_metrics(
    font_system: &mut FontSystem,
    font_size: f32,
) -> DecorationMetrics {
    use cosmic_text::ttf_parser;

    // Query the monospace face that matches the configured monospace family
    // (set via `set_monospace_family`), rather than picking an arbitrary
    // monospaced face from the database.
    let mono_id = font_system.db().query(&cosmic_text::fontdb::Query {
        families: &[Family::Monospace],
        ..Default::default()
    });

    if let Some(id) = mono_id {
        let result = font_system.db().with_face_data(id, |data, index| {
            if let Ok(face) = ttf_parser::Face::parse(data, index) {
                let upem = face.units_per_em() as f32;
                let scale = font_size / upem;

                // Underline: from POST table (negative means below baseline)
                let ul_pos = face.underline_metrics().map(|m| m.position as f32 * scale);
                let ul_thick = face.underline_metrics().map(|m| m.thickness as f32 * scale);

                // Strikethrough: from OS/2 table
                let st_pos = face.strikeout_metrics().map(|m| m.position as f32 * scale);
                let st_thick = face.strikeout_metrics().map(|m| m.thickness as f32 * scale);

                let stroke = ul_thick.or(st_thick).unwrap_or(font_size * 0.07).max(1.0);

                return DecorationMetrics {
                    // Negate: font gives negative for below baseline, we want positive offset
                    underline_offset: -(ul_pos.unwrap_or(-font_size * 0.15)),
                    strikeout_offset: st_pos.unwrap_or(font_size * 0.3),
                    stroke_size: stroke,
                };
            }
            // Parse failed — fallback
            DecorationMetrics {
                underline_offset: font_size * 0.15,
                strikeout_offset: font_size * 0.3,
                stroke_size: (font_size / 14.0).max(1.0),
            }
        });

        if let Some(dm) = result {
            return dm;
        }
    }

    // Fallback: approximate from font size
    DecorationMetrics {
        underline_offset: font_size * 0.15,
        strikeout_offset: font_size * 0.3,
        stroke_size: (font_size / 14.0).max(1.0),
    }
}
