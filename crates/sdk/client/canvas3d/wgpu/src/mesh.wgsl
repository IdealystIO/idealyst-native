// canvas3d mesh shader: glTF metallic-roughness, forward-lit by up to
// MAX_LIGHTS directional lights + an ambient term.
//
// WebGL2-safe by construction: uniform buffers only (no storage), no compute,
// three inter-stage variables, a uniform-bounded light loop. Every derivative
// (`dpdx`/`dpdy`) and texture sample happens BEFORE any data-dependent branch
// or `discard`, so they stay in uniform control flow on every backend.
//
// Output: the frame target holds sRGB-ENCODED, PREMULTIPLIED colour (see
// gpu-surface's crate docs) — lighting runs in linear light, then the result
// is encoded and multiplied by alpha.

const MAX_LIGHTS: u32 = 4u;
const PI: f32 = 3.14159265;

struct Frame {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // rgb = linear ambient colour × intensity.
    ambient: vec4<f32>,
    // xyz = direction the light TRAVELS (world space, normalised).
    light_dir: array<vec4<f32>, 4>,
    // rgb = linear colour × intensity.
    light_color: array<vec4<f32>, 4>,
    // x = light count.
    counts: vec4<u32>,
};
@group(0) @binding(0) var<uniform> frame: Frame;

struct Object {
    model: mat4x4<f32>,
    // Inverse-transpose of `model` (upper 3×3 used).
    normal: mat4x4<f32>,
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    // metallic, roughness, normal_scale, occlusion_strength
    params: vec4<f32>,
    // alpha_cutoff, alpha_mode (0 opaque, 1 mask, 2 blend), unlit, has_normal_map
    params2: vec4<f32>,
};
@group(1) @binding(0) var<uniform> object: Object;

// Base colour and emissive are sRGB textures (sampling returns linear); the
// rest are linear data.
@group(2) @binding(0) var t_base: texture_2d<f32>;
@group(2) @binding(1) var t_mr: texture_2d<f32>;
@group(2) @binding(2) var t_normal: texture_2d<f32>;
@group(2) @binding(3) var t_occlusion: texture_2d<f32>;
@group(2) @binding(4) var t_emissive: texture_2d<f32>;
@group(2) @binding(5) var samp: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let world = object.model * vec4<f32>(in.pos, 1.0);
    var out: VsOut;
    out.world = world.xyz;
    out.clip = frame.view_proj * world;
    out.normal = (object.normal * vec4<f32>(in.normal, 0.0)).xyz;
    out.uv = in.uv;
    return out;
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn d_ggx(n_dot_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn g_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    let gv = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let gl = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return gv * gl;
}

fn fresnel(f0: vec3<f32>, v_dot_h: f32) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - v_dot_h, 5.0);
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Uniform control flow: every sample and derivative first.
    let base_tex = textureSample(t_base, samp, in.uv);
    let mr_tex = textureSample(t_mr, samp, in.uv);
    let normal_tex = textureSample(t_normal, samp, in.uv);
    let occlusion_tex = textureSample(t_occlusion, samp, in.uv);
    let emissive_tex = textureSample(t_emissive, samp, in.uv);
    let dp1 = dpdx(in.world);
    let dp2 = dpdy(in.world);
    let duv1 = dpdx(in.uv);
    let duv2 = dpdy(in.uv);

    let base = object.base_color * base_tex;
    let alpha_mode = object.params2.y;
    if (alpha_mode > 0.5 && alpha_mode < 1.5 && base.a < object.params2.x) {
        discard;
    }

    var n = normalize(in.normal);
    if (!front) {
        n = -n;
    }
    if (object.params2.w > 0.5) {
        // Tangent frame from screen-space derivatives (no per-vertex
        // tangents needed): Schüler's cotangent frame.
        let dp2perp = cross(dp2, n);
        let dp1perp = cross(n, dp1);
        let t = dp2perp * duv1.x + dp1perp * duv2.x;
        let b = dp2perp * duv1.y + dp1perp * duv2.y;
        let len2 = max(dot(t, t), dot(b, b));
        if (len2 > 1e-20) {
            let inv = inverseSqrt(len2);
            let s = object.params.z;
            let tn = (normal_tex.xyz * 2.0 - 1.0) * vec3<f32>(s, s, 1.0);
            n = normalize(t * inv * tn.x + b * inv * tn.y + n * tn.z);
        }
    }

    var color: vec3<f32>;
    if (object.params2.z > 0.5) {
        color = base.rgb;
    } else {
        let metallic = clamp(object.params.x * mr_tex.b, 0.0, 1.0);
        let roughness = clamp(object.params.y * mr_tex.g, 0.045, 1.0);
        let v = normalize(frame.camera_pos.xyz - in.world);
        let n_dot_v = max(dot(n, v), 1e-4);
        let f0 = mix(vec3<f32>(0.04), base.rgb, metallic);
        let diffuse = base.rgb * (1.0 - metallic);
        let a = roughness * roughness;
        color = vec3<f32>(0.0);
        let count = min(frame.counts.x, MAX_LIGHTS);
        for (var i = 0u; i < count; i = i + 1u) {
            let l = -frame.light_dir[i].xyz;
            let h = normalize(l + v);
            let n_dot_l = max(dot(n, l), 0.0);
            let n_dot_h = max(dot(n, h), 0.0);
            let v_dot_h = max(dot(v, h), 0.0);
            let f = fresnel(f0, v_dot_h);
            let spec = d_ggx(n_dot_h, a) * g_smith(n_dot_v, n_dot_l, roughness) * f
                / (4.0 * n_dot_l * n_dot_v + 1e-4);
            let kd = (vec3<f32>(1.0) - f) * diffuse / PI;
            color = color + (kd + spec) * frame.light_color[i].rgb * n_dot_l;
        }
        let ao = mix(1.0, occlusion_tex.r, object.params.w);
        // No image-based lighting: the ambient term lights the diffuse part and
        // gives metals their F0 tint so they don't go black between lights.
        color = color + frame.ambient.rgb * (diffuse + f0) * ao;
        color = color + object.emissive.rgb * emissive_tex.rgb;
    }

    var alpha = 1.0;
    if (alpha_mode > 1.5) {
        alpha = base.a;
    }
    let encoded = linear_to_srgb(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)));
    return vec4<f32>(encoded * alpha, alpha);
}
