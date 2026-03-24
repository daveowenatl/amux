mod callback;
mod pipeline;
pub mod snapshot;

use callback::{PhysRect, TerminalGpuResources, TerminalPaintCallback};
use pipeline::BackgroundPipeline;
pub use snapshot::TerminalSnapshot;

/// GPU-accelerated terminal renderer using wgpu.
///
/// Renders terminal panes via instanced quad drawing inside egui's render pass,
/// using `egui_wgpu::CallbackTrait` for custom paint callbacks.
pub struct GpuRenderer {
    #[allow(dead_code)]
    render_state: egui_wgpu::RenderState,
    cell_width: f32,
    cell_height: f32,
}

impl GpuRenderer {
    /// Create a new GPU renderer from eframe's render state.
    ///
    /// Initializes the background pipeline and registers GPU resources
    /// in egui's callback resource map.
    pub fn new(render_state: egui_wgpu::RenderState) -> Self {
        let target_format = render_state.target_format;
        let device = &render_state.device;

        let bg_pipeline = BackgroundPipeline::new(device, target_format);

        // Register resources in egui's callback_resources for access during prepare/paint.
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(TerminalGpuResources {
                bg_pipeline,
                bg_instance_count: 0,
            });

        Self {
            render_state,
            // Placeholder cell dimensions — will be set properly when cosmic-text
            // font measurement is added in PR 8c. For now, use egui's measurements
            // passed in via paint_callback().
            cell_width: 0.0,
            cell_height: 0.0,
        }
    }

    /// Create an egui `PaintCallback` that will render the terminal pane
    /// into the given rect during egui's render pass.
    ///
    /// `snapshot` contains pre-extracted terminal state (cells, colors, cursor).
    /// `cell_width` and `cell_height` are in logical points (will be scaled by pixels_per_point).
    /// `pixels_per_point` is the current DPI scale factor.
    pub fn paint_callback(
        &self,
        rect: egui::Rect,
        snapshot: TerminalSnapshot,
        cell_width: f32,
        cell_height: f32,
        pixels_per_point: f32,
    ) -> egui::PaintCallback {
        let phys_cell_w = cell_width * pixels_per_point;
        let phys_cell_h = cell_height * pixels_per_point;

        let callback = TerminalPaintCallback {
            snapshot,
            phys_rect: PhysRect {
                x: rect.min.x * pixels_per_point,
                y: rect.min.y * pixels_per_point,
                width: rect.width() * pixels_per_point,
                height: rect.height() * pixels_per_point,
            },
            cell_width: phys_cell_w,
            cell_height: phys_cell_h,
        };
        egui_wgpu::Callback::new_paint_callback(rect, callback)
    }

    /// Get the cell width in logical points.
    ///
    /// Returns 0.0 until cosmic-text font measurement is implemented (PR 8c).
    pub fn cell_width(&self) -> f32 {
        self.cell_width
    }

    /// Get the cell height in logical points.
    ///
    /// Returns 0.0 until cosmic-text font measurement is implemented (PR 8c).
    pub fn cell_height(&self) -> f32 {
        self.cell_height
    }
}
