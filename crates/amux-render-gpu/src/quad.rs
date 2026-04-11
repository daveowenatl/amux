//! Per-instance vertex types for the terminal's instanced quad pipelines.
//!
//! The three `*Instance` structs below are the per-cell (or per-glyph / per-image)
//! records uploaded into the instance buffers consumed by the background,
//! foreground, and image render pipelines. Instance / vertex type definitions
//! live here; pipeline construction lives in the pipelines module.
//!
//! `ensure_instance_buffer` is a shared helper for creating or growing an
//! instance buffer when the number of instances exceeds the current capacity.

use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;

/// Per-cell background instance data.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct CellBgInstance {
    /// Cell position in physical pixels (top-left corner).
    pub pos: [f32; 2],
    /// Cell size in physical pixels.
    pub size: [f32; 2],
    /// Background color (sRGB or linear RGBA, depending on render target format).
    pub color: [f32; 4],
}

/// Per-glyph foreground instance data.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct CellFgInstance {
    /// Glyph position in physical pixels (top-left corner).
    pub pos: [f32; 2],
    /// Glyph size in physical pixels.
    pub size: [f32; 2],
    /// Atlas UV min (top-left).
    pub uv_min: [f32; 2],
    /// Atlas UV max (bottom-right).
    pub uv_max: [f32; 2],
    /// Foreground color (sRGB or linear RGBA, depending on render target format).
    pub color: [f32; 4],
    /// 1.0 for color emoji, 0.0 for monochrome glyphs.
    pub is_color: f32,
    pub _pad: [f32; 3],
}

/// Per-quad image instance data.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ImageQuadInstance {
    /// Quad position in physical pixels (top-left corner).
    pub pos: [f32; 2],
    /// Quad size in physical pixels.
    pub size: [f32; 2],
    /// Texture UV min (top-left).
    pub uv_min: [f32; 2],
    /// Texture UV max (bottom-right).
    pub uv_max: [f32; 2],
}

/// Create or grow an instance buffer if needed. Returns the buffer and its capacity.
pub fn ensure_instance_buffer<T: Pod>(
    device: &wgpu::Device,
    existing: Option<&wgpu::Buffer>,
    existing_capacity: usize,
    required: usize,
    label: &str,
) -> Option<(wgpu::Buffer, usize)> {
    if required <= existing_capacity && existing.is_some() {
        return None; // existing buffer is fine
    }
    let new_capacity = required.max(1024).next_power_of_two();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: new_capacity as u64 * std::mem::size_of::<T>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    Some((buffer, new_capacity))
}
