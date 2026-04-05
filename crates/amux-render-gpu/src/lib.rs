mod atlas;
mod callback;
mod color;
mod custom_glyphs;
mod decorations;
mod pipeline;
pub mod snapshot;

use amux_term::font::{self, FontConfig};
use cosmic_text::{Metrics, SwashCache};

use atlas::GlyphAtlas;
use callback::{PhysRect, TerminalGpuResources, TerminalPaintCallback};
use pipeline::{BackgroundPipeline, ForegroundPipeline, ImagePipeline};
pub use snapshot::TerminalSnapshot;

/// Atlas texture size (2048×2048, ~4MB for R8).
const ATLAS_SIZE: u32 = 2048;

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
    /// Initializes pipelines, glyph atlas, and font system. Registers GPU
    /// resources in egui's callback resource map.
    pub fn new(render_state: egui_wgpu::RenderState, font_config: &FontConfig) -> Self {
        let target_format = render_state.target_format;
        let target_is_srgb = target_format.is_srgb();
        tracing::info!("GPU renderer target_format: {target_format:?} (sRGB: {target_is_srgb})");
        let device = &render_state.device;

        let bg_pipeline = BackgroundPipeline::new(device, target_format);
        let fg_pipeline = ForegroundPipeline::new(device, target_format);
        let img_pipeline = ImagePipeline::new(device, target_format);
        let atlas = GlyphAtlas::new(device, ATLAS_SIZE);

        let image_sampler = device.create_sampler(&egui_wgpu::wgpu::SamplerDescriptor {
            label: Some("image_sampler"),
            address_mode_u: egui_wgpu::wgpu::AddressMode::ClampToEdge,
            address_mode_v: egui_wgpu::wgpu::AddressMode::ClampToEdge,
            mag_filter: egui_wgpu::wgpu::FilterMode::Linear,
            min_filter: egui_wgpu::wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let mut font_system = font::create_font_system(font_config);
        let swash_cache = SwashCache::new();

        let font_size = font_config.size;
        let line_height = (font_size * 1.3).ceil();
        let metrics = Metrics::new(font_size, line_height);
        // Use raw fractional advance width — no logical-pixel rounding.
        // Ghostty rounds only after DPI scaling; rounding at logical size
        // either inflates (ceil) or clips (round) the grid.
        let cell_width = font::measure_cell_width(&mut font_system, metrics);
        let cell_height = line_height;
        let decoration_metrics = font::measure_decoration_metrics(&mut font_system, font_size);

        // Register resources in egui's callback_resources.
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(TerminalGpuResources {
                bg_pipeline,
                fg_pipeline,
                img_pipeline,
                atlas,
                font_system,
                swash_cache,
                metrics,
                decoration_metrics,
                atlas_bind_group_dirty: true,
                target_is_srgb,
                pane_states: std::collections::HashMap::new(),
                image_cache: std::collections::HashMap::new(),
                shape_cache: std::collections::HashMap::new(),
                image_sampler,
                curly_tile: None,
                dotted_tile: None,
                last_pixels_per_point: 0.0,
            });

        Self {
            render_state,
            cell_width,
            cell_height,
        }
    }

    /// Create an egui `PaintCallback` that will render the terminal pane
    /// into the given rect during egui's render pass.
    ///
    /// `snapshot` contains pre-extracted terminal state (cells, colors, cursor).
    /// `pixels_per_point` is the current DPI scale factor.
    pub fn paint_callback(
        &self,
        rect: egui::Rect,
        snapshot: TerminalSnapshot,
        pixels_per_point: f32,
    ) -> egui::PaintCallback {
        let phys_cell_w = self.cell_width * pixels_per_point;
        let phys_cell_h = self.cell_height * pixels_per_point;

        let pane_id = snapshot.pane_id;
        let callback = TerminalPaintCallback {
            pane_id,
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
    pub fn cell_width(&self) -> f32 {
        self.cell_width
    }

    /// Get the cell height in logical points.
    pub fn cell_height(&self) -> f32 {
        self.cell_height
    }

    /// Update font size, re-measure cell dimensions, and invalidate caches.
    pub fn set_font_size(&mut self, font_size: f32) {
        let line_height = (font_size * 1.3).ceil();
        let metrics = Metrics::new(font_size, line_height);

        if let Some(r) = self
            .render_state
            .renderer
            .write()
            .callback_resources
            .get_mut::<TerminalGpuResources>()
        {
            let cell_width = font::measure_cell_width(&mut r.font_system, metrics);
            r.metrics = metrics;
            r.decoration_metrics = font::measure_decoration_metrics(&mut r.font_system, font_size);
            // Clear all pane render states to force full rebuild with new metrics.
            r.pane_states.clear();
            // Clear shape cache since glyph sizes change with font size.
            r.shape_cache.clear();
            // Invalidate decoration tiles so they're re-rasterized at new size.
            r.curly_tile = None;
            r.dotted_tile = None;
            // Mark atlas bind group dirty since glyph sizes will change.
            r.atlas_bind_group_dirty = true;
            self.cell_width = cell_width;
            self.cell_height = line_height;
        }
    }

    /// Remove cached render state for panes that no longer exist
    /// and evict unreferenced image textures.
    pub fn retain_panes(&self, live_pane_ids: &[u64]) {
        if let Some(r) = self
            .render_state
            .renderer
            .write()
            .callback_resources
            .get_mut::<TerminalGpuResources>()
        {
            r.retain_panes(live_pane_ids);
            r.evict_unused_images();
        }
    }
}
