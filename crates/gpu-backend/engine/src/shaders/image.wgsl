// Textured-quad shader for the `Image` primitive.
//
// One draw per image — each call has its own texture + sampler
// bound at group(1). The vertex stage expands a unit quad into
// the instance's screen rect and rotates it around its center
// (same convention as rect.wgsl). The fragment stage samples
// the texture at the interpolated uv and multiplies by an
// instance-supplied tint × opacity.
//
// Per-instance UV rect lets a future atlas pack many images
// into one texture without changing the shader — pass
// `(u0, v0, du, dv)` and the vertex stage maps the unit-quad
// corner to the sub-rect.

struct Globals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var img_tex: texture_2d<f32>;
@group(1) @binding(1) var img_sampler: sampler;

struct Instance {
    @location(0) rect: vec4<f32>,     // x, y, w, h in px (top-left)
    @location(1) uv_rect: vec4<f32>,  // u0, v0, du, dv inside the texture
    @location(2) tint: vec4<f32>,     // multiplies sampled RGBA
    @location(3) rotation: f32,
    @location(4) opacity: f32,
    @location(5) _pad: vec2<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
    @location(2) opacity: f32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    inst: Instance,
) -> VertexOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0),
        vec2(1.0, 0.0),
        vec2(0.0, 1.0),
        vec2(1.0, 0.0),
        vec2(1.0, 1.0),
        vec2(0.0, 1.0),
    );
    let unit = corners[vi];
    let local_px = unit * inst.rect.zw;
    let center = inst.rect.zw * 0.5;
    let centered = local_px - center;
    let c = cos(inst.rotation);
    let s = sin(inst.rotation);
    let rotated = vec2(c * centered.x - s * centered.y,
                       s * centered.x + c * centered.y);
    let screen_px = inst.rect.xy + center + rotated;
    let ndc = vec2(
        (screen_px.x / globals.viewport.x) * 2.0 - 1.0,
        1.0 - (screen_px.y / globals.viewport.y) * 2.0,
    );

    var out: VertexOut;
    out.clip = vec4(ndc, 0.0, 1.0);
    out.uv = inst.uv_rect.xy + unit * inst.uv_rect.zw;
    out.tint = inst.tint;
    out.opacity = inst.opacity;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let texel = textureSample(img_tex, img_sampler, in.uv);
    // Tint multiplies (1,1,1,1) by default — opacity ramp lets
    // a fade-in animation reuse the same pipeline.
    var out = texel * in.tint;
    out.a = out.a * in.opacity;
    return out;
}
