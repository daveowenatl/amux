use std::collections::{HashMap, HashSet};

use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache};
use egui_wgpu::wgpu;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use wezterm_surface::{CursorShape, CursorVisibility};

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
    cursor_visibility: CursorVisibility,
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
            cursor_visibility: CursorVisibility::Visible,
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
            || self.cursor_x != snap.cursor.x
            || self.cursor_y != snap.cursor.y
            || self.cursor_visibility != snap.cursor.visibility
            || self.cursor_shape != snap.cursor.shape
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
        self.cursor_x = snap.cursor.x;
        self.cursor_y = snap.cursor.y;
        self.cursor_visibility = snap.cursor.visibility;
        self.cursor_shape = snap.cursor.shape;
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

/// Cached result of shaping a single cell's text through cosmic-text.
/// Avoids re-running the full shaping pipeline on every frame for unchanged glyphs.
#[derive(Clone)]
pub(crate) struct ShapedGlyphEntry {
    /// Physical glyph x offset within the cell.
    physical_x: i32,
    /// Physical glyph y offset within the cell.
    physical_y: i32,
    /// cosmic-text cache key for atlas lookup.
    cache_key: cosmic_text::CacheKey,
    /// Baseline y from layout run.
    line_y: f32,
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

            for cell in &snap.cells {
                if cell.text.is_empty() || cell.text == " " {
                    continue;
                }

                // Check for procedurally-rendered box-drawing / block characters.
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

                let color = maybe_linearize(cell.fg, linearize);
                let prev_len = fg.len();

                shape_and_rasterize(
                    &cell.text,
                    cell.bold,
                    cell.italic,
                    color,
                    self.phys_rect.x + cell.col as f32 * self.cell_width,
                    self.phys_rect.y + cell.row as f32 * self.cell_height,
                    self.cell_width,
                    self.cell_height,
                    pixels_per_point,
                    resources,
                    queue,
                    &mut fg,
                );

                // Cache the glyph layout (position/UV) for color-only rebuilds.
                for inst in &fg[prev_len..] {
                    cached.push(CachedGlyph {
                        col: cell.col,
                        row: cell.row,
                        pos: inst.pos,
                        size: inst.size,
                        uv_min: inst.uv_min,
                        uv_max: inst.uv_max,
                        is_color: inst.is_color,
                    });
                }
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

        // --- Cursor ---
        let cursor = &snap.cursor;
        if snap.is_focused
            && snap.scroll_offset == 0
            && cursor.visibility == CursorVisibility::Visible
            && cursor.y >= 0
            && (cursor.y as usize) < snap.rows
            && cursor.x < snap.cols
        {
            let cx = self.phys_rect.x + cursor.x as f32 * self.cell_width;
            let cy = self.phys_rect.y + cursor.y as f32 * self.cell_height;
            let cursor_bg = maybe_linearize(snap.cursor_bg, linearize);

            match cursor.shape {
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

/// Shape text with cosmic-text and rasterize glyphs into the atlas,
/// appending foreground instances for each glyph.
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
