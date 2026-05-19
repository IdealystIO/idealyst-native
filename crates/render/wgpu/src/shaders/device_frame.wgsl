// Device-frame pass: paints opaque black in the region OUTSIDE
// a viewport-sized rounded rect. Drawn after all other passes
// so the area surrounding the simulated device's rounded
// display (the corner cutouts) reads as a solid black "device
// off-screen" frame regardless of what the app painted
// underneath.
//
// Single fullscreen quad, single SDF compute per fragment.
// Fragments inside the rounded path are discarded so the
// composited pixels show through unchanged.

struct Globals {
    viewport: vec2<f32>,
    corner_radius: f32,
    _pad: f32,
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    // Pixel coordinate (0..viewport) — used by the fragment
    // shader to compute the SDF in pixel space.
    @location(0) frag_pos: vec2<f32>,
}

@group(0) @binding(0) var<uniform> g: Globals;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    // Fullscreen quad as two triangles. NDC corners; the
    // fragment's pixel position is computed from the lerped
    // location below.
    var pos = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0),
        vec2( 1.0, -1.0),
        vec2(-1.0,  1.0),
        vec2( 1.0, -1.0),
        vec2( 1.0,  1.0),
        vec2(-1.0,  1.0),
    );
    let p = pos[i];
    // Map NDC (-1..1, -1..1) → pixel (0..vw, 0..vh). Y is
    // flipped because the renderer's coordinate convention has
    // the origin at the top-left.
    let px = (p.x * 0.5 + 0.5) * g.viewport.x;
    let py = (1.0 - (p.y * 0.5 + 0.5)) * g.viewport.y;
    var out: VertexOut;
    out.clip = vec4(p, 0.0, 1.0);
    out.frag_pos = vec2(px, py);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Centered-rect SDF. Standard iquilezles rounded-box
    // formula; corner_radius is uniform so the per-corner
    // swizzle song-and-dance from the regular rect shader
    // isn't needed here.
    let center = g.viewport * 0.5;
    let half = g.viewport * 0.5;
    let p = in.frag_pos - center;
    let r = g.corner_radius;
    let q = abs(p) - half + vec2(r);
    let d = min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0))) - r;
    // d <= 0: inside the rounded display, leave the app paint
    // intact. d > 0: outside, paint opaque black with a 1px
    // antialiased ramp so the rounded edge looks smooth.
    let aa = 1.0;
    let alpha = clamp(0.5 + d / aa, 0.0, 1.0);
    if alpha <= 0.0 {
        discard;
    }
    return vec4(0.0, 0.0, 0.0, alpha);
}
