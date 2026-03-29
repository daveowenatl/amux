use std::collections::HashMap;
use std::sync::Arc;

use amux_term::color::{resolve_color, srgba_to_f32};
use wezterm_term::color::{ColorPalette, SrgbaTuple};
use wezterm_term::image::{ImageData, ImageDataType};
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
        let mut images = Vec::new();
        let mut seen_images: HashMap<[u8; 32], Arc<ImageData>> = HashMap::new();

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

                // Extract inline images (Kitty protocol)
                if let Some(image_cells) = attrs.images() {
                    for image_cell in &image_cells {
                        let image_data = image_cell.image_data();
                        let hash = image_data.hash();
                        seen_images
                            .entry(hash)
                            .or_insert_with(|| Arc::clone(image_data));

                        let tl = image_cell.top_left();
                        let br = image_cell.bottom_right();
                        images.push(ImagePlacement {
                            col: col_idx,
                            row: row_idx,
                            uv_min: [tl.x.into_inner(), tl.y.into_inner()],
                            uv_max: [br.x.into_inner(), br.y.into_inner()],
                            image_hash: hash,
                            z_index: image_cell.z_index(),
                        });
                    }
                }

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

        // Decode collected images to RGBA.
        let decoded_images = decode_images(seen_images);

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
            images,
            decoded_images,
        }
    }
}

/// Decode image data from wezterm-term into raw RGBA.
fn decode_images(seen: HashMap<[u8; 32], Arc<ImageData>>) -> Vec<DecodedImage> {
    let mut result = Vec::with_capacity(seen.len());
    for (hash, image_data) in seen {
        let locked: std::sync::MutexGuard<'_, ImageDataType> = image_data.data();
        match &*locked {
            ImageDataType::Rgba8 {
                data,
                width,
                height,
                ..
            } => {
                result.push(DecodedImage {
                    hash,
                    data: data.clone(),
                    width: *width,
                    height: *height,
                });
            }
            ImageDataType::AnimRgba8 {
                frames,
                width,
                height,
                ..
            } => {
                // Use first frame for static rendering.
                if let Some(frame) = frames.first() {
                    result.push(DecodedImage {
                        hash,
                        data: frame.clone(),
                        width: *width,
                        height: *height,
                    });
                }
            }
            ImageDataType::EncodedFile(bytes) => {
                // Decode encoded image (PNG, JPEG, GIF, etc.) to RGBA.
                if let Ok(img) = image::load_from_memory(bytes) {
                    let rgba = img.to_rgba8();
                    result.push(DecodedImage {
                        hash,
                        data: rgba.as_raw().clone(),
                        width: rgba.width(),
                        height: rgba.height(),
                    });
                }
            }
            _ => {}
        }
    }
    result
}

/// Dim a color by multiplying RGB channels toward black.
fn dim_color(c: [f32; 4], factor: f32) -> [f32; 4] {
    [c[0] * factor, c[1] * factor, c[2] * factor, c[3]]
}
