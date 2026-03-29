use std::collections::HashMap;

use amux_term::color::{resolve_color, srgba_to_rgba8};
use amux_term::font::{self, FontConfig};
use cosmic_text::fontdb::Family;
use cosmic_text::{
    Attrs, Buffer, CacheKey, FontSystem, Metrics, Shaping, SwashCache, SwashContent, SwashImage,
};
use wezterm_term::color::ColorPalette;

/// RGBA pixel (4 bytes).
pub type Pixel = [u8; 4];

/// CPU-side terminal renderer using cosmic-text for glyph rasterization.
///
/// Renders a terminal screen into an RGBA pixel buffer suitable for uploading
/// as a texture to egui/wgpu.
pub struct SoftRenderer {
    pub font_system: FontSystem,
    swash_cache: SwashCache,
    glyph_cache: HashMap<CacheKey, Option<CachedGlyph>>,
    pub cell_width: f32,
    pub cell_height: f32,
    metrics: Metrics,
}

#[derive(Clone)]
struct CachedGlyph {
    placement_left: i32,
    placement_top: i32,
    width: u32,
    height: u32,
    data: Vec<u8>,
    is_color: bool,
}

impl SoftRenderer {
    /// Create a new renderer with the given font configuration.
    pub fn new(font_config: &FontConfig) -> Self {
        let mut font_system = font::create_font_system(font_config);
        let swash_cache = SwashCache::new();

        let font_size = font_config.size;
        let line_height = (font_size * 1.3).ceil();
        let metrics = Metrics::new(font_size, line_height);

        let cell_width = font::measure_cell_width(&mut font_system, metrics).ceil();
        let cell_height = line_height;

        Self {
            font_system,
            swash_cache,
            glyph_cache: HashMap::new(),
            cell_width,
            cell_height,
            metrics,
        }
    }

    /// Render the terminal screen to an RGBA pixel buffer.
    ///
    /// Returns (width, height, pixels) where pixels is row-major RGBA.
    pub fn render(
        &mut self,
        screen: &wezterm_term::screen::Screen,
        palette: &ColorPalette,
        cursor: &wezterm_term::CursorPosition,
        cols: usize,
        rows: usize,
    ) -> (u32, u32, Vec<u8>) {
        let pixel_width = (cols as f32 * self.cell_width).ceil() as u32;
        let pixel_height = (rows as f32 * self.cell_height).ceil() as u32;
        let mut pixels = vec![0u8; (pixel_width * pixel_height * 4) as usize];

        let bg_default = srgba_to_rgba8(palette.background);

        // Fill background
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&bg_default);
        }

        // Get physical lines for the visible region
        // Note: scrollback_rows() returns total line count (lines.len()), not just scrollback
        let total = screen.scrollback_rows();
        let start = total.saturating_sub(rows);
        let lines = screen.lines_in_phys_range(start..total);

        // Two-pass rendering per row: draw all cell backgrounds first, then
        // all glyphs. This prevents a cell's background from erasing the
        // overhang of an italic glyph drawn in the preceding cell.
        for (row_idx, line) in lines.iter().enumerate() {
            let y_offset = (row_idx as f32 * self.cell_height) as i32;
            let mut pending_glyphs: Vec<(String, i32, i32, Pixel, bool, bool)> = Vec::new();

            for cell_ref in line.visible_cells() {
                let col_idx = cell_ref.cell_index();
                if col_idx >= cols {
                    break;
                }

                let x_offset = (col_idx as f32 * self.cell_width) as i32;
                let attrs = cell_ref.attrs();
                let reverse = attrs.reverse();

                // Resolve colors
                let bg_color = resolve_color(&attrs.background(), palette, false, reverse);
                let fg_color = resolve_color(&attrs.foreground(), palette, true, reverse);

                let bg_rgba = srgba_to_rgba8(bg_color);
                let fg_rgba = srgba_to_rgba8(fg_color);

                // Draw cell background
                if bg_rgba != bg_default {
                    self.fill_rect(
                        &mut pixels,
                        pixel_width,
                        pixel_height,
                        x_offset,
                        y_offset,
                        self.cell_width.ceil() as u32,
                        self.cell_height.ceil() as u32,
                        bg_rgba,
                    );
                }

                // Collect glyph for second pass
                let text = cell_ref.str();
                if !text.is_empty() && text != " " {
                    pending_glyphs.push((
                        text.to_owned(),
                        x_offset,
                        y_offset,
                        fg_rgba,
                        attrs.intensity() == wezterm_term::Intensity::Bold,
                        attrs.italic(),
                    ));
                }
            }

            // Second pass: draw glyphs on top of all backgrounds
            for (text, x, y, fg, bold, italic) in pending_glyphs {
                self.draw_glyph(
                    &mut pixels,
                    pixel_width,
                    pixel_height,
                    &text,
                    x,
                    y,
                    fg,
                    bold,
                    italic,
                );
            }
        }

        // Draw cursor
        if cursor.y >= 0 && (cursor.y as usize) < rows && cursor.x < cols {
            let cx = (cursor.x as f32 * self.cell_width) as i32;
            let cy = (cursor.y as f32 * self.cell_height) as i32;
            let cursor_color = srgba_to_rgba8(palette.cursor_bg);
            self.fill_rect(
                &mut pixels,
                pixel_width,
                pixel_height,
                cx,
                cy,
                self.cell_width.ceil() as u32,
                self.cell_height.ceil() as u32,
                cursor_color,
            );
        }

        (pixel_width, pixel_height, pixels)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_glyph(
        &mut self,
        pixels: &mut [u8],
        buf_width: u32,
        buf_height: u32,
        text: &str,
        x_origin: i32,
        y_origin: i32,
        fg_color: Pixel,
        bold: bool,
        italic: bool,
    ) {
        // Layout the glyph using cosmic-text buffer
        let weight = if bold {
            cosmic_text::Weight::BOLD
        } else {
            cosmic_text::Weight::NORMAL
        };
        let style = if italic {
            cosmic_text::Style::Italic
        } else {
            cosmic_text::Style::Normal
        };
        let attrs = Attrs::new()
            .family(Family::Monospace)
            .weight(weight)
            .style(style);

        let mut buffer = Buffer::new_empty(self.metrics);
        {
            let mut borrowed = buffer.borrow_with(&mut self.font_system);
            borrowed.set_size(Some(self.cell_width * 2.0), Some(self.cell_height));
            borrowed.set_text(text, attrs, Shaping::Advanced);
            borrowed.shape_until_scroll(true);
        }

        // Extract glyphs and render
        for run in buffer.layout_runs() {
            let line_y = y_origin as f32 + run.line_top;
            for glyph in run.glyphs.iter() {
                let physical = glyph.physical((0.0, 0.0), 1.0);

                let cached = self.get_or_rasterize(physical.cache_key);
                if let Some(cached) = cached {
                    let gx = x_origin + physical.x + cached.placement_left;
                    let gy = line_y as i32 + physical.y - cached.placement_top;

                    self.blit_glyph(pixels, buf_width, buf_height, &cached, gx, gy, fg_color);
                }
            }
        }
    }

    fn get_or_rasterize(&mut self, cache_key: CacheKey) -> Option<CachedGlyph> {
        if let Some(cached) = self.glyph_cache.get(&cache_key) {
            return cached.clone();
        }

        let image: Option<SwashImage> = self
            .swash_cache
            .get_image_uncached(&mut self.font_system, cache_key);

        let cached = image.map(|img| CachedGlyph {
            placement_left: img.placement.left,
            placement_top: img.placement.top,
            width: img.placement.width,
            height: img.placement.height,
            data: img.data,
            is_color: img.content == SwashContent::Color,
        });

        self.glyph_cache.insert(cache_key, cached.clone());
        cached
    }

    #[allow(clippy::too_many_arguments)]
    fn blit_glyph(
        &self,
        pixels: &mut [u8],
        buf_width: u32,
        buf_height: u32,
        glyph: &CachedGlyph,
        gx: i32,
        gy: i32,
        fg_color: Pixel,
    ) {
        if glyph.is_color {
            // Color glyph (emoji): 4 bytes per pixel RGBA
            for row in 0..glyph.height {
                for col in 0..glyph.width {
                    let px = gx + col as i32;
                    let py = gy + row as i32;
                    if px < 0 || py < 0 || px >= buf_width as i32 || py >= buf_height as i32 {
                        continue;
                    }
                    let src_idx = ((row * glyph.width + col) * 4) as usize;
                    if src_idx + 3 >= glyph.data.len() {
                        continue;
                    }
                    let dst_idx = ((py as u32 * buf_width + px as u32) * 4) as usize;
                    let alpha = glyph.data[src_idx + 3];
                    if alpha > 0 {
                        blend_pixel(
                            &mut pixels[dst_idx..dst_idx + 4],
                            &glyph.data[src_idx..src_idx + 4],
                        );
                    }
                }
            }
        } else {
            // Mask glyph: 1 byte alpha per pixel
            for row in 0..glyph.height {
                for col in 0..glyph.width {
                    let px = gx + col as i32;
                    let py = gy + row as i32;
                    if px < 0 || py < 0 || px >= buf_width as i32 || py >= buf_height as i32 {
                        continue;
                    }
                    let src_idx = (row * glyph.width + col) as usize;
                    if src_idx >= glyph.data.len() {
                        continue;
                    }
                    let alpha = glyph.data[src_idx];
                    if alpha > 0 {
                        let dst_idx = ((py as u32 * buf_width + px as u32) * 4) as usize;
                        let src = [fg_color[0], fg_color[1], fg_color[2], alpha];
                        blend_pixel(&mut pixels[dst_idx..dst_idx + 4], &src);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fill_rect(
        &self,
        pixels: &mut [u8],
        buf_width: u32,
        buf_height: u32,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        color: Pixel,
    ) {
        for row in 0..h {
            let py = y + row as i32;
            if py < 0 || py >= buf_height as i32 {
                continue;
            }
            for col in 0..w {
                let px = x + col as i32;
                if px < 0 || px >= buf_width as i32 {
                    continue;
                }
                let idx = ((py as u32 * buf_width + px as u32) * 4) as usize;
                pixels[idx..idx + 4].copy_from_slice(&color);
            }
        }
    }
}

/// Measure monospace cell width by laying out "M" and reading the advance.
/// Alpha-blend src over dst (premultiplied).
fn blend_pixel(dst: &mut [u8], src: &[u8]) {
    let sa = src[3] as u32;
    let da = 255 - sa;
    dst[0] = ((src[0] as u32 * sa + dst[0] as u32 * da) / 255) as u8;
    dst[1] = ((src[1] as u32 * sa + dst[1] as u32 * da) / 255) as u8;
    dst[2] = ((src[2] as u32 * sa + dst[2] as u32 * da) / 255) as u8;
    dst[3] = (sa + dst[3] as u32 * da / 255).min(255) as u8;
}
