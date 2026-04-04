use std::collections::{HashMap, HashSet};

use amux_term::backend::{CursorShape, UnderlineStyle};
use amux_term::font::DecorationMetrics;
use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache};
use egui_wgpu::wgpu;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};

use crate::atlas::GlyphAtlas;
use crate::pipeline::{
    ensure_instance_buffer, BackgroundPipeline, CellBgInstance, CellFgInstance, ForegroundPipeline,
    ImagePipeline, ImageQuadInstance,
};
use crate::snapshot::TerminalSnapshot;

#[allow(clippy::too_many_arguments)]
/// Emit procedural rectangle quads for a custom glyph (box-drawing, block, shade).
/// Returns `true` if the character was handled, `false` if it should fall through
/// to normal font shaping.
fn emit_custom_glyph(
    ch: char,
    col: usize,
    row: usize,
    fg_color: [f32; 4],
    phys_x: f32,
    phys_y: f32,
    cell_width: f32,
    cell_height: f32,
    bg_instances: &mut Vec<CellBgInstance>,
) -> bool {
    if let Some(rects) = crate::custom_glyphs::custom_glyph_rects(ch) {
        let cell_px = phys_x + col as f32 * cell_width;
        let cell_py = phys_y + row as f32 * cell_height;
        for r in rects {
            // Round both edges and derive size from the difference so adjacent
            // cells meet exactly without gaps or overlaps at fractional DPI.
            let x0 = (cell_px + r.x * cell_width).round();
            let y0 = (cell_py + r.y * cell_height).round();
            let x1 = (cell_px + (r.x + r.w) * cell_width).round();
            let y1 = (cell_py + (r.y + r.h) * cell_height).round();
            let pw = (x1 - x0).max(1.0);
            let ph = (y1 - y0).max(1.0);
            bg_instances.push(CellBgInstance {
                pos: [x0, y0],
                size: [pw, ph],
                color: fg_color,
            });
        }
        true
    } else {
        false
    }
}

/// Compute a simple hash of highlight ranges for dirty tracking.
fn hash_highlight_ranges(ranges: &[(usize, usize, usize)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ranges.hash(&mut hasher);
    hasher.finish()
}

/// Cached glyph position/UV from a previous full reshape.
/// Stored per visible glyph so we can rebuild fg instances with new colors
/// without re-running cosmic-text shaping.
#[derive(Clone)]
struct CachedGlyph {
    col: usize,
    row: usize,
    pos: [f32; 2],
    size: [f32; 2],
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    is_color: f32,
}

/// Per-pane GPU state: instance buffers and dirty-tracking fingerprint.
pub struct PaneRenderState {
    /// False until first successful prepare; forces initial rebuild.
    initialized: bool,
    // Dirty-tracking fields — terminal state
    seqno: usize,
    cursor_x: usize,
    cursor_y: i64,
    cursor_visible: bool,
    cursor_blink_hidden: bool,
    cursor_shape: CursorShape,
    scroll_offset: usize,
    is_focused: bool,
    selection_range: Option<((usize, usize), (usize, usize))>,
    highlight_hash: u64,
    current_highlight: Option<usize>,

    // Dirty-tracking fields — geometry (pane position/size, cell dimensions)
    rect_x: f32,
    rect_y: f32,
    rect_w: f32,
    rect_h: f32,
    cell_width: f32,
    cell_height: f32,

    // Per-pane GPU instance buffers
    pub bg_buffer: wgpu::Buffer,
    pub bg_capacity: usize,
    pub bg_count: u32,
    pub fg_buffer: wgpu::Buffer,
    pub fg_capacity: usize,
    pub fg_count: u32,

    /// Image draw calls for this frame: (image_hash, instance_buffer, instance_count).
    pub image_draws: Vec<([u8; 32], wgpu::Buffer, u32)>,

    /// Cached glyph layouts from last full reshape. Reused when only colors change
    /// (e.g., selection or highlight updates) to avoid expensive cosmic-text shaping.
    cached_glyph_layouts: Vec<CachedGlyph>,
}

impl PaneRenderState {
    fn new(device: &wgpu::Device) -> Self {
        let initial_capacity = 1024;
        let bg_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pane_bg_instance_buffer"),
            size: (initial_capacity * std::mem::size_of::<CellBgInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fg_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pane_fg_instance_buffer"),
            size: (initial_capacity * std::mem::size_of::<CellFgInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            initialized: false,
            seqno: 0,
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: true,
            cursor_blink_hidden: false,
            cursor_shape: CursorShape::Default,
            scroll_offset: 0,
            is_focused: false,
            selection_range: None,
            highlight_hash: 0,
            current_highlight: None,
            rect_x: 0.0,
            rect_y: 0.0,
            rect_w: 0.0,
            rect_h: 0.0,
            cell_width: 0.0,
            cell_height: 0.0,
            bg_buffer,
            bg_capacity: initial_capacity,
            bg_count: 0,
            fg_buffer,
            fg_capacity: initial_capacity,
            fg_count: 0,
            image_draws: Vec::new(),
            cached_glyph_layouts: Vec::new(),
        }
    }

    /// Check if terminal content or geometry changed (requires full reshape).
    fn is_content_dirty(
        &self,
        snap: &TerminalSnapshot,
        rect: &PhysRect,
        cell_w: f32,
        cell_h: f32,
    ) -> bool {
        !self.initialized
            || self.seqno != snap.seqno
            || self.cursor_x != snap.cursor_x
            || self.cursor_y != snap.cursor_y
            || self.cursor_visible != snap.cursor_visible
            || self.cursor_shape != snap.cursor_shape
            || self.scroll_offset != snap.scroll_offset
            || self.is_focused != snap.is_focused
            || self.rect_x != rect.x
            || self.rect_y != rect.y
            || self.rect_w != rect.width
            || self.rect_h != rect.height
            || self.cell_width != cell_w
            || self.cell_height != cell_h
    }

    /// Check if only appearance changed (selection/highlights — can reuse cached glyphs).
    fn is_appearance_dirty(&self, snap: &TerminalSnapshot) -> bool {
        self.selection_range != snap.selection_range
            || self.highlight_hash != hash_highlight_ranges(&snap.highlight_ranges)
            || self.current_highlight != snap.current_highlight
            || self.cursor_blink_hidden != snap.cursor_blink_hidden
    }

    /// Update the fingerprint to match the current state.
    fn update_fingerprint(
        &mut self,
        snap: &TerminalSnapshot,
        rect: &PhysRect,
        cell_w: f32,
        cell_h: f32,
    ) {
        self.initialized = true;
        self.seqno = snap.seqno;
        self.cursor_x = snap.cursor_x;
        self.cursor_y = snap.cursor_y;
        self.cursor_visible = snap.cursor_visible;
        self.cursor_blink_hidden = snap.cursor_blink_hidden;
        self.cursor_shape = snap.cursor_shape;
        self.scroll_offset = snap.scroll_offset;
        self.is_focused = snap.is_focused;
        self.selection_range = snap.selection_range;
        self.highlight_hash = hash_highlight_ranges(&snap.highlight_ranges);
        self.current_highlight = snap.current_highlight;
        self.rect_x = rect.x;
        self.rect_y = rect.y;
        self.rect_w = rect.width;
        self.rect_h = rect.height;
        self.cell_width = cell_w;
        self.cell_height = cell_h;
    }
}

/// Cached GPU texture for an inline terminal image.
pub struct ImageTextureEntry {
    #[allow(dead_code)]
    pub texture: wgpu::Texture,
    pub bind_group: wgpu::BindGroup,
}

/// Cached result of shaping a text run through cosmic-text.
/// Avoids re-running the full shaping pipeline on every frame for unchanged glyphs.
#[derive(Clone)]
pub(crate) struct ShapedGlyphEntry {
    /// Physical glyph x offset within the run.
    physical_x: i32,
    /// Physical glyph y offset within the run.
    physical_y: i32,
    /// cosmic-text cache key for atlas lookup.
    cache_key: cosmic_text::CacheKey,
    /// Baseline y from layout run.
    line_y: f32,
    /// Cell column offset within the run (0-based), for CachedGlyph mapping.
    source_col_offset: usize,
}

/// A contiguous run of same-style cells to be shaped together for ligature support.
struct TextRun {
    row: usize,
    col_start: usize,
    col_count: usize,
    text: String,
    /// Byte offset where each cell's text starts within `text`.
    cell_byte_offsets: Vec<usize>,
    bold: bool,
    italic: bool,
    faint: bool,
    fg: [f32; 4],
}

/// Key for the shape cache: (text content, bold, italic).
pub(crate) type ShapeCacheKey = (String, bool, bool);

/// Resources stored in egui's `CallbackResources` for the terminal renderer.
pub struct TerminalGpuResources {
    pub bg_pipeline: BackgroundPipeline,
    pub fg_pipeline: ForegroundPipeline,
    pub img_pipeline: ImagePipeline,
    pub atlas: GlyphAtlas,
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub metrics: Metrics,
    pub decoration_metrics: DecorationMetrics,
    pub atlas_bind_group_dirty: bool,
    /// Whether the render target uses an sRGB format. When true, colors must
    /// be converted to linear space because the hardware applies linear→sRGB
    /// on store. When false (e.g., Bgra8Unorm on macOS), sRGB values are
    /// passed through directly.
    pub target_is_srgb: bool,
    /// Per-pane render state (instance buffers + dirty tracking).
    pub pane_states: HashMap<u64, PaneRenderState>,
    /// Cached GPU textures for inline images, keyed by image hash.
    pub image_cache: HashMap<[u8; 32], ImageTextureEntry>,
    /// Shape cache: maps (text, bold, italic) → shaped glyph data.
    /// Avoids re-running cosmic-text shaping for previously seen glyphs.
    pub shape_cache: HashMap<ShapeCacheKey, Vec<ShapedGlyphEntry>>,
    /// Shared sampler for image textures.
    pub image_sampler: wgpu::Sampler,
    /// Cached curly underline atlas tile (one cell-width sine wave).
    pub curly_tile: Option<crate::atlas::AtlasEntry>,
    /// Cached dotted underline atlas tile (row of circles).
    pub dotted_tile: Option<crate::atlas::AtlasEntry>,
    /// Last pixels_per_point used for shape cache and decoration tiles.
    /// When DPI changes, these caches are invalidated.
    pub last_pixels_per_point: f32,
}

impl TerminalGpuResources {
    /// Remove render state for panes that are no longer alive.
    pub fn retain_panes(&mut self, live_pane_ids: &[u64]) {
        let live: HashSet<u64> = live_pane_ids.iter().copied().collect();
        self.pane_states.retain(|id, _| live.contains(id));
    }

    /// Remove image textures not referenced by any current pane's image draws.
    pub fn evict_unused_images(&mut self) {
        let mut referenced: HashSet<[u8; 32]> = HashSet::new();
        for state in self.pane_states.values() {
            for (hash, _, _) in &state.image_draws {
                referenced.insert(*hash);
            }
        }
        self.image_cache.retain(|hash, _| referenced.contains(hash));
    }
}

/// Paint callback for a single terminal pane.
pub struct TerminalPaintCallback {
    pub pane_id: u64,
    pub snapshot: TerminalSnapshot,
    pub phys_rect: PhysRect,
    pub cell_width: f32,
    pub cell_height: f32,
}

/// Physical pixel rectangle for the pane area.
pub struct PhysRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl CallbackTrait for TerminalPaintCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let resources = callback_resources
            .get_mut::<TerminalGpuResources>()
            .expect("TerminalGpuResources not initialized");

        let pixels_per_point = screen_descriptor.pixels_per_point;

        // Invalidate caches when DPI/scale factor changes.
        if (resources.last_pixels_per_point - pixels_per_point).abs() > f32::EPSILON {
            resources.shape_cache.clear();
            resources.curly_tile = None;
            resources.dotted_tile = None;
            resources.atlas_bind_group_dirty = true;
            resources.last_pixels_per_point = pixels_per_point;
        }

        let viewport_width = screen_descriptor.size_in_pixels[0] as f32;
        let viewport_height = screen_descriptor.size_in_pixels[1] as f32;
        let target_is_srgb = resources.target_is_srgb;
        resources.bg_pipeline.upload_viewport(
            queue,
            viewport_width,
            viewport_height,
            target_is_srgb,
        );
        resources.fg_pipeline.upload_viewport(
            queue,
            viewport_width,
            viewport_height,
            target_is_srgb,
        );
        resources.img_pipeline.upload_viewport(
            queue,
            viewport_width,
            viewport_height,
            target_is_srgb,
        );

        // Get or create per-pane state.
        let pane_state = resources
            .pane_states
            .entry(self.pane_id)
            .or_insert_with(|| PaneRenderState::new(device));

        let content_dirty = pane_state.is_content_dirty(
            &self.snapshot,
            &self.phys_rect,
            self.cell_width,
            self.cell_height,
        );
        let appearance_dirty = pane_state.is_appearance_dirty(&self.snapshot);

        // Skip if nothing changed at all.
        if !content_dirty && !appearance_dirty {
            return Vec::new();
        }

        let snap = &self.snapshot;
        let linearize = resources.target_is_srgb;

        // --- Background instances (span-based to avoid hairline gaps) ---
        let mut bg_instances = Vec::with_capacity(snap.cells.len() / 4 + 1);

        // Full-rect background quad (absolute physical pixel positions)
        let default_bg = maybe_linearize(snap.default_bg, linearize);
        bg_instances.push(CellBgInstance {
            pos: [self.phys_rect.x, self.phys_rect.y],
            size: [self.phys_rect.width, self.phys_rect.height],
            color: default_bg,
        });

        // Batch consecutive cells on the same row with the same bg into spans.
        // This eliminates sub-pixel gaps between adjacent cell backgrounds.
        {
            let mut span_row: Option<usize> = None;
            let mut span_col_start: usize = 0;
            let mut span_col_end: usize = 0;
            let mut span_bg: [f32; 4] = [0.0; 4];

            let flush_span = |instances: &mut Vec<CellBgInstance>,
                              row: usize,
                              col_start: usize,
                              col_end: usize,
                              color: [f32; 4],
                              phys_rect: &PhysRect,
                              cell_w: f32,
                              cell_h: f32| {
                let px = phys_rect.x + col_start as f32 * cell_w;
                let py = phys_rect.y + row as f32 * cell_h;
                let w = (col_end - col_start) as f32 * cell_w;
                instances.push(CellBgInstance {
                    pos: [px, py],
                    size: [w, cell_h],
                    color,
                });
            };

            for cell in &snap.cells {
                if cell.bg == snap.default_bg {
                    // Flush pending span
                    if let Some(r) = span_row {
                        flush_span(
                            &mut bg_instances,
                            r,
                            span_col_start,
                            span_col_end,
                            span_bg,
                            &self.phys_rect,
                            self.cell_width,
                            self.cell_height,
                        );
                        span_row = None;
                    }
                    continue;
                }

                let bg = maybe_linearize(cell.bg, linearize);

                // Try to extend current span
                if let Some(r) = span_row {
                    if cell.row == r && cell.col == span_col_end && bg == span_bg {
                        span_col_end = cell.col + 1;
                        continue;
                    }
                    // Flush previous span
                    flush_span(
                        &mut bg_instances,
                        r,
                        span_col_start,
                        span_col_end,
                        span_bg,
                        &self.phys_rect,
                        self.cell_width,
                        self.cell_height,
                    );
                }

                // Start new span
                span_row = Some(cell.row);
                span_col_start = cell.col;
                span_col_end = cell.col + 1;
                span_bg = bg;
            }

            // Flush last span
            if let Some(r) = span_row {
                flush_span(
                    &mut bg_instances,
                    r,
                    span_col_start,
                    span_col_end,
                    span_bg,
                    &self.phys_rect,
                    self.cell_width,
                    self.cell_height,
                );
            }
        }

        // --- Foreground instances (glyphs) ---
        // When only appearance changed (selection/highlights), reuse cached glyph
        // layouts and just update colors. This skips expensive cosmic-text shaping.
        let mut fg_instances = if content_dirty {
            // Full reshape needed.
            let mut fg = Vec::with_capacity(snap.cells.len());
            let mut cached = Vec::new();

            // --- Run-based shaping for ligature support ---
            // Group adjacent same-style cells into text runs, then shape each
            // run as a unit so cosmic-text / HarfBuzz can produce ligatures.
            let cursor_col = snap.cursor_x;
            let cursor_row = snap.cursor_y as usize;
            let cursor_breaks = snap.cursor_visible && snap.scroll_offset == 0;

            let mut runs: Vec<TextRun> = Vec::new();
            let mut current_run: Option<TextRun> = None;

            for cell in &snap.cells {
                // Skip empty / space cells (implicitly breaks runs).
                if cell.text.is_empty() || cell.text == " " {
                    if let Some(run) = current_run.take() {
                        runs.push(run);
                    }
                    continue;
                }

                // Custom box-drawing / block glyphs: emit directly, break run.
                if cell.text.len() <= 4 {
                    if let Some(ch) = cell.text.chars().next() {
                        if cell.text.chars().nth(1).is_none() {
                            let fg_color = maybe_linearize(cell.fg, linearize);
                            if emit_custom_glyph(
                                ch,
                                cell.col,
                                cell.row,
                                fg_color,
                                self.phys_rect.x,
                                self.phys_rect.y,
                                self.cell_width,
                                self.cell_height,
                                &mut bg_instances,
                            ) {
                                if let Some(run) = current_run.take() {
                                    runs.push(run);
                                }
                                continue;
                            }
                        }
                    }
                }

                let cell_fg = maybe_linearize(cell.fg, linearize);
                let is_cursor_cell =
                    cursor_breaks && cell.col == cursor_col && cell.row == cursor_row;

                // Flush before cursor cell so it becomes its own run.
                if is_cursor_cell {
                    if let Some(run) = current_run.take() {
                        runs.push(run);
                    }
                }

                // Check if cell can extend current run.
                let can_extend = match &current_run {
                    Some(run) => {
                        cell.row == run.row
                            && cell.col == run.col_start + run.col_count
                            && cell.bold == run.bold
                            && cell.italic == run.italic
                            && cell.faint == run.faint
                            && cell_fg == run.fg
                            && !is_cursor_cell
                    }
                    None => false,
                };

                if can_extend {
                    let run = current_run.as_mut().unwrap();
                    run.cell_byte_offsets.push(run.text.len());
                    run.text.push_str(&cell.text);
                    run.col_count += 1;
                } else {
                    if let Some(run) = current_run.take() {
                        runs.push(run);
                    }
                    let mut text = String::with_capacity(cell.text.len());
                    text.push_str(&cell.text);
                    current_run = Some(TextRun {
                        row: cell.row,
                        col_start: cell.col,
                        col_count: 1,
                        cell_byte_offsets: vec![0],
                        text,
                        bold: cell.bold,
                        italic: cell.italic,
                        faint: cell.faint,
                        fg: cell_fg,
                    });
                }

                // Flush after cursor cell so it stands alone.
                if is_cursor_cell {
                    if let Some(run) = current_run.take() {
                        runs.push(run);
                    }
                }
            }
            if let Some(run) = current_run.take() {
                runs.push(run);
            }

            // Shape each run.
            for run in &runs {
                shape_run(
                    run,
                    self.cell_width,
                    self.cell_height,
                    self.phys_rect.x,
                    self.phys_rect.y,
                    pixels_per_point,
                    resources,
                    queue,
                    &mut fg,
                    &mut cached,
                );
            }

            // Store cache for future appearance-only rebuilds.
            // Re-borrow pane_state to update the cache.
            resources
                .pane_states
                .get_mut(&self.pane_id)
                .unwrap()
                .cached_glyph_layouts = cached;

            fg
        } else {
            // Appearance-only change: reuse cached glyph layouts, apply new colors.
            // Build a color lookup from snapshot cells.
            let mut color_map: HashMap<(usize, usize), [f32; 4]> = HashMap::new();
            for cell in &snap.cells {
                if cell.text.is_empty() || cell.text == " " {
                    continue;
                }
                // Re-emit custom glyph rects (they live in bg_instances, not cached glyphs).
                if cell.text.len() <= 4 {
                    if let Some(ch) = cell.text.chars().next() {
                        if cell.text.chars().nth(1).is_none() {
                            let fg_color = maybe_linearize(cell.fg, linearize);
                            if emit_custom_glyph(
                                ch,
                                cell.col,
                                cell.row,
                                fg_color,
                                self.phys_rect.x,
                                self.phys_rect.y,
                                self.cell_width,
                                self.cell_height,
                                &mut bg_instances,
                            ) {
                                continue;
                            }
                        }
                    }
                }
                color_map.insert((cell.col, cell.row), maybe_linearize(cell.fg, linearize));
            }

            let cached = &pane_state.cached_glyph_layouts;
            let mut fg = Vec::with_capacity(cached.len());
            for glyph in cached {
                let color = color_map
                    .get(&(glyph.col, glyph.row))
                    .copied()
                    .unwrap_or([1.0, 1.0, 1.0, 1.0]);
                fg.push(CellFgInstance {
                    pos: glyph.pos,
                    size: glyph.size,
                    uv_min: glyph.uv_min,
                    uv_max: glyph.uv_max,
                    color,
                    is_color: glyph.is_color,
                    _pad: [0.0; 3],
                });
            }
            fg
        };

        // --- Text decorations (underlines, strikethrough) ---
        // Uses font metrics from OpenType tables for accurate positioning.
        // Curly and dotted underlines are rendered as anti-aliased atlas tiles.
        {
            let dm = &resources.decoration_metrics;
            let line_thickness = (dm.stroke_size * pixels_per_point).max(1.0);
            // Underline: below baseline. The font's underline_offset is distance
            // from baseline (positive = below). Baseline is at ~line_top + ascent.
            // For a terminal cell: baseline is approximately at line_y from cosmic-text.
            // We use the font's own metric scaled by ppem.
            let baseline_y = self.cell_height * 0.78; // approximate baseline
            let underline_y_offset = (baseline_y + dm.underline_offset * pixels_per_point).max(0.0);
            let strikethrough_y_offset =
                (baseline_y - dm.strikeout_offset * pixels_per_point - line_thickness / 2.0)
                    .max(0.0);

            // Lazily rasterize curly/dotted tiles into the atlas on first use.
            if resources.curly_tile.is_none() {
                let (w, h, data) = rasterize_curly_tile(self.cell_width, line_thickness);
                if let Some(entry) = resources.atlas.insert_raw_mono(queue, w, h, &data) {
                    resources.curly_tile = Some(entry);
                    resources.atlas_bind_group_dirty = true;
                }
            }
            if resources.dotted_tile.is_none() {
                let (w, h, data) = rasterize_dotted_tile(self.cell_width, line_thickness);
                if let Some(entry) = resources.atlas.insert_raw_mono(queue, w, h, &data) {
                    resources.dotted_tile = Some(entry);
                    resources.atlas_bind_group_dirty = true;
                }
            }
            let curly_tile = resources.curly_tile;
            let dotted_tile = resources.dotted_tile;

            for cell in &snap.cells {
                let mut fg_color = maybe_linearize(cell.fg, linearize);
                if cell.faint {
                    fg_color[3] *= 0.5;
                }

                let px = self.phys_rect.x + cell.col as f32 * self.cell_width;
                let py = self.phys_rect.y + cell.row as f32 * self.cell_height;

                // Strikethrough
                if cell.strikethrough {
                    bg_instances.push(CellBgInstance {
                        pos: [px, py + strikethrough_y_offset],
                        size: [self.cell_width, line_thickness],
                        color: fg_color,
                    });
                }

                // Underline
                match cell.underline {
                    UnderlineStyle::None => {}
                    UnderlineStyle::Single => {
                        let color = cell
                            .underline_color
                            .map(|c| maybe_linearize(c, linearize))
                            .unwrap_or(fg_color);
                        bg_instances.push(CellBgInstance {
                            pos: [px, py + underline_y_offset],
                            size: [self.cell_width, line_thickness],
                            color,
                        });
                    }
                    UnderlineStyle::Double => {
                        let color = cell
                            .underline_color
                            .map(|c| maybe_linearize(c, linearize))
                            .unwrap_or(fg_color);
                        // Ghostty: gap = thickness (1:1 ratio)
                        bg_instances.push(CellBgInstance {
                            pos: [px, py + underline_y_offset - line_thickness],
                            size: [self.cell_width, line_thickness],
                            color,
                        });
                        bg_instances.push(CellBgInstance {
                            pos: [px, py + underline_y_offset + line_thickness],
                            size: [self.cell_width, line_thickness],
                            color,
                        });
                    }
                    UnderlineStyle::Dotted => {
                        let color = cell
                            .underline_color
                            .map(|c| maybe_linearize(c, linearize))
                            .unwrap_or(fg_color);
                        // Use atlas tile for anti-aliased circles
                        if let Some(tile) = dotted_tile {
                            let tile_h = tile.height as f32;
                            fg_instances.push(CellFgInstance {
                                pos: [px, py + underline_y_offset - tile_h / 2.0],
                                size: [tile.width as f32, tile_h],
                                uv_min: [tile.uv[0], tile.uv[1]],
                                uv_max: [tile.uv[2], tile.uv[3]],
                                color,
                                is_color: 0.0,
                                _pad: [0.0; 3],
                            });
                        } else {
                            // Fallback: rect-based dots
                            let dot_w = (line_thickness * 1.5).max(2.0);
                            let mut x = px;
                            let x_end = px + self.cell_width;
                            while x < x_end {
                                let w = dot_w.min(x_end - x);
                                bg_instances.push(CellBgInstance {
                                    pos: [x, py + underline_y_offset],
                                    size: [w, line_thickness],
                                    color,
                                });
                                x += dot_w * 2.0;
                            }
                        }
                    }
                    UnderlineStyle::Dashed => {
                        let color = cell
                            .underline_color
                            .map(|c| maybe_linearize(c, linearize))
                            .unwrap_or(fg_color);
                        // Ghostty: width/3 + 1px, every-other pattern
                        let dash_w = self.cell_width / 3.0 + 1.0;
                        let dash_count = ((self.cell_width / dash_w).ceil() as u32 + 1).max(1);
                        for i in (0..dash_count).step_by(2) {
                            let x = px + i as f32 * dash_w;
                            let w = dash_w.min(px + self.cell_width - x);
                            if w > 0.0 {
                                bg_instances.push(CellBgInstance {
                                    pos: [x, py + underline_y_offset],
                                    size: [w, line_thickness],
                                    color,
                                });
                            }
                        }
                    }
                    UnderlineStyle::Curly => {
                        let color = cell
                            .underline_color
                            .map(|c| maybe_linearize(c, linearize))
                            .unwrap_or(fg_color);
                        // Use atlas tile for anti-aliased sine wave
                        if let Some(tile) = curly_tile {
                            let tile_h = tile.height as f32;
                            fg_instances.push(CellFgInstance {
                                pos: [px, py + underline_y_offset - tile_h / 2.0],
                                size: [tile.width as f32, tile_h],
                                uv_min: [tile.uv[0], tile.uv[1]],
                                uv_max: [tile.uv[2], tile.uv[3]],
                                color,
                                is_color: 0.0,
                                _pad: [0.0; 3],
                            });
                        } else {
                            // Fallback: rect-based wave
                            let segments = 8u32;
                            let seg_w = self.cell_width / segments as f32;
                            let amplitude = line_thickness * 1.5;
                            let y_base = py + underline_y_offset;
                            for i in 0..segments {
                                let t = i as f32 / segments as f32;
                                let angle = t * std::f32::consts::TAU;
                                let y_off = angle.sin() * amplitude;
                                bg_instances.push(CellBgInstance {
                                    pos: [px + i as f32 * seg_w, y_base + y_off],
                                    size: [seg_w + 0.5, line_thickness],
                                    color,
                                });
                            }
                        }
                    }
                }
            }
        }

        // --- Faint text: dim glyph colors in fg_instances for faint cells. ---
        {
            let faint_cells: HashSet<(usize, usize)> = snap
                .cells
                .iter()
                .filter(|c| c.faint)
                .map(|c| (c.col, c.row))
                .collect();
            if !faint_cells.is_empty() {
                for inst in &mut fg_instances {
                    // Map pixel position back to cell coordinates, clamping to valid
                    // range to handle glyphs with negative bearings or ligature offsets.
                    let col_f = (inst.pos[0] - self.phys_rect.x) / self.cell_width;
                    let row_f = (inst.pos[1] - self.phys_rect.y) / self.cell_height;
                    if col_f < 0.0 || row_f < 0.0 {
                        continue;
                    }
                    let col = (col_f.round() as usize).min(snap.cols.saturating_sub(1));
                    let row = (row_f.round() as usize).min(snap.rows.saturating_sub(1));
                    if faint_cells.contains(&(col, row)) {
                        inst.color[3] *= 0.5;
                    }
                }
            }
        }

        // --- Cursor ---
        if snap.is_focused
            && snap.scroll_offset == 0
            && snap.cursor_visible
            && !snap.cursor_blink_hidden
            && snap.cursor_y >= 0
            && (snap.cursor_y as usize) < snap.rows
            && snap.cursor_x < snap.cols
        {
            let cx = self.phys_rect.x + snap.cursor_x as f32 * self.cell_width;
            let cy = self.phys_rect.y + snap.cursor_y as f32 * self.cell_height;
            let cursor_bg = maybe_linearize(snap.cursor_bg, linearize);

            match snap.cursor_shape {
                CursorShape::BlinkingBar | CursorShape::SteadyBar => {
                    bg_instances.push(CellBgInstance {
                        pos: [cx, cy],
                        size: [2.0, self.cell_height],
                        color: cursor_bg,
                    });
                }
                CursorShape::BlinkingUnderline | CursorShape::SteadyUnderline => {
                    bg_instances.push(CellBgInstance {
                        pos: [cx, cy + self.cell_height - 2.0],
                        size: [self.cell_width, 2.0],
                        color: cursor_bg,
                    });
                }
                CursorShape::Default | CursorShape::BlinkingBlock | CursorShape::SteadyBlock => {
                    bg_instances.push(CellBgInstance {
                        pos: [cx, cy],
                        size: [self.cell_width, self.cell_height],
                        color: cursor_bg,
                    });
                    if !snap.cursor_text.is_empty() {
                        shape_and_rasterize(
                            &snap.cursor_text,
                            snap.cursor_text_bold,
                            snap.cursor_text_italic,
                            maybe_linearize(snap.cursor_fg, linearize),
                            cx,
                            cy,
                            self.cell_width,
                            self.cell_height,
                            pixels_per_point,
                            resources,
                            queue,
                            &mut fg_instances,
                        );
                    }
                }
            }
        }

        // --- Image textures and instances ---
        let mut image_draws: Vec<([u8; 32], wgpu::Buffer, u32)> = Vec::new();
        if !snap.images.is_empty() {
            // Upload any new image textures.
            for decoded in &snap.decoded_images {
                if !resources.image_cache.contains_key(&decoded.hash) {
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("image_texture"),
                        size: wgpu::Extent3d {
                            width: decoded.width,
                            height: decoded.height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    // Pad rows to wgpu's required alignment if needed.
                    let unpadded_bpr = 4 * decoded.width;
                    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
                    let padded_bpr = unpadded_bpr.div_ceil(align) * align;

                    if unpadded_bpr == padded_bpr {
                        queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &texture,
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: wgpu::TextureAspect::All,
                            },
                            &decoded.data,
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(unpadded_bpr),
                                rows_per_image: Some(decoded.height),
                            },
                            wgpu::Extent3d {
                                width: decoded.width,
                                height: decoded.height,
                                depth_or_array_layers: 1,
                            },
                        );
                    } else {
                        let mut padded = vec![0u8; (padded_bpr * decoded.height) as usize];
                        let src_stride = unpadded_bpr as usize;
                        let dst_stride = padded_bpr as usize;
                        for row in 0..decoded.height as usize {
                            padded[row * dst_stride..row * dst_stride + src_stride]
                                .copy_from_slice(
                                    &decoded.data[row * src_stride..row * src_stride + src_stride],
                                );
                        }
                        queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &texture,
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: wgpu::TextureAspect::All,
                            },
                            &padded,
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(padded_bpr),
                                rows_per_image: Some(decoded.height),
                            },
                            wgpu::Extent3d {
                                width: decoded.width,
                                height: decoded.height,
                                depth_or_array_layers: 1,
                            },
                        );
                    }
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("image_bind_group"),
                        layout: &resources.img_pipeline.image_bind_group_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&resources.image_sampler),
                            },
                        ],
                    });
                    resources.image_cache.insert(
                        decoded.hash,
                        ImageTextureEntry {
                            texture,
                            bind_group,
                        },
                    );
                }
            }

            // Group image placements by hash and build instance buffers.
            let mut grouped: HashMap<[u8; 32], Vec<ImageQuadInstance>> = HashMap::new();
            for placement in &snap.images {
                let px = self.phys_rect.x + placement.col as f32 * self.cell_width;
                let py = self.phys_rect.y + placement.row as f32 * self.cell_height;
                grouped
                    .entry(placement.image_hash)
                    .or_default()
                    .push(ImageQuadInstance {
                        pos: [px, py],
                        size: [self.cell_width, self.cell_height],
                        uv_min: placement.uv_min,
                        uv_max: placement.uv_max,
                    });
            }

            for (hash, instances) in grouped {
                if resources.image_cache.contains_key(&hash) {
                    let buf = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("image_instance_buffer"),
                        size: (instances.len() * std::mem::size_of::<ImageQuadInstance>()) as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    queue.write_buffer(&buf, 0, bytemuck::cast_slice(&instances));
                    image_draws.push((hash, buf, instances.len() as u32));
                }
            }
        }

        // --- Update atlas bind group if glyphs were added ---
        if resources.atlas_bind_group_dirty {
            resources.fg_pipeline.update_atlas_bind_group(
                device,
                resources.atlas.mono_texture_view(),
                resources.atlas.color_texture_view(),
                &resources.atlas.sampler,
            );
            resources.atlas_bind_group_dirty = false;
        }

        // --- Upload to per-pane buffers ---
        // Re-borrow pane_state after releasing the mutable borrow on resources.
        let pane_state = resources.pane_states.get_mut(&self.pane_id).unwrap();

        // Grow bg buffer if needed.
        if let Some((buf, cap)) = ensure_instance_buffer::<CellBgInstance>(
            device,
            Some(&pane_state.bg_buffer),
            pane_state.bg_capacity,
            bg_instances.len(),
            "pane_bg_instance_buffer",
        ) {
            pane_state.bg_buffer = buf;
            pane_state.bg_capacity = cap;
        }
        if !bg_instances.is_empty() {
            queue.write_buffer(
                &pane_state.bg_buffer,
                0,
                bytemuck::cast_slice(&bg_instances),
            );
        }
        pane_state.bg_count = bg_instances.len() as u32;

        // Grow fg buffer if needed.
        if let Some((buf, cap)) = ensure_instance_buffer::<CellFgInstance>(
            device,
            Some(&pane_state.fg_buffer),
            pane_state.fg_capacity,
            fg_instances.len(),
            "pane_fg_instance_buffer",
        ) {
            pane_state.fg_buffer = buf;
            pane_state.fg_capacity = cap;
        }
        if !fg_instances.is_empty() {
            queue.write_buffer(
                &pane_state.fg_buffer,
                0,
                bytemuck::cast_slice(&fg_instances),
            );
        }
        pane_state.fg_count = fg_instances.len() as u32;

        pane_state.image_draws = image_draws;

        pane_state.update_fingerprint(
            &self.snapshot,
            &self.phys_rect,
            self.cell_width,
            self.cell_height,
        );

        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        let resources = callback_resources
            .get::<TerminalGpuResources>()
            .expect("TerminalGpuResources not initialized");

        let Some(pane_state) = resources.pane_states.get(&self.pane_id) else {
            return;
        };

        // Override egui's per-callback viewport (which is set to the callback rect)
        // with the full window viewport. Our shader computes NDC from absolute physical
        // pixel positions, so the viewport must cover the full framebuffer. The scissor
        // rect (set by egui) still clips rendering to the callback area.
        let [w, h] = info.screen_size_px;
        render_pass.set_viewport(0.0, 0.0, w as f32, h as f32, 0.0, 1.0);

        resources
            .bg_pipeline
            .draw(render_pass, &pane_state.bg_buffer, pane_state.bg_count);
        resources
            .fg_pipeline
            .draw(render_pass, &pane_state.fg_buffer, pane_state.fg_count);

        // Render inline images on top of text.
        for (hash, instance_buffer, count) in &pane_state.image_draws {
            if let Some(entry) = resources.image_cache.get(hash) {
                resources.img_pipeline.draw(
                    render_pass,
                    &entry.bind_group,
                    instance_buffer,
                    *count,
                );
            }
        }
    }
}

/// Shape a multi-cell text run for ligature support.
///
/// Groups of adjacent same-style cells are shaped together through cosmic-text
/// so HarfBuzz can produce ligature substitutions (e.g., `=>` → single glyph).
/// Glyph positions are mapped back to cell columns via `cell_byte_offsets`.
#[allow(clippy::too_many_arguments)]
fn shape_run(
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
fn byte_offset_to_col_offset(cell_byte_offsets: &[usize], byte_pos: usize) -> usize {
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
fn shape_and_rasterize(
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

/// Convert an sRGB color to linear if the target is sRGB, otherwise pass through.
fn maybe_linearize(color: [f32; 4], target_is_srgb: bool) -> [f32; 4] {
    if target_is_srgb {
        [
            srgb_to_linear(color[0]),
            srgb_to_linear(color[1]),
            srgb_to_linear(color[2]),
            color[3],
        ]
    } else {
        color
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Rasterize a curly (wavy) underline tile into a grayscale bitmap.
/// Uses Ghostty-style approach: cubic Bézier-approximated sine wave.
/// Returns (width, height, pixel_data).
fn rasterize_curly_tile(cell_width: f32, thickness: f32) -> (u32, u32, Vec<u8>) {
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
fn rasterize_dotted_tile(cell_width: f32, thickness: f32) -> (u32, u32, Vec<u8>) {
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
