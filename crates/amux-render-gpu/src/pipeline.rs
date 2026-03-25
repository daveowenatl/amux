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
    /// Background color (sRGB or linear RGBA, depending on render target format).
    pub color: [f32; 4],
}

/// Viewport uniform data.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct ViewportUniform {
    /// Viewport size in physical pixels.
    size: [f32; 2],
    /// 1.0 if the render target is sRGB (hardware does linear→sRGB on store),
    /// 0.0 if non-sRGB (values are passed through directly).
    target_is_srgb: f32,
    _pad: f32,
}

/// Unit quad vertex.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct QuadVertex {
    pos: [f32; 2],
}

/// Background rendering pipeline: instanced colored quads for cell backgrounds.
///
/// Instance buffers are stored per-pane in `PaneRenderState`, not here.
/// This struct holds the shared render pipeline, vertex/index buffers, and viewport uniform.
pub struct BackgroundPipeline {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
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
                target_is_srgb: 0.0,
                _pad: 0.0,
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

        Self {
            pipeline,
            vertex_buffer,
            index_buffer,
            viewport_buffer,
            viewport_bind_group,
        }
    }

    /// Update the viewport uniform.
    pub fn upload_viewport(
        &self,
        queue: &wgpu::Queue,
        viewport_width: f32,
        viewport_height: f32,
        target_is_srgb: bool,
    ) {
        queue.write_buffer(
            &self.viewport_buffer,
            0,
            bytemuck::cast_slice(&[ViewportUniform {
                size: [viewport_width, viewport_height],
                target_is_srgb: if target_is_srgb { 1.0 } else { 0.0 },
                _pad: 0.0,
            }]),
        );
    }

    /// Record draw commands using a per-pane instance buffer.
    pub fn draw(
        &self,
        render_pass: &mut wgpu::RenderPass<'static>,
        instance_buffer: &wgpu::Buffer,
        instance_count: u32,
    ) {
        if instance_count == 0 {
            return;
        }
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.viewport_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_vertex_buffer(1, instance_buffer.slice(..));
        render_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        render_pass.draw_indexed(0..6, 0, 0..instance_count);
    }
}

// ---------------------------------------------------------------------------
// Foreground pipeline (textured glyph quads)
// ---------------------------------------------------------------------------

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

/// Foreground rendering pipeline: instanced textured quads for glyphs.
///
/// Instance buffers are stored per-pane in `PaneRenderState`, not here.
pub struct ForegroundPipeline {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    atlas_bind_group_layout: wgpu::BindGroupLayout,
    atlas_bind_group: Option<wgpu::BindGroup>,
}

impl ForegroundPipeline {
    /// Create the foreground pipeline.
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("foreground_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/foreground.wgsl").into()),
        });

        // Group 0: viewport uniform
        let viewport_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("fg_viewport_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        // Group 1: mono atlas + color atlas + sampler
        let atlas_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("fg_atlas_bind_group_layout"),
                entries: &[
                    // binding 0: mono atlas (R8Unorm)
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // binding 1: color atlas (Rgba8UnormSrgb)
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // binding 2: shared sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fg_pipeline_layout"),
            bind_group_layouts: &[&viewport_bind_group_layout, &atlas_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fg_pipeline"),
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
                    // Instance buffer: per-glyph data
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<CellFgInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            1 => Float32x2, // pos
                            2 => Float32x2, // size
                            3 => Float32x2, // uv_min
                            4 => Float32x2, // uv_max
                            5 => Float32x4, // color
                            6 => Float32x4, // is_color + padding
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
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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

        // Shared unit quad (same as background pipeline)
        let vertices = [
            QuadVertex { pos: [0.0, 0.0] },
            QuadVertex { pos: [1.0, 0.0] },
            QuadVertex { pos: [1.0, 1.0] },
            QuadVertex { pos: [0.0, 1.0] },
        ];
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fg_vertex_buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fg_index_buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let viewport_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fg_viewport_uniform"),
            contents: bytemuck::cast_slice(&[ViewportUniform {
                size: [1.0, 1.0],
                target_is_srgb: 0.0,
                _pad: 0.0,
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fg_viewport_bind_group"),
            layout: &viewport_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            vertex_buffer,
            index_buffer,
            viewport_buffer,
            viewport_bind_group,
            atlas_bind_group_layout,
            atlas_bind_group: None,
        }
    }

    /// Update the atlas bind group when atlas textures change.
    pub fn update_atlas_bind_group(
        &mut self,
        device: &wgpu::Device,
        mono_view: &wgpu::TextureView,
        color_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) {
        self.atlas_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fg_atlas_bind_group"),
            layout: &self.atlas_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(mono_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        }));
    }

    /// Update the viewport uniform.
    pub fn upload_viewport(
        &self,
        queue: &wgpu::Queue,
        viewport_width: f32,
        viewport_height: f32,
        target_is_srgb: bool,
    ) {
        queue.write_buffer(
            &self.viewport_buffer,
            0,
            bytemuck::cast_slice(&[ViewportUniform {
                size: [viewport_width, viewport_height],
                target_is_srgb: if target_is_srgb { 1.0 } else { 0.0 },
                _pad: 0.0,
            }]),
        );
    }

    /// Record draw commands using a per-pane instance buffer.
    pub fn draw(
        &self,
        render_pass: &mut wgpu::RenderPass<'static>,
        instance_buffer: &wgpu::Buffer,
        instance_count: u32,
    ) {
        if instance_count == 0 {
            return;
        }
        let Some(atlas_bind_group) = &self.atlas_bind_group else {
            return;
        };
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.viewport_bind_group, &[]);
        render_pass.set_bind_group(1, atlas_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_vertex_buffer(1, instance_buffer.slice(..));
        render_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        render_pass.draw_indexed(0..6, 0, 0..instance_count);
    }
}

// ---------------------------------------------------------------------------
// Image pipeline (inline terminal images via Kitty protocol)
// ---------------------------------------------------------------------------

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

/// Image rendering pipeline: instanced textured quads for inline images.
///
/// Each draw call binds a single image texture. Instance buffers provide
/// per-cell position/UV data for cells referencing that image.
pub struct ImagePipeline {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    pub image_bind_group_layout: wgpu::BindGroupLayout,
}

impl ImagePipeline {
    /// Create the image pipeline.
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/image.wgsl").into()),
        });

        // Group 0: viewport uniform
        let viewport_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("img_viewport_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        // Group 1: image texture + sampler
        let image_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("img_texture_bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("img_pipeline_layout"),
            bind_group_layouts: &[&viewport_bind_group_layout, &image_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("img_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<QuadVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<ImageQuadInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            1 => Float32x2, // pos
                            2 => Float32x2, // size
                            3 => Float32x2, // uv_min
                            4 => Float32x2, // uv_max
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
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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

        let vertices = [
            QuadVertex { pos: [0.0, 0.0] },
            QuadVertex { pos: [1.0, 0.0] },
            QuadVertex { pos: [1.0, 1.0] },
            QuadVertex { pos: [0.0, 1.0] },
        ];
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("img_vertex_buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("img_index_buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let viewport_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("img_viewport_uniform"),
            contents: bytemuck::cast_slice(&[ViewportUniform {
                size: [1.0, 1.0],
                target_is_srgb: 0.0,
                _pad: 0.0,
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("img_viewport_bind_group"),
            layout: &viewport_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            vertex_buffer,
            index_buffer,
            viewport_buffer,
            viewport_bind_group,
            image_bind_group_layout,
        }
    }

    /// Update the viewport uniform.
    pub fn upload_viewport(
        &self,
        queue: &wgpu::Queue,
        viewport_width: f32,
        viewport_height: f32,
        target_is_srgb: bool,
    ) {
        queue.write_buffer(
            &self.viewport_buffer,
            0,
            bytemuck::cast_slice(&[ViewportUniform {
                size: [viewport_width, viewport_height],
                target_is_srgb: if target_is_srgb { 1.0 } else { 0.0 },
                _pad: 0.0,
            }]),
        );
    }

    /// Record draw commands for a single image (one bind group + instance buffer).
    pub fn draw(
        &self,
        render_pass: &mut wgpu::RenderPass<'static>,
        image_bind_group: &wgpu::BindGroup,
        instance_buffer: &wgpu::Buffer,
        instance_count: u32,
    ) {
        if instance_count == 0 {
            return;
        }
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.viewport_bind_group, &[]);
        render_pass.set_bind_group(1, image_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_vertex_buffer(1, instance_buffer.slice(..));
        render_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        render_pass.draw_indexed(0..6, 0, 0..instance_count);
    }
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
        size: (new_capacity * std::mem::size_of::<T>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    Some((buffer, new_capacity))
}
