use wezterm_term::color::{ColorAttribute, ColorPalette, SrgbaTuple};
use wezterm_term::CursorPosition;

/// Pre-extracted terminal state for GPU rendering.
///
/// Built on the main thread (where the terminal screen borrow is held),
/// then moved into the paint callback which must be `Send + Sync`.
pub struct TerminalSnapshot {
    pub pane_id: u64,
    pub seqno: usize,
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<CellData>,
    pub cursor: CursorPosition,
    pub default_bg: [f32; 4],
    pub cursor_bg: [f32; 4],
    pub cursor_fg: [f32; 4],
    pub is_focused: bool,
    pub scroll_offset: usize,
    /// Text under the cursor (for block cursor rendering).
    pub cursor_text: String,
    pub cursor_text_bold: bool,
    pub cursor_text_italic: bool,
    /// Selection start/end for dirty tracking (None if no selection).
    pub selection_range: Option<((usize, usize), (usize, usize))>,
    /// Find/search highlight ranges as (phys_row, start_col, end_col_exclusive).
    /// The current match (if any) uses a distinct color.
    pub highlight_ranges: Vec<(usize, usize, usize)>,
    pub current_highlight: Option<usize>,
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
    pub hyperlink_url: Option<String>,
}

/// Selection range for highlight rendering.
pub struct SelectionRange {
    pub start: (usize, usize), // (col, stable_row)
    pub end: (usize, usize),   // (col, stable_row)
}

impl SelectionRange {
    fn contains(&self, col: usize, stable_row: usize) -> bool {
        if stable_row < self.start.1 || stable_row > self.end.1 {
            return false;
        }
        if stable_row == self.start.1 && stable_row == self.end.1 {
            return col >= self.start.0 && col <= self.end.0;
        }
        if stable_row == self.start.1 {
            return col >= self.start.0;
        }
        if stable_row == self.end.1 {
            return col <= self.end.0;
        }
        true
    }
}

impl TerminalSnapshot {
    /// Extract a snapshot from the terminal screen.
    ///
    /// `scroll_offset` is the number of lines scrolled back from the bottom.
    /// `selection` is an optional normalized selection range for highlight rendering.
    #[allow(clippy::too_many_arguments)]
    pub fn from_screen(
        screen: &wezterm_term::screen::Screen,
        palette: &ColorPalette,
        cursor: &CursorPosition,
        cols: usize,
        rows: usize,
        scroll_offset: usize,
        is_focused: bool,
        selection: Option<SelectionRange>,
        pane_id: u64,
        seqno: usize,
        highlight_ranges: Vec<(usize, usize, usize)>,
        current_highlight: Option<usize>,
    ) -> Self {
        let selection_range = selection.as_ref().map(|s| (s.start, s.end));
        let default_bg = srgba_to_f32(palette.background);
        let cursor_bg = srgba_to_f32(palette.cursor_bg);
        let cursor_fg = srgba_to_f32(palette.cursor_fg);

        let total = screen.scrollback_rows();
        let end = total.saturating_sub(scroll_offset);
        let start = end.saturating_sub(rows);
        let lines = screen.lines_in_phys_range(start..end);

        let mut cells = Vec::with_capacity(cols * rows);
        let mut cursor_text = String::new();
        let mut cursor_text_bold = false;
        let mut cursor_text_italic = false;

        for (row_idx, line) in lines.iter().enumerate() {
            for cell_ref in line.visible_cells() {
                let col_idx = cell_ref.cell_index();
                if col_idx >= cols {
                    break;
                }

                let attrs = cell_ref.attrs();
                let reverse = attrs.reverse();

                let mut fg = resolve_color(&attrs.foreground(), palette, true, reverse);
                let mut bg = resolve_color(&attrs.background(), palette, false, reverse);

                // Apply selection highlighting (swap fg/bg)
                let stable_row = start + row_idx;
                if let Some(ref sel) = selection {
                    if sel.contains(col_idx, stable_row) {
                        std::mem::swap(&mut fg, &mut bg);
                        // Ensure selected empty cells have visible bg
                        if srgba_to_f32(bg) == default_bg {
                            bg = palette.foreground;
                            fg = palette.background;
                        }
                    }
                }

                // Apply find/search highlighting
                for (i, &(h_row, h_start, h_end)) in highlight_ranges.iter().enumerate() {
                    if h_row == stable_row && col_idx >= h_start && col_idx < h_end {
                        if current_highlight == Some(i) {
                            // Current match: bright orange bg
                            bg = SrgbaTuple(1.0, 0.6, 0.0, 1.0);
                            fg = SrgbaTuple(0.0, 0.0, 0.0, 1.0);
                        } else {
                            // Other matches: yellow bg
                            bg = SrgbaTuple(1.0, 1.0, 0.0, 0.7);
                            fg = SrgbaTuple(0.0, 0.0, 0.0, 1.0);
                        }
                        break;
                    }
                }

                // Capture text under cursor for block cursor rendering
                if row_idx == cursor.y as usize && col_idx == cursor.x {
                    let text = cell_ref.str();
                    if !text.is_empty() && text != " " {
                        cursor_text = text.to_string();
                        cursor_text_bold = attrs.intensity() == wezterm_term::Intensity::Bold;
                        cursor_text_italic = attrs.italic();
                    }
                }

                let hyperlink_url = attrs.hyperlink().map(|h| h.uri().to_string());

                cells.push(CellData {
                    col: col_idx,
                    row: row_idx,
                    text: cell_ref.str().to_string(),
                    fg: srgba_to_f32(fg),
                    bg: srgba_to_f32(bg),
                    bold: attrs.intensity() == wezterm_term::Intensity::Bold,
                    italic: attrs.italic(),
                    hyperlink_url,
                });
            }
        }

        // Dim background colors for unfocused panes.
        let dim_factor = if is_focused { 1.0 } else { 0.6 };
        let dimmed_bg = dim_color(default_bg, dim_factor);
        if !is_focused {
            for cell in &mut cells {
                cell.bg = dim_color(cell.bg, dim_factor);
            }
        }

        Self {
            pane_id,
            seqno,
            cols,
            rows,
            cells,
            cursor: *cursor,
            default_bg: dimmed_bg,
            cursor_bg,
            cursor_fg,
            is_focused,
            scroll_offset,
            cursor_text,
            cursor_text_bold,
            cursor_text_italic,
            selection_range,
            highlight_ranges,
            current_highlight,
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

/// Convert wezterm-term color tuple to [f32; 4].
///
/// Colors are kept in sRGB space here. The GPU callback converts to linear
/// only when the render target uses an sRGB format (which applies hardware
/// linear→sRGB on store). For non-sRGB targets (e.g., Bgra8Unorm on macOS),
/// sRGB values are passed through directly.
fn srgba_to_f32(c: SrgbaTuple) -> [f32; 4] {
    [c.0, c.1, c.2, c.3]
}

/// Dim a color by multiplying RGB channels toward black.
fn dim_color(c: [f32; 4], factor: f32) -> [f32; 4] {
    [c[0] * factor, c[1] * factor, c[2] * factor, c[3]]
}
