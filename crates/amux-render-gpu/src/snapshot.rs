use amux_term::backend::{Color, CursorShape, TerminalBackend, UnderlineStyle};

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
    // Cursor fields (amux-native types)
    pub cursor_x: usize,
    pub cursor_y: i64,
    pub cursor_visible: bool,
    /// Cursor hidden by blink animation (separate from cursor_visible to keep
    /// ligature run-breaking stable across blink cycles).
    pub cursor_blink_hidden: bool,
    pub cursor_shape: CursorShape,
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
    /// Inline image placements (Kitty image protocol).
    pub images: Vec<ImagePlacement>,
    /// Decoded image data, deduplicated by hash.
    pub decoded_images: Vec<DecodedImage>,
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
    pub underline: UnderlineStyle,
    /// Underline color override (None = use fg color).
    pub underline_color: Option<[f32; 4]>,
    pub strikethrough: bool,
    /// Faint/dim text (SGR 2) — renderer halves fg alpha.
    pub faint: bool,
    pub hyperlink_url: Option<String>,
}

/// A single cell's image placement within the terminal grid.
pub struct ImagePlacement {
    pub col: usize,
    pub row: usize,
    /// Texture UV top-left for this cell's portion of the image.
    pub uv_min: [f32; 2],
    /// Texture UV bottom-right for this cell's portion of the image.
    pub uv_max: [f32; 2],
    /// Hash of the source image (indexes into `decoded_images`).
    pub image_hash: [u8; 32],
    /// Z-index: negative = behind text, >= 0 = above text.
    pub z_index: i32,
}

/// Decoded RGBA image data, deduplicated by hash.
pub struct DecodedImage {
    pub hash: [u8; 32],
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
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

fn color_to_f32(c: Color) -> [f32; 4] {
    [c.0, c.1, c.2, c.3]
}

impl TerminalSnapshot {
    /// Build a snapshot from a `TerminalBackend` using amux-native types.
    #[allow(clippy::too_many_arguments)]
    pub fn from_backend(
        backend: &dyn TerminalBackend,
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
        let palette = backend.palette();
        let cursor = backend.cursor();
        let selection_range = selection.as_ref().map(|s| (s.start, s.end));
        let default_bg = color_to_f32(palette.background);
        let cursor_bg = color_to_f32(palette.cursor_bg);
        let cursor_fg = color_to_f32(palette.cursor_fg);

        let (screen_rows, start) = if backend.manages_own_scroll() {
            // Backend manages viewport scrolling internally (e.g., libghostty).
            // read_screen_cells returns the already-scrolled viewport.
            let total = backend.scrollback_rows();
            let vp_start = total.saturating_sub(rows);
            (backend.read_screen_cells(0), vp_start)
        } else {
            let total = backend.scrollback_rows();
            let end = total.saturating_sub(scroll_offset);
            let start = end.saturating_sub(rows);
            (backend.read_cells_range(start, end), start)
        };

        let mut cells = Vec::with_capacity(cols * rows);
        let mut cursor_text = String::new();
        let mut cursor_text_bold = false;
        let mut cursor_text_italic = false;

        for (row_idx, screen_row) in screen_rows.iter().enumerate() {
            for (col_idx, cell) in screen_row.cells.iter().enumerate() {
                if col_idx >= cols {
                    break;
                }

                let (fg_color, bg_color) = if cell.reverse {
                    (cell.bg, cell.fg)
                } else {
                    (cell.fg, cell.bg)
                };
                let mut fg = color_to_f32(fg_color);
                let mut bg = color_to_f32(bg_color);

                // Apply selection highlighting using theme colors
                let stable_row = start + row_idx;
                if let Some(ref sel) = selection {
                    if sel.contains(col_idx, stable_row) {
                        bg = color_to_f32(palette.selection_bg);
                        fg = color_to_f32(palette.selection_fg);
                    }
                }

                // Apply find/search highlighting
                for (i, &(h_row, h_start, h_end)) in highlight_ranges.iter().enumerate() {
                    if h_row == stable_row && col_idx >= h_start && col_idx < h_end {
                        if current_highlight == Some(i) {
                            bg = [1.0, 0.6, 0.0, 1.0];
                            fg = [0.0, 0.0, 0.0, 1.0];
                        } else {
                            bg = [1.0, 1.0, 0.0, 0.7];
                            fg = [0.0, 0.0, 0.0, 1.0];
                        }
                        break;
                    }
                }

                // Capture text under cursor for block cursor rendering
                if cursor.y >= 0
                    && row_idx == cursor.y as usize
                    && col_idx == cursor.x
                    && !cell.text.is_empty()
                    && cell.text != " "
                {
                    cursor_text = cell.text.clone();
                    cursor_text_bold = cell.bold;
                    cursor_text_italic = cell.italic;
                }

                cells.push(CellData {
                    col: col_idx,
                    row: row_idx,
                    text: cell.text.clone(),
                    fg,
                    bg,
                    bold: cell.bold,
                    italic: cell.italic,
                    underline: cell.underline,
                    underline_color: cell.underline_color.map(color_to_f32),
                    strikethrough: cell.strikethrough,
                    faint: cell.faint,
                    hyperlink_url: cell.hyperlink_url.clone(),
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
            cursor_x: cursor.x,
            cursor_y: cursor.y,
            cursor_visible: cursor.visible,
            cursor_blink_hidden: false,
            cursor_shape: cursor.shape,
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
            images: Vec::new(),
            decoded_images: Vec::new(),
        }
    }
}

/// Dim a color by multiplying RGB channels toward black.
fn dim_color(c: [f32; 4], factor: f32) -> [f32; 4] {
    [c[0] * factor, c[1] * factor, c[2] * factor, c[3]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use amux_term::backend::*;
    use amux_term::osc::NotificationEvent;
    use std::io::Read as IoRead;
    use url::Url;

    /// Minimal mock backend for snapshot tests.
    struct MockBackend {
        cells: Vec<ScreenRow>,
        palette: Palette,
    }

    impl TerminalBackend for MockBackend {
        fn advance(&mut self) -> AdvanceResult {
            AdvanceResult::Eof
        }
        fn resize(&mut self, _cols: u16, _rows: u16) -> Result<(), TermError> {
            Ok(())
        }
        fn write_bytes(&mut self, _data: &[u8]) -> Result<(), TermError> {
            Ok(())
        }
        fn feed_bytes(&mut self, _data: &[u8]) {}
        fn take_reader(&mut self) -> Option<Box<dyn IoRead + Send>> {
            None
        }
        fn title(&self) -> &str {
            ""
        }
        fn working_dir(&self) -> Option<&Url> {
            None
        }
        fn dimensions(&self) -> (usize, usize) {
            (80, 1)
        }
        fn cursor(&self) -> CursorPos {
            CursorPos::default()
        }
        fn palette(&self) -> Palette {
            self.palette.clone()
        }
        fn is_alt_screen_active(&self) -> bool {
            false
        }
        fn bracketed_paste_enabled(&self) -> bool {
            false
        }
        fn child_pid(&self) -> Option<u32> {
            None
        }
        fn is_alive(&mut self) -> bool {
            false
        }
        fn exit_status(&mut self) -> Option<ProcessExit> {
            None
        }
        fn changed_lines(&self) -> Vec<StableRow> {
            Vec::new()
        }
        fn mark_rendered(&mut self) {}
        fn current_seqno(&self) -> SequenceNo {
            0
        }
        fn rendered_seqno(&self) -> SequenceNo {
            0
        }
        fn scrollback_rows(&self) -> usize {
            self.cells.len()
        }
        fn read_screen_lines(&self, _spec: &str, _ansi: bool) -> String {
            String::new()
        }
        fn read_screen_text(&self) -> String {
            String::new()
        }
        fn read_scrollback_text(&self, _max: usize) -> String {
            String::new()
        }
        fn read_scrollback_text_range(&self, _start: usize, _end: usize) -> String {
            String::new()
        }
        fn search_scrollback(&self, _query: &str) -> Vec<(usize, usize, usize)> {
            Vec::new()
        }
        fn read_screen_cells(&self, _offset: usize) -> Vec<ScreenRow> {
            self.cells.clone()
        }
        fn read_cells_range(&self, start: usize, end: usize) -> Vec<ScreenRow> {
            self.cells[start..end.min(self.cells.len())].to_vec()
        }
        fn erase_scrollback(&mut self) {}
        fn focus_changed(&mut self, _focused: bool) {}
        fn encode_key(
            &mut self,
            _key: amux_term::key_types::Key,
            _mods: amux_term::key_types::Mods,
            _action: amux_term::key_types::Action,
            _text: Option<&str>,
            _unshifted_codepoint: Option<char>,
        ) -> Option<Vec<u8>> {
            None
        }
        fn drain_notifications(&self) -> Vec<NotificationEvent> {
            Vec::new()
        }
    }

    #[test]
    fn from_backend_swaps_fg_bg_when_reverse() {
        let fg = Color(1.0, 0.0, 0.0, 1.0); // red
        let bg = Color(0.0, 0.0, 1.0, 1.0); // blue

        let normal_cell = ScreenCell {
            text: "A".to_string(),
            fg,
            bg,
            bold: false,
            italic: false,
            underline: UnderlineStyle::None,
            underline_color: None,
            strikethrough: false,
            faint: false,
            reverse: false,
            hyperlink_url: None,
        };
        let reverse_cell = ScreenCell {
            text: "B".to_string(),
            fg,
            bg,
            reverse: true,
            ..normal_cell.clone()
        };
        let row = ScreenRow {
            cells: vec![normal_cell, reverse_cell],
            wrapped: false,
        };
        let backend = MockBackend {
            cells: vec![row],
            palette: Palette::default(),
        };

        let snap =
            TerminalSnapshot::from_backend(&backend, 2, 1, 0, true, None, 0, 0, Vec::new(), None);

        // Normal cell: fg=red, bg=blue (unchanged)
        assert_eq!(snap.cells[0].fg, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(snap.cells[0].bg, [0.0, 0.0, 1.0, 1.0]);

        // Reverse cell: fg and bg should be swapped
        assert_eq!(snap.cells[1].fg, [0.0, 0.0, 1.0, 1.0]); // was bg (blue)
        assert_eq!(snap.cells[1].bg, [1.0, 0.0, 0.0, 1.0]); // was fg (red)
    }
}
