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

use canvas_core::{LayerSource, TextureLayer};
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

use crate::layer_blit::{layer_blit, slot_offset, LAYER_BLIT_WGSL, LAYER_STRIDE, LAYER_UNIFORM_SIZE, MAX_LAYERS};

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
                        min_binding_size: std::num::NonZeroU64::new(LAYER_UNIFORM_SIZE),
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

    /// Composite the layers `which` names (indices into `layers`, in order)
    /// over the target. Each layer's source is resolved + imported (cached),
    /// positioned at its rect (logical → physical via `scale`), crop/fit-mapped
    /// ([`layer_blit`]), rounded, and opacity-blended. No-op per layer whose
    /// source has no native surface yet.
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
            let use_src_alpha = matches!(resolved, Resolved::Image(_));

            // Crop + fit + mask geometry shared with every renderer.
            let Some(blit) =
                layer_blit(layer, cam_w, cam_h, use_src_alpha, scale, target_w, target_h)
            else {
                continue;
            };
            queue.write_buffer(&self.uniforms, offset, &blit.uniform);

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
            let (vx, vy, vw, vh) = blit.viewport;
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
                        size: std::num::NonZeroU64::new(LAYER_UNIFORM_SIZE),
                    }),
                },
            ],
        })
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
