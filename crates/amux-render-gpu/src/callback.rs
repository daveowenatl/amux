use egui_wgpu::wgpu;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};

use crate::pipeline::{BackgroundPipeline, CellBgInstance};
use crate::snapshot::TerminalSnapshot;

/// Resources stored in egui's `CallbackResources` for the terminal renderer.
///
/// Created once during `GpuRenderer::new()` and retrieved during prepare/paint.
pub struct TerminalGpuResources {
    pub bg_pipeline: BackgroundPipeline,
    pub bg_instance_count: u32,
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

        // Build background instances: one quad per cell with non-default background.
        let mut bg_instances = Vec::with_capacity(snap.cells.len());

        for cell in &snap.cells {
            // Skip cells with default background (they're already cleared by egui).
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

        // Also draw the default background as a full-rect quad so the pane
        // has a solid background even when egui's background differs.
        // Insert it first so it's drawn behind cell-specific backgrounds.
        bg_instances.insert(
            0,
            CellBgInstance {
                pos: [self.phys_rect.x, self.phys_rect.y],
                size: [self.phys_rect.width, self.phys_rect.height],
                color: snap.default_bg,
            },
        );

        let viewport_width = screen_descriptor.size_in_pixels[0] as f32;
        let viewport_height = screen_descriptor.size_in_pixels[1] as f32;

        resources.bg_instance_count = resources.bg_pipeline.upload(
            device,
            queue,
            &bg_instances,
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

        resources
            .bg_pipeline
            .draw(render_pass, resources.bg_instance_count);
    }
}
