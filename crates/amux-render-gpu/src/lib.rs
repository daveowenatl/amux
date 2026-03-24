use egui_wgpu::wgpu;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};

/// GPU-accelerated terminal renderer using wgpu.
///
/// Renders terminal panes via instanced quad drawing inside egui's render pass,
/// using `egui_wgpu::CallbackTrait` for custom paint callbacks.
pub struct GpuRenderer {
    render_state: egui_wgpu::RenderState,
}

impl GpuRenderer {
    /// Create a new GPU renderer from eframe's render state.
    pub fn new(render_state: egui_wgpu::RenderState) -> Self {
        Self { render_state }
    }

    /// Create an egui `PaintCallback` that will render the terminal pane
    /// into the given rect during egui's render pass.
    ///
    /// For now this is a placeholder that clears to the default background.
    pub fn paint_callback(&self, rect: egui::Rect) -> egui::PaintCallback {
        let callback = TerminalPaintCallback {
            _render_state: self.render_state.clone(),
        };
        egui_wgpu::Callback::new_paint_callback(rect, callback)
    }
}

/// Placeholder paint callback for terminal pane rendering.
///
/// Implements `CallbackTrait` to hook into egui's wgpu render pass.
/// Currently a no-op — will be extended with background and foreground
/// rendering passes in subsequent PRs.
struct TerminalPaintCallback {
    _render_state: egui_wgpu::RenderState,
}

impl CallbackTrait for TerminalPaintCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        _callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _render_pass: &mut wgpu::RenderPass<'static>,
        _callback_resources: &CallbackResources,
    ) {
        // No-op placeholder. Background and foreground passes will be added here.
    }
}

// Compile-time check that the callback is Send + Sync as required by egui_wgpu.
fn _assert_send_sync<T: Send + Sync>() {}
const _: fn() = _assert_send_sync::<TerminalPaintCallback>;
