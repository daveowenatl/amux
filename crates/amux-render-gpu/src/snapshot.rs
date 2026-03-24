use wezterm_term::color::{ColorAttribute, ColorPalette, SrgbaTuple};
use wezterm_term::CursorPosition;

/// Pre-extracted terminal state for GPU rendering.
///
/// Built on the main thread (where the terminal screen borrow is held),
/// then moved into the paint callback which must be `Send + Sync`.
pub struct TerminalSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<CellData>,
    pub cursor: CursorPosition,
    pub default_bg: [f32; 4],
    pub cursor_color: [f32; 4],
}

/// Data for a single terminal cell.
pub struct CellData {
    pub col: usize,
    pub row: usize,
    pub text: String,
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub bold: bool,
    pub italic: bool,
}

impl TerminalSnapshot {
    /// Extract a snapshot from the terminal screen.
    ///
    /// `scroll_offset` is the number of lines scrolled back from the bottom.
    pub fn from_screen(
        screen: &wezterm_term::screen::Screen,
        palette: &ColorPalette,
        cursor: &CursorPosition,
        cols: usize,
        rows: usize,
        scroll_offset: usize,
    ) -> Self {
        let default_bg = srgba_to_f32(palette.background);
        let cursor_color = srgba_to_f32(palette.cursor_bg);

        let total = screen.scrollback_rows();
        let end = total.saturating_sub(scroll_offset);
        let start = end.saturating_sub(rows);
        let lines = screen.lines_in_phys_range(start..end);

        let mut cells = Vec::with_capacity(cols * rows);

        for (row_idx, line) in lines.iter().enumerate() {
            for cell_ref in line.visible_cells() {
                let col_idx = cell_ref.cell_index();
                if col_idx >= cols {
                    break;
                }

                let attrs = cell_ref.attrs();
                let reverse = attrs.reverse();

                let fg_attr = attrs.foreground();
                let bg_attr = attrs.background();

                let fg_color = resolve_color(&fg_attr, palette, true, reverse);
                let bg_color = resolve_color(&bg_attr, palette, false, reverse);

                cells.push(CellData {
                    col: col_idx,
                    row: row_idx,
                    text: cell_ref.str().to_string(),
                    fg: srgba_to_f32(fg_color),
                    bg: srgba_to_f32(bg_color),
                    bold: attrs.intensity() == wezterm_term::Intensity::Bold,
                    italic: attrs.italic(),
                });
            }
        }

        Self {
            cols,
            rows,
            cells,
            cursor: *cursor,
            default_bg,
            cursor_color,
        }
    }
}

fn resolve_color(
    color: &ColorAttribute,
    palette: &ColorPalette,
    is_fg: bool,
    reverse: bool,
) -> SrgbaTuple {
    let effective_is_fg = if reverse { !is_fg } else { is_fg };
    if effective_is_fg {
        palette.resolve_fg(*color)
    } else {
        palette.resolve_bg(*color)
    }
}

fn srgba_to_f32(c: SrgbaTuple) -> [f32; 4] {
    [c.0, c.1, c.2, c.3]
}
