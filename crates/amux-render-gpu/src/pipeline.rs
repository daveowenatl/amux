use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Per-cell background instance data.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct CellBgInstance {
    /// Cell position in physical pixels (top-left corner).
    pub pos: [f32; 2],
    /// Cell size in physical pixels.
    pub size: [f32; 2],
    /// Background color (linear RGBA).
    pub color: [f32; 4],
}

/// Viewport uniform data.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct ViewportUniform {
    /// Viewport size in physical pixels.
    size: [f32; 2],
    _pad: [f32; 2],
}

/// Unit quad vertex.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct QuadVertex {
    pos: [f32; 2],
}

/// Background rendering pipeline: instanced colored quads for cell backgrounds.
pub struct BackgroundPipeline {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
}

impl BackgroundPipeline {
    /// Create the background pipeline.
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("background_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/background.wgsl").into()),
        });

        // Viewport uniform bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bg_viewport_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bg_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bg_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    // Vertex buffer: unit quad
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<QuadVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    },
                    // Instance buffer: per-cell data
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<CellBgInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            1 => Float32x2, // pos
                            2 => Float32x2, // size
                            3 => Float32x4, // color
                        ],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Unit quad: two triangles covering (0,0) to (1,1)
        let vertices = [
            QuadVertex { pos: [0.0, 0.0] },
            QuadVertex { pos: [1.0, 0.0] },
            QuadVertex { pos: [1.0, 1.0] },
            QuadVertex { pos: [0.0, 1.0] },
        ];
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bg_vertex_buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bg_index_buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let viewport_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bg_viewport_uniform"),
            contents: bytemuck::cast_slice(&[ViewportUniform {
                size: [1.0, 1.0],
                _pad: [0.0; 2],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg_viewport_bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });

        // Pre-allocate instance buffer for a typical terminal (200 cols × 50 rows)
        let initial_capacity = 10_000;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg_instance_buffer"),
            size: (initial_capacity * std::mem::size_of::<CellBgInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            vertex_buffer,
            index_buffer,
            viewport_buffer,
            viewport_bind_group,
            instance_buffer,
            instance_capacity: initial_capacity,
        }
    }

    /// Upload instance data and viewport size. Returns the number of instances.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[CellBgInstance],
        viewport_width: f32,
        viewport_height: f32,
    ) -> u32 {
        // Update viewport uniform
        queue.write_buffer(
            &self.viewport_buffer,
            0,
            bytemuck::cast_slice(&[ViewportUniform {
                size: [viewport_width, viewport_height],
                _pad: [0.0; 2],
            }]),
        );

        if instances.is_empty() {
            return 0;
        }

        // Grow instance buffer if needed
        if instances.len() > self.instance_capacity {
            self.instance_capacity = instances.len().next_power_of_two();
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bg_instance_buffer"),
                size: (self.instance_capacity * std::mem::size_of::<CellBgInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));
        instances.len() as u32
    }

    /// Record draw commands into a render pass.
    ///
    /// The `'static` lifetime on the render pass is required by egui_wgpu's
    /// `CallbackTrait::paint()`. We use `wgpu::RenderPass::set_pipeline` etc.
    /// which internally hold references via `Arc`, so this is safe.
    pub fn draw(&self, render_pass: &mut wgpu::RenderPass<'static>, instance_count: u32) {
        if instance_count == 0 {
            return;
        }
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.viewport_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        render_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        render_pass.draw_indexed(0..6, 0, 0..instance_count);
    }
}
