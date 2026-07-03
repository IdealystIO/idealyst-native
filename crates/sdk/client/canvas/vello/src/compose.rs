//! Full-frame texture compositor: lays one `Rgba8Unorm` texture over another
//! with straight-alpha source-over blending, in a single fullscreen-triangle
//! draw.
//!
//! Used by the hybrid render path (see [`render`](crate::render)): the instanced
//! [`ShapePass`](crate::shape_pass) draws a pure-shape **backdrop** into the
//! target, then vello renders the remaining ops (ink / media / overlays, over a
//! transparent base) into a separate texture, and this pass composites that
//! content ON TOP of the backdrop. Result: the backdrop is GPU-instanced while
//! everything else stays exact vello — in the one canvas that's both displayed
//! and self-captured, so recordings are unaffected.
//!
//! # Alpha convention
//!
//! vello's fine shader un-premultiplies on store (`vello_shaders` `fine.wgsl`:
//! `rgb * (1/a)`), so its target holds STRAIGHT-alpha sRGB bytes — the same
//! convention [`ShapePass`](crate::shape_pass) writes. So a standard
//! `ALPHA_BLENDING` (`SrcAlpha`, `OneMinusSrcAlpha`) source-over composite is
//! exactly right: where the content is transparent the backdrop shows through,
//! where it's opaque the content wins, and AA edges blend. The target is
//! deliberately non-sRGB, so the sampled bytes are taken verbatim (no re-gamma),
//! matching the blitter's straight copy to the surface.

/// The vello target / backdrop are `Rgba8Unorm`; the composite draws into the
/// same format.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const COMPOSE_WGSL: &str = r#"
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    // One oversized triangle covering the whole viewport (clipped to it).
    var corners = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0),
    );
    let c = corners[i];
    var out: VsOut;
    out.pos = vec4<f32>(c, 0.0, 1.0);
    // Clip space → uv, flip y (texture origin is top-left).
    out.uv = vec2<f32>((c.x + 1.0) * 0.5, (1.0 - c.y) * 0.5);
    return out;
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    // Straight-alpha sample; the pipeline's ALPHA_BLENDING does the over-composite.
    return textureSample(src, samp, in.uv);
}
"#;

pub(crate) struct OverlayCompositor {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_layout: wgpu::BindGroupLayout,
}

impl OverlayCompositor {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay-compose-shader"),
            source: wgpu::ShaderSource::Wgsl(COMPOSE_WGSL.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay-compose-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay-compose-pl"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay-compose-pipeline"),
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
                // Straight-alpha source-over: content on top of the backdrop.
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        // 1:1 target→source mapping (same dimensions), so Nearest is exact and
        // avoids any half-texel filtering at the edges.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay-compose-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self { pipeline, sampler, bind_layout }
    }

    /// Composite `src` (straight-alpha content over a transparent base) ON TOP of
    /// whatever `dst` already holds (the instanced backdrop), in place. `dst` is
    /// loaded, not cleared, so the backdrop survives where `src` is transparent.
    /// `src` and `dst` must be the same size.
    pub(crate) fn composite(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
    ) {
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay-compose-bind-group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay-compose-pass"),
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
        pass.draw(0..3, 0..1);
    }
}
