// Rounded-rectangle shader for the wgpu preview backend.
//
// One draw instance per painted node. The vertex stage expands a
// unit quad to the node's rect in screen-space; the fragment stage
// does a signed-distance test against the rounded rectangle to
// produce the antialiased fill (and a future border ring).
//
// When `shadow_blur > 0` the instance is interpreted as a *shadow*
// quad: the rect covers the inflated bounds (original rect +
// offset, expanded by `shadow_blur` on every side); the rounded
// rect SDF is evaluated against the *inner* rect (whose half-extent
// equals the quad's half-extent minus `shadow_blur`); the fragment
// fades from `bg.a * full` at the inner-rect edge to 0 at the
// quad's outer edge, using a smoothstep window of size
// `2 * shadow_blur`.

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
    @location(5) rotation: f32,             // rotation around rect center, in radians
    @location(6) shadow_blur: f32,          // 0 = normal rect; > 0 = shadow falloff
    // 0 = no gradient (use `bg`), 1 = linear, 2 = radial. f32 to
    // keep the vertex layout uniform; the fragment branches on it.
    @location(7) gradient_kind: f32,
    // Linear: (dir.x, dir.y, _, _) — unit vector in rect-frac space.
    // Radial: (cx, cy, rx, ry) — center + radii in rect-frac.
    @location(8) gradient_params: vec4<f32>,
    // Stop offsets in 0..=1, ascending; trailing slots = 1.0.
    // Offsets 0-3 in `gradient_offsets`, offset 4 in
    // `gradient_offset_4` (scalar; vertex attribute count is at
    // WebGPU's portable max already, so splitting saves a slot).
    @location(9) gradient_offsets: vec4<f32>,
    @location(10) gradient_offset_4: f32,
    // Per-stop colors; trailing slots repeat last real stop.
    @location(11) gradient_stop0: vec4<f32>,
    @location(12) gradient_stop1: vec4<f32>,
    @location(13) gradient_stop2: vec4<f32>,
    @location(14) gradient_stop3: vec4<f32>,
    @location(15) gradient_stop4: vec4<f32>,
};

// Inter-stage variables are limited to 15 locations (WebGPU's
// `max_inter_stage_shader_variables = 14`, 0-based). The vertex
// shader packs four unrelated f32 scalars into a single vec4 to
// stay within budget — see the `scalars` field below. Decoded in
// `fs_main` via component access.
struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    // (local.x, local.y, rect_size.x, rect_size.y) — two vec2s
    // packed into one inter-stage vec4.
    @location(0) local_size: vec4<f32>,
    @location(1) bg: vec4<f32>,
    @location(2) corner_radius: vec4<f32>,
    @location(3) border_color: vec4<f32>,
    // (border_width, shadow_blur, gradient_kind, gradient_offset_4)
    // — four scalars packed into one inter-stage vec4. The
    // gradient discriminant rides in `.z`; the bracket-ladder's
    // final offset rides in `.w`.
    @location(4) scalars: vec4<f32>,
    @location(5) gradient_params: vec4<f32>,
    @location(6) gradient_offsets: vec4<f32>,
    @location(7) gradient_stop0: vec4<f32>,
    @location(8) gradient_stop1: vec4<f32>,
    @location(9) gradient_stop2: vec4<f32>,
    @location(10) gradient_stop3: vec4<f32>,
    @location(11) gradient_stop4: vec4<f32>,
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
    // Rotate the quad's corners around the rect's center in
    // screen space. The local SDF coordinate stays axis-aligned
    // (passed to fragment via `out.local = local_px`) so corner
    // radii and borders still compute against the unrotated
    // rectangle.
    let center = inst.rect.zw * 0.5;
    let centered = local_px - center;
    let c = cos(inst.rotation);
    let s = sin(inst.rotation);
    let rotated = vec2(c * centered.x - s * centered.y,
                       s * centered.x + c * centered.y);
    let screen_px = inst.rect.xy + center + rotated;

    // Top-left origin → NDC (-1..1 with y flipped).
    let ndc = vec2(
        (screen_px.x / globals.viewport.x) * 2.0 - 1.0,
        1.0 - (screen_px.y / globals.viewport.y) * 2.0,
    );

    var out: VertexOut;
    out.clip = vec4(ndc, 0.0, 1.0);
    out.local_size = vec4(local_px, inst.rect.zw);
    out.bg = inst.bg;
    out.corner_radius = inst.corner_radius;
    out.border_color = inst.border_color;
    out.scalars = vec4(
        inst.border_width,
        inst.shadow_blur,
        inst.gradient_kind,
        inst.gradient_offset_4,
    );
    out.gradient_params = inst.gradient_params;
    out.gradient_offsets = inst.gradient_offsets;
    out.gradient_stop0 = inst.gradient_stop0;
    out.gradient_stop1 = inst.gradient_stop1;
    out.gradient_stop2 = inst.gradient_stop2;
    out.gradient_stop3 = inst.gradient_stop3;
    out.gradient_stop4 = inst.gradient_stop4;
    return out;
}

/// Interpolate the five-stop gradient palette at `t` in `0..=1`.
///
/// Stop offsets are ascending; trailing slots carry `1.0` so a
/// gradient with N < 5 stops degenerates into N stops + a constant
/// "rest" past `offsets[N-1]`. Each branch computes the local
/// bracket mix; the cascade picks the active bracket by ascending
/// offset and falls through to the last bracket once `t` passes
/// every offset.
fn sample_gradient(
    t: f32,
    offsets: vec4<f32>,
    offset_4: f32,
    s0: vec4<f32>, s1: vec4<f32>, s2: vec4<f32>, s3: vec4<f32>, s4: vec4<f32>,
) -> vec4<f32> {
    let tc = clamp(t, 0.0, 1.0);
    // [s0, s1]
    let denom01 = max(offsets.y - offsets.x, 1e-6);
    let c01 = mix(s0, s1, clamp((tc - offsets.x) / denom01, 0.0, 1.0));
    // [s1, s2]
    let denom12 = max(offsets.z - offsets.y, 1e-6);
    let c12 = mix(s1, s2, clamp((tc - offsets.y) / denom12, 0.0, 1.0));
    // [s2, s3]
    let denom23 = max(offsets.w - offsets.z, 1e-6);
    let c23 = mix(s2, s3, clamp((tc - offsets.z) / denom23, 0.0, 1.0));
    // [s3, s4]
    let denom34 = max(offset_4 - offsets.w, 1e-6);
    let c34 = mix(s3, s4, clamp((tc - offsets.w) / denom34, 0.0, 1.0));
    var out = c01;
    if (tc > offsets.y) { out = c12; }
    if (tc > offsets.z) { out = c23; }
    if (tc > offsets.w) { out = c34; }
    return out;
}

// Signed distance from `p` to a rounded rectangle centered at the
// origin with half-extent `b` and per-corner radii `r` (xy = right
// corners, zw = left corners). Standard iquilezles construction.
//
// Per-corner radii are clamped to `min(b.x, b.y)` — the SDF's inner
// rect collapses to a 0×0 point when `rr.x == min(b.x, b.y)` (the
// "full pill" / "perfect circle" limit), and any radius beyond that
// makes `q` strictly positive for every interior point so the rect
// erases itself. CSS-style "max radius" idioms (`border-radius:
// 999px` to force a pill / circle) depend on this clamp; iOS clamps
// in its `apply_style_to_view` path, and the welcome page's sun
// disc relies on the same behavior here.
fn sd_rounded_box(p: vec2<f32>, b: vec2<f32>, r: vec4<f32>) -> f32 {
    let max_r = min(b.x, b.y);
    var rr = clamp(r, vec4<f32>(0.0), vec4<f32>(max_r));
    rr = select(rr.zwxy, rr, p.x > 0.0);
    rr.x = select(rr.y, rr.x, p.y > 0.0);
    let q = abs(p) - b + rr.x;
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0))) - rr.x;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Unpack the inter-stage-packed fields. See VertexOut's docs
    // on the packing scheme — this is the dual side of the vertex
    // shader's pack.
    let local = in.local_size.xy;
    let rect_size = in.local_size.zw;
    let border_width = in.scalars.x;
    let shadow_blur = in.scalars.y;
    let gradient_kind = in.scalars.z;
    let gradient_offset_4 = in.scalars.w;

    let half_size = rect_size * 0.5;
    let p = local - half_size;

    if shadow_blur > 0.0 {
        // Shadow path. The quad's half-extent already includes
        // `shadow_blur` of padding on every side; the actual
        // visual rect we're shadowing has half-extent
        // `half_size - shadow_blur`. SDF against that inner
        // rect → soft falloff over a `2 * shadow_blur` window.
        let inner_half = max(half_size - vec2(shadow_blur), vec2(0.0));
        // Per-corner radii layout: (tl, tr, br, bl) →
        // sd helper wants (tr, br, tl, bl).
        let r = vec4(
            in.corner_radius.y,
            in.corner_radius.z,
            in.corner_radius.x,
            in.corner_radius.w,
        );
        let d = sd_rounded_box(p, inner_half, r);
        // smoothstep returns 0 inside (d ≤ -blur) → full
        // shadow alpha, 1 outside (d ≥ +blur) → no alpha.
        let t = smoothstep(-shadow_blur, shadow_blur, d);
        let alpha = (1.0 - t) * in.bg.a;
        return vec4(in.bg.rgb, alpha);
    }

    // Per-corner radii packed as (tl, tr, br, bl); the sd helper
    // expects (right-top, right-bottom, left-top, left-bottom) →
    // (tr, br, tl, bl).
    let r = vec4(in.corner_radius.y, in.corner_radius.z, in.corner_radius.x, in.corner_radius.w);
    let d = sd_rounded_box(p, half_size, r);

    // 1px antialiased edge.
    let aa = 1.0;
    let fill_alpha = clamp(0.5 - d / aa, 0.0, 1.0);

    // Pick the fill: `bg` for the default path, or the gradient
    // palette sampled at the per-fragment `t`. `gradient_kind` is
    // a per-instance switch; the cost of computing the unused
    // branch is negligible on modern GPUs (uniform branch within
    // a wavefront).
    var fill_color = in.bg;
    if (gradient_kind > 0.5) {
        // Local fragment in rect-fraction space.
        let local_frac = local / max(rect_size, vec2<f32>(1e-6, 1e-6));
        var t: f32 = 0.0;
        if (gradient_kind < 1.5) {
            // Linear: project (local - 0.5) onto the unit direction
            // and shift to [0..1]. `dir` is a unit vector in
            // rect-frac space (axes are aspect-distorted on
            // non-square boxes — matches CSS's elliptical default,
            // and the welcome's vignette bands are square enough
            // for the distortion to be invisible).
            let dir = in.gradient_params.xy;
            t = dot(local_frac - vec2<f32>(0.5, 0.5), dir) + 0.5;
        } else {
            // Radial: elliptical distance from center in rect-frac.
            let center = in.gradient_params.xy;
            let radii = in.gradient_params.zw;
            let diff = local_frac - center;
            let nx = diff.x / max(radii.x, 1e-6);
            let ny = diff.y / max(radii.y, 1e-6);
            t = sqrt(nx * nx + ny * ny);
        }
        fill_color = sample_gradient(
            t,
            in.gradient_offsets,
            gradient_offset_4,
            in.gradient_stop0,
            in.gradient_stop1,
            in.gradient_stop2,
            in.gradient_stop3,
            in.gradient_stop4,
        );
        // Carry the instance's own alpha modulation (set by the
        // CPU side from accumulated opacity) through: the staging
        // code writes the desired final alpha to `bg.a` for
        // gradient instances, mirroring the solid-fill convention.
        fill_color.a = fill_color.a * in.bg.a;
    }

    var color = fill_color;
    color.a = color.a * fill_alpha;

    // Border ring: pixels within `border_width` of the outer edge
    // get the border color blended in. `inner_d` is the SD against
    // the inset rect; positive between the two = on the ring.
    if border_width > 0.0 {
        let on_ring = clamp(0.5 - (-d - 0.0) / aa, 0.0, 1.0)
                    - clamp(0.5 - (-d + border_width) / aa, 0.0, 1.0);
        let bw = in.border_color.a * on_ring;
        color = mix(color, vec4(in.border_color.rgb, 1.0), bw);
    }

    return color;
}
