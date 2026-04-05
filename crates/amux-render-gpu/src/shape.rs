//! cosmic-text shaping helpers for the GPU render callback.
//!
//! Two entry points:
//! - [`shape_run`] shapes a contiguous run of same-style cells together so
//!   HarfBuzz can produce ligature substitutions.
//! - [`shape_and_rasterize`] shapes a single cell's text (used for the
//!   cursor text overlay).
//!
//! Both paths funnel through the `shape_cache` on `TerminalGpuResources`
//! so we avoid re-running cosmic-text shaping for previously seen glyphs.

use cosmic_text::{Attrs, Buffer, Metrics, Shaping};
use egui_wgpu::wgpu;

use crate::callback::{CachedGlyph, ShapedGlyphEntry, TerminalGpuResources, TextRun};
use crate::pipeline::CellFgInstance;

/// Shape a multi-cell text run for ligature support.
///
/// Groups of adjacent same-style cells are shaped together through cosmic-text
/// so HarfBuzz can produce ligature substitutions (e.g., `=>` → single glyph).
/// Glyph positions are mapped back to cell columns via `cell_byte_offsets`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shape_run(
    run: &TextRun,
    cell_width: f32,
    cell_height: f32,
    phys_x: f32,
    phys_y: f32,
    pixels_per_point: f32,
    resources: &mut TerminalGpuResources,
    queue: &wgpu::Queue,
    fg_instances: &mut Vec<CellFgInstance>,
    cached_glyphs: &mut Vec<CachedGlyph>,
) {
    let cache_key = (run.text.clone(), run.bold, run.italic);
    let base_x = phys_x + run.col_start as f32 * cell_width;
    let base_y = phys_y + run.row as f32 * cell_height;

    // Check shape cache first.
    if let Some(shaped) = resources.shape_cache.get(&cache_key) {
        let shaped = shaped.clone();
        for sg in &shaped {
            let (entry, newly_inserted) = resources.atlas.get_or_insert(
                queue,
                &mut resources.font_system,
                &mut resources.swash_cache,
                sg.cache_key,
            );
            if let Some(entry) = entry {
                let gx = base_x + sg.physical_x as f32 + entry.placement_left as f32;
                let gy = base_y + sg.line_y + sg.physical_y as f32 - entry.placement_top as f32;
                fg_instances.push(CellFgInstance {
                    pos: [gx, gy],
                    size: [entry.width as f32, entry.height as f32],
                    uv_min: [entry.uv[0], entry.uv[1]],
                    uv_max: [entry.uv[2], entry.uv[3]],
                    color: run.fg,
                    is_color: if entry.is_color { 1.0 } else { 0.0 },
                    _pad: [0.0; 3],
                });
                let glyph_col = run.col_start + sg.source_col_offset;
                cached_glyphs.push(CachedGlyph {
                    col: glyph_col,
                    row: run.row,
                    pos: [gx, gy],
                    size: [entry.width as f32, entry.height as f32],
                    uv_min: [entry.uv[0], entry.uv[1]],
                    uv_max: [entry.uv[2], entry.uv[3]],
                    is_color: if entry.is_color { 1.0 } else { 0.0 },
                });
                if newly_inserted {
                    resources.atlas_bind_group_dirty = true;
                }
            }
        }
        return;
    }

    // Cache miss: run full cosmic-text shaping.
    let weight = if run.bold {
        cosmic_text::Weight::BOLD
    } else {
        cosmic_text::Weight::NORMAL
    };
    let style = if run.italic {
        cosmic_text::Style::Italic
    } else {
        cosmic_text::Style::Normal
    };
    let attrs = Attrs::new()
        .family(cosmic_text::fontdb::Family::Monospace)
        .weight(weight)
        .style(style);

    let phys_metrics = Metrics::new(
        resources.metrics.font_size * pixels_per_point,
        resources.metrics.line_height * pixels_per_point,
    );
    let buffer_width = f32::max(run.col_count as f32 * cell_width, cell_width * 2.0);
    let mut buffer = Buffer::new_empty(phys_metrics);
    {
        let mut borrowed = buffer.borrow_with(&mut resources.font_system);
        borrowed.set_size(Some(buffer_width), Some(cell_height));
        borrowed.set_text(&run.text, attrs, Shaping::Advanced);
        borrowed.shape_until_scroll(true);
    }

    let mut shaped_entries = Vec::new();

    for layout_run in buffer.layout_runs() {
        for glyph in layout_run.glyphs.iter() {
            let physical = glyph.physical((0.0, 0.0), 1.0);

            // Map glyph back to source cell via byte offset.
            let source_col_offset = byte_offset_to_col_offset(&run.cell_byte_offsets, glyph.start);

            shaped_entries.push(ShapedGlyphEntry {
                physical_x: physical.x,
                physical_y: physical.y,
                cache_key: physical.cache_key,
                line_y: layout_run.line_y,
                source_col_offset,
            });

            let (entry, newly_inserted) = resources.atlas.get_or_insert(
                queue,
                &mut resources.font_system,
                &mut resources.swash_cache,
                physical.cache_key,
            );

            if let Some(entry) = entry {
                let gx = base_x + physical.x as f32 + entry.placement_left as f32;
                let gy =
                    base_y + layout_run.line_y + physical.y as f32 - entry.placement_top as f32;

                fg_instances.push(CellFgInstance {
                    pos: [gx, gy],
                    size: [entry.width as f32, entry.height as f32],
                    uv_min: [entry.uv[0], entry.uv[1]],
                    uv_max: [entry.uv[2], entry.uv[3]],
                    color: run.fg,
                    is_color: if entry.is_color { 1.0 } else { 0.0 },
                    _pad: [0.0; 3],
                });

                let glyph_col = run.col_start + source_col_offset;
                cached_glyphs.push(CachedGlyph {
                    col: glyph_col,
                    row: run.row,
                    pos: [gx, gy],
                    size: [entry.width as f32, entry.height as f32],
                    uv_min: [entry.uv[0], entry.uv[1]],
                    uv_max: [entry.uv[2], entry.uv[3]],
                    is_color: if entry.is_color { 1.0 } else { 0.0 },
                });

                if newly_inserted {
                    resources.atlas_bind_group_dirty = true;
                }
            }
        }
    }

    resources.shape_cache.insert(cache_key, shaped_entries);
}

/// Map a byte offset in run text to the cell column index within the run.
pub(crate) fn byte_offset_to_col_offset(cell_byte_offsets: &[usize], byte_pos: usize) -> usize {
    cell_byte_offsets
        .partition_point(|&o| o <= byte_pos)
        .saturating_sub(1)
}

/// Shape a single cell's text with cosmic-text (used for cursor text overlay).
///
/// Uses a shape cache to avoid re-running cosmic-text shaping for
/// previously seen (text, bold, italic) combinations. The atlas already
/// caches rasterized bitmaps, but getting the CacheKey requires shaping
/// which is the expensive part (Buffer alloc + font lookup + shaping).
#[allow(clippy::too_many_arguments)]
pub(crate) fn shape_and_rasterize(
    text: &str,
    bold: bool,
    italic: bool,
    color: [f32; 4],
    cell_px: f32,
    cell_py: f32,
    cell_width: f32,
    cell_height: f32,
    pixels_per_point: f32,
    resources: &mut TerminalGpuResources,
    queue: &wgpu::Queue,
    fg_instances: &mut Vec<CellFgInstance>,
) {
    let cache_key = (text.to_string(), bold, italic);

    // Check shape cache first to avoid cosmic-text shaping.
    if let Some(shaped) = resources.shape_cache.get(&cache_key) {
        let shaped = shaped.clone(); // Clone to release borrow on resources
        for sg in &shaped {
            let (entry, newly_inserted) = resources.atlas.get_or_insert(
                queue,
                &mut resources.font_system,
                &mut resources.swash_cache,
                sg.cache_key,
            );
            if let Some(entry) = entry {
                let gx = cell_px + sg.physical_x as f32 + entry.placement_left as f32;
                let gy = cell_py + sg.line_y + sg.physical_y as f32 - entry.placement_top as f32;
                fg_instances.push(CellFgInstance {
                    pos: [gx, gy],
                    size: [entry.width as f32, entry.height as f32],
                    uv_min: [entry.uv[0], entry.uv[1]],
                    uv_max: [entry.uv[2], entry.uv[3]],
                    color,
                    is_color: if entry.is_color { 1.0 } else { 0.0 },
                    _pad: [0.0; 3],
                });
                if newly_inserted {
                    resources.atlas_bind_group_dirty = true;
                }
            }
        }
        return;
    }

    // Cache miss: run full cosmic-text shaping.
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
        .family(cosmic_text::fontdb::Family::Monospace)
        .weight(weight)
        .style(style);

    let phys_metrics = Metrics::new(
        resources.metrics.font_size * pixels_per_point,
        resources.metrics.line_height * pixels_per_point,
    );
    let mut buffer = Buffer::new_empty(phys_metrics);
    {
        let mut borrowed = buffer.borrow_with(&mut resources.font_system);
        borrowed.set_size(Some(cell_width * 2.0), Some(cell_height));
        borrowed.set_text(text, attrs, Shaping::Advanced);
        borrowed.shape_until_scroll(true);
    }

    let mut shaped_entries = Vec::new();

    for run in buffer.layout_runs() {
        for glyph in run.glyphs.iter() {
            let physical = glyph.physical((0.0, 0.0), 1.0);

            shaped_entries.push(ShapedGlyphEntry {
                physical_x: physical.x,
                physical_y: physical.y,
                cache_key: physical.cache_key,
                line_y: run.line_y,
                source_col_offset: 0, // single-cell: always column 0
            });

            let (entry, newly_inserted) = resources.atlas.get_or_insert(
                queue,
                &mut resources.font_system,
                &mut resources.swash_cache,
                physical.cache_key,
            );

            if let Some(entry) = entry {
                let gx = cell_px + physical.x as f32 + entry.placement_left as f32;
                let gy = cell_py + run.line_y + physical.y as f32 - entry.placement_top as f32;

                fg_instances.push(CellFgInstance {
                    pos: [gx, gy],
                    size: [entry.width as f32, entry.height as f32],
                    uv_min: [entry.uv[0], entry.uv[1]],
                    uv_max: [entry.uv[2], entry.uv[3]],
                    color,
                    is_color: if entry.is_color { 1.0 } else { 0.0 },
                    _pad: [0.0; 3],
                });

                if newly_inserted {
                    resources.atlas_bind_group_dirty = true;
                }
            }
        }
    }

    resources.shape_cache.insert(cache_key, shaped_entries);
}
