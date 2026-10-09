//! Full-frame source-over compositor: lays one frame-sized texture over
//! another in a single fullscreen-triangle draw.
//!
//! Used wherever a renderer draws content into a separate texture and then
//! puts it on top of what the frame target already holds: canvas-vello's
//! hybrid path (vello content over the instanced shape backdrop) and the 3D
//! view's 2D overlay (vello / vello_cpu over the rendered scene).
//!
//! # Alpha conventions
//!
//! Source and destination each declare a [`FrameAlpha`]:
//!
//! - **Straight over straight** (canvas-vello): `ALPHA_BLENDING`
//!   (`SrcAlpha`, `OneMinusSrcAlpha`). Exact where the destination is opaque —
//!   canvas-vello's backdrops are — and the convention its target already uses.
//! - **Anything over premultiplied** (the 3D frame): the fragment converts the
//!   source to premultiplied and blends with `PREMULTIPLIED_ALPHA_BLENDING`
//!   (`One`, `OneMinusSrcAlpha`), which is exact for any destination alpha —
//!   a 3D view with a transparent clear colour composites its overlay without
//!   darkening the overlay's edges.
//!
//! Colour stays sRGB-encoded and the target is non-sRGB, so sampled bytes are
//! blended as stored (matching the present blit, which never re-encodes).

use crate::{FrameAlpha, TARGET_FORMAT};

fn shader_source(src: FrameAlpha, dst: FrameAlpha) -> String {
    let body = match (src, dst) {
        (FrameAlpha::Straight, FrameAlpha::Straight)
        | (FrameAlpha::Premultiplied, FrameAlpha::Premultiplied) => "return c;",
        (FrameAlpha::Straight, FrameAlpha::Premultiplied) => {
            "return vec4<f32>(c.rgb * c.a, c.a);"
        }
        (FrameAlpha::Premultiplied, FrameAlpha::Straight) => {
            "return vec4<f32>(c.rgb / max(c.a, 1e-6), c.a);"
        }
    };
    format!(
        r#"
struct VsOut {{ @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {{
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
}}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {{
    let c = textureSample(src, samp, in.uv);
    {body}
}}
"#
    )
}

fn blend_for(dst: FrameAlpha) -> wgpu::BlendState {
    match dst {
        FrameAlpha::Straight => wgpu::BlendState::ALPHA_BLENDING,
        FrameAlpha::Premultiplied => wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
    }
}

/// Source-over compositor for same-sized [`TARGET_FORMAT`] textures.
pub struct OverlayCompositor {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_layout: wgpu::BindGroupLayout,
}

impl OverlayCompositor {
    /// `src`: the convention of the textures composited from; `dst`: the
    /// convention of the target composited into.
    pub fn new(device: &wgpu::Device, src: FrameAlpha, dst: FrameAlpha) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay-compose-shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source(src, dst).into()),
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
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(blend_for(dst)),
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

    /// Composite `src` ON TOP of whatever `dst` already holds, in place. `dst`
    /// is loaded, not cleared, so it survives where `src` is transparent.
    /// `src` and `dst` must be the same size.
    pub fn composite(
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
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shader_variant_validates() {
        for src in [FrameAlpha::Straight, FrameAlpha::Premultiplied] {
            for dst in [FrameAlpha::Straight, FrameAlpha::Premultiplied] {
                let s = shader_source(src, dst);
                let module = naga::front::wgsl::parse_str(&s)
                    .unwrap_or_else(|e| panic!("{src:?}/{dst:?}: {}", e.emit_to_string(&s)));
                naga::valid::Validator::new(
                    naga::valid::ValidationFlags::all(),
                    naga::valid::Capabilities::empty(),
                )
                .validate(&module)
                .unwrap_or_else(|e| panic!("{src:?}/{dst:?}: {e:?}"));
            }
        }
    }
}
