//! macOS zero-copy capture target for canvas-vello.
//!
//! A small ring of IOSurface-backed `Bgra8Unorm` textures, imported into the
//! canvas's OWN wgpu Metal device (`as_hal` → `newTextureWithDescriptor:
//! iosurface:plane:` → `create_texture_from_hal` — the seam proven by
//! `tests/iosurface_zerocopy_spike.rs`). Each render blits the vello target into
//! the next surface in the ring and publishes that IOSurface to the stream's
//! native source. The GPU format conversion writes BGRA directly, so there is
//! **no CPU read-back and no swizzle** — the encoder wraps the same IOSurface in
//! a `CVPixelBuffer` and hardware-encodes it.
//!
//! Why a ring (not one surface): `appendPixelBuffer:` reads the surface on the
//! encoder's own queue, asynchronously. Rendering the next frame into a
//! DIFFERENT surface means the canvas never overwrites one the encoder is still
//! reading. At the canvas's frame cadence the GPU blit for frame N completes
//! microseconds after submit — long before the surface is reused `POOL` frames
//! later — so no explicit fence is needed (the cadence is the sync).

use canvas_core::{Fit, LayerSource, TextureLayer};
use media_stream::FrameWriter;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString};
use objc2_io_surface::{
    kIOSurfaceBytesPerElement, kIOSurfaceHeight, kIOSurfacePixelFormat, kIOSurfaceWidth,
    IOSurfaceRef,
};
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTextureDescriptor, MTLTextureType,
    MTLTextureUsage,
};
use std::ffi::c_void;

/// Surfaces in the ring. 3 = standard triple-buffering: enough that the encoder
/// (and the display) are never reading the surface the canvas is rendering into.
const POOL: usize = 3;
/// `'BGRA'` IOSurface pixel format — matches `Bgra8Unorm` and the encoder's
/// `kCVPixelFormatType_32BGRA` pixel buffer, so no channel swap anywhere.
const PIXEL_FORMAT_BGRA: i32 = 0x4247_5241;

struct PoolItem {
    /// Keeps one retain on the IOSurface for the pool's lifetime.
    _iosurface: CFRetained<IOSurfaceRef>,
    /// Raw `IOSurfaceRef` for `publish_surface` (which adds the slot's retain).
    surface_ptr: *const c_void,
    /// wgpu view of the IOSurface-backed texture (the blit's render target).
    view: wgpu::TextureView,
    _texture: wgpu::Texture,
}

/// The canvas's native capture ring. Lazily (re)built to match the drawable
/// size; idle (empty pool) until a recorder taps the stream.
pub(crate) struct NativeCapture {
    writer: FrameWriter,
    pool: Vec<PoolItem>,
    next: usize,
    size: (u32, u32),
    /// Our OWN `Bgra8Unorm` blitter (built lazily) — independent of the surface
    /// format, so the vello `Rgba8Unorm` target always maps cleanly into the
    /// BGRA IOSurface regardless of what the swapchain format happens to be.
    blitter: Option<wgpu::util::TextureBlitter>,
}

impl NativeCapture {
    pub(crate) fn new(writer: FrameWriter) -> Self {
        Self { writer, pool: Vec::new(), next: 0, size: (0, 0), blitter: None }
    }

    /// True only while a recorder holds a `NativeTap` on the stream — gates all
    /// the GPU capture work so an un-recorded canvas pays nothing.
    pub(crate) fn wants(&self) -> bool {
        self.writer.wants_native()
    }

    /// Blit `src_view` (the vello `Rgba8Unorm` target) into the next ring
    /// surface, recording the copy into `encoder` (submitted with the frame).
    /// Returns the ring index to [`publish`](Self::publish) AFTER the submit.
    /// The RGBA→BGRA mapping happens in the GPU store, not on the CPU.
    pub(crate) fn blit_into(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        w: u32,
        h: u32,
    ) -> Option<usize> {
        self.ensure_pool(device, w, h);
        let (Some(blitter), false) = (self.blitter.as_ref(), self.pool.is_empty()) else {
            return None;
        };
        let idx = self.next;
        blitter.copy(device, encoder, src_view, &self.pool[idx].view);
        self.next = (self.next + 1) % self.pool.len();
        Some(idx)
    }

    /// Publish the ring surface at `idx` to the stream's native source. Call
    /// after the GPU submit so the blit is in flight; the ring guarantees this
    /// surface isn't reused until `POOL` frames later.
    pub(crate) fn publish(&self, idx: usize) {
        // SAFETY: `surface_ptr` is a live IOSurfaceRef the pool retains;
        // `publish_surface` adds its own retain for the slot.
        unsafe { self.writer.publish_surface(self.pool[idx].surface_ptr) };
    }

    /// (Re)build the ring when the drawable size changes (or on first use).
    fn ensure_pool(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        if !self.pool.is_empty() && self.size == (w, h) {
            return;
        }
        self.pool.clear();
        self.next = 0;
        self.size = (w, h);
        if w == 0 || h == 0 {
            return;
        }
        if self.blitter.is_none() {
            self.blitter =
                Some(wgpu::util::TextureBlitter::new(device, wgpu::TextureFormat::Bgra8Unorm));
        }
        // wgpu's own MTLDevice — the IOSurface textures must live on it for
        // `create_texture_from_hal` to import them.
        let Some(hal_device) = (unsafe { device.as_hal::<wgpu::hal::api::Metal>() }) else {
            return; // not a Metal device (shouldn't happen on macOS) — stay CPU.
        };
        let mtl_device: &ProtocolObject<dyn MTLDevice> = hal_device.raw_device();

        for _ in 0..POOL {
            let Some(item) = make_pool_item(device, mtl_device, w, h) else {
                self.pool.clear();
                return;
            };
            self.pool.push(item);
        }
    }
}

fn make_pool_item(
    device: &wgpu::Device,
    mtl_device: &ProtocolObject<dyn MTLDevice>,
    w: u32,
    h: u32,
) -> Option<PoolItem> {
    let iosurface = create_bgra_iosurface(w, h)?;
    let surface_ptr = CFRetained::as_ptr(&iosurface).as_ptr() as *const c_void;

    // SAFETY: standard MTLTextureDescriptor + IOSurface-backed texture on wgpu's
    // device; BGRA8Unorm matches the IOSurface format and w×h, plane 0.
    let mtl_texture: Retained<ProtocolObject<dyn objc2_metal::MTLTexture>> = unsafe {
        let desc = MTLTextureDescriptor::new();
        desc.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        desc.setWidth(w as usize);
        desc.setHeight(h as usize);
        desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        desc.setStorageMode(MTLStorageMode::Shared);
        mtl_device.newTextureWithDescriptor_iosurface_plane(&desc, &iosurface, 0)?
    };

    // Import the MTLTexture into wgpu as a Bgra8Unorm RENDER_ATTACHMENT.
    let hal_tex = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            mtl_texture,
            wgpu::TextureFormat::Bgra8Unorm,
            MTLTextureType::Type2D,
            1,
            1,
            wgpu::hal::CopyExtent { width: w, height: h, depth: 1 },
        )
    };
    let texture = unsafe {
        device.create_texture_from_hal::<wgpu::hal::api::Metal>(
            hal_tex,
            &wgpu::TextureDescriptor {
                label: Some("canvas-vello-iosurface"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        )
    };
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Some(PoolItem { _iosurface: iosurface, surface_ptr, view, _texture: texture })
}

// ============================================================================
// Layer compositor: draw a stack of `TextureLayer`s (live MediaStreams) over
// the painted scene — each a positioned, fit-cropped, rounded, opacity-blended
// quad. Zero-copy: a layer's BGRA IOSurface is imported as a sampled Metal
// texture (cached by pointer, reused across frames) and blitted into the canvas
// target, so both the on-screen canvas AND the recording show it. No CPU frame.
// ============================================================================

/// WGSL for a layer blit: a fullscreen triangle clipped to the render-pass
/// viewport (the layer rect). The fragment applies the fit crop (`uv`), a
/// rounded-rectangle SDF mask, and opacity. UV flips Y for top-down textures.
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
    // Contain letterbox: sample outside [0,1] → transparent bars.
    let inside = all(suv >= vec2<f32>(0.0)) && all(suv <= vec2<f32>(1.0));
    let inb = select(0.0, 1.0, inside);
    let texel = textureSample(tex, samp, clamp(suv, vec2<f32>(0.0), vec2<f32>(1.0)));
    let col = texel.rgb;
    // Rounded-rect mask in pixel space across the rect (anti-aliased ~1px edge).
    let size = layer.geo.xy;
    let radius = layer.geo.z;
    let opacity = layer.geo.w;
    let pp = (in.uv - vec2<f32>(0.5)) * size;
    let d = sd_round_box(pp, size * 0.5, radius);
    let aa = 1.0 - smoothstep(-1.0, 1.0, d);
    // Video layers are opaque (`use_src_alpha` = 0) → source alpha ignored, mask
    // by corner + fit + opacity. Image layers (watermark/logo, `use_src_alpha`
    // = 1) multiply in the texel's straight alpha so transparent PNG regions
    // read through. Straight alpha throughout; the pipeline alpha-blends over
    // the strokes.
    let use_src_alpha = layer.border.y;
    let src_a = mix(1.0, texel.a, use_src_alpha);
    var rgb = col;
    var a = aa * inb * opacity * src_a;
    // Border ring, composited WITH the image so the frame stays locked to the
    // picture. `aa` is the outer rounded-rect coverage; `inner` is the coverage of
    // the rect shrunk inward by the border width — their difference is the ring
    // (anti-aliased on both edges).
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

/// Per-layer uniform stride — ≥ the 256-byte uniform offset alignment, so each
/// layer's uniform sits in its own dynamic-offset slot (N layers in one encoder
/// must not clobber a shared slot — queue writes all land before the draws run).
const LAYER_STRIDE: u64 = 256;
/// Max layers per canvas (sizes the uniform buffer); excess layers are skipped.
const MAX_LAYERS: usize = 16;
/// Soft cap on cached textures; cleared (and rebuilt) if a source churns
/// pointers without bound. Real pools (camera, screen share) are far smaller.
const MAX_CACHE: usize = 32;

pub(crate) struct LayerCompositor {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_layout: wgpu::BindGroupLayout,
    /// Per-layer uniforms, one `LAYER_STRIDE` slot each; bound with a dynamic
    /// offset so the layers in one encoder don't clobber each other.
    uniforms: wgpu::Buffer,
    /// Imported textures keyed by IOSurface pointer — imported once per surface
    /// and reused across frames (the camera's pooled surfaces, a screen-share's,
    /// …), so there's no per-frame re-import. `(bind_group, texture, (w, h))`.
    cache: std::collections::HashMap<*const c_void, (wgpu::BindGroup, wgpu::Texture, (u32, u32))>,
    /// Uploaded static images keyed by [`ImageSource::id`] — uploaded once and
    /// reused across frames; the stored `generation` forces a re-upload only when
    /// the pixels change under the same id. `(bind_group, texture, (w, h), gen)`.
    image_cache:
        std::collections::HashMap<u64, (wgpu::BindGroup, wgpu::Texture, (u32, u32), u64)>,
}

/// Which cache entry a resolved layer lives in, so the shared draw code can
/// re-borrow its bind group after the (mutable) cache-fill step.
enum Resolved {
    Surface(*const c_void),
    Image(u64),
}

impl LayerCompositor {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("layer-blit-shader"),
            source: wgpu::ShaderSource::Wgsl(LAYER_BLIT_WGSL.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer-blit-bgl"),
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
                        // Per-layer dynamic offset into the shared uniform buffer.
                        has_dynamic_offset: true,
                        min_binding_size: std::num::NonZeroU64::new(64),
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layer-blit-pl"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("layer-blit-pipeline"),
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
                // The vello target is Rgba8Unorm — match it (we draw INTO the same
                // target the strokes are in). Alpha-blend so rounded corners +
                // letterbox + opacity reveal the strokes behind.
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
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
            label: Some("layer-blit-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layer-blit-uniforms"),
            size: LAYER_STRIDE * MAX_LAYERS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            sampler,
            bind_layout,
            uniforms,
            cache: std::collections::HashMap::new(),
            image_cache: std::collections::HashMap::new(),
        }
    }

    /// Composite `layers` (in order) over the target. Each layer's source is
    /// resolved + imported (cached), positioned at its rect (logical → physical
    /// via `scale`), fit-cropped, rounded, and opacity-blended. No-op per layer
    /// whose source has no native surface yet.
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
            // Resolve the layer's texture into the appropriate cache (a live
            // stream's zero-copy IOSurface, or a static image uploaded once),
            // then re-borrow the bind group for the shared draw below.
            let resolved = match &layer.source {
                LayerSource::Stream(f) => {
                    let Some(stream) = f() else { continue };
                    let Some(src) = stream
                        .native_source()
                        .and_then(|ns| ns.downcast::<media_stream::SurfaceSource>().ok())
                    else {
                        continue;
                    };
                    let ptr = src.acquire();
                    if ptr.is_null() {
                        continue;
                    }
                    if !self.cache.contains_key(&ptr) {
                        if let Some(entry) = self.import(device, ptr) {
                            if self.cache.len() >= MAX_CACHE {
                                self.cache.clear();
                            }
                            self.cache.insert(ptr, entry);
                        }
                    }
                    // The MTLTexture retains the IOSurface, so the cache keeps it
                    // alive; release our acquire retain.
                    unsafe { src.release(ptr) };
                    if !self.cache.contains_key(&ptr) {
                        continue;
                    }
                    Resolved::Surface(ptr)
                }
                LayerSource::Image(f) => {
                    let Some(img) = f() else { continue };
                    if !img.is_valid() {
                        continue;
                    }
                    let stale = match self.image_cache.get(&img.id) {
                        Some((_, _, _, gen)) => *gen != img.generation,
                        None => true,
                    };
                    if stale {
                        if let Some(entry) = self.upload_image(device, queue, &img) {
                            if self.image_cache.len() >= MAX_CACHE {
                                self.image_cache.clear();
                            }
                            self.image_cache.insert(img.id, entry);
                        }
                    }
                    if !self.image_cache.contains_key(&img.id) {
                        continue;
                    }
                    Resolved::Image(img.id)
                }
            };

            let (bind_group, cam_w, cam_h) = match &resolved {
                Resolved::Surface(ptr) => {
                    let (bg, _, (w, h)) = self.cache.get(ptr).expect("just inserted");
                    (bg, *w, *h)
                }
                Resolved::Image(id) => {
                    let (bg, _, (w, h), _) = self.image_cache.get(id).expect("just inserted");
                    (bg, *w, *h)
                }
            };
            // Image layers carry meaningful alpha (transparent PNG regions);
            // stream layers are opaque and ignore source alpha.
            let use_src_alpha = matches!(resolved, Resolved::Image(_)) as u32 as f32;

            let (lx, ly, lw, lh) = (layer.rect)();
            let (rx, ry, rw, rh) = (lx * scale, ly * scale, lw * scale, lh * scale);
            if rw < 1.0 || rh < 1.0 {
                continue;
            }
            // Clamp to the target so a partially-offscreen rect doesn't trip
            // wgpu's "viewport out of bounds" validation.
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
            // [uv_scale.xy, uv_offset.xy] [rect_w, rect_h, radius_px, opacity]
            // [border_px, use_src_alpha, _, _] [border r, g, b, a]
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
                label: Some("layer-composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    depth_slice: None,
                    resolve_target: None,
                    // Preserve the strokes (and earlier layers) in the target.
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

    /// Import a layer source's IOSurface (`ptr`) as a sampled `Bgra8Unorm`
    /// texture + its bind group. Returns the texture's `(w, h)` for fit math.
    fn import(
        &self,
        device: &wgpu::Device,
        ptr: *const c_void,
    ) -> Option<(wgpu::BindGroup, wgpu::Texture, (u32, u32))> {
        let surface_ref: &IOSurfaceRef = unsafe { &*(ptr as *const IOSurfaceRef) };
        let w = surface_ref.width() as u32;
        let h = surface_ref.height() as u32;
        if w == 0 || h == 0 {
            return None;
        }
        let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }?;
        let mtl_device: &ProtocolObject<dyn MTLDevice> = hal_device.raw_device();
        let mtl_texture: Retained<ProtocolObject<dyn objc2_metal::MTLTexture>> = unsafe {
            let desc = MTLTextureDescriptor::new();
            desc.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            desc.setWidth(w as usize);
            desc.setHeight(h as usize);
            desc.setUsage(MTLTextureUsage::ShaderRead);
            desc.setStorageMode(MTLStorageMode::Shared);
            mtl_device.newTextureWithDescriptor_iosurface_plane(&desc, surface_ref, 0)?
        };
        let hal_tex = unsafe {
            wgpu::hal::metal::Device::texture_from_raw(
                mtl_texture,
                wgpu::TextureFormat::Bgra8Unorm,
                MTLTextureType::Type2D,
                1,
                1,
                wgpu::hal::CopyExtent { width: w, height: h, depth: 1 },
            )
        };
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu::hal::api::Metal>(
                hal_tex,
                &wgpu::TextureDescriptor {
                    label: Some("camera-iosurface"),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.bind_group_for(device, &view);
        Some((bind_group, texture, (w, h)))
    }

    /// Upload a static [`ImageSource`]'s straight-RGBA8 pixels into a sampled
    /// `Rgba8Unorm` texture + bind group. Unlike [`import`](Self::import) (a
    /// zero-copy IOSurface), this is a one-time `write_texture` copy; the caller
    /// caches the result by `id`/`generation` so it isn't re-uploaded per frame.
    /// The texture keeps its straight alpha so a transparent-PNG watermark blends
    /// correctly (the shader's `use_src_alpha` path).
    fn upload_image(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        img: &canvas_core::ImageSource,
    ) -> Option<(wgpu::BindGroup, wgpu::Texture, (u32, u32), u64)> {
        let (w, h) = (img.width, img.height);
        if w == 0 || h == 0 {
            return None;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer-image"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.bind_group_for(device, &view);
        Some((bind_group, texture, (w, h), img.generation))
    }

    /// Build a layer bind group over `view` + the shared sampler + the dynamic
    /// per-layer uniform slot. Shared by the IOSurface-import and image-upload
    /// paths so both draw through the identical pipeline.
    fn bind_group_for(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer-bind-group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry {
                    binding: 2,
                    // One `LAYER_STRIDE` slot; the per-draw dynamic offset selects
                    // this layer's uniform. `size` is the actual data (64 bytes).
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.uniforms,
                        offset: 0,
                        size: std::num::NonZeroU64::new(64),
                    }),
                },
            ],
        })
    }
}

/// UV scale + offset mapping the source into the destination rect for a [`Fit`].
/// `suv = quad_uv * (sx, sy) + (ox, oy)`. Cover samples a centered sub-rect
/// (crop); Contain maps into a centered band (the rest letterboxes via the
/// shader's out-of-`[0,1]` clip); Fill stretches.
fn uv_transform(fit: Fit, cam_aspect: f32, dst_aspect: f32) -> (f32, f32, f32, f32) {
    match fit {
        Fit::Fill => (1.0, 1.0, 0.0, 0.0),
        Fit::Cover => {
            if cam_aspect > dst_aspect {
                let sx = dst_aspect / cam_aspect; // camera wider → crop sides
                (sx, 1.0, (1.0 - sx) * 0.5, 0.0)
            } else {
                let sy = cam_aspect / dst_aspect; // camera taller → crop top/bottom
                (1.0, sy, 0.0, (1.0 - sy) * 0.5)
            }
        }
        Fit::Contain => {
            if cam_aspect > dst_aspect {
                // Fit width, letterbox vertically: texture occupies fraction f of
                // the rect height; uv runs outside [0,1] in the bars.
                let f = dst_aspect / cam_aspect;
                (1.0, 1.0 / f, 0.0, (f - 1.0) / (2.0 * f))
            } else {
                let f = cam_aspect / dst_aspect;
                (1.0 / f, 1.0, (f - 1.0) / (2.0 * f), 0.0)
            }
        }
    }
}

/// Create a BGRA, W×H IOSurface (CPU+GPU shared memory).
fn create_bgra_iosurface(w: u32, h: u32) -> Option<CFRetained<IOSurfaceRef>> {
    let width = CFNumber::new_i32(w as i32);
    let height = CFNumber::new_i32(h as i32);
    let bpe = CFNumber::new_i32(4);
    let pix = CFNumber::new_i32(PIXEL_FORMAT_BGRA);
    // SAFETY: kIOSurface* are valid CFString statics; IOSurfaceCreate over a
    // well-formed properties dict returns a +1 retained surface (or null).
    unsafe {
        let keys: [&CFString; 4] = [
            kIOSurfaceWidth,
            kIOSurfaceHeight,
            kIOSurfaceBytesPerElement,
            kIOSurfacePixelFormat,
        ];
        let values: [&CFNumber; 4] = [&width, &height, &bpe, &pix];
        let props = CFDictionary::from_slices(&keys, &values);
        // `from_slices` is typed `CFDictionary<CFString, CFNumber>`; IOSurface
        // wants the untyped one. Same CF object, PhantomData params — sound cast.
        let props_opaque: &CFDictionary =
            &*(&*props as *const CFDictionary<CFString, CFNumber> as *const CFDictionary);
        IOSurfaceRef::new(props_opaque)
    }
}
