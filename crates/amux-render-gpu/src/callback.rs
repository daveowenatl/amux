use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache};
use egui_wgpu::wgpu;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};

use crate::atlas::GlyphAtlas;
use crate::pipeline::{BackgroundPipeline, CellBgInstance, CellFgInstance, ForegroundPipeline};
use crate::snapshot::TerminalSnapshot;

/// Resources stored in egui's `CallbackResources` for the terminal renderer.
///
/// Created once during `GpuRenderer::new()` and retrieved during prepare/paint.
pub struct TerminalGpuResources {
    pub bg_pipeline: BackgroundPipeline,
    pub fg_pipeline: ForegroundPipeline,
    pub atlas: GlyphAtlas,
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub metrics: Metrics,
    pub bg_instance_count: u32,
    pub fg_instance_count: u32,
    pub atlas_bind_group_dirty: bool,
}

/// Paint callback for a single terminal pane.
///
/// Built per-frame with a fresh `TerminalSnapshot`. Implements `CallbackTrait`
/// to prepare GPU buffers and draw instanced quads in egui's render pass.
pub struct TerminalPaintCallback {
    pub snapshot: TerminalSnapshot,
    /// Rect in physical pixels (already scaled by pixels_per_point).
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

        let snap = &self.snapshot;

        // Build background instances
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

        // Build foreground instances (glyphs)
        let mut fg_instances = Vec::with_capacity(snap.cells.len());

        for cell in &snap.cells {
            if cell.text.is_empty() || cell.text == " " {
                continue;
            }

            // Shape the glyph with cosmic-text to get the cache key
            let weight = if cell.bold {
                cosmic_text::Weight::BOLD
            } else {
                cosmic_text::Weight::NORMAL
            };
            let style = if cell.italic {
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
                borrowed.set_size(Some(self.cell_width * 2.0), Some(self.cell_height));
                borrowed.set_text(&cell.text, attrs, Shaping::Advanced);
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
                        let cell_px = self.phys_rect.x + cell.col as f32 * self.cell_width;
                        let cell_py = self.phys_rect.y + cell.row as f32 * self.cell_height;

                        let gx = cell_px + physical.x as f32 + entry.placement_left as f32;
                        let gy =
                            cell_py + run.line_top + physical.y as f32 - entry.placement_top as f32;

                        fg_instances.push(CellFgInstance {
                            pos: [gx, gy],
                            size: [entry.width as f32, entry.height as f32],
                            uv_min: [entry.uv[0], entry.uv[1]],
                            uv_max: [entry.uv[2], entry.uv[3]],
                            color: cell.fg,
                        });

                        resources.atlas_bind_group_dirty = true;
                    }
                }
            }
        }

        // Update atlas bind group if glyphs were added
        if resources.atlas_bind_group_dirty {
            resources.fg_pipeline.update_atlas_bind_group(
                device,
                &resources.atlas.texture_view,
                &resources.atlas.sampler,
            );
            resources.atlas_bind_group_dirty = false;
        }

        let viewport_width = screen_descriptor.size_in_pixels[0] as f32;
        let viewport_height = screen_descriptor.size_in_pixels[1] as f32;

        resources.bg_instance_count = resources.bg_pipeline.upload(
            device,
            queue,
            &bg_instances,
            viewport_width,
            viewport_height,
        );

        resources.fg_instance_count = resources.fg_pipeline.upload(
            device,
            queue,
            &fg_instances,
            viewport_width,
            viewport_height,
        );

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

        // Draw backgrounds first, then glyphs on top.
        resources
            .bg_pipeline
            .draw(render_pass, resources.bg_instance_count);
        resources
            .fg_pipeline
            .draw(render_pass, resources.fg_instance_count);
    }
}
