// Background pass: renders one colored quad per terminal cell with a non-default background.
// Uses instancing: the unit quad vertices are shared, per-instance data provides position/size/color.

struct Viewport {
    // Viewport size in physical pixels.
    size: vec2<f32>,
}

@group(0) @binding(0)
var<uniform> viewport: Viewport;

struct VertexInput {
    // Unit quad vertex position (0..1).
    @location(0) vertex_pos: vec2<f32>,
}

struct InstanceInput {
    // Cell position in physical pixels (top-left corner).
    @location(1) cell_pos: vec2<f32>,
    // Cell size in physical pixels.
    @location(2) cell_size: vec2<f32>,
    // Cell background color (linear RGBA).
    @location(3) color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(vertex: VertexInput, instance: InstanceInput) -> VertexOutput {
    // Scale unit quad to cell size and translate to cell position.
    let pixel_pos = instance.cell_pos + vertex.vertex_pos * instance.cell_size;

    // Convert pixel coordinates to NDC: (0,0) = top-left, (w,h) = bottom-right
    // NDC range: x [-1, 1], y [-1, 1] (y up in NDC, y down in screen)
    let ndc = vec2<f32>(
        pixel_pos.x / viewport.size.x * 2.0 - 1.0,
        1.0 - pixel_pos.y / viewport.size.y * 2.0,
    );

    var out: VertexOutput;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.color = instance.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
