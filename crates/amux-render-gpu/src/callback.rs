use std::collections::HashMap;

use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache};
use egui_wgpu::wgpu;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use wezterm_surface::{CursorShape, CursorVisibility};

use crate::atlas::GlyphAtlas;
use crate::pipeline::{
    ensure_instance_buffer, BackgroundPipeline, CellBgInstance, CellFgInstance, ForegroundPipeline,
};
use crate::snapshot::TerminalSnapshot;

/// Per-pane GPU state: instance buffers and dirty-tracking fingerprint.
pub struct PaneRenderState {
    // Dirty-tracking fields
    seqno: usize,
    cursor_x: usize,
    cursor_y: i64,
    scroll_offset: usize,
    is_focused: bool,
    has_selection: bool,

    // Per-pane GPU instance buffers
    pub bg_buffer: wgpu::Buffer,
    pub bg_capacity: usize,
    pub bg_count: u32,
    pub fg_buffer: wgpu::Buffer,
    pub fg_capacity: usize,
    pub fg_count: u32,
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
            seqno: 0,
            cursor_x: 0,
            cursor_y: 0,
            scroll_offset: 0,
            is_focused: false,
            has_selection: false,
            bg_buffer,
            bg_capacity: initial_capacity,
            bg_count: 0,
            fg_buffer,
            fg_capacity: initial_capacity,
            fg_count: 0,
        }
    }

    /// Check if the pane content has changed since last render.
    fn is_dirty(&self, snap: &TerminalSnapshot) -> bool {
        self.seqno != snap.seqno
            || self.cursor_x != snap.cursor.x
            || self.cursor_y != snap.cursor.y
            || self.scroll_offset != snap.scroll_offset
            || self.is_focused != snap.is_focused
            || self.has_selection != snap.has_selection
    }

    /// Update the fingerprint to match the current snapshot.
    fn update_fingerprint(&mut self, snap: &TerminalSnapshot) {
        self.seqno = snap.seqno;
        self.cursor_x = snap.cursor.x;
        self.cursor_y = snap.cursor.y;
        self.scroll_offset = snap.scroll_offset;
        self.is_focused = snap.is_focused;
        self.has_selection = snap.has_selection;
    }
}

/// Resources stored in egui's `CallbackResources` for the terminal renderer.
pub struct TerminalGpuResources {
    pub bg_pipeline: BackgroundPipeline,
    pub fg_pipeline: ForegroundPipeline,
    pub atlas: GlyphAtlas,
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub metrics: Metrics,
    pub atlas_bind_group_dirty: bool,
    /// Per-pane render state (instance buffers + dirty tracking).
    pub pane_states: HashMap<u64, PaneRenderState>,
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

        let viewport_width = screen_descriptor.size_in_pixels[0] as f32;
        let viewport_height = screen_descriptor.size_in_pixels[1] as f32;
        resources
            .bg_pipeline
            .upload_viewport(queue, viewport_width, viewport_height);
        resources
            .fg_pipeline
            .upload_viewport(queue, viewport_width, viewport_height);

        // Get or create per-pane state.
        let pane_state = resources
            .pane_states
            .entry(self.pane_id)
            .or_insert_with(|| PaneRenderState::new(device));

        // Skip expensive instance rebuild if nothing changed.
        if !pane_state.is_dirty(&self.snapshot) {
            return Vec::new();
        }

        let snap = &self.snapshot;

        // --- Background instances ---
        let mut bg_instances = Vec::with_capacity(snap.cells.len() + 1);

        // Full-rect background quad
        bg_instances.push(CellBgInstance {
            pos: [self.phys_rect.x, self.phys_rect.y],
            size: [self.phys_rect.width, self.phys_rect.height],
            color: snap.default_bg,
        });

        for cell in &snap.cells {
            if cell.bg == snap.default_bg {
                continue;
            }
            let px = self.phys_rect.x + cell.col as f32 * self.cell_width;
            let py = self.phys_rect.y + cell.row as f32 * self.cell_height;
            bg_instances.push(CellBgInstance {
                pos: [px, py],
                size: [self.cell_width, self.cell_height],
                color: cell.bg,
            });
        }

        // --- Foreground instances (glyphs) ---
        let mut fg_instances = Vec::with_capacity(snap.cells.len());

        for cell in &snap.cells {
            if cell.text.is_empty() || cell.text == " " {
                continue;
            }

            shape_and_rasterize(
                &cell.text,
                cell.bold,
                cell.italic,
                cell.fg,
                self.phys_rect.x + cell.col as f32 * self.cell_width,
                self.phys_rect.y + cell.row as f32 * self.cell_height,
                self.cell_width,
                self.cell_height,
                resources,
                queue,
                &mut fg_instances,
            );
        }

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

            match cursor.shape {
                CursorShape::BlinkingBar | CursorShape::SteadyBar => {
                    bg_instances.push(CellBgInstance {
                        pos: [cx, cy],
                        size: [2.0, self.cell_height],
                        color: snap.cursor_bg,
                    });
                }
                CursorShape::BlinkingUnderline | CursorShape::SteadyUnderline => {
                    bg_instances.push(CellBgInstance {
                        pos: [cx, cy + self.cell_height - 2.0],
                        size: [self.cell_width, 2.0],
                        color: snap.cursor_bg,
                    });
                }
                CursorShape::Default | CursorShape::BlinkingBlock | CursorShape::SteadyBlock => {
                    bg_instances.push(CellBgInstance {
                        pos: [cx, cy],
                        size: [self.cell_width, self.cell_height],
                        color: snap.cursor_bg,
                    });
                    if !snap.cursor_text.is_empty() {
                        shape_and_rasterize(
                            &snap.cursor_text,
                            snap.cursor_text_bold,
                            false,
                            snap.cursor_fg,
                            cx,
                            cy,
                            self.cell_width,
                            self.cell_height,
                            resources,
                            queue,
                            &mut fg_instances,
                        );
                    }
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

        pane_state.update_fingerprint(&self.snapshot);

        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        let resources = callback_resources
            .get::<TerminalGpuResources>()
            .expect("TerminalGpuResources not initialized");

        let Some(pane_state) = resources.pane_states.get(&self.pane_id) else {
            return;
        };

        resources
            .bg_pipeline
            .draw(render_pass, &pane_state.bg_buffer, pane_state.bg_count);
        resources
            .fg_pipeline
            .draw(render_pass, &pane_state.fg_buffer, pane_state.fg_count);
    }
}

/// Shape text with cosmic-text and rasterize glyphs into the atlas,
/// appending foreground instances for each glyph.
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
    resources: &mut TerminalGpuResources,
    queue: &wgpu::Queue,
    fg_instances: &mut Vec<CellFgInstance>,
) {
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

    let mut buffer = Buffer::new_empty(resources.metrics);
    {
        let mut borrowed = buffer.borrow_with(&mut resources.font_system);
        borrowed.set_size(Some(cell_width * 2.0), Some(cell_height));
        borrowed.set_text(text, attrs, Shaping::Advanced);
        borrowed.shape_until_scroll(true);
    }

    for run in buffer.layout_runs() {
        for glyph in run.glyphs.iter() {
            let physical = glyph.physical((0.0, 0.0), 1.0);

            let entry = resources.atlas.get_or_insert(
                queue,
                &mut resources.font_system,
                &mut resources.swash_cache,
                physical.cache_key,
            );

            if let Some(entry) = entry {
                let gx = cell_px + physical.x as f32 + entry.placement_left as f32;
                let gy = cell_py + run.line_top + physical.y as f32 - entry.placement_top as f32;

                fg_instances.push(CellFgInstance {
                    pos: [gx, gy],
                    size: [entry.width as f32, entry.height as f32],
                    uv_min: [entry.uv[0], entry.uv[1]],
                    uv_max: [entry.uv[2], entry.uv[3]],
                    color,
                    is_color: if entry.is_color { 1.0 } else { 0.0 },
                    _pad: [0.0; 3],
                });

                resources.atlas_bind_group_dirty = true;
            }
        }
    }
}
