// Rounded-rectangle shader for the wgpu preview backend.
//
// One draw instance per painted node. The vertex stage expands a
// unit quad to the node's rect in screen-space; the fragment stage
// does a signed-distance test against the rounded rectangle to
// produce the antialiased fill (and a future border ring).
//
// The pipeline is set up with a viewport-sized projection so the
// CPU writes pixel coordinates directly into Instance.rect.

struct Globals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Instance {
    @location(0) rect: vec4<f32>,           // x, y, w, h in px (top-left origin)
    @location(1) bg: vec4<f32>,             // background RGBA (premultiplied at fragment stage)
    @location(2) corner_radius: vec4<f32>,  // tl, tr, br, bl in px
    @location(3) border_color: vec4<f32>,   // uniform border color for the MVP
    @location(4) border_width: f32,         // uniform border width for the MVP
    @location(5) _pad: vec3<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,          // px from rect top-left
    @location(1) rect_size: vec2<f32>,
    @location(2) bg: vec4<f32>,
    @location(3) corner_radius: vec4<f32>,
    @location(4) border_color: vec4<f32>,
    @location(5) border_width: f32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    inst: Instance,
) -> VertexOut {
    // Unit quad — two triangles, vertex order 0..6.
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
    let screen_px = inst.rect.xy + local_px;

    // Top-left origin → NDC (-1..1 with y flipped).
    let ndc = vec2(
        (screen_px.x / globals.viewport.x) * 2.0 - 1.0,
        1.0 - (screen_px.y / globals.viewport.y) * 2.0,
    );

    var out: VertexOut;
    out.clip = vec4(ndc, 0.0, 1.0);
    out.local = local_px;
    out.rect_size = inst.rect.zw;
    out.bg = inst.bg;
    out.corner_radius = inst.corner_radius;
    out.border_color = inst.border_color;
    out.border_width = inst.border_width;
    return out;
}

// Signed distance from `p` to a rounded rectangle centered at the
// origin with half-extent `b` and per-corner radii `r` (xy = right
// corners, zw = left corners). Standard iquilezles construction.
fn sd_rounded_box(p: vec2<f32>, b: vec2<f32>, r: vec4<f32>) -> f32 {
    var rr = r;
    rr = select(rr.zwxy, rr, p.x > 0.0);
    rr.x = select(rr.y, rr.x, p.y > 0.0);
    let q = abs(p) - b + rr.x;
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0))) - rr.x;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let half_size = in.rect_size * 0.5;
    let p = in.local - half_size;
    // Per-corner radii packed as (tl, tr, br, bl); the sd helper
    // expects (right-top, right-bottom, left-top, left-bottom) →
    // (tr, br, tl, bl).
    let r = vec4(in.corner_radius.y, in.corner_radius.z, in.corner_radius.x, in.corner_radius.w);
    let d = sd_rounded_box(p, half_size, r);

    // 1px antialiased edge.
    let aa = 1.0;
    let fill_alpha = clamp(0.5 - d / aa, 0.0, 1.0);

    var color = in.bg;
    color.a = color.a * fill_alpha;

    // Border ring: pixels within `border_width` of the outer edge
    // get the border color blended in. `inner_d` is the SD against
    // the inset rect; positive between the two = on the ring.
    if in.border_width > 0.0 {
        let on_ring = clamp(0.5 - (-d - 0.0) / aa, 0.0, 1.0)
                    - clamp(0.5 - (-d + in.border_width) / aa, 0.0, 1.0);
        let bw = in.border_color.a * on_ring;
        color = mix(color, vec4(in.border_color.rgb, 1.0), bw);
    }

    return color;
}
