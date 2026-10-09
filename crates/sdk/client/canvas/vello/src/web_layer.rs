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

use canvas_core::{LayerSource, TextureLayer};
use wasm_bindgen::JsCast;
use web_glue::dom::MediaStream;
use web_sys::{Document, HtmlVideoElement};

use gpu_surface::TARGET_FORMAT;
// The blit shader, crop/fit geometry and uniform slots are shared with the
// native compositors (`crate::layer_blit`), so web and native composite layers
// the same way (and match the CPU renderers' `TextureLayer::source_rects`).
use crate::layer_blit::{layer_blit, slot_offset, LAYER_BLIT_WGSL, LAYER_STRIDE, LAYER_UNIFORM_SIZE, MAX_LAYERS};

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
    fn ensure_stream(&mut self, ms: &MediaStream) {
        let id = ms.id();
        if self.stream_id.as_deref() != Some(id.as_str()) {
            // HYBRID-BRIDGE: wgpu. The layer's native source is the glue
            // `MediaStream` every media producer publishes; this `<video>` is
            // a web-sys element because wgpu's `ExternalImageSource` takes
            // one, so the stream crosses out once per (re)attach.
            let ms: web_sys::MediaStream = web_glue::bridge::to_bindgen(ms).unchecked_into();
            self.video.set_src_object(Some(&ms));
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
                        size: std::num::NonZeroU64::new(LAYER_UNIFORM_SIZE),
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
                        min_binding_size: std::num::NonZeroU64::new(LAYER_UNIFORM_SIZE),
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

    /// Composite the layers `which` names (indices into `layers`, in order)
    /// over the target. Mirrors the native
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
        which: &[u32],
        target_view: &wgpu::TextureView,
        scale: f32,
        target_w: u32,
        target_h: u32,
    ) {
        for &index in which {
            let i = index as usize;
            // Slot = the layer's index in `layers`, not its position in `which`:
            // see "Uniform slots" in `layer_blit`. Layers past MAX_LAYERS skip.
            let (Some(layer), Some(offset)) = (layers.get(i), slot_offset(i)) else {
                continue;
            };
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
                        .and_then(|rc| rc.downcast::<MediaStream>().ok())
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
                    (cam_w, cam_h, false)
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
                    (img.width, img.height, true)
                }
            };
            let Some((_, _, bind_group, _)) = slot.tex.as_ref() else { continue };

            // Crop + fit + mask geometry shared with every renderer.
            let Some(blit) =
                layer_blit(layer, cam_w, cam_h, use_src_alpha, scale, target_w, target_h)
            else {
                continue;
            };
            queue.write_buffer(&self.uniforms, offset, &blit.uniform);

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
            let (vx, vy, vw, vh) = blit.viewport;
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            pass.draw(0..3, 0..1);
        }
    }
}
