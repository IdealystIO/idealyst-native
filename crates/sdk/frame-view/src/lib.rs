//! Display a live RGBA frame stream by rendering it into the framework's
//! `graphics` primitive (a GPU surface).
//!
//! The raw-frame capability SDKs — `camera`, `screen-recorder` — hand you
//! pixels and deliberately render nothing. This crate is the display half:
//! a small wgpu textured-quad blit that draws those frames, aspect-fit,
//! into a `graphics` surface, on every backend that supports `graphics`
//! (iOS, macOS, Android, web).
//!
//! ```no_run
//! use frame_view::{frame_view, FrameSink};
//!
//! # fn demo() -> runtime_core::Element {
//! // One sink, shared between the capture callback and the view.
//! let sink = FrameSink::new();
//!
//! // Feed it from a capture SDK (off the main thread):
//! // camera.open(cfg, {
//! //     let sink = sink.clone();
//! //     move |f| sink.present_rgba8(f.width, f.height, f.data)
//! // }).await?;
//!
//! // Render it (sizes to its parent — wrap it in a sized view):
//! frame_view(&sink)
//! # }
//! ```
//!
//! # How it fits together
//!
//! - [`FrameSink`] is the thread-safe hand-off. The capture callback runs
//!   on a capture thread and only ever copies its latest frame into the
//!   sink (cheap, lock-guarded) — it never touches the GPU.
//! - [`frame_view`] builds a `graphics` element. On `on_ready` it stands up
//!   a wgpu surface/device and starts a `raf_loop` on the main thread; each
//!   frame it checks the sink, uploads any new frame to a GPU texture, and
//!   presents it letterboxed to preserve aspect ratio.
//!
//! This split is why it's safe: all wgpu work is on the main thread (where
//! the surface lives); the only cross-thread contact is the sink's mutex.
//!
//! # Where it works
//!
//! Anywhere `graphics` is implemented: iOS, macOS, Android, web. On the CPU
//! / terminal / SSR backends (no GPU surface) and over the runtime-server
//! wire (graphics can't cross it) the element renders nothing — the same
//! limitation as any other `graphics` use.

#![deny(missing_docs)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use runtime_core::primitives::graphics::{graphics, GraphicsSurface, OnReadyEvent, OnResizeEvent};
use runtime_core::{Element, IntoElement};

// ---------------------------------------------------------------------------
// FrameSink — the capture → display hand-off.
// ---------------------------------------------------------------------------

struct OwnedFrame {
    width: u32,
    height: u32,
    /// Tightly-packed RGBA8, `width * height * 4` bytes.
    rgba: Vec<u8>,
}

#[derive(Default)]
struct SinkInner {
    latest: Mutex<Option<OwnedFrame>>,
    /// Bumped on every `present_*`; the render loop reads it to skip
    /// re-uploading an unchanged frame.
    generation: AtomicU64,
}

/// A thread-safe sink the capture callback writes the latest frame into and
/// a [`frame_view`] reads from. Cheap to clone (`Arc` inside) and `Send`, so
/// the same sink can be moved into a capture callback running on another
/// thread and shared with the on-screen view.
///
/// Only the *latest* frame is retained — if the renderer falls behind, it
/// skips straight to the newest frame (live preview semantics, no backlog).
#[derive(Clone, Default)]
pub struct FrameSink {
    inner: Arc<SinkInner>,
}

impl FrameSink {
    /// Create an empty sink. Until the first `present_*`, a bound
    /// [`frame_view`] renders nothing (transparent/cleared).
    pub fn new() -> Self {
        Self::default()
    }

    /// Hand the sink a tightly-packed top-down `RGBA8` frame (the `camera`
    /// SDK's format — pass `frame.data`). Copies the bytes in; the source
    /// slice need not outlive the call. Frames whose length is smaller than
    /// `width * height * 4` are ignored.
    pub fn present_rgba8(&self, width: u32, height: u32, data: &[u8]) {
        let need = match checked_len(width, height, data) {
            Some(n) => n,
            None => return,
        };
        let mut slot = self.inner.latest.lock().unwrap();
        let frame = slot.get_or_insert_with(|| OwnedFrame {
            width,
            height,
            rgba: Vec::new(),
        });
        frame.width = width;
        frame.height = height;
        frame.rgba.clear();
        frame.rgba.extend_from_slice(&data[..need]);
        drop(slot);
        self.inner.generation.fetch_add(1, Ordering::Release);
    }

    /// Hand the sink a tightly-packed top-down `BGRA8` frame (the Apple /
    /// Windows capture layout, e.g. `screen-recorder`'s `Bgra8`). The
    /// channels are swizzled to `RGBA8` during the copy. Frames shorter
    /// than `width * height * 4` are ignored.
    pub fn present_bgra8(&self, width: u32, height: u32, data: &[u8]) {
        let need = match checked_len(width, height, data) {
            Some(n) => n,
            None => return,
        };
        let mut slot = self.inner.latest.lock().unwrap();
        let frame = slot.get_or_insert_with(|| OwnedFrame {
            width,
            height,
            rgba: Vec::new(),
        });
        frame.width = width;
        frame.height = height;
        frame.rgba.clear();
        frame.rgba.reserve(need);
        for px in data[..need].chunks_exact(4) {
            // B G R A -> R G B A
            frame.rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
        drop(slot);
        self.inner.generation.fetch_add(1, Ordering::Release);
    }

    fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Acquire)
    }

    /// Copy the latest frame's pixels into `buf`, returning its dimensions.
    /// `None` if no frame has arrived yet.
    fn copy_latest(&self, buf: &mut Vec<u8>) -> Option<(u32, u32)> {
        let slot = self.inner.latest.lock().unwrap();
        let frame = slot.as_ref()?;
        buf.clear();
        buf.extend_from_slice(&frame.rgba);
        Some((frame.width, frame.height))
    }
}

/// `width * height * 4`, or `None` if it's zero-sized or `data` is too short.
fn checked_len(width: u32, height: u32, data: &[u8]) -> Option<usize> {
    if width == 0 || height == 0 {
        return None;
    }
    let need = width as usize * height as usize * 4;
    (data.len() >= need).then_some(need)
}

// ---------------------------------------------------------------------------
// frame_view — the graphics element + render loop.
// ---------------------------------------------------------------------------

/// Build a `graphics` element that displays the live frames written to
/// `sink`, aspect-fit (letterboxed) within its bounds.
///
/// The element has **no intrinsic size** — like any `graphics` surface it
/// fills the space its parent gives it, so place it inside a sized view
/// (a fixed height, or a `flex_grow` container).
///
/// Returns nothing visible on backends without a GPU surface (CPU /
/// terminal / SSR) or over the runtime-server wire.
pub fn frame_view(sink: &FrameSink) -> Element {
    // Shared, main-thread-only renderer slot. `on_ready` fills it (async),
    // the `raf_loop` renders from it, `on_resize` reconfigures it, `on_lost`
    // clears it.
    let slot: Rc<RefCell<Option<Renderer>>> = Rc::new(RefCell::new(None));
    // A resize that arrives before async init finishes is stashed here and
    // applied once the renderer exists.
    let pending_resize: Rc<Cell<Option<(u32, u32)>>> = Rc::new(Cell::new(None));

    let on_ready = {
        let slot = slot.clone();
        let pending_resize = pending_resize.clone();
        let sink = sink.clone();
        move |event: OnReadyEvent| {
            let OnReadyEvent { surface, size } = event;

            // wgpu adapter/device come up async on every target. Drive it on
            // the framework's cross-platform async executor; store the
            // renderer when ready.
            {
                let slot = slot.clone();
                let pending_resize = pending_resize.clone();
                runtime_core::driver::spawn_async(async move {
                    if let Some(renderer) = Renderer::new(surface, size).await {
                        *slot.borrow_mut() = Some(renderer);
                        if let Some(sz) = pending_resize.take() {
                            if let Some(r) = slot.borrow_mut().as_mut() {
                                r.resize(sz);
                            }
                        }
                    }
                });
            }

            // Render loop on the main thread. Uploads only on a new frame
            // (generation change); presents every tick so a resize/clear is
            // always reflected.
            let slot = slot.clone();
            let sink = sink.clone();
            let mut last_generation = u64::MAX;
            let mut scratch: Vec<u8> = Vec::new();
            let raf = runtime_core::raf_loop(move || {
                let mut borrow = slot.borrow_mut();
                let Some(renderer) = borrow.as_mut() else {
                    return;
                };
                let generation = sink.generation();
                if generation != last_generation {
                    last_generation = generation;
                    if let Some((w, h)) = sink.copy_latest(&mut scratch) {
                        renderer.upload(w, h, &scratch);
                    }
                }
                renderer.render();
            });
            // Page-lifetime loop; the `on_lost`/scope drop tears the surface
            // down, after which the loop's `borrow.as_mut()` is `None`.
            std::mem::forget(raf);
        }
    };

    let on_resize = {
        let slot = slot.clone();
        let pending_resize = pending_resize.clone();
        move |event: OnResizeEvent| match slot.borrow_mut().as_mut() {
            Some(r) => r.resize(event.size),
            None => pending_resize.set(Some(event.size)),
        }
    };

    let on_lost = {
        let slot = slot.clone();
        move || {
            *slot.borrow_mut() = None;
        }
    };

    graphics(on_ready)
        .on_resize(on_resize)
        .on_lost(on_lost)
        .into_element()
}

// ---------------------------------------------------------------------------
// Renderer — wgpu surface + textured-quad blit.
// ---------------------------------------------------------------------------

/// The blit shader: a unit quad placed by a `fit` uniform (`xy` = clip-space
/// half-extents, `zw` = centre offset) so the frame keeps its aspect ratio,
/// sampling the RGBA texture (V flipped — frames are top-down).
const SHADER: &str = r#"
@group(0) @binding(0) var<uniform> fit: vec4<f32>;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corners[vi];
    let ndc = (c * 2.0 - vec2<f32>(1.0, 1.0)) * fit.xy + fit.zw;
    var out: VsOut;
    out.pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.uv = vec2<f32>(c.x, 1.0 - c.y);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}
"#;

struct TextureEntry {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    fit_buffer: wgpu::Buffer,
    texture: Option<TextureEntry>,
}

impl Renderer {
    async fn new(surface: GraphicsSurface, size: (u32, u32)) -> Option<Renderer> {
        // Per-target backend selection, mirroring the gpu-backend hosts:
        // WebGL2 on the web, the platform-primary (Metal / Vulkan) natively.
        #[cfg(target_arch = "wasm32")]
        let backends = wgpu::Backends::GL;
        #[cfg(not(target_arch = "wasm32"))]
        let backends = wgpu::Backends::PRIMARY;

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::empty(),
            memory_budget_thresholds: Default::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });

        let surface = instance.create_surface(surface).ok()?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok()?;

        // WebGL2 has its own conservative limit set; native clamps the
        // downlevel defaults up to whatever the adapter actually offers.
        #[cfg(target_arch = "wasm32")]
        let required_limits = wgpu::Limits::downlevel_webgl2_defaults();
        #[cfg(not(target_arch = "wasm32"))]
        let required_limits =
            wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frame-view-device"),
                required_features: wgpu::Features::empty(),
                required_limits,
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .ok()?;

        let caps = surface.get_capabilities(&adapter);
        // Prefer an sRGB surface so an sRGB-encoded texture round-trips
        // without manual gamma handling (same pick as the gpu-backend hosts).
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let (width, height) = (size.0.max(1), size.1.max(1));
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let (pipeline, bind_group_layout) = build_pipeline(&device, format);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("frame-view-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let fit_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame-view-fit"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Some(Renderer {
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group_layout,
            sampler,
            fit_buffer,
            texture: None,
        })
    }

    fn resize(&mut self, size: (u32, u32)) {
        let (w, h) = (size.0.max(1), size.1.max(1));
        if w == self.config.width && h == self.config.height {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
    }

    /// Upload a new RGBA frame. Allocates the texture + bind group on first
    /// use and whenever the dimensions change; otherwise re-uploads pixels
    /// into the existing texture (steady state — no per-frame allocation).
    fn upload(&mut self, width: u32, height: u32, rgba: &[u8]) {
        if width == 0 || height == 0 || rgba.len() < width as usize * height as usize * 4 {
            return;
        }
        let needs_alloc = self
            .texture
            .as_ref()
            .map(|t| t.width != width || t.height != height)
            .unwrap_or(true);

        if needs_alloc {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frame-view-texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("frame-view-bind-group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.fit_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.texture = Some(TextureEntry {
                width,
                height,
                texture,
                bind_group,
            });
        }

        // Write the pixels into the (now-current) texture.
        if let Some(entry) = self.texture.as_ref() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &entry.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba[..width as usize * height as usize * 4],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * width),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    fn render(&mut self) {
        // Aspect-fit the texture inside the surface; black bars otherwise.
        if let Some(entry) = self.texture.as_ref() {
            let fit = fit_rect(
                entry.width,
                entry.height,
                self.config.width,
                self.config.height,
            );
            let mut bytes = [0u8; 16];
            for (i, v) in fit.iter().enumerate() {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
            }
            self.queue.write_buffer(&self.fit_buffer, 0, &bytes);
        }

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                // The surface was resized/invalidated under us; reconfigure
                // and skip this frame.
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(_) => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame-view-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame-view-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(entry) = self.texture.as_ref() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &entry.bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}

fn build_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("frame-view-shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("frame-view-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(16),
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

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("frame-view-pl"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("frame-view-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    (pipeline, bind_group_layout)
}

/// Compute the `fit` uniform (`[half_w, half_h, off_x, off_y]` in clip
/// space) that letterboxes a `tex_w × tex_h` frame inside a `surf_w × surf_h`
/// surface, centred.
fn fit_rect(tex_w: u32, tex_h: u32, surf_w: u32, surf_h: u32) -> [f32; 4] {
    if tex_w == 0 || tex_h == 0 || surf_w == 0 || surf_h == 0 {
        return [1.0, 1.0, 0.0, 0.0];
    }
    let tex_aspect = tex_w as f32 / tex_h as f32;
    let surf_aspect = surf_w as f32 / surf_h as f32;
    // A full-surface quad is half-extents (1, 1). Shrink the axis that would
    // otherwise stretch the frame.
    if tex_aspect > surf_aspect {
        // Frame is wider: fill width, bar top/bottom.
        [1.0, surf_aspect / tex_aspect, 0.0, 0.0]
    } else {
        // Frame is taller: fill height, bar left/right.
        [tex_aspect / surf_aspect, 1.0, 0.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_starts_empty() {
        let sink = FrameSink::new();
        let mut buf = Vec::new();
        assert_eq!(sink.copy_latest(&mut buf), None);
        assert_eq!(sink.generation(), 0);
    }

    #[test]
    fn present_rgba8_stores_and_bumps_generation() {
        let sink = FrameSink::new();
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8]; // 2x1 RGBA
        sink.present_rgba8(2, 1, &data);
        assert_eq!(sink.generation(), 1);
        let mut buf = Vec::new();
        assert_eq!(sink.copy_latest(&mut buf), Some((2, 1)));
        assert_eq!(buf, data);
    }

    #[test]
    fn present_bgra8_swizzles_to_rgba() {
        let sink = FrameSink::new();
        // One BGRA pixel B=10 G=20 R=30 A=40 -> RGBA 30,20,10,40.
        sink.present_bgra8(1, 1, &[10, 20, 30, 40]);
        let mut buf = Vec::new();
        assert_eq!(sink.copy_latest(&mut buf), Some((1, 1)));
        assert_eq!(buf, vec![30, 20, 10, 40]);
    }

    #[test]
    fn short_frames_are_ignored() {
        let sink = FrameSink::new();
        sink.present_rgba8(4, 4, &[0u8; 10]); // needs 64 bytes
        assert_eq!(sink.generation(), 0);
        let mut buf = Vec::new();
        assert_eq!(sink.copy_latest(&mut buf), None);
    }

    #[test]
    fn fit_rect_wide_frame_bars_top_bottom() {
        // 2:1 frame in a 1:1 surface -> full width, half height.
        let fit = fit_rect(200, 100, 100, 100);
        assert!((fit[0] - 1.0).abs() < 1e-6);
        assert!((fit[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn fit_rect_tall_frame_bars_left_right() {
        // 1:2 frame in a 1:1 surface -> half width, full height.
        let fit = fit_rect(100, 200, 100, 100);
        assert!((fit[0] - 0.5).abs() < 1e-6);
        assert!((fit[1] - 1.0).abs() < 1e-6);
    }
}
