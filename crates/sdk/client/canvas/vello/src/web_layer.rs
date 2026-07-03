//! WebGPU texture-layer compositor (web).
//!
//! The native renderer composites `TextureLayer`s (the camera) with the macOS-only
//! IOSurface-backed [`LayerCompositor`](crate::native_capture). On web there's no
//! IOSurface — the camera is a browser `MediaStream`. This module is the web
//! equivalent: per layer it keeps a hidden `<video>` playing the stream (exactly
//! like the Canvas2D path's `LayerVideo`), copies the video's CURRENT frame into a
//! wgpu texture each frame via [`Queue::copy_external_image_to_texture`] (a GPU
//! copy from the `HTMLVideoElement` — no CPU readback), then samples it with the
//! SAME fit / rounded-rect / opacity / border shader the native compositor uses.
//!
//! With this in place, a canvas WITH texture layers no longer has to fall back to
//! Canvas2D on web: it can stay on the WebGPU/vello path (so the dots backdrop is
//! GPU-instanced) and still composite the camera into the same canvas — which is
//! what the web self-capture (`captureStream`) records.
//!
//! # Color
//!
//! The frame is copied into a non-sRGB `Rgba8Unorm` texture with `color_space =
//! Srgb`, so it holds straight-alpha sRGB bytes — the same convention vello's
//! target uses. The shader treats video as opaque (mask by corners/fit/opacity)
//! and the pipeline alpha-blends over the scene, matching `LayerCompositor`.

use canvas_core::{Fit, LayerSource, TextureLayer};
use wasm_bindgen::JsCast;
use web_sys::{Document, HtmlVideoElement, MediaStream as WebMediaStream};

/// The vello target is `Rgba8Unorm`; the compositor draws into it.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Per-layer uniform stride (≥ the 256-byte uniform offset alignment).
const LAYER_STRIDE: u64 = 256;
/// Max layers per canvas (sizes the uniform buffer); excess layers are skipped.
const MAX_LAYERS: usize = 16;

/// Same blit shader as the native [`LayerCompositor`](crate::native_capture):
/// fullscreen triangle clipped to the render-pass viewport (the layer rect),
/// fragment applies the fit crop (`uv`), a rounded-rect SDF mask, opacity, and a
/// border ring. Kept byte-for-byte identical so web and native composite layers
/// the same way.
const LAYER_BLIT_WGSL: &str = r#"
struct Layer {
    uv: vec4<f32>,     // uv_scale.xy, uv_offset.xy
    geo: vec4<f32>,    // rect_w_px, rect_h_px, radius_px, opacity
    border: vec4<f32>, // border_width_px, use_src_alpha, _, _
    bcolor: vec4<f32>, // border r, g, b, a (0..1)
};
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> layer: Layer;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var out: VsOut;
    let xy = p[i];
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = vec2<f32>((xy.x + 1.0) * 0.5, (1.0 - xy.y) * 0.5);
    return out;
}

fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let suv = in.uv * layer.uv.xy + layer.uv.zw;
    let inside = all(suv >= vec2<f32>(0.0)) && all(suv <= vec2<f32>(1.0));
    let inb = select(0.0, 1.0, inside);
    let texel = textureSample(tex, samp, clamp(suv, vec2<f32>(0.0), vec2<f32>(1.0)));
    let col = texel.rgb;
    let size = layer.geo.xy;
    let radius = layer.geo.z;
    let opacity = layer.geo.w;
    let pp = (in.uv - vec2<f32>(0.5)) * size;
    let d = sd_round_box(pp, size * 0.5, radius);
    let aa = 1.0 - smoothstep(-1.0, 1.0, d);
    // Streams are opaque (use_src_alpha=0); image layers multiply their straight
    // alpha so transparent watermark regions read through.
    let src_a = mix(1.0, texel.a, layer.border.y);
    var rgb = col;
    var a = aa * inb * opacity * src_a;
    let bw = layer.border.x;
    if (bw > 0.0) {
        let inner = 1.0 - smoothstep(-1.0, 1.0, d + bw);
        let bcov = clamp(aa - inner, 0.0, 1.0);
        rgb = mix(rgb, layer.bcolor.rgb, bcov);
        a = mix(a, layer.bcolor.a * opacity, bcov);
    }
    return vec4<f32>(rgb, a);
}
"#;

/// One layer's persistent state: a hidden `<video>` playing its stream, plus the
/// wgpu texture (+ its bind group) the current frame is copied into. The texture
/// is (re)created when the video's intrinsic size changes.
struct LayerSlot {
    video: HtmlVideoElement,
    /// The web `MediaStream.id` currently attached — only re-`set_src_object` when
    /// it changes (camera opened / swapped).
    stream_id: Option<String>,
    /// For an IMAGE layer: the `(id, generation)` currently uploaded into `tex`, so
    /// a static watermark is `write_texture`'d once and only re-uploaded when its
    /// pixels change. `None` while the slot holds a stream (video) frame.
    image_key: Option<(u64, u64)>,
    /// `(texture, view, bind_group, (w, h))`, sized to the video frame.
    tex: Option<(wgpu::Texture, wgpu::TextureView, wgpu::BindGroup, (u32, u32))>,
}

impl LayerSlot {
    fn new(document: &Document) -> Self {
        let video: HtmlVideoElement = document
            .create_element("video")
            .expect("create_element(video)")
            .dyn_into()
            .expect("video element cast");
        // Muted + autoplay so a detached element plays without a user gesture;
        // playsinline avoids iOS Safari fullscreen takeover. Matches the Canvas2D
        // path's `LayerVideo`.
        video.set_muted(true);
        video.set_autoplay(true);
        let _ = video.set_attribute("playsinline", "");
        Self { video, stream_id: None, image_key: None, tex: None }
    }

    /// Attach `ms` to the `<video>` (only when the stream id changes).
    fn ensure_stream(&mut self, ms: &WebMediaStream) {
        let id = ms.id();
        if self.stream_id.as_deref() != Some(id.as_str()) {
            self.video.set_src_object(Some(ms));
            let _ = self.video.play(); // Promise; ignore
            self.stream_id = Some(id);
        }
    }

    /// Ensure the texture exists and matches `(w, h)`; (re)build it + its bind
    /// group when absent or resized.
    fn ensure_tex(
        &mut self,
        device: &wgpu::Device,
        bind_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        uniforms: &wgpu::Buffer,
        w: u32,
        h: u32,
    ) {
        if let Some((_, _, _, (tw, th))) = &self.tex {
            if *tw == w && *th == h {
                return;
            }
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("web-layer-frame"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            // COPY_DST + RENDER_ATTACHMENT: WebGPU's `copyExternalImageToTexture`
            // requires both on the destination. TEXTURE_BINDING: the blit samples it.
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("web-layer-bind-group"),
            layout: bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: uniforms,
                        offset: 0,
                        // One layer slot's worth; the draw selects it via dynamic offset.
                        size: std::num::NonZeroU64::new(64),
                    }),
                },
            ],
        });
        self.tex = Some((texture, view, bind_group, (w, h)));
    }
}

pub(crate) struct WebLayerCompositor {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    slots: Vec<LayerSlot>,
    document: Document,
}

impl WebLayerCompositor {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("web-layer-blit-shader"),
            source: wgpu::ShaderSource::Wgsl(LAYER_BLIT_WGSL.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("web-layer-blit-bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: std::num::NonZeroU64::new(64),
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("web-layer-blit-pl"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("web-layer-blit-pipeline"),
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
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("web-layer-blit-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("web-layer-blit-uniforms"),
            size: LAYER_STRIDE * MAX_LAYERS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let document = web_sys::window()
            .and_then(|w| w.document())
            .expect("window.document");
        Self { pipeline, sampler, bind_layout, uniforms, slots: Vec::new(), document }
    }

    /// Composite `layers` (in order) over the target. Mirrors the native
    /// [`LayerCompositor::composite_layers`]: resolve each layer's `MediaStream`,
    /// copy its current video frame into a texture, then draw a fit-cropped,
    /// rounded, opacity-blended quad clipped to the layer rect. No-op per layer
    /// whose stream is absent or whose first frame hasn't decoded yet.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn composite_layers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        layers: &[TextureLayer],
        target_view: &wgpu::TextureView,
        scale: f32,
        target_w: u32,
        target_h: u32,
    ) {
        for (i, layer) in layers.iter().enumerate().take(MAX_LAYERS) {
            while self.slots.len() <= i {
                self.slots.push(LayerSlot::new(&self.document));
            }
            // Disjoint borrows: pull the shared GPU handles out before the per-slot
            // &mut borrow (mirrors the native compositor's local bindings).
            let bind_layout = &self.bind_layout;
            let sampler = &self.sampler;
            let uniforms = &self.uniforms;
            let slot = &mut self.slots[i];

            // Resolve the layer's current frame into `slot.tex`, from either the
            // stream's `<video>` (GPU copy) or a static image (`write_texture`).
            let (cam_w, cam_h, use_src_alpha) = match &layer.source {
                LayerSource::Stream(f) => {
                    let Some(stream) = f() else { continue };
                    let Some(ms) = stream
                        .native_source()
                        .and_then(|rc| rc.downcast::<WebMediaStream>().ok())
                    else {
                        continue;
                    };
                    slot.image_key = None;
                    slot.ensure_stream(&ms);

                    let (cam_w, cam_h) = (slot.video.video_width(), slot.video.video_height());
                    if cam_w < 1 || cam_h < 1 {
                        continue; // first frames not decoded yet
                    }
                    // A video element keeps its dimensions (metadata) even when its
                    // CURRENT frame has no GPU-importable backing — e.g. the brief
                    // window after the stream is (re)attached while toggling the
                    // camera off/on. Importing it then fails ("video element that
                    // doesn't have back resource") and wgpu `unwrap()`s that into a
                    // panic, so skip until a frame is decodable. `readyState >=
                    // HAVE_CURRENT_DATA (2)` means a current frame exists.
                    if slot.video.ready_state() < 2 {
                        continue;
                    }
                    slot.ensure_tex(device, bind_layout, sampler, uniforms, cam_w, cam_h);
                    let Some((texture, _, _, _)) = slot.tex.as_ref() else { continue };

                    // Copy the video's CURRENT frame into the texture (GPU copy).
                    queue.copy_external_image_to_texture(
                        &wgpu::CopyExternalImageSourceInfo {
                            source: wgpu::ExternalImageSource::HTMLVideoElement(slot.video.clone()),
                            origin: wgpu::Origin2d::ZERO,
                            flip_y: false,
                        },
                        wgpu::CopyExternalImageDestInfo {
                            texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                            color_space: wgpu::PredefinedColorSpace::Srgb,
                            premultiplied_alpha: false,
                        },
                        wgpu::Extent3d { width: cam_w, height: cam_h, depth_or_array_layers: 1 },
                    );
                    (cam_w, cam_h, 0.0f32)
                }
                LayerSource::Image(f) => {
                    let Some(img) = f() else { continue };
                    if !img.is_valid() {
                        continue;
                    }
                    slot.ensure_tex(device, bind_layout, sampler, uniforms, img.width, img.height);
                    // Upload once; re-upload only when the pixels change under a
                    // stable id (generation bump) or the slot switched sources.
                    if slot.image_key != Some((img.id, img.generation)) {
                        if let Some((texture, _, _, _)) = slot.tex.as_ref() {
                            queue.write_texture(
                                wgpu::TexelCopyTextureInfo {
                                    texture,
                                    mip_level: 0,
                                    origin: wgpu::Origin3d::ZERO,
                                    aspect: wgpu::TextureAspect::All,
                                },
                                &img.rgba,
                                wgpu::TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: Some(img.width * 4),
                                    rows_per_image: Some(img.height),
                                },
                                wgpu::Extent3d {
                                    width: img.width,
                                    height: img.height,
                                    depth_or_array_layers: 1,
                                },
                            );
                            slot.image_key = Some((img.id, img.generation));
                        }
                    }
                    (img.width, img.height, 1.0f32)
                }
            };
            let Some((_, _, bind_group, _)) = slot.tex.as_ref() else { continue };

            // Logical layer rect → physical-pixel viewport.
            let (lx, ly, lw, lh) = (layer.rect)();
            let (rx, ry, rw, rh) = (lx * scale, ly * scale, lw * scale, lh * scale);
            if rw < 1.0 || rh < 1.0 {
                continue;
            }
            let vx = rx.clamp(0.0, target_w as f32);
            let vy = ry.clamp(0.0, target_h as f32);
            let vw = (rx + rw).clamp(0.0, target_w as f32) - vx;
            let vh = (ry + rh).clamp(0.0, target_h as f32) - vy;
            if vw < 1.0 || vh < 1.0 {
                continue;
            }

            let cam_aspect = cam_w as f32 / (cam_h as f32).max(1.0);
            let dst_aspect = vw / vh;
            let (sx, sy, ox, oy) = uv_transform(layer.fit, cam_aspect, dst_aspect);
            let radius_px = ((layer.corner_radius)() * scale).max(0.0);
            let border_px = (layer.border_width * scale).max(0.0);
            let bc = layer.border_color;
            let u = [
                sx, sy, ox, oy,
                vw, vh, radius_px, layer.opacity.clamp(0.0, 1.0),
                border_px, use_src_alpha, 0.0, 0.0,
                bc.r as f32 / 255.0, bc.g as f32 / 255.0, bc.b as f32 / 255.0, bc.a as f32 / 255.0,
            ];
            let mut bytes = [0u8; 64];
            for (j, f) in u.iter().enumerate() {
                bytes[j * 4..j * 4 + 4].copy_from_slice(&f.to_ne_bytes());
            }
            let offset = i as u64 * LAYER_STRIDE;
            queue.write_buffer(&self.uniforms, offset, &bytes);

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("web-layer-composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    depth_slice: None,
                    resolve_target: None,
                    // Preserve the scene (and earlier layers) in the target.
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bind_group, &[offset as u32]);
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            pass.draw(0..3, 0..1);
        }
    }
}

/// `suv = quad_uv * (sx, sy) + (ox, oy)`. Cover samples a centered sub-rect
/// (crop); Contain maps into a centered band (the rest letterboxes via the
/// shader's out-of-`[0,1]` clip); Fill stretches. Identical to the native
/// compositor's `uv_transform`.
fn uv_transform(fit: Fit, cam_aspect: f32, dst_aspect: f32) -> (f32, f32, f32, f32) {
    match fit {
        Fit::Fill => (1.0, 1.0, 0.0, 0.0),
        Fit::Cover => {
            if cam_aspect > dst_aspect {
                let sx = dst_aspect / cam_aspect;
                (sx, 1.0, (1.0 - sx) * 0.5, 0.0)
            } else {
                let sy = cam_aspect / dst_aspect;
                (1.0, sy, 0.0, (1.0 - sy) * 0.5)
            }
        }
        Fit::Contain => {
            if cam_aspect > dst_aspect {
                let f = dst_aspect / cam_aspect;
                (1.0, 1.0 / f, 0.0, (f - 1.0) / (2.0 * f))
            } else {
                let f = cam_aspect / dst_aspect;
                (1.0 / f, 1.0, (f - 1.0) / (2.0 * f), 0.0)
            }
        }
    }
}
