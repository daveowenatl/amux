// Image pass: renders textured quads for inline terminal images (Kitty protocol).
// Uses instancing: unit quad vertices are shared, per-instance data provides
// position/size/UV coordinates. Each draw call binds a single image texture.

struct Viewport {
    size: vec2<f32>,
    target_is_srgb: f32,
}

@group(0) @binding(0)
var<uniform> viewport: Viewport;

@group(1) @binding(0)
var image_texture: texture_2d<f32>;
@group(1) @binding(1)
var image_sampler: sampler;

struct VertexInput {
    @location(0) vertex_pos: vec2<f32>,
}

struct InstanceInput {
    @location(1) quad_pos: vec2<f32>,
    @location(2) quad_size: vec2<f32>,
    @location(3) uv_min: vec2<f32>,
    @location(4) uv_max: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(vertex: VertexInput, instance: InstanceInput) -> VertexOutput {
    let pixel_pos = instance.quad_pos + vertex.vertex_pos * instance.quad_size;

    let ndc = vec2<f32>(
        pixel_pos.x / viewport.size.x * 2.0 - 1.0,
        1.0 - pixel_pos.y / viewport.size.y * 2.0,
    );

    let uv = mix(instance.uv_min, instance.uv_max, vertex.vertex_pos);

    var out: VertexOutput;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    return out;
}

// Convert a single linear channel to sRGB.
fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        return v * 12.92;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Image textures are Rgba8UnormSrgb (returns linear values).
    var texel = textureSample(image_texture, image_sampler, in.uv);
    // On non-sRGB targets, convert linear→sRGB since hardware won't do it.
    if viewport.target_is_srgb < 0.5 {
        texel = vec4<f32>(
            linear_to_srgb(texel.r),
            linear_to_srgb(texel.g),
            linear_to_srgb(texel.b),
            texel.a,
        );
    }
    return vec4<f32>(texel.rgb * texel.a, texel.a);
}
