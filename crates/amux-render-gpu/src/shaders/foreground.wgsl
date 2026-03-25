// Foreground pass: renders one textured quad per visible glyph.
// Uses instancing: unit quad vertices are shared, per-instance data provides
// position/size/UV/color. Supports both monochrome (alpha-only) and color (emoji)
// atlas textures, selected by the is_color instance flag.

struct Viewport {
    size: vec2<f32>,
    // 1.0 if target is sRGB (hardware converts linear→sRGB on store),
    // 0.0 if non-sRGB (values pass through directly).
    target_is_srgb: f32,
}

@group(0) @binding(0)
var<uniform> viewport: Viewport;

@group(1) @binding(0)
var mono_atlas: texture_2d<f32>;
@group(1) @binding(1)
var color_atlas: texture_2d<f32>;
@group(1) @binding(2)
var atlas_sampler: sampler;

struct VertexInput {
    @location(0) vertex_pos: vec2<f32>,
}

struct InstanceInput {
    // Glyph position in physical pixels (top-left corner).
    @location(1) glyph_pos: vec2<f32>,
    // Glyph size in physical pixels.
    @location(2) glyph_size: vec2<f32>,
    // Atlas UV coordinates: [u_min, v_min, u_max, v_max].
    @location(3) uv_min: vec2<f32>,
    @location(4) uv_max: vec2<f32>,
    // Foreground color (sRGB or linear RGBA, depending on target format).
    @location(5) color: vec4<f32>,
    // is_color flag (x component): 1.0 = color emoji, 0.0 = monochrome.
    @location(6) is_color_pad: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) is_color: f32,
}

@vertex
fn vs_main(vertex: VertexInput, instance: InstanceInput) -> VertexOutput {
    let pixel_pos = instance.glyph_pos + vertex.vertex_pos * instance.glyph_size;

    let ndc = vec2<f32>(
        pixel_pos.x / viewport.size.x * 2.0 - 1.0,
        1.0 - pixel_pos.y / viewport.size.y * 2.0,
    );

    // Interpolate UV from min to max across the unit quad.
    let uv = mix(instance.uv_min, instance.uv_max, vertex.vertex_pos);

    var out: VertexOutput;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.color = instance.color;
    out.is_color = instance.is_color_pad.x;
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
    if in.is_color > 0.5 {
        // Color emoji: sample from Rgba8UnormSrgb atlas (returns linear values).
        var texel = textureSample(color_atlas, atlas_sampler, in.uv);
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
    } else {
        // Monochrome glyph: sample alpha from mono atlas, multiply by fg color.
        // Use alpha * color.a for correct premultiplication when color.a < 1.0.
        let alpha = textureSample(mono_atlas, atlas_sampler, in.uv).r;
        let final_alpha = alpha * in.color.a;
        return vec4<f32>(in.color.rgb * final_alpha, final_alpha);
    }
}
