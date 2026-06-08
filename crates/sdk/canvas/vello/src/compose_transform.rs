//! Transformed-quad compositor: draws a cached layer texture into the target
//! under an affine camera transform, at a uniform opacity, with straight-alpha
//! source-over blending — one quad draw, no per-element work.
//!
//! This is the GPU half of [`DrawOp::LayerCached`](canvas_core::DrawOp::LayerCached)
//! (see [`render`](crate::render) / [`render_web`](crate::render_web)): the layer's
//! ops are baked **once** into a viewport-sized texture (`src`); every frame this
//! pass composites that texture under the live camera `transform`, so panning and
//! zooming an infinite canvas costs one textured quad regardless of how many ops
//! the layer holds. Contrast [`OverlayCompositor`](crate::compose), which composites
//! a same-size texture 1:1 with no transform.
//!
//! # Coordinate mapping
//!
//! The cached texture is viewport-sized (`vp_w × vp_h` device px) and holds content
//! baked over the logical viewport rect `(0, 0)..(vp_w/scale, vp_h/scale)` (the dpr
//! `scale` was applied at bake time). A quad covering that logical rect is mapped to
//! the target by `device = scale · transform · logical`, then to clip space (y
//! flipped, device-down → clip-up). The texture is sampled with `uv = corner /
//! layer_size` (i.e. `(0,0)..(1,1)` over the quad), linearly filtered so a zoomed
//! layer stays smooth while gesturing.
//!
//! # Alpha convention
//!
//! `src` holds STRAIGHT-alpha sRGB bytes (vello un-premultiplies on store), matching
//! [`OverlayCompositor`]. The fragment scales the sampled alpha by the layer
//! `alpha`, and the pipeline's `ALPHA_BLENDING` (`SrcAlpha, OneMinusSrcAlpha`) does
//! the over-composite — so a transparent baked pixel shows the backdrop, an opaque
//! one wins, and edges blend. The target is non-sRGB, so sampled bytes are taken
//! verbatim (no re-gamma), matching the blitter's straight copy.

use canvas_core::Transform;

/// The cached layer texture / target are `Rgba8Unorm`; the composite draws into
/// the same format.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const COMPOSE_WGSL: &str = r#"
struct Params {
    // Combined device affine `scale · transform`, 2x3: linear part (a,b,c,d).
    lin: vec4<f32>,
    // (e, f, layer_w, layer_h): translate + the layer's LOGICAL size.
    trans_size: vec4<f32>,
    // (viewport_w, viewport_h, alpha, _pad): device viewport + layer opacity.
    vp_alpha: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    // Unit-quad corners as a triangle strip: (0,0) (1,0) (0,1) (1,1).
    var uvs = array<vec2<f32>, 4>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 1.0),
    );
    let uv = uvs[i];
    let m = params.lin;
    let e = params.trans_size.x;
    let f = params.trans_size.y;
    let size = params.trans_size.zw;       // logical layer size
    let vp = params.vp_alpha.xy;           // device viewport
    // Quad corner in LOGICAL coords, then map to device via the combined affine.
    let corner = uv * size;
    let dev = vec2<f32>(
        m.x * corner.x + m.z * corner.y + e,
        m.y * corner.x + m.w * corner.y + f,
    );
    // Device (y-down, [0,vp]) → clip space (y-up, [-1,1]).
    let clip = vec2<f32>(dev.x / vp.x * 2.0 - 1.0, 1.0 - dev.y / vp.y * 2.0);
    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(src, samp, in.uv);
    // Straight-alpha: scale coverage by the layer opacity; ALPHA_BLENDING composites.
    return vec4<f32>(c.rgb, c.a * params.vp_alpha.z);
}
"#;

/// The uniform block, 48 bytes (`std140`-compatible: three `vec4`s).
#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    lin: [f32; 4],
    trans_size: [f32; 4],
    vp_alpha: [f32; 4],
}

pub(crate) struct TransformCompositor {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_layout: wgpu::BindGroupLayout,
    /// Reused uniform buffer, rewritten per composite (the transform changes
    /// every frame). 48 bytes — a single small upload per cached layer.
    uniform: wgpu::Buffer,
}

impl TransformCompositor {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cached-layer-compose-shader"),
            source: wgpu::ShaderSource::Wgsl(COMPOSE_WGSL.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cached-layer-compose-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cached-layer-compose-pl"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cached-layer-compose-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            // A unit quad drawn as a 4-vertex triangle strip.
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        // Linear filtering (smooth while zooming) + clamp so the quad's edges
        // don't wrap-sample the opposite side of the cached texture.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cached-layer-compose-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cached-layer-compose-uniform"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, sampler, bind_layout, uniform }
    }

    /// Composite the cached layer texture `src` into `dst` under the camera
    /// `transform` (logical space) at `alpha`, source-over. `dst` is LOADED, not
    /// cleared, so an earlier layer composited beneath survives where this one is
    /// transparent. `scale` is the dpr base; `(vp_w, vp_h)` is the device target
    /// size. The caller writes the uniform via `queue` before encoding the draw.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn composite(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        transform: &Transform,
        scale: f32,
        alpha: f32,
        vp_w: u32,
        vp_h: u32,
    ) {
        // Combined device affine M = scale · transform (each component ×scale):
        // device = scale · (transform · logical).
        let (a, b, c, d, e, f) = (transform.a, transform.b, transform.c, transform.d, transform.e, transform.f);
        let s = scale;
        let (vw, vh) = (vp_w.max(1) as f32, vp_h.max(1) as f32);
        // The cached texture spans the logical viewport: device size / dpr.
        let (lw, lh) = (vw / s, vh / s);
        let params = Params {
            lin: [s * a, s * b, s * c, s * d],
            trans_size: [s * e, s * f, lw, lh],
            vp_alpha: [vw, vh, alpha.clamp(0.0, 1.0), 0.0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck_cast(&params));

        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cached-layer-compose-bind-group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cached-layer-compose-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..4, 0..1);
    }
}

/// Clear `view` to transparent — a bare render pass with `LoadOp::Clear`. The
/// cached-layer composites LOAD the target (so each layer survives under the
/// next), so the first one needs a clean base. The other render paths clear
/// implicitly (vello via `base_color`, the instanced `ShapePass` owns its clear),
/// but a `Cached` frame's backdrop is built entirely from composite-loads, so it
/// needs this explicit clear first.
pub(crate) fn clear_to_transparent(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
) {
    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("cached-layer-clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
}

/// Reinterpret the POD `Params` as bytes for the uniform upload. `Params` is
/// `#[repr(C)]` of three `[f32; 4]` (no padding, no pointers), so the cast is
/// sound — a local 1-line shim to avoid pulling `bytemuck` just for this.
fn bytemuck_cast(p: &Params) -> &[u8] {
    // SAFETY: `Params` is `#[repr(C)]`, all-`f32`, 48 bytes, fully initialized.
    unsafe { std::slice::from_raw_parts(p as *const Params as *const u8, std::mem::size_of::<Params>()) }
}
