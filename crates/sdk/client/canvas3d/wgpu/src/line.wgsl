// canvas3d world-space lines (grids, selection boxes): one colour per batch.
// Output is the frame target's convention: sRGB-encoded, premultiplied — the
// CPU side writes `color` already encoded and premultiplied.

struct Line {
    view_proj: mat4x4<f32>,
    color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> line: Line;

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    return line.view_proj * vec4<f32>(pos, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return line.color;
}
