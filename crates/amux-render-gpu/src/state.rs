//! Per-pane GPU render state, cached glyph/shape entries, and related types.
//!
//! Extracted from `callback.rs` to keep the paint loop module focused on
//! the `CallbackTrait` impl. These types are pure data / dirty-tracking;
//! the actual GPU paint logic lives in `callback.rs`.

use amux_term::backend::CursorShape;
use egui_wgpu::wgpu;

use crate::quad::{CellBgInstance, CellFgInstance};
use crate::snapshot::TerminalSnapshot;

/// Physical pixel rectangle for the pane area.
pub struct PhysRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Compute a simple hash of highlight ranges for dirty tracking.
pub(crate) fn hash_highlight_ranges(ranges: &[(usize, usize, usize)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ranges.hash(&mut hasher);
    hasher.finish()
}

/// Cached glyph position/UV from a previous full reshape.
/// Stored per visible glyph so we can rebuild fg instances with new colors
/// without re-running cosmic-text shaping.
#[derive(Clone)]
pub(crate) struct CachedGlyph {
    pub(crate) col: usize,
    pub(crate) row: usize,
    pub(crate) pos: [f32; 2],
    pub(crate) size: [f32; 2],
    pub(crate) uv_min: [f32; 2],
    pub(crate) uv_max: [f32; 2],
    pub(crate) is_color: f32,
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
    pub(crate) cached_glyph_layouts: Vec<CachedGlyph>,
}

impl PaneRenderState {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
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
    pub(crate) fn is_content_dirty(
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
    pub(crate) fn is_appearance_dirty(&self, snap: &TerminalSnapshot) -> bool {
        self.selection_range != snap.selection_range
            || self.highlight_hash != hash_highlight_ranges(&snap.highlight_ranges)
            || self.current_highlight != snap.current_highlight
            || self.cursor_blink_hidden != snap.cursor_blink_hidden
    }

    /// Update the fingerprint to match the current state.
    pub(crate) fn update_fingerprint(
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
    pub(crate) physical_x: i32,
    /// Physical glyph y offset within the run.
    pub(crate) physical_y: i32,
    /// cosmic-text cache key for atlas lookup.
    pub(crate) cache_key: cosmic_text::CacheKey,
    /// Baseline y from layout run.
    pub(crate) line_y: f32,
    /// Cell column offset within the run (0-based), for CachedGlyph mapping.
    pub(crate) source_col_offset: usize,
}

/// A contiguous run of same-style cells to be shaped together for ligature support.
pub(crate) struct TextRun {
    pub(crate) row: usize,
    pub(crate) col_start: usize,
    pub(crate) col_count: usize,
    pub(crate) text: String,
    /// Byte offset where each cell's text starts within `text`.
    pub(crate) cell_byte_offsets: Vec<usize>,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) faint: bool,
    pub(crate) fg: [f32; 4],
}

/// Key for the shape cache: (text content, bold, italic).
pub(crate) type ShapeCacheKey = (String, bool, bool);
