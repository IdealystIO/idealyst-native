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
// Camera-as-texture: composite a live `MediaStream` (the camera's IOSurface)
// into the canvas target at a rect, so both the on-screen canvas AND the
// recording show it. Zero-copy: the camera publishes a BGRA IOSurface, we
// import it as a sampled Metal texture and draw it as a positioned quad over
// the strokes — no CPU frame.
// ============================================================================

/// WGSL for the positioned camera blit: a fullscreen triangle clipped to the
/// render pass viewport (set to the camera rect), sampling the camera texture
/// across that rect. UV flips Y to match top-down texture data, then applies a
/// `cover` crop (`uv_scale`/`uv_offset`) so the camera fills the rect without
/// distortion (the overflow is cropped, centered).
const CAMERA_BLIT_WGSL: &str = r#"
struct Crop { uv_scale: vec2<f32>, uv_offset: vec2<f32> };
@group(0) @binding(2) var<uniform> crop: Crop;

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
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv * crop.uv_scale + crop.uv_offset;
    return textureSample(tex, samp, uv);
}
"#;

pub(crate) struct CameraComposite {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_layout: wgpu::BindGroupLayout,
    /// `Crop` uniform (uv_scale.xy, uv_offset.xy) — updated per frame for the
    /// current camera-vs-rect aspect (cover fit). Persistent; the per-texture
    /// bind groups reference it.
    crop_buffer: wgpu::Buffer,
    /// Cache keyed by the camera's current IOSurface pointer: re-imported only
    /// when the camera publishes a different surface. `(ptr, bind_group, tex,
    /// (camera_w, camera_h))`.
    cached: Option<(*const c_void, wgpu::BindGroup, wgpu::Texture, (u32, u32))>,
}

impl CameraComposite {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("camera-blit-shader"),
            source: wgpu::ShaderSource::Wgsl(CAMERA_BLIT_WGSL.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera-blit-bgl"),
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
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("camera-blit-pl"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("camera-blit-pipeline"),
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
                // The vello target is Rgba8Unorm — match it (the composite draws
                // INTO the same target the strokes are in).
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
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
            label: Some("camera-blit-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // 4 f32: uv_scale.xy, uv_offset.xy. Initialised to identity (full frame).
        let crop_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera-blit-crop"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, sampler, bind_layout, crop_buffer, cached: None }
    }

    /// Composite the camera's latest frame into `target_view` at `rect_phys`
    /// (x, y, w, h in physical pixels). No-op if the stream has no native
    /// surface yet or the rect is degenerate. Records into `encoder`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn composite(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        stream: &media_stream::MediaStream,
        target_view: &wgpu::TextureView,
        rect_phys: (f32, f32, f32, f32),
        target_w: u32,
        target_h: u32,
    ) {
        let (rx, ry, rw, rh) = rect_phys;
        if rw < 1.0 || rh < 1.0 {
            return;
        }
        // The camera's zero-copy surface (its published IOSurface).
        let Some(src) = stream
            .native_source()
            .and_then(|ns| ns.downcast::<media_stream::SurfaceSource>().ok())
        else {
            return;
        };
        let ptr = src.acquire();
        if ptr.is_null() {
            return;
        }
        let fresh = self.cached.as_ref().map(|c| c.0) != Some(ptr);
        if fresh {
            if let Some((bind_group, texture, dims)) = self.import(device, ptr) {
                self.cached = Some((ptr, bind_group, texture, dims));
            }
        }
        // The MTLTexture retains the IOSurface, so the cache keeps it alive;
        // release our acquire retain.
        unsafe { src.release(ptr) };

        let Some((_, bind_group, _, (cam_w, cam_h))) = &self.cached else {
            return;
        };

        // Clamp the viewport to the target so a partially-offscreen camera rect
        // doesn't trip wgpu's "viewport out of bounds" validation.
        let vx = rx.clamp(0.0, target_w as f32);
        let vy = ry.clamp(0.0, target_h as f32);
        let vw = (rx + rw).clamp(0.0, target_w as f32) - vx;
        let vh = (ry + rh).clamp(0.0, target_h as f32) - vy;
        if vw < 1.0 || vh < 1.0 {
            return;
        }

        // Cover fit: scale the camera to fill the rect preserving aspect, crop
        // the overflow (centered). Shrink the sampled UV range along whichever
        // axis the camera over-extends.
        let cam_aspect = *cam_w as f32 / (*cam_h as f32).max(1.0);
        let dst_aspect = vw / vh;
        let (sx, sy) = if cam_aspect > dst_aspect {
            (dst_aspect / cam_aspect, 1.0) // camera wider → crop sides
        } else {
            (1.0, cam_aspect / dst_aspect) // camera taller → crop top/bottom
        };
        let crop = [sx, sy, (1.0 - sx) * 0.5, (1.0 - sy) * 0.5];
        let mut crop_bytes = [0u8; 16];
        for (i, f) in crop.iter().enumerate() {
            crop_bytes[i * 4..i * 4 + 4].copy_from_slice(&f.to_ne_bytes());
        }
        queue.write_buffer(&self.crop_buffer, 0, &crop_bytes);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("camera-composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Preserve the strokes already in the target.
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
        pass.draw(0..3, 0..1);
    }

    /// Import the camera's IOSurface (`ptr`) as a sampled `Bgra8Unorm` texture +
    /// its bind group. Returns the texture's `(w, h)` for cover-fit math.
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera-bind-group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.crop_buffer.as_entire_binding(),
                },
            ],
        });
        Some((bind_group, texture, (w, h)))
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
