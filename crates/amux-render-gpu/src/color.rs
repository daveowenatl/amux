//! sRGB colour conversion helpers used by the GPU render callback.

/// Convert an sRGB color to linear if the target is sRGB, otherwise pass through.
pub(crate) fn maybe_linearize(color: [f32; 4], target_is_srgb: bool) -> [f32; 4] {
    if target_is_srgb {
        [
            srgb_to_linear(color[0]),
            srgb_to_linear(color[1]),
            srgb_to_linear(color[2]),
            color[3],
        ]
    } else {
        color
    }
}

pub(crate) fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
