//! Rasterizers for underline decoration tiles (curly, dotted).
//!
//! Each function returns `(width, height, pixel_data)` as a grayscale R8 bitmap
//! that is uploaded to the atlas and tiled across decorated cells.

/// Rasterize a curly (wavy) underline tile into a grayscale bitmap.
/// Uses Ghostty-style approach: cubic Bézier-approximated sine wave.
/// Returns (width, height, pixel_data).
pub(crate) fn rasterize_curly_tile(cell_width: f32, thickness: f32) -> (u32, u32, Vec<u8>) {
    let w = cell_width.ceil() as u32;
    // Ghostty uses amplitude = width/π with Bézier curvature 0.4.
    // Since we use a sine wave (which hits full amplitude), scale down
    // to match the visual height of Ghostty's Bézier wave.
    let amplitude = (cell_width / std::f32::consts::PI * 0.4).max(thickness);
    let h = (amplitude * 2.0 + thickness * 2.0).ceil() as u32;
    let mut pixels = vec![0u8; (w * h) as usize];

    let center_y = h as f32 / 2.0;
    let half_t = thickness / 2.0;

    // Sample the sine wave densely and paint thick anti-aliased strokes.
    let steps = w * 4; // 4 sub-pixel samples per pixel column
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = t * (w as f32 - 1.0);
        let y = center_y + (t * std::f32::consts::TAU).sin() * amplitude;

        // Paint a filled circle at each sample point for smooth coverage.
        let radius = half_t + 0.5; // slight padding for AA
        let x_min = (x - radius).floor().max(0.0) as u32;
        let x_max = ((x + radius).ceil() as u32).min(w - 1);
        let y_min = (y - radius).floor().max(0.0) as u32;
        let y_max = ((y + radius).ceil() as u32).min(h - 1);

        for py in y_min..=y_max {
            for px in x_min..=x_max {
                let dx = px as f32 - x;
                let dy = py as f32 - y;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist <= half_t + 0.5 {
                    let alpha = if dist <= half_t {
                        255
                    } else {
                        ((1.0 - (dist - half_t)) * 255.0) as u8
                    };
                    let idx = (py * w + px) as usize;
                    pixels[idx] = pixels[idx].max(alpha);
                }
            }
        }
    }

    (w, h, pixels)
}

/// Rasterize a dotted underline tile: a row of circles across one cell width.
/// Returns (width, height, pixel_data).
pub(crate) fn rasterize_dotted_tile(cell_width: f32, thickness: f32) -> (u32, u32, Vec<u8>) {
    let w = cell_width.ceil() as u32;
    let radius = (thickness * std::f32::consts::SQRT_2 / 2.0).max(1.0);
    let h = (radius * 2.0 + 2.0).ceil() as u32;
    let mut pixels = vec![0u8; (w * h) as usize];

    let center_y = h as f32 / 2.0;

    // Dynamic dot count (Ghostty approach)
    let dot_count = ((cell_width / (4.0 * radius)).ceil() as u32)
        .min((cell_width / (3.0 * radius)).floor() as u32)
        .max(1);
    let spacing = cell_width / dot_count as f32;

    for d in 0..dot_count {
        let cx = spacing * (d as f32 + 0.5);

        let x_min = (cx - radius - 0.5).floor().max(0.0) as u32;
        let x_max = ((cx + radius + 0.5).ceil() as u32).min(w - 1);
        let y_min = (center_y - radius - 0.5).floor().max(0.0) as u32;
        let y_max = ((center_y + radius + 0.5).ceil() as u32).min(h - 1);

        for py in y_min..=y_max {
            for px in x_min..=x_max {
                let dx = px as f32 - cx;
                let dy = py as f32 - center_y;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist <= radius + 0.5 {
                    let alpha = if dist <= radius {
                        255
                    } else {
                        ((1.0 - (dist - radius)) * 255.0) as u8
                    };
                    let idx = (py * w + px) as usize;
                    pixels[idx] = pixels[idx].max(alpha);
                }
            }
        }
    }

    (w, h, pixels)
}
