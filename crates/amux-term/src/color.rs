use wezterm_term::color::{ColorAttribute, ColorPalette, SrgbaTuple};

/// Resolve a `ColorAttribute` to a concrete RGBA color tuple.
///
/// Handles Default, PaletteIndex, TrueColor variants. When `reverse` is true,
/// the fg/bg roles are swapped before resolving.
pub fn resolve_color(
    color: &ColorAttribute,
    palette: &ColorPalette,
    is_fg: bool,
    reverse: bool,
) -> SrgbaTuple {
    // When reverse video is active, swap fg/bg resolution
    let effective_is_fg = if reverse { !is_fg } else { is_fg };

    if effective_is_fg {
        palette.resolve_fg(*color)
    } else {
        palette.resolve_bg(*color)
    }
}

/// Convert an `SrgbaTuple` to `[f32; 4]` (identity — for GPU uniform buffers).
pub fn srgba_to_f32(color: SrgbaTuple) -> [f32; 4] {
    [color.0, color.1, color.2, color.3]
}

/// Convert an `SrgbaTuple` (f32 components 0.0–1.0) to 8-bit RGBA.
pub fn srgba_to_rgba8(color: SrgbaTuple) -> [u8; 4] {
    [
        (color.0 * 255.0).round() as u8,
        (color.1 * 255.0).round() as u8,
        (color.2 * 255.0).round() as u8,
        (color.3 * 255.0).round() as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_default_fg() {
        let palette = ColorPalette::default();
        let color = ColorAttribute::Default;
        let resolved = resolve_color(&color, &palette, true, false);
        // Default fg should equal palette.foreground
        assert_eq!(resolved, palette.foreground);
    }

    #[test]
    fn resolve_default_bg() {
        let palette = ColorPalette::default();
        let color = ColorAttribute::Default;
        let resolved = resolve_color(&color, &palette, false, false);
        assert_eq!(resolved, palette.background);
    }

    #[test]
    fn resolve_reverse_swaps_fg_bg() {
        let palette = ColorPalette::default();
        let color = ColorAttribute::Default;
        // With reverse=true, asking for fg should give bg color
        let resolved = resolve_color(&color, &palette, true, true);
        assert_eq!(resolved, palette.background);
    }

    #[test]
    fn srgba_to_rgba8_white() {
        let white = SrgbaTuple(1.0, 1.0, 1.0, 1.0);
        assert_eq!(srgba_to_rgba8(white), [255, 255, 255, 255]);
    }

    #[test]
    fn srgba_to_rgba8_black() {
        let black = SrgbaTuple(0.0, 0.0, 0.0, 1.0);
        assert_eq!(srgba_to_rgba8(black), [0, 0, 0, 255]);
    }
}
