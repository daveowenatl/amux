//! Procedural rendering of box-drawing, block-element, and shade characters.
//!
//! Instead of relying on font glyphs (which have side bearings and don't fill
//! the cell edge-to-edge), we define each character as a set of filled
//! rectangles in normalized cell coordinates (0.0–1.0). The GPU callback
//! emits these as foreground-colored background quads, producing pixel-perfect
//! lines that connect seamlessly across adjacent cells.
//!
//! This matches the approach used by wezterm and Ghostty, both of which render
//! these characters procedurally rather than from fonts.

/// A filled rectangle in normalized cell coordinates.
/// (0.0, 0.0) is the top-left corner; (1.0, 1.0) is the bottom-right.
#[derive(Debug, Clone, Copy)]
pub struct CustomRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Line thickness as a fraction of cell height.
/// Light ≈ 1/16, Heavy ≈ 3/16. Clamped to at least 1 physical pixel at render time.
const LIGHT: f32 = 1.0 / 16.0;
const HEAVY: f32 = 3.0 / 16.0;

/// Half-thicknesses for centering lines.
const HL: f32 = LIGHT / 2.0;
const HH: f32 = HEAVY / 2.0;

// ---------------------------------------------------------------------------
// Helpers for building rectangle arrays
// ---------------------------------------------------------------------------

/// Horizontal line spanning the full cell width at the vertical center.
const fn hline_full(half_t: f32) -> CustomRect {
    CustomRect {
        x: 0.0,
        y: 0.5 - half_t,
        w: 1.0,
        h: half_t * 2.0,
    }
}

/// Vertical line spanning the full cell height at the horizontal center.
const fn vline_full(half_t: f32) -> CustomRect {
    CustomRect {
        x: 0.5 - half_t,
        y: 0.0,
        w: half_t * 2.0,
        h: 1.0,
    }
}

/// Horizontal line from x0 to x1 at vertical center.
const fn hline(x0: f32, x1: f32, half_t: f32) -> CustomRect {
    CustomRect {
        x: x0,
        y: 0.5 - half_t,
        w: x1 - x0,
        h: half_t * 2.0,
    }
}

/// Vertical line from y0 to y1 at horizontal center.
const fn vline(y0: f32, y1: f32, half_t: f32) -> CustomRect {
    CustomRect {
        x: 0.5 - half_t,
        y: y0,
        w: half_t * 2.0,
        h: y1 - y0,
    }
}

/// A filled block covering a fraction of the cell.
const fn block(x: f32, y: f32, w: f32, h: f32) -> CustomRect {
    CustomRect { x, y, w, h }
}

// ---------------------------------------------------------------------------
// Character lookup
// ---------------------------------------------------------------------------

/// Helper macro to create `&'static [CustomRect]` from const-fn expressions.
/// Each invocation creates a `const` item so the array lives in static memory.
macro_rules! rects {
    ($($rect:expr),+ $(,)?) => {{
        const R: &[CustomRect] = &[$($rect),+];
        Some(R)
    }};
}

/// Returns the set of rectangles for a procedurally-rendered character,
/// or `None` if the character should use the normal font glyph path.
pub fn custom_glyph_rects(ch: char) -> Option<&'static [CustomRect]> {
    match ch as u32 {
        // =================================================================
        // BOX DRAWING — Light lines
        // =================================================================

        // ─ Light horizontal
        0x2500 => rects![hline_full(HL)],
        // │ Light vertical
        0x2502 => rects![vline_full(HL)],

        // ┌ Light down and right
        0x250C => rects![hline(0.5, 1.0, HL), vline(0.5, 1.0, HL)],
        // ┐ Light down and left
        0x2510 => rects![hline(0.0, 0.5 + HL, HL), vline(0.5, 1.0, HL)],
        // └ Light up and right
        0x2514 => rects![hline(0.5, 1.0, HL), vline(0.0, 0.5 + HL, HL)],
        // ┘ Light up and left
        0x2518 => rects![hline(0.0, 0.5 + HL, HL), vline(0.0, 0.5 + HL, HL)],

        // ├ Light vertical and right
        0x251C => rects![vline_full(HL), hline(0.5, 1.0, HL)],
        // ┤ Light vertical and left
        0x2524 => rects![vline_full(HL), hline(0.0, 0.5 + HL, HL)],
        // ┬ Light down and horizontal
        0x252C => rects![hline_full(HL), vline(0.5, 1.0, HL)],
        // ┴ Light up and horizontal
        0x2534 => rects![hline_full(HL), vline(0.0, 0.5 + HL, HL)],
        // ┼ Light vertical and horizontal
        0x253C => rects![hline_full(HL), vline_full(HL)],

        // =================================================================
        // BOX DRAWING — Heavy lines
        // =================================================================

        // ━ Heavy horizontal
        0x2501 => rects![hline_full(HH)],
        // ┃ Heavy vertical
        0x2503 => rects![vline_full(HH)],

        // ┏ Heavy down and right
        0x250F => rects![hline(0.5, 1.0, HH), vline(0.5, 1.0, HH)],
        // ┓ Heavy down and left
        0x2513 => rects![hline(0.0, 0.5 + HH, HH), vline(0.5, 1.0, HH)],
        // ┗ Heavy up and right
        0x2517 => rects![hline(0.5, 1.0, HH), vline(0.0, 0.5 + HH, HH)],
        // ┛ Heavy up and left
        0x251B => rects![hline(0.0, 0.5 + HH, HH), vline(0.0, 0.5 + HH, HH)],

        // ┣ Heavy vertical and right
        0x2523 => rects![vline_full(HH), hline(0.5, 1.0, HH)],
        // ┫ Heavy vertical and left
        0x252B => rects![vline_full(HH), hline(0.0, 0.5 + HH, HH)],
        // ┳ Heavy down and horizontal
        0x2533 => rects![hline_full(HH), vline(0.5, 1.0, HH)],
        // ┻ Heavy up and horizontal
        0x253B => rects![hline_full(HH), vline(0.0, 0.5 + HH, HH)],
        // ╋ Heavy vertical and horizontal
        0x254B => rects![hline_full(HH), vline_full(HH)],

        // =================================================================
        // BOX DRAWING — Dashed lines (light)
        // =================================================================

        // ┄ Light triple dash horizontal
        0x2504 => rects![
            hline(0.0 / 6.0, 1.0 / 6.0, HL),
            hline(2.0 / 6.0, 3.5 / 6.0, HL),
            hline(4.5 / 6.0, 1.0, HL),
        ],
        // ┆ Light triple dash vertical
        0x2506 => rects![
            vline(0.0 / 6.0, 1.0 / 6.0, HL),
            vline(2.0 / 6.0, 3.5 / 6.0, HL),
            vline(4.5 / 6.0, 1.0, HL),
        ],
        // ┈ Light quadruple dash horizontal
        0x2508 => rects![
            hline(0.0 / 8.0, 1.0 / 8.0, HL),
            hline(2.0 / 8.0, 3.0 / 8.0, HL),
            hline(4.5 / 8.0, 5.5 / 8.0, HL),
            hline(6.5 / 8.0, 1.0, HL),
        ],
        // ┊ Light quadruple dash vertical
        0x250A => rects![
            vline(0.0 / 8.0, 1.0 / 8.0, HL),
            vline(2.0 / 8.0, 3.0 / 8.0, HL),
            vline(4.5 / 8.0, 5.5 / 8.0, HL),
            vline(6.5 / 8.0, 1.0, HL),
        ],

        // ┅ Heavy triple dash horizontal
        0x2505 => rects![
            hline(0.0 / 6.0, 1.0 / 6.0, HH),
            hline(2.0 / 6.0, 3.5 / 6.0, HH),
            hline(4.5 / 6.0, 1.0, HH),
        ],
        // ┇ Heavy triple dash vertical
        0x2507 => rects![
            vline(0.0 / 6.0, 1.0 / 6.0, HH),
            vline(2.0 / 6.0, 3.5 / 6.0, HH),
            vline(4.5 / 6.0, 1.0, HH),
        ],
        // ┉ Heavy quadruple dash horizontal
        0x2509 => rects![
            hline(0.0 / 8.0, 1.0 / 8.0, HH),
            hline(2.0 / 8.0, 3.0 / 8.0, HH),
            hline(4.5 / 8.0, 5.5 / 8.0, HH),
            hline(6.5 / 8.0, 1.0, HH),
        ],
        // ┋ Heavy quadruple dash vertical
        0x250B => rects![
            vline(0.0 / 8.0, 1.0 / 8.0, HH),
            vline(2.0 / 8.0, 3.0 / 8.0, HH),
            vline(4.5 / 8.0, 5.5 / 8.0, HH),
            vline(6.5 / 8.0, 1.0, HH),
        ],

        // =================================================================
        // BOX DRAWING — Mixed light/heavy (most common combinations)
        // =================================================================

        // ┍ Down light and right heavy
        0x250D => rects![hline(0.5, 1.0, HH), vline(0.5, 1.0, HL)],
        // ┎ Down heavy and right light
        0x250E => rects![hline(0.5, 1.0, HL), vline(0.5, 1.0, HH)],
        // ┑ Down light and left heavy
        0x2511 => rects![hline(0.0, 0.5 + HL, HH), vline(0.5, 1.0, HL)],
        // ┒ Down heavy and left light
        0x2512 => rects![hline(0.0, 0.5 + HH, HL), vline(0.5, 1.0, HH)],
        // ┕ Up light and right heavy
        0x2515 => rects![hline(0.5, 1.0, HH), vline(0.0, 0.5 + HL, HL)],
        // ┖ Up heavy and right light
        0x2516 => rects![hline(0.5, 1.0, HL), vline(0.0, 0.5 + HH, HH)],
        // ┙ Up light and left heavy
        0x2519 => rects![hline(0.0, 0.5 + HL, HH), vline(0.0, 0.5 + HL, HL)],
        // ┚ Up heavy and left light
        0x251A => rects![hline(0.0, 0.5 + HH, HL), vline(0.0, 0.5 + HH, HH)],

        // ┝ Vertical light and right heavy
        0x251D => rects![vline_full(HL), hline(0.5, 1.0, HH)],
        // ┞ Up heavy and right down light — simplified to vertical + right stub
        0x251E => rects![
            vline(0.0, 0.5, HH),
            vline(0.5, 1.0, HL),
            hline(0.5, 1.0, HL)
        ],
        // ┠ Down heavy and right up light — simplified
        0x2520 => rects![
            vline(0.0, 0.5, HL),
            vline(0.5, 1.0, HH),
            hline(0.5, 1.0, HL)
        ],
        // ┡ Down light and right up heavy
        0x2521 => rects![vline_full(HH), hline(0.5, 1.0, HL)],
        // ┢ Down heavy and right up light (variant)
        0x2522 => rects![vline_full(HL), hline(0.5, 1.0, HH)],

        // ┥ Vertical light and left heavy
        0x2525 => rects![vline_full(HL), hline(0.0, 0.5 + HL, HH)],
        // ┩ Down light and left up heavy
        0x2529 => rects![vline_full(HH), hline(0.0, 0.5 + HH, HL)],

        // ┭ Left heavy and right down light
        0x252D => rects![
            hline(0.0, 0.5, HH),
            hline(0.5, 1.0, HL),
            vline(0.5, 1.0, HL)
        ],
        // ┮ Left light and right down heavy (variant)
        0x252E => rects![hline_full(HL), vline(0.5, 1.0, HH)],
        // ┯ Down light and horizontal heavy
        0x252F => rects![hline_full(HH), vline(0.5, 1.0, HL)],
        // ┰ Down heavy and horizontal light
        0x2530 => rects![hline_full(HL), vline(0.5, 1.0, HH)],

        // ┵ Left heavy and right up light
        0x2535 => rects![
            hline(0.0, 0.5, HH),
            hline(0.5, 1.0, HL),
            vline(0.0, 0.5 + HL, HL)
        ],
        // ┷ Up light and horizontal heavy
        0x2537 => rects![hline_full(HH), vline(0.0, 0.5 + HL, HL)],
        // ┸ Up heavy and horizontal light
        0x2538 => rects![hline_full(HL), vline(0.0, 0.5 + HH, HH)],

        // ┽ Left heavy and right vertical light
        0x253D => rects![vline_full(HL), hline(0.0, 0.5, HH), hline(0.5, 1.0, HL)],
        // ┾ Left light and right vertical heavy (variant)
        0x253E => rects![vline_full(HL), hline(0.0, 0.5, HL), hline(0.5, 1.0, HH)],
        // ┿ Vertical light and horizontal heavy
        0x253F => rects![hline_full(HH), vline_full(HL)],
        // ╀ Up heavy, down horizontal light
        0x2540 => rects![hline_full(HL), vline(0.0, 0.5, HH), vline(0.5, 1.0, HL)],
        // ╁ Down heavy, up horizontal light
        0x2541 => rects![hline_full(HL), vline(0.0, 0.5, HL), vline(0.5, 1.0, HH)],
        // ╂ Vertical heavy and horizontal light
        0x2542 => rects![hline_full(HL), vline_full(HH)],

        // =================================================================
        // BOX DRAWING — Double lines
        // =================================================================

        // ═ Double horizontal
        0x2550 => rects![
            hline_full(HL),
            CustomRect {
                x: 0.0,
                y: 0.5 + HL * 2.0,
                w: 1.0,
                h: LIGHT,
            },
        ],
        // ║ Double vertical
        0x2551 => rects![
            vline_full(HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.0,
                w: LIGHT,
                h: 1.0,
            },
        ],

        // ╔ Double down and right
        0x2554 => rects![
            hline(0.5 - HL, 1.0, HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.5 + HL * 2.0,
                w: 0.5 - HL * 2.0,
                h: LIGHT,
            },
            vline(0.5 - HL, 1.0, HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.5 + HL * 2.0,
                w: LIGHT,
                h: 0.5 - HL * 2.0,
            },
        ],
        // ╗ Double down and left
        0x2557 => rects![
            hline(0.0, 0.5 + HL, HL),
            CustomRect {
                x: 0.0,
                y: 0.5 + HL * 2.0,
                w: 0.5 - HL * 2.0 + LIGHT,
                h: LIGHT,
            },
            vline(0.5 - HL, 1.0, HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.5 + HL * 2.0,
                w: LIGHT,
                h: 0.5 - HL * 2.0,
            },
        ],
        // ╚ Double up and right
        0x255A => rects![
            hline(0.5 - HL, 1.0, HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.0,
                w: 0.5 - HL * 2.0,
                h: LIGHT,
            },
            vline(0.0, 0.5 + HL, HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.0,
                w: LIGHT,
                h: 0.5 - HL * 2.0 + LIGHT,
            },
        ],
        // ╝ Double up and left
        0x255D => rects![
            hline(0.0, 0.5 + HL, HL),
            CustomRect {
                x: 0.0,
                y: 0.5 - HL - LIGHT,
                w: 0.5 - HL * 2.0 + LIGHT,
                h: LIGHT,
            },
            vline(0.0, 0.5 + HL, HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.0,
                w: LIGHT,
                h: 0.5 - HL * 2.0 + LIGHT,
            },
        ],

        // ╠ Double vertical and right
        0x2560 => rects![
            vline_full(HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.0,
                w: LIGHT,
                h: 0.5 - HL - LIGHT,
            },
            hline(0.5 + HL * 2.0, 1.0, HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.5 + HL + LIGHT,
                w: LIGHT,
                h: 0.5 - HL - LIGHT,
            },
        ],
        // ╣ Double vertical and left
        0x2563 => rects![
            vline_full(HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.0,
                w: LIGHT,
                h: 0.5 - HL - LIGHT,
            },
            hline(0.0, 0.5 - HL * 2.0 + LIGHT, HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.5 + HL + LIGHT,
                w: LIGHT,
                h: 0.5 - HL - LIGHT,
            },
        ],
        // ╦ Double horizontal and down
        0x2566 => rects![
            hline_full(HL),
            CustomRect {
                x: 0.5 - HL,
                y: 0.5 + HL * 2.0,
                w: LIGHT,
                h: 0.5 - HL * 2.0,
            },
            CustomRect {
                x: 0.0,
                y: 0.5 + HL * 2.0,
                w: 1.0,
                h: LIGHT,
            },
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.5 + HL * 2.0,
                w: LIGHT,
                h: 0.5 - HL * 2.0,
            },
        ],
        // ╩ Double horizontal and up
        0x2569 => rects![
            hline_full(HL),
            CustomRect {
                x: 0.5 - HL,
                y: 0.0,
                w: LIGHT,
                h: 0.5 - HL * 2.0 + LIGHT,
            },
            CustomRect {
                x: 0.0,
                y: 0.5 - HL - LIGHT,
                w: 1.0,
                h: LIGHT,
            },
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.0,
                w: LIGHT,
                h: 0.5 - HL * 2.0 + LIGHT,
            },
        ],
        // ╬ Double vertical and horizontal
        0x256C => rects![
            vline(0.0, 0.5 - HL - LIGHT, HL),
            vline(0.5 + HL + LIGHT, 1.0, HL),
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.0,
                w: LIGHT,
                h: 0.5 - HL - LIGHT,
            },
            CustomRect {
                x: 0.5 + HL * 2.0,
                y: 0.5 + HL + LIGHT,
                w: LIGHT,
                h: 0.5 - HL - LIGHT,
            },
            hline(0.0, 0.5 - HL - LIGHT, HL),
            hline(0.5 + HL + LIGHT, 1.0, HL),
            CustomRect {
                x: 0.0,
                y: 0.5 + HL * 2.0,
                w: 0.5 - HL - LIGHT,
                h: LIGHT,
            },
            CustomRect {
                x: 0.5 + HL + LIGHT,
                y: 0.5 + HL * 2.0,
                w: 0.5 - HL - LIGHT,
                h: LIGHT,
            },
        ],

        // =================================================================
        // BOX DRAWING — Rounded corners (approximated as right angles)
        // =================================================================

        // ╭ Arc down and right
        0x256D => rects![hline(0.5, 1.0, HL), vline(0.5, 1.0, HL)],
        // ╮ Arc down and left
        0x256E => rects![hline(0.0, 0.5 + HL, HL), vline(0.5, 1.0, HL)],
        // ╯ Arc up and left
        0x256F => rects![hline(0.0, 0.5 + HL, HL), vline(0.0, 0.5 + HL, HL)],
        // ╰ Arc up and right
        0x2570 => rects![hline(0.5, 1.0, HL), vline(0.0, 0.5 + HL, HL)],

        // =================================================================
        // BLOCK ELEMENTS
        // =================================================================

        // ▀ Upper half block
        0x2580 => rects![block(0.0, 0.0, 1.0, 0.5)],
        // ▁ Lower one eighth block
        0x2581 => rects![block(0.0, 7.0 / 8.0, 1.0, 1.0 / 8.0)],
        // ▂ Lower one quarter block
        0x2582 => rects![block(0.0, 6.0 / 8.0, 1.0, 2.0 / 8.0)],
        // ▃ Lower three eighths block
        0x2583 => rects![block(0.0, 5.0 / 8.0, 1.0, 3.0 / 8.0)],
        // ▄ Lower half block
        0x2584 => rects![block(0.0, 0.5, 1.0, 0.5)],
        // ▅ Lower five eighths block
        0x2585 => rects![block(0.0, 3.0 / 8.0, 1.0, 5.0 / 8.0)],
        // ▆ Lower three quarters block
        0x2586 => rects![block(0.0, 2.0 / 8.0, 1.0, 6.0 / 8.0)],
        // ▇ Lower seven eighths block
        0x2587 => rects![block(0.0, 1.0 / 8.0, 1.0, 7.0 / 8.0)],
        // █ Full block
        0x2588 => rects![block(0.0, 0.0, 1.0, 1.0)],
        // ▉ Left seven eighths block
        0x2589 => rects![block(0.0, 0.0, 7.0 / 8.0, 1.0)],
        // ▊ Left three quarters block
        0x258A => rects![block(0.0, 0.0, 6.0 / 8.0, 1.0)],
        // ▋ Left five eighths block
        0x258B => rects![block(0.0, 0.0, 5.0 / 8.0, 1.0)],
        // ▌ Left half block
        0x258C => rects![block(0.0, 0.0, 0.5, 1.0)],
        // ▍ Left three eighths block
        0x258D => rects![block(0.0, 0.0, 3.0 / 8.0, 1.0)],
        // ▎ Left one quarter block
        0x258E => rects![block(0.0, 0.0, 2.0 / 8.0, 1.0)],
        // ▏ Left one eighth block
        0x258F => rects![block(0.0, 0.0, 1.0 / 8.0, 1.0)],
        // ▐ Right half block
        0x2590 => rects![block(0.5, 0.0, 0.5, 1.0)],

        // ▔ Upper one eighth block
        0x2594 => rects![block(0.0, 0.0, 1.0, 1.0 / 8.0)],
        // ▕ Right one eighth block
        0x2595 => rects![block(7.0 / 8.0, 0.0, 1.0 / 8.0, 1.0)],

        // Quadrant blocks
        // ▖ Quadrant lower left
        0x2596 => rects![block(0.0, 0.5, 0.5, 0.5)],
        // ▗ Quadrant lower right
        0x2597 => rects![block(0.5, 0.5, 0.5, 0.5)],
        // ▘ Quadrant upper left
        0x2598 => rects![block(0.0, 0.0, 0.5, 0.5)],
        // ▙ Quadrant upper left and lower left and lower right
        0x2599 => rects![block(0.0, 0.0, 0.5, 0.5), block(0.0, 0.5, 1.0, 0.5),],
        // ▚ Quadrant upper left and lower right
        0x259A => rects![block(0.0, 0.0, 0.5, 0.5), block(0.5, 0.5, 0.5, 0.5)],
        // ▛ Quadrant upper left and upper right and lower left
        0x259B => rects![block(0.0, 0.0, 1.0, 0.5), block(0.0, 0.5, 0.5, 0.5),],
        // ▜ Quadrant upper left and upper right and lower right
        0x259C => rects![block(0.0, 0.0, 1.0, 0.5), block(0.5, 0.5, 0.5, 0.5),],
        // ▝ Quadrant upper right
        0x259D => rects![block(0.5, 0.0, 0.5, 0.5)],
        // ▞ Quadrant upper right and lower left
        0x259E => rects![block(0.5, 0.0, 0.5, 0.5), block(0.0, 0.5, 0.5, 0.5)],
        // ▟ Quadrant upper right and lower left and lower right
        0x259F => rects![block(0.5, 0.0, 0.5, 0.5), block(0.0, 0.5, 1.0, 0.5),],

        // =================================================================
        // SHADE CHARACTERS
        // =================================================================
        // ░ ▒ ▓ — shade characters approximated with horizontal stripes.
        // True shade rendering needs alpha blending (shader support); we
        // approximate coverage by distributing opaque stripe area across
        // the cell. This is a known limitation — results are banded rather
        // than uniformly dithered.

        // ░ Light shade (~25% coverage: 4 thin stripes)
        0x2591 => rects![
            block(0.0, 0.0 / 4.0, 1.0, 1.0 / 16.0),
            block(0.0, 1.0 / 4.0, 1.0, 1.0 / 16.0),
            block(0.0, 2.0 / 4.0, 1.0, 1.0 / 16.0),
            block(0.0, 3.0 / 4.0, 1.0, 1.0 / 16.0),
        ],
        // ▒ Medium shade (~50% coverage: 4 medium stripes)
        0x2592 => rects![
            block(0.0, 0.0 / 4.0, 1.0, 1.0 / 8.0),
            block(0.0, 1.0 / 4.0, 1.0, 1.0 / 8.0),
            block(0.0, 2.0 / 4.0, 1.0, 1.0 / 8.0),
            block(0.0, 3.0 / 4.0, 1.0, 1.0 / 8.0),
        ],
        // ▓ Dark shade (~75% coverage: full cell minus thin gaps)
        0x2593 => rects![
            block(0.0, 0.0, 1.0, 3.0 / 16.0),
            block(0.0, 1.0 / 4.0, 1.0, 3.0 / 16.0),
            block(0.0, 2.0 / 4.0, 1.0, 3.0 / 16.0),
            block(0.0, 3.0 / 4.0, 1.0, 3.0 / 16.0),
        ],

        _ => None,
    }
}
