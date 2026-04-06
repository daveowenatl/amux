//! Flash animation easing curves and keyframe interpolation.
//!
//! Implements the double-pulse flash pattern matching cmux's notification
//! ring animation. `flash_alpha(t)` returns the opacity at time `t` seconds.

/// Total duration of the double-pulse flash animation.
pub const FLASH_DURATION: f32 = 0.9;

/// Keyframe times as fractions of FLASH_DURATION.
const FLASH_KEY_TIMES: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// Opacity values at each keyframe.
const FLASH_VALUES: [f32; 5] = [0.0, 1.0, 0.0, 1.0, 0.0];

/// Compute flash opacity for the double-pulse pattern at time `t` seconds.
/// Returns 0.0 when `t >= FLASH_DURATION`.
pub fn flash_alpha(t: f32) -> f32 {
    if !(0.0..FLASH_DURATION).contains(&t) {
        return 0.0;
    }
    let frac = t / FLASH_DURATION;
    // Find which segment we're in
    for i in 0..FLASH_KEY_TIMES.len() - 1 {
        let t0 = FLASH_KEY_TIMES[i];
        let t1 = FLASH_KEY_TIMES[i + 1];
        if frac >= t0 && frac < t1 {
            let local = (frac - t0) / (t1 - t0);
            // Alternate easeOut / easeIn per cmux pattern
            let eased = if i % 2 == 0 {
                ease_out(local)
            } else {
                ease_in(local)
            };
            let v0 = FLASH_VALUES[i];
            let v1 = FLASH_VALUES[i + 1];
            return v0 + (v1 - v0) * eased;
        }
    }
    0.0
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}

fn ease_in(t: f32) -> f32 {
    t * t
}
