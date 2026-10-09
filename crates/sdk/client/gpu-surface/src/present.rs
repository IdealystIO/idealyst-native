//! The present blit: frame target → surface (or GL framebuffer), converting
//! the frame's alpha convention to the destination's.
//!
//! # Why not `wgpu::util::TextureBlitter`
//!
//! `TextureBlitter` is a straight copy. The frame target's alpha convention is
//! the renderer's ([`FrameAlpha`]): vello stores STRAIGHT alpha (`fine.wgsl`
//! divides `rgb` by `a` on store), while the surface is configured
//! `PreMultiplied` wherever the platform offers it (the view composites over
//! the UI). Copying straight bytes into a premultiplied surface makes the
//! compositor add `rgb` on top of `bg·(1-a)` — every partially transparent
//! pixel (anti-aliased edges, soft shadows) comes out too bright over a dark
//! background. That was canvas-vello's behaviour before this blit existed
//! (`regression_straight_frame_is_premultiplied_for_a_premultiplied_surface`).
//!
//! Colour stays sRGB-encoded throughout; premultiplying encoded values is the
//! convention 8-bit premultiplied surfaces use, so no linearisation happens
//! here.
//!
//! The same pass also does the GTK4 GL present, which additionally flips V
//! (`GlTarget::origin() == BottomLeft`) and writes premultiplied alpha (GTK
//! composites GL areas as premultiplied).

use crate::FrameAlpha;

/// What the destination wants the colour channels to mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Conversion {
    Copy,
    Premultiply,
    Unpremultiply,
}

fn conversion(src: FrameAlpha, dst_premultiplied: bool) -> Conversion {
    match (src, dst_premultiplied) {
        (FrameAlpha::Straight, true) => Conversion::Premultiply,
        (FrameAlpha::Premultiplied, false) => Conversion::Unpremultiply,
        _ => Conversion::Copy,
    }
}

/// Does a surface configured with `mode` expect premultiplied colour?
///
/// `PostMultiplied` is the only straight-alpha mode. `Opaque` ignores alpha,
/// and premultiplied output is what "composited over black" looks like there;
/// `Auto`/`Inherit` resolve to the platform default, which is premultiplied on
/// every platform wgpu targets (CoreAnimation, DirectComposition, browsers).
pub fn surface_wants_premultiplied(mode: wgpu::CompositeAlphaMode) -> bool {
    !matches!(mode, wgpu::CompositeAlphaMode::PostMultiplied)
}

fn shader_source(conv: Conversion, flip_y: bool) -> String {
    // Clip space → uv. The usual mapping is `(1 - y) / 2` (texture origin top-left);
    // a bottom-left destination (GL framebuffer) uses `(y + 1) / 2`.
    let v = if flip_y { "(xy.y + 1.0) * 0.5" } else { "(1.0 - xy.y) * 0.5" };
    let body = match conv {
        Conversion::Copy => "return c;",
        Conversion::Premultiply => "return vec4<f32>(c.rgb * c.a, c.a);",
        // `max` keeps a fully transparent texel at rgb 0 instead of NaN.
        Conversion::Unpremultiply => "return vec4<f32>(c.rgb / max(c.a, 1e-6), c.a);",
    };
    format!(
        r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VsOut {{
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}};

@vertex
fn vs(@builtin(vertex_index) idx: u32) -> VsOut {{
    // One oversized triangle covering the viewport.
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    let xy = corners[idx];
    var out: VsOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = vec2<f32>((xy.x + 1.0) * 0.5, {v});
    return out;
}}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {{
    let c = textureSample(src, samp, in.uv);
    {body}
}}
"#
    )
}

/// Fullscreen-triangle blit of a frame-sized texture into a same-sized
/// destination, with the alpha conversion baked into the pipeline.
pub struct PresentBlit {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
}

impl PresentBlit {
    /// `dst_format`: the surface (or framebuffer) format. `src_alpha`: the
    /// frame's convention. `dst_premultiplied`: whether the destination
    /// expects premultiplied colour ([`surface_wants_premultiplied`]).
    /// `flip_y`: destination origin is bottom-left (GL framebuffer).
    pub fn new(
        device: &wgpu::Device,
        dst_format: wgpu::TextureFormat,
        src_alpha: FrameAlpha,
        dst_premultiplied: bool,
        flip_y: bool,
    ) -> Self {
        let src = shader_source(conversion(src_alpha, dst_premultiplied), flip_y);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpu-surface-present-blit"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpu-surface-present-bgl"),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu-surface-present-pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gpu-surface-present-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                // No blending: the destination is replaced. A non-sRGB
                // destination stores the (already sRGB-encoded) bytes verbatim.
                targets: &[Some(dst_format.into())],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        // Source and destination are the same (drawable) size: a 1:1 texel copy,
        // so Nearest is exact and never blends neighbours at the edges.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gpu-surface-present-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self { pipeline, sampler, layout }
    }

    /// Record the blit of `src` into `dst` (cleared first) on `encoder`.
    pub fn draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
    ) {
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu-surface-present-bind"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gpu-surface-present-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_table() {
        assert_eq!(conversion(FrameAlpha::Straight, true), Conversion::Premultiply);
        assert_eq!(conversion(FrameAlpha::Straight, false), Conversion::Copy);
        assert_eq!(conversion(FrameAlpha::Premultiplied, true), Conversion::Copy);
        assert_eq!(conversion(FrameAlpha::Premultiplied, false), Conversion::Unpremultiply);
    }

    #[test]
    fn only_post_multiplied_surfaces_take_straight_colour() {
        use wgpu::CompositeAlphaMode as A;
        assert!(!surface_wants_premultiplied(A::PostMultiplied));
        for m in [A::PreMultiplied, A::Opaque, A::Auto, A::Inherit] {
            assert!(surface_wants_premultiplied(m), "{m:?}");
        }
    }

    /// Every variant of the generated shader must parse and validate — the
    /// source is assembled from fragments, so a typo in one branch would only
    /// show up on the platform that selects it.
    #[test]
    fn every_shader_variant_validates() {
        for conv in [Conversion::Copy, Conversion::Premultiply, Conversion::Unpremultiply] {
            for flip in [false, true] {
                let src = shader_source(conv, flip);
                let module = naga::front::wgsl::parse_str(&src)
                    .unwrap_or_else(|e| panic!("{conv:?}/{flip}: {}", e.emit_to_string(&src)));
                naga::valid::Validator::new(
                    naga::valid::ValidationFlags::all(),
                    naga::valid::Capabilities::empty(),
                )
                .validate(&module)
                .unwrap_or_else(|e| panic!("{conv:?}/{flip}: {e:?}"));
            }
        }
    }
}
