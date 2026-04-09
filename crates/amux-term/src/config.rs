use crate::backend::{Color, Palette};

/// Terminal configuration for amux panes.
#[derive(Debug)]
pub struct AmuxTermConfig {
    pub scrollback_lines: usize,
    pub color_palette: Palette,
}

impl Default for AmuxTermConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            color_palette: default_palette(),
        }
    }
}

/// Build a color palette using standard xterm ANSI colors (0-15) + 216 cube + 24 grayscale.
fn default_palette() -> Palette {
    let xterm_ansi: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00), // 0  Black
        (0xcd, 0x00, 0x00), // 1  Red
        (0x00, 0xcd, 0x00), // 2  Green
        (0xcd, 0xcd, 0x00), // 3  Yellow
        (0x00, 0x00, 0xee), // 4  Blue
        (0xcd, 0x00, 0xcd), // 5  Magenta
        (0x00, 0xcd, 0xcd), // 6  Cyan
        (0xe5, 0xe5, 0xe5), // 7  White
        (0x7f, 0x7f, 0x7f), // 8  Bright Black (Grey)
        (0xff, 0x00, 0x00), // 9  Bright Red
        (0x00, 0xff, 0x00), // 10 Bright Green
        (0xff, 0xff, 0x00), // 11 Bright Yellow
        (0x5c, 0x5c, 0xff), // 12 Bright Blue
        (0xff, 0x00, 0xff), // 13 Bright Magenta
        (0x00, 0xff, 0xff), // 14 Bright Cyan
        (0xff, 0xff, 0xff), // 15 Bright White
    ];

    let mut colors = Vec::with_capacity(256);
    for (r, g, b) in &xterm_ansi {
        colors.push(Color(
            *r as f32 / 255.0,
            *g as f32 / 255.0,
            *b as f32 / 255.0,
            1.0,
        ));
    }
    // 216-color cube (indices 16-231)
    for r in 0..6u8 {
        for g in 0..6u8 {
            for b in 0..6u8 {
                let ri = if r > 0 { 55 + 40 * r } else { 0 };
                let gi = if g > 0 { 55 + 40 * g } else { 0 };
                let bi = if b > 0 { 55 + 40 * b } else { 0 };
                colors.push(Color(
                    ri as f32 / 255.0,
                    gi as f32 / 255.0,
                    bi as f32 / 255.0,
                    1.0,
                ));
            }
        }
    }
    // 24-step grayscale (indices 232-255)
    for i in 0..24u8 {
        let v = (8 + 10 * i) as f32 / 255.0;
        colors.push(Color(v, v, v, 1.0));
    }

    let fg = Color(
        0xe5 as f32 / 255.0,
        0xe5 as f32 / 255.0,
        0xe5 as f32 / 255.0,
        1.0,
    );

    Palette {
        foreground: fg,
        background: Color::BLACK,
        cursor_fg: Color::BLACK,
        cursor_bg: Color::WHITE,
        cursor_border: Color::WHITE,
        selection_fg: Color::BLACK,
        selection_bg: Color(0.4, 0.6, 1.0, 1.0),
        colors,
    }
}
