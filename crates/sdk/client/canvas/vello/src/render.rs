//! Native vello renderer: translates a `canvas_core::Scene` into a
//! `vello::Scene` and paints it onto the framework's `graphics` surface
//! via `wgpu`.
//!
//! # Coordinate space
//!
//! The `graphics` primitive reports the drawable `size` in **physical pixels**
//! plus a device `scale` (dpr). This renderer paints the author's
//! LOGICAL-coordinate `Scene` with base transform = `Affine::scale(scale)`, so
//! it fills the physical surface and matches the native renderers'
//! logical-coordinate behavior (no retina under-fill). Backends that don't yet
//! report a real dpr send `scale: 1.0` (render at physical scale, the historical
//! behavior); today macOS reports `backingScaleFactor`, others are `1.0` pending
//! per-backend dpr wiring.

use crate::compose::OverlayCompositor;
use crate::compose_transform::TransformCompositor;
use crate::encode::encode_scene;
use crate::native_capture::{LayerCompositor, NativeCapture};
use crate::plan::{plan_scene, CachedRef, ScenePlan};
use crate::shape_pass::ShapePass;
use canvas_core::{
    paint_scene, CanvasProps, DrawOp, Scene as CanvasScene, TextureLayer,
};
use media_stream::FrameWriter;
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::graphics::{GraphicsTarget, OnReadyEvent, OnResizeEvent};
use runtime_core::{effect, Backend, RegisterExternal};

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use vello::kurbo::Affine;
use vello::peniko::Color;
use vello::{AaConfig, AaSupport, Renderer, RendererOptions, RenderParams, Scene as VelloScene};

/// vello renders into a storage texture of this format; the blitter copies
/// it to the surface (whatever the surface's own format is).
pub(crate) const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Register the vello canvas renderer. Generic over any backend that
/// supports externals + graphics surfaces — the surface is obtained from
/// `Backend::create_graphics`, so there's no per-platform code.
///
/// **Self-gating:** only takes over (from canvas-native, registered first) if
/// this GPU can actually run vello's shaders. The one known-incompatible case
/// is Vulkan WITHOUT the f16 capability — vello's `flatten` shader requires
/// `SHADER_FLOAT16_IN_FLOAT32`, which naga enforces on Vulkan (the Android
/// EMULATOR's Vulkan lacks it) but not on Metal. So an app can register both
/// canvas-native and canvas-vello uniformly on every platform: vello wins on a
/// real GPU (Metal, or Vulkan with f16) and steps aside on the emulator,
/// leaving canvas-native — no per-environment `cfg` or `is_simulator()` needed.
pub fn register<B: RegisterExternal>(backend: &mut B) {
    canvas_core::ensure_wire_serde();
    if gpu_can_run_vello() {
        backend.register_external::<CanvasProps, _>(|props, b| build_canvas(props, b));
    }
}

/// Probe the default adapter for vello compatibility. Returns `false` when
/// vello's GPU-driven pipeline is known to fail on this adapter. A headless
/// adapter request — no surface needed; runs once at startup. If no GPU adapter
/// exists at all, vello can't run → `false`.
///
/// Two known-bad classes, both *emulator/simulator* GPUs (real devices pass):
/// - **Vulkan without f16** — the Android emulator's Goldfish GFXStream Vulkan
///   never exposes `SHADER_F16`, which vello's `flatten` shader requires (naga
///   rejects it: "requires capability SHADER_FLOAT16_IN_FLOAT32"). Metal/DX12
///   don't enforce the explicit feature.
/// - **No INDIRECT_EXECUTION** — the iOS Simulator's Metal lacks indirect GPU
///   dispatch (`create_buffer 'vello.reduced_buf'` fails: "Downlevel flags
///   INDIRECT_EXECUTION are required but not supported"). vello is GPU-driven and
///   needs it unconditionally; every real iOS/Apple GPU (A11+) supports it.
///
/// Both are capability checks, NOT platform checks — any GPU lacking the
/// capability steps aside for canvas-native, uniformly.
fn gpu_can_run_vello() -> bool {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    })) else {
        return false;
    };
    let info = adapter.get_info();
    let has_f16 = adapter.features().contains(wgpu::Features::SHADER_F16);
    let has_indirect = adapter
        .get_downlevel_capabilities()
        .flags
        .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION);
    let f16_ok = !(info.backend == wgpu::Backend::Vulkan && !has_f16);
    let ok = f16_ok && has_indirect;
    if !ok {
        // One-line startup diagnostic for the recurring "why isn't the GPU canvas
        // active?" question. The Android emulator (Vulkan, no f16) and the iOS
        // Simulator (Metal, no INDIRECT_EXECUTION) both land here and fall back to
        // canvas-native; real devices report both and vello wins.
        let missing = if !has_indirect { "INDIRECT_EXECUTION" } else { "SHADER_F16" };
        log::warn!(
            "canvas-vello: {:?} adapter {:?} lacks {missing} — using canvas-native (GPU canvas unsupported here)",
            info.backend, info.name
        );
    }
    ok
}

fn build_canvas<B: Backend>(props: &Rc<CanvasProps>, backend: &mut B) -> B::Node {
    // Latest painted scene + GPU state, shared between the reactive effect
    // and the surface lifecycle callbacks.
    let scene_cell: Rc<RefCell<CanvasScene>> = Rc::new(RefCell::new(CanvasScene::new()));
    let state_cell: Rc<RefCell<Option<RenderState>>> = Rc::new(RefCell::new(None));

    // Reactive repaint: re-paint the scene and redraw whenever a signal the
    // draw closure reads changes (animation redraws every frame). On the
    // first run the surface isn't ready yet; on_ready does the first draw.
    // Built in the canvas walker, so the component scope owns it. The clones
    // are hoisted so the macro's `move` captures them (cloned once).
    {
        let props = props.clone();
        let scene_cell = scene_cell.clone();
        let state_cell = state_cell.clone();
        effect!({
            *scene_cell.borrow_mut() = paint_scene(&props);
            if let Some(state) = state_cell.borrow_mut().as_mut() {
                state.render(&scene_cell.borrow());
            }
        });
    }

    let on_ready = {
        let scene_cell = scene_cell.clone();
        let state_cell = state_cell.clone();
        // Self-capture sink (app's MediaStream producer half), if the canvas
        // was built with `capture: Some(writer)`. `FrameWriter` is Clone.
        let capture = props.capture.clone();
        // Texture layers composited into the canvas (Clone: MediaStream + Rc).
        let layers = props.layers.clone();
        move |ev: OnReadyEvent| {
            let (size, scale) = (ev.size, ev.scale);
            // Two kinds of target: a raw window handle (swapchain — iOS/Android/
            // macOS/web) or a lent GL context (GTK4, which can't hand out a
            // per-widget window handle — see `GraphicsTarget`). `RenderState::new`
            // branches on both: the swapchain path is universal; the GL path
            // adopts the lent context through wgpu's GL backend (Linux only) and
            // presents by flip-blitting into the GLArea framebuffer. If the target
            // can't be driven here (a GL context off-Linux, or a failed adopt),
            // `new` returns `None` and the canvas stays on canvas-native.
            let is_gl = matches!(ev.target, GraphicsTarget::Gl(_));
            // Clone the lent GL target BEFORE `RenderState::new` consumes
            // `ev.target`, so the in-signal blit can be registered on it once the
            // state exists. `GlTarget` is `Clone` (an Rc bump) and clones address
            // the same context. Nothing to capture on the swapchain path.
            #[cfg(target_os = "linux")]
            let gl_target = match &ev.target {
                GraphicsTarget::Gl(g) => Some(g.clone()),
                GraphicsTarget::RawWindow(_) => None,
            };
            match RenderState::new(ev.target, size, scale, capture.clone(), layers.clone()) {
                Some(mut state) => {
                    if is_gl {
                        log::info!("canvas-vello: adopted GL context — GPU canvas active");
                    }
                    let presented = state.render(&scene_cell.borrow());
                    *state_cell.borrow_mut() = Some(state);
                    // GL (GTK4): register the in-signal FBO blit. `render()` above
                    // rendered vello into `self.target` and scheduled a pass via
                    // `present()`; GTK's render signal then runs this callback,
                    // which flip-blits `target` into the GLArea framebuffer — the
                    // ONLY place GtkGLArea composites what's drawn (see
                    // `RenderState::blit_to_fbo`). `try_borrow_mut` guards against
                    // re-entrancy (a present() emitted from within a render pass).
                    #[cfg(target_os = "linux")]
                    if let Some(gl_target) = gl_target {
                        gl_target.on_render(Box::new({
                            let state_cell = state_cell.clone();
                            move || {
                                if let Ok(mut s) = state_cell.try_borrow_mut() {
                                    if let Some(s) = s.as_mut() {
                                        s.blit_to_fbo();
                                    }
                                }
                            }
                        }));
                    }
                    // First drawable often isn't acquirable on the deferred on_ready
                    // tick (macOS CAMetalLayer) — retry until the initial scene lands
                    // so the canvas doesn't show dark until the first repaint. On GL
                    // the blit is driven by the render signal, so `render()` returns
                    // true and this doesn't fire; the scheduled pass does the work.
                    if !presented {
                        retry_first_frame(scene_cell.clone(), state_cell.clone(), 120);
                    }
                }
                None => {
                    if is_gl {
                        log::warn!(
                            "canvas-vello: could not adopt the lent GL context \
                             (GL unsupported on this target/platform) — using canvas-native",
                        );
                    }
                }
            }
        }
    };

    let on_resize = {
        let scene_cell = scene_cell.clone();
        let state_cell = state_cell.clone();
        move |ev: OnResizeEvent| {
            if let Some(state) = state_cell.borrow_mut().as_mut() {
                state.scale = ev.scale.max(0.0) as f64;
                state.resize(ev.size);
                state.render(&scene_cell.borrow());
            }
        }
    };

    let on_lost = {
        let state_cell = state_cell.clone();
        move || {
            // Drop all GPU state derived from the lost surface; a fresh
            // on_ready follows if the surface returns.
            *state_cell.borrow_mut() = None;
        }
    };

    backend.create_graphics(
        Box::new(on_ready),
        Box::new(on_resize),
        Box::new(on_lost),
        &AccessibilityProps::default(),
    )
}

// ============================================================================
// GPU render state
// ============================================================================

struct RenderState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The swapchain surface (RawWindow targets). `None` on the GL path
    /// (GTK4), where there is no surface — presentation goes through `gl`
    /// instead. Branching on `Some`/`None` here selects the present path in
    /// `render()`/`resize()`.
    surface: Option<wgpu::Surface<'static>>,
    /// GL present path (Linux/GTK4 only): the lent GL context + a flip-blit.
    /// `Some` exactly when `surface` is `None`. Vello still renders into
    /// `self.target` as usual; this only replaces the final present — a
    /// vertical-flip straight-copy of `self.target` into the GLArea's
    /// framebuffer (see `GlPresent`).
    #[cfg(target_os = "linux")]
    gl: Option<GlPresent>,
    /// Size + format record. On the swapchain path this configures the
    /// surface; on the GL path it's kept purely as the size/format book so
    /// the ~20 `self.config.width/height` reads elsewhere are path-agnostic
    /// (format = `Rgba8Unorm`, width/height = drawable size). It is NEVER used
    /// to configure a surface on the GL path (GTK owns the buffers).
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    /// A SECOND vello renderer used ONLY to bake cached layers that contain
    /// images. vello keeps one persistent image atlas per `Renderer`, resized to
    /// fit each baked scene's images — so baking an image-less layer (grid, ink)
    /// through the same renderer shrinks the atlas to 1×1 and a later image bake
    /// resizes it back WITHOUT re-uploading the (cached) image → the image renders
    /// blank ("media vanishes when you pan/zoom/draw"). Keeping image layers on
    /// their own renderer means their atlas is never shrunk, so the cache stays
    /// valid. Lazily built on the first image bake (it compiles vello's shaders).
    image_renderer: Option<Renderer>,
    scene: VelloScene,
    /// Intermediate Rgba8Unorm storage texture vello renders into (the
    /// surface itself can't be a compute storage target). Blitted to the
    /// surface each frame; also the COPY_SRC for self-capture read-back.
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    blitter: wgpu::util::TextureBlitter,
    /// Device pixel ratio (physical px / logical pt) from the graphics
    /// event. The author's `Scene` is in LOGICAL coords; we apply this as
    /// the base transform so it fills the physical-pixel surface instead of
    /// under-filling on a HiDPI (retina) drawable. `1.0` = render at physical
    /// scale (the historical behavior for backends not yet reporting dpr).
    scale: f64,
    /// Self-capture sink (`CanvasProps.capture`). When present AND a consumer is
    /// tapping (`wants_cpu_frames`), each rendered frame is read back GPU→CPU and
    /// written here as RGBA8, feeding the app's `MediaStream` (recording).
    capture: Option<FrameWriter>,
    /// Row-padded read-back buffer, lazily (re)created to match the target size.
    /// `(buffer, padded_bytes_per_row)`.
    readback: Option<(wgpu::Buffer, u32)>,
    /// Zero-copy capture ring (macOS): when a recorder taps the stream, each
    /// frame's vello target is GPU-blitted into an IOSurface and published — no
    /// CPU read-back, no swizzle. `None` only when the canvas has no `capture`
    /// sink. The CPU `capture_frame` path is the fallback when this is idle.
    native_capture: Option<NativeCapture>,
    /// The next scheduled capture time — throttles recording capture to
    /// `CAPTURE_INTERVAL` (~60fps) independent of the (faster) on-screen render
    /// rate, so a 120fps screen records a capped 60fps video and pays the
    /// capture-blit cost only ~60×/sec. Advanced by a fixed interval per capture
    /// (a phase accumulator) so the AVERAGE rate stays 60fps even when render
    /// timing is uneven. `None` until the first captured frame.
    next_capture: Option<std::time::Instant>,
    /// Texture layers (camera, screen share, …) composited over the scene each
    /// frame (macOS), so the strokes + layers are one image — on-screen and in
    /// the recording. Empty when the canvas has no `layers`.
    layers: Vec<TextureLayer>,
    layer_compositor: Option<LayerCompositor>,
    /// Instanced analytic-shape pass, built lazily the first time a scene with a
    /// leading shape batch is rendered (a canvas that never uses `DrawOp::Shapes`
    /// pays nothing). Draws a rounded-box-SDF grid in one instanced draw instead
    /// of one tessellated fill per shape. See [`ShapePass`].
    shape_pass: Option<ShapePass>,
    /// Secondary Rgba8Unorm target vello renders the `rest` of a HYBRID scene
    /// into (over a transparent base), then [`compose`](crate::compose) lays it
    /// over the instanced backdrop in `target`. Lazily created, invalidated on
    /// resize. `None` until the first hybrid frame.
    overlay: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// Full-frame source-over compositor for the hybrid path, built lazily
    /// alongside `overlay`. See [`OverlayCompositor`].
    overlay_compositor: Option<OverlayCompositor>,
    /// Baked, viewport-sized textures for `DrawOp::LayerCached`, keyed by layer
    /// id. Re-rendered only on a `dirty` bake; composited under the camera
    /// transform every frame (the infinite pan/zoom fast path). Cleared on
    /// resize (stale-size); the app re-bakes on the resize repaint.
    cached_layers: HashMap<u32, (wgpu::Texture, wgpu::TextureView)>,
    /// Last DIRTY ops baked per cached layer. `layer_cached(dirty=false)` carries
    /// EMPTY ops (the renderer is meant to reuse the retained texture); but if the
    /// texture was dropped (resize) and a `dirty=false` frame must bake the now-
    /// missing layer, baking those empty ops yields a transparent (black) layer.
    /// Retaining the ops lets us re-bake the real content instead. Keyed by id.
    cached_ops: HashMap<u32, Vec<DrawOp>>,
    /// Transformed-quad compositor for `cached_layers`, built lazily the first
    /// time a cached-layer scene is rendered. See [`TransformCompositor`].
    transform_compositor: Option<TransformCompositor>,
}

/// Overscan margin as a fraction of the viewport, per side, for cached layers —
/// read once from `OVERSCAN_FRAC` (default `0.0` = no overscan, the original
/// viewport-sized behavior). When `> 0`, cached layers bake into a texture that
/// extends `frac`·viewport beyond each edge so a pan up to that margin composites
/// (O(1)) with no black edge. Must exceed the app's `far` recenter threshold
/// (0.4) so the re-bake fires before the margin is exhausted. Clamped to [0, 1].
pub(crate) fn overscan_frac() -> f32 {
    thread_local! {
        static FRAC: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
    }
    FRAC.with(|c| match c.get() {
        Some(v) => v,
        None => {
            let v = std::env::var("OVERSCAN_FRAC")
                .ok()
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            c.set(Some(v));
            v
        }
    })
}

/// The overscanned device dimensions for a `(w, h)` viewport: `(1 + 2·frac)` on
/// each axis (rounded). `frac == 0` returns `(w, h)` unchanged.
pub(crate) fn overscan_dims(w: u32, h: u32, frac: f32) -> (u32, u32) {
    if frac <= 0.0 {
        return (w, h);
    }
    let scale = 1.0 + 2.0 * frac;
    (
        ((w as f32) * scale).round() as u32,
        ((h as f32) * scale).round() as u32,
    )
}

/// Build a vello `Renderer` with the canvas's standard options (area AA, GPU).
/// Shared by the main renderer, the eager `image_renderer`, and the lazy
/// fallbacks so all three stay configured identically.
///
/// `single_threaded_init` MUST be `true` on the GL (GTK4) path. vello's default
/// shader init (`num_init_threads: None`) compiles pipelines on spawned WORKER
/// threads for parallelism — but an adopted GL context is thread-bound (current
/// only on the thread that called `make_current`), so those workers create GL
/// programs with NO current context and panic (`wgpu-hal gles/device.rs`).
/// `num_init_threads: Some(1)` makes vello take its synchronous path and compile
/// every shader on THIS (current-context) thread — see vello's `Renderer::new`,
/// which only calls `use_parallel_initialisation()` when the count isn't 1.
/// Swapchain (Metal/Vulkan/WebGPU) contexts aren't thread-bound this way, so
/// they keep the faster parallel init (`false`).
pub(crate) fn new_vello_renderer(
    device: &wgpu::Device,
    single_threaded_init: bool,
) -> Option<Renderer> {
    Renderer::new(
        device,
        RendererOptions {
            use_cpu: false,
            antialiasing_support: AaSupport::area_only(),
            num_init_threads: if single_threaded_init {
                std::num::NonZeroUsize::new(1)
            } else {
                None
            },
            pipeline_cache: None,
        },
    )
    .ok()
}

pub(crate) fn make_target(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("canvas-vello-target"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        // STORAGE_BINDING: vello compute-writes the scene. TEXTURE_BINDING: the
        // blitter samples it. COPY_SRC: CPU read-back fallback. RENDER_ATTACHMENT:
        // the camera composite draws its quad INTO this target (over the strokes).
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// A headless render result: tightly-packed, top-down `RGBA8`
/// (`width * height * 4` bytes).
pub struct RenderedImage {
    /// Tightly-packed top-down RGBA8 pixels.
    pub data: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Render a canvas [`Scene`](canvas_core::Scene) to RGBA8 pixels **offscreen** —
/// no window, no display. This is the headless export path (server-side
/// thumbnails, PDF/image export, OG images): the same vello pipeline the live
/// canvas uses, pointed at an offscreen texture instead of a surface.
///
/// Acquires a GPU adapter, falling back to a **software** adapter (Mesa lavapipe
/// / DX WARP) when no GPU is present, so it runs on a headless server. Fonts and
/// images travel inside the scene's ops (`FontResource` / `ImageSource`), so the
/// result is self-contained — no system font / asset loading.
///
/// **Blocking** (drives wgpu via `pollster`): call it off the async/request path
/// — a blocking task or a background worker. Returns `None` if no usable adapter
/// / device is available or the render fails.
/// Acquire a headless wgpu `(device, queue)` — a real GPU adapter first, then a
/// **software** adapter (Mesa lavapipe / DX WARP) so it runs where there's no
/// GPU (CI, servers). `compatible_surface: None` (no window). Shared by the
/// one-shot [`render_to_rgba`] export and the persistent
/// [`HeadlessCompositor`](crate::headless::HeadlessCompositor). **Blocking**
/// (drives wgpu via `pollster`) — call off the async/request path.
pub(crate) fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    let request = |fallback: bool| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: fallback,
            compatible_surface: None,
        }))
    };
    // Real GPU first; software adapter (lavapipe / WARP) if there's none.
    let adapter = request(false).or_else(|_| request(true)).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("canvas-vello-headless"),
        ..Default::default()
    }))
    .ok()
}

pub fn render_to_rgba(scene: &CanvasScene, width: u32, height: u32) -> Option<RenderedImage> {
    let (w, h) = (width.max(1), height.max(1));

    let (device, queue) = headless_device()?;
    // Headless: a fresh native (non-GL) device, so parallel shader init is fine.
    let mut renderer = new_vello_renderer(&device, false)?;

    let mut vs = VelloScene::new();
    encode_scene(scene.ops(), &mut vs, Affine::scale(1.0));
    let (target, target_view) = make_target(&device, w, h);
    let params = RenderParams {
        base_color: Color::from_rgba8(0, 0, 0, 0),
        width: w,
        height: h,
        antialiasing_method: AaConfig::Area,
    };
    renderer
        .render_to_texture(&device, &queue, &vs, &target_view, &params)
        .ok()?;

    Some(read_target_rgba(&device, &queue, &target, w, h))
}

/// Copy an `Rgba8Unorm` `target` texture back to CPU as tightly-packed, top-down
/// RGBA8. `bytes_per_row` must be 256-aligned, so the copy is padded and the
/// per-row padding is stripped to a tight `w*4*h` buffer. **Blocking** (polls the
/// device to completion). Shared by the one-shot export and the persistent
/// [`HeadlessCompositor`](crate::headless::HeadlessCompositor)'s `read_rgba`.
pub(crate) fn read_target_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &wgpu::Texture,
    w: u32,
    h: u32,
) -> RenderedImage {
    let unpadded = (w * 4) as usize;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = unpadded.div_ceil(align) * align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("canvas-vello-readback"),
        size: (padded * h as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    let mapped = buffer.slice(..).get_mapped_range();
    let mut data = Vec::with_capacity(unpadded * h as usize);
    for row in 0..h as usize {
        let start = row * padded;
        data.extend_from_slice(&mapped[start..start + unpadded]);
    }
    RenderedImage { data, width: w, height: h }
}

// Scene classification (`ScenePlan` / `plan_scene`) lives in `crate::plan`, shared
// with the web renderer.

impl RenderState {
    fn new(
        target: GraphicsTarget,
        size: (u32, u32),
        scale: f32,
        capture: Option<FrameWriter>,
        layers: Vec<TextureLayer>,
    ) -> Option<Self> {
        let (w, h) = (size.0.max(1), size.1.max(1));

        // Per-target GPU bringup. Both arms yield a `device`/`queue` and a
        // `config` (size + `Rgba8Unorm` target format) read uniformly by the
        // rest of the renderer. The RawWindow arm also configures a swapchain
        // `surface`; the GL arm (GTK4) adopts the lent context and builds a
        // flip-blit `gl` present path instead.
        let device: wgpu::Device;
        let queue: wgpu::Queue;
        let config: wgpu::SurfaceConfiguration;
        let surface: Option<wgpu::Surface<'static>>;
        #[cfg(target_os = "linux")]
        let gl: Option<GlPresent>;

        match target {
            GraphicsTarget::RawWindow(surface_target) => {
                let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                    backends: wgpu::Backends::PRIMARY,
                    flags: wgpu::InstanceFlags::default(),
                    memory_budget_thresholds: Default::default(),
                    backend_options: wgpu::BackendOptions::default(),
                    // `None` lets wgpu fall back to the per-surface handle (only
                    // GLES/Wayland need an explicit display handle).
                    display: None,
                });

                // The GraphicsSurface is 'static + Send + Sync and impls the
                // raw-window-handle traits, so it converts into a wgpu surface
                // target directly, yielding a Surface<'static>.
                let surf = instance.create_surface(surface_target).ok()?;

                // Deferred to a runloop turn by the backend, so block_on is safe.
                let adapter =
                    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::HighPerformance,
                        force_fallback_adapter: false,
                        compatible_surface: Some(&surf),
                    }))
                    .ok()?;

                // vello's `flatten` shader needs the f16 capability on Vulkan (naga
                // rejects it otherwise: "requires capability SHADER_FLOAT16_IN_FLOAT32").
                // Metal doesn't require the explicit feature, but Vulkan does — so
                // request `SHADER_F16` when the adapter offers it. A GPU without f16
                // (e.g. the Android emulator's Vulkan) can't run vello at all; it stays
                // on canvas-native there.
                let f16 = wgpu::Features::SHADER_F16 & adapter.features();
                // Request the adapter's OWN limits, not `Limits::default()`. The default
                // baseline asks for `max_inter_stage_shader_variables: 16`, but iOS Metal
                // (simulator AND device) caps that at 15 — so `request_device` with the
                // default fails outright (`LimitsExceeded`). macOS Metal allows 16, which
                // is why the default worked there and masked this. Taking `adapter.limits()`
                // requests exactly what the GPU provides — always grantable, never over-asks
                // — and it's uniform across backends (a no-op widening on macOS/desktop,
                // the needed downgrade on iOS). vello's own minimums are validated by
                // `Renderer::new` below; a GPU too weak for vello fails there → canvas-native.
                let (dev, q) =
                    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                        label: Some("canvas-vello-device"),
                        required_features: f16,
                        required_limits: adapter.limits(),
                        memory_hints: wgpu::MemoryHints::default(),
                        experimental_features: wgpu::ExperimentalFeatures::default(),
                        trace: wgpu::Trace::Off,
                    }))
                    .ok()?;

                let caps = surf.get_capabilities(&adapter);
                // Prefer a NON-sRGB surface format. vello writes already-sRGB-encoded
                // bytes into the linear `Rgba8Unorm` target; the blit is a straight
                // copy, so the surface must store those bytes verbatim. An sRGB surface
                // (`*UnormSrgb`, often `caps.formats[0]` on macOS) would gamma-encode
                // them AGAIN on store — washing the on-screen colors out so they no
                // longer match the palette (or the recording, whose IOSurface is
                // non-sRGB). Fall back to the default if no linear format is offered.
                let format = caps
                    .formats
                    .iter()
                    .copied()
                    .find(|f| !f.is_srgb())
                    .unwrap_or(caps.formats[0]);
                // The canvas surface is NOT opaque — it composites over the app UI behind
                // it, and a scene is transparent wherever the author didn't paint (a PDF
                // page paints its content but leaves un-drawn regions clear). An `Opaque`
                // alpha mode makes the window IGNORE that alpha and show the raw RGB —
                // i.e. clear regions (RGB 0) render as solid BLACK instead of letting the
                // background show through (the "PDF watermark is a black blob" bug). Pick
                // an alpha-respecting mode; vello writes PREMULTIPLIED alpha, so prefer
                // `PreMultiplied`, then `PostMultiplied`, falling back only if neither is
                // offered.
                use wgpu::CompositeAlphaMode::{PostMultiplied, PreMultiplied};
                let alpha_mode = if caps.alpha_modes.contains(&PreMultiplied) {
                    PreMultiplied
                } else if caps.alpha_modes.contains(&PostMultiplied) {
                    PostMultiplied
                } else {
                    caps.alpha_modes[0]
                };
                let cfg = wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format,
                    width: w,
                    height: h,
                    present_mode: wgpu::PresentMode::AutoVsync,
                    desired_maximum_frame_latency: 2,
                    alpha_mode,
                    view_formats: vec![],
                };
                surf.configure(&dev, &cfg);

                device = dev;
                queue = q;
                config = cfg;
                surface = Some(surf);
                #[cfg(target_os = "linux")]
                {
                    gl = None;
                }
            }
            GraphicsTarget::Gl(gl_target) => {
                // GTK4 (Linux): the backend lends a live GL context instead of a
                // window handle. Adopt it through wgpu's GL backend and present by
                // flip-blitting `self.target` into the GLArea framebuffer. Only
                // compiled on Linux (wgpu-hal/gles + the GTK backend live there);
                // any other platform that somehow produced a GL target bails to
                // canvas-native.
                #[cfg(target_os = "linux")]
                {
                    // Contract: the context must be current for the adopt call and
                    // for every use/drop of what it returns (see `GlTarget` /
                    // `adopt`). render()/resize()/Drop each re-establish it.
                    gl_target.make_current();
                    let (dev, q) = adopt(&gl_target)?;
                    // Scriptable evidence the GPU path engaged, INDEPENDENT of
                    // whether the embedding app installed a `log` backend
                    // (`host-gtk` installs none, so the `log::info!` in `on_ready`
                    // is silent there) — same rationale as host-linux-desktop's
                    // `eprintln!` first-frame diagnostic.
                    eprintln!(
                        "[canvas-vello] adopted GtkGLArea GL context — GPU canvas active ({w}x{h})"
                    );
                    let blit = FlipBlit::new(&dev);
                    // A size/format record ONLY — no create_surface, no configure.
                    // Format is `Rgba8Unorm` (the GLArea FBO's format, and the same
                    // format as `self.target`), so the flip-blit is a straight copy.
                    // The non-`format`/`width`/`height` fields are inert here (never
                    // used to configure a surface) but kept valid for shape parity.
                    config = wgpu::SurfaceConfiguration {
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        format: TARGET_FORMAT,
                        width: w,
                        height: h,
                        present_mode: wgpu::PresentMode::AutoVsync,
                        desired_maximum_frame_latency: 2,
                        alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
                        view_formats: vec![],
                    };
                    device = dev;
                    queue = q;
                    surface = None;
                    gl = Some(GlPresent { target: gl_target, blit });
                }
                #[cfg(not(target_os = "linux"))]
                {
                    // Only GTK4 lends a GL context, and that backend only builds on
                    // Linux; elsewhere this renderer needs a swapchain.
                    let _ = gl_target;
                    return None;
                }
            }
        }

        // `format` for the swapchain blitter: on the GL path this is
        // `Rgba8Unorm` (unused there — the flip-blit does the copy instead).
        let format = config.format;

        // GL path (no swapchain surface) ⇒ vello must compile shaders on THIS
        // (current-context) thread; a thread-bound GL context makes vello's
        // parallel worker-thread init panic (see `new_vello_renderer`). Native
        // swapchain contexts keep the faster parallel init.
        let single_threaded_init = surface.is_none();

        let renderer = new_vello_renderer(&device, single_threaded_init)?;
        // Build the dedicated image renderer up front, not lazily on the first
        // image bake. Its first `render_to_texture` would otherwise run on a
        // just-created renderer (shaders/pipelines compiling) on the same frame
        // an image is first placed — a one-shot, timing-sensitive path that
        // intermittently left freshly-placed media blank until the next
        // interaction. Created eagerly here it's always warm by first use. A
        // `None` (transient build failure) is non-fatal: the lazy sites below
        // retry, and an image-less canvas simply never touches it.
        let image_renderer = new_vello_renderer(&device, single_threaded_init);

        let (target, target_view) = make_target(&device, w, h);
        let blitter = wgpu::util::TextureBlitter::new(&device, format);
        // Built once if the canvas has any texture layers (needs `device` before
        // it's moved into the struct).
        let layer_compositor =
            (!layers.is_empty()).then(|| LayerCompositor::new(&device));

        Some(Self {
            device,
            queue,
            surface,
            #[cfg(target_os = "linux")]
            gl,
            config,
            renderer,
            image_renderer,
            scene: VelloScene::new(),
            target,
            target_view,
            blitter,
            // `1.0` is the historical "no dpr reported" behavior (render at
            // physical scale); a backend reporting the real factor (macOS
            // backingScaleFactor) makes the logical scene fill the surface.
            scale: if scale > 0.0 { scale as f64 } else { 1.0 },
            native_capture: capture.clone().map(NativeCapture::new),
            next_capture: None,
            layer_compositor,
            layers,
            capture,
            readback: None,
            shape_pass: None,
            overlay: None,
            overlay_compositor: None,
            cached_layers: HashMap::new(),
            cached_ops: HashMap::new(),
            transform_compositor: None,
        })
    }

    fn resize(&mut self, size: (u32, u32)) {
        self.config.width = size.0.max(1);
        self.config.height = size.1.max(1);
        // GL (GTK4): re-establish the lent context before recreating GPU objects
        // (new_external's contract). GTK owns the framebuffer — it's re-wrapped
        // every frame in render() — so there's nothing to configure here.
        #[cfg(target_os = "linux")]
        if let Some(gl) = &self.gl {
            gl.target.make_current();
        }
        // Swapchain path only: GTK reallocates its own framebuffer.
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        // Read-back buffer + hybrid overlay are sized to the target; invalidate
        // so render() recreates them for the new dimensions. Cached layer
        // textures are viewport-sized too — drop them so a stale-size raster is
        // never composited; the app re-bakes (`dirty`) on the resize repaint.
        self.readback = None;
        self.overlay = None;
        self.cached_layers.clear();
        let (target, target_view) =
            make_target(&self.device, self.config.width, self.config.height);
        self.target = target;
        self.target_view = target_view;
    }

    /// (Re)bake the dirty cached layers in a `ScenePlan::Cached` backdrop into
    /// their viewport-sized textures. A layer is rendered only when `dirty` (or
    /// the first time we see it / after a resize dropped its texture) — that's
    /// the whole point: a `dirty: false` pan reuses the retained texture and this
    /// does nothing, so per-frame cost is the composite, not the raster.
    fn bake_cached_layers(&mut self, layers: &[CachedRef]) {
        let (w, h) = (self.config.width, self.config.height);
        // Overscan: bake each cached layer into a texture larger than the viewport
        // by `frac` on every side, so a pan up to `frac`·viewport composites the
        // retained texture under a transform (O(1)) with no black edge — re-baking
        // only when the pan runs past the margin (the app's `far` recenter). `0.0`
        // (the default) is exactly the old viewport-sized behavior. Device size is
        // `(1 + 2·frac)·viewport`; content is rendered offset by `frac·viewport`
        // (logical) so the texture's top-left maps to screen `-frac·viewport`.
        let frac = overscan_frac();
        let (ow, oh) = overscan_dims(w, h, frac);
        let margin = (
            frac as f64 * (w as f64) / self.scale,
            frac as f64 * (h as f64) / self.scale,
        );
        for layer in layers {
            let missing = !self.cached_layers.contains_key(&layer.id);
            if !(layer.dirty || missing) {
                continue;
            }
            // `layer_cached(dirty=false)` carries EMPTY ops (reuse the retained
            // texture). After a resize drops the texture (`missing`), a non-dirty
            // frame would bake those empty ops → a transparent (black) layer — the
            // "canvas goes black until I draw" bug. So retain the last DIRTY ops
            // and re-bake from them when we must bake a missing layer. Move them
            // out for encoding (so `cached_ops` isn't borrowed while `scene`/the
            // renderer are), then put them back.
            if layer.dirty {
                self.cached_ops.insert(layer.id, layer.ops.to_vec());
            }
            let Some(ops) = self.cached_ops.remove(&layer.id) else {
                continue; // never baked dirty yet → nothing to re-bake
            };
            if missing {
                self.cached_layers.insert(layer.id, make_target(&self.device, ow, oh));
            }
            // Encode the ops at the dpr base (+ overscan offset) into a fresh vello
            // scene, then compute-render into the layer texture.
            self.scene.reset();
            encode_scene(
                &ops,
                &mut self.scene,
                Affine::scale(self.scale) * Affine::translate(margin),
            );
            let params = RenderParams {
                base_color: Color::from_rgba8(0, 0, 0, 0),
                width: ow,
                height: oh,
                antialiasing_method: AaConfig::Area,
            };
            // Bake image layers on the dedicated `image_renderer` so the grid/ink
            // bakes (which run on `self.renderer`) can't shrink the image atlas out
            // from under them (see the `image_renderer` field). Image-less layers
            // stay on the main renderer.
            let has_image = ops.iter().any(|op| matches!(op, DrawOp::Image { .. }));
            // Normally built eagerly in the constructor; retry here only if that
            // transiently failed.
            if has_image && self.image_renderer.is_none() {
                // GL path (surface None) needs single-threaded shader init.
                self.image_renderer = new_vello_renderer(&self.device, self.surface.is_none());
            }
            let view = &self.cached_layers.get(&layer.id).unwrap().1;
            let renderer = match (has_image, self.image_renderer.as_mut()) {
                (true, Some(r)) => r,
                _ => &mut self.renderer,
            };
            let _ =
                renderer.render_to_texture(&self.device, &self.queue, &self.scene, view, &params);
            self.cached_ops.insert(layer.id, ops);
        }
    }

    /// Render the scene and present a frame. Returns `true` iff a frame was
    /// actually presented; `false` when the swapchain texture couldn't be
    /// acquired (drawable not ready yet — common for the very first frame on a
    /// freshly-created `CAMetalLayer`) or vello's encode failed. `on_ready`
    /// uses the return to retry the FIRST frame until it lands, so the initial
    /// scene (e.g. the canvas's white background) isn't lost to a dark surface.
    fn render(&mut self, canvas_scene: &CanvasScene) -> bool {
        // GL (GTK4): the lent context must be current before ANY GPU work this
        // frame — vello's compute render into `self.target`, the shape/overlay
        // composites, the capture read-back, and the final flip-blit all require
        // it, and GTK may have unbound it since the last frame (a resize, or
        // another GLArea drawing in between). `make_current` also re-binds the
        // GLArea's own FBO (attach_buffers), which `wrap_framebuffer` reads in the
        // present tail. No-op on the swapchain path (the surface owns presentation).
        #[cfg(target_os = "linux")]
        if let Some(gl) = &self.gl {
            gl.target.make_current();
        }
        // Decide how to draw this scene from its leading ops (see `ScenePlan`).
        // A scene that's entirely Normal-blend shape batches is drawn by the
        // instanced pass alone; a scene whose LEADING ops are shapes (a backdrop)
        // instances those and composites vello's content over them; anything else
        // is plain vello. All three converge on the same pixels (CLAUDE.md §7).
        let plan = plan_scene(canvas_scene.ops());

        // vello renders its content (the whole scene for `Vello`, only `rest` for
        // `Hybrid`) over a transparent base. `Vello` targets the main `target`;
        // `Hybrid` targets the separate `overlay`, so the instanced backdrop drawn
        // into `target` below survives underneath. `Shapes` skips vello entirely.
        let (content_ops, to_overlay): (Option<&[DrawOp]>, bool) = match &plan {
            ScenePlan::Vello => (Some(canvas_scene.ops()), false),
            ScenePlan::Hybrid { rest, .. } => {
                if self.overlay.is_none() {
                    self.overlay =
                        Some(make_target(&self.device, self.config.width, self.config.height));
                }
                (Some(rest), true)
            }
            ScenePlan::Shapes(_) => (None, false),
            ScenePlan::Cached { rest, layers } => {
                // Cached layers form the backdrop (composited from their textures
                // below); the live ink (`rest`) renders through vello into the
                // overlay so the backdrop survives underneath, exactly like Hybrid.
                //
                // A `Cached` frame must carry SOME vello `render_to_texture` submit
                // or it won't present (the surface stays on the last submitted
                // frame — the "pan doesn't update until you draw" freeze). A bake
                // (a dirty/first-seen layer) is such a submit; live `rest` is too.
                // So we only force an EMPTY `rest` through vello when NEITHER
                // happens — a pure composite-only reuse frame (a pan that re-uses
                // every cached texture). When a layer bakes (e.g. drawing re-bakes
                // the ink layer every point), forcing an empty `rest` would add a
                // WASTED full-viewport vello pass per frame — a real cost on the
                // recorder's drawing path. The empty overlay is never composited
                // below (guarded by `!rest.is_empty()`).
                let any_bake = layers
                    .iter()
                    .any(|l| l.dirty || !self.cached_layers.contains_key(&l.id));
                if rest.is_empty() && any_bake {
                    (None, false)
                } else {
                    if self.overlay.is_none() {
                        self.overlay = Some(make_target(
                            &self.device,
                            self.config.width,
                            self.config.height,
                        ));
                    }
                    (Some(rest), true)
                }
            }
        };
        // Bake any dirty cached layers into their viewport-sized textures (only
        // re-rasters on `dirty`); composited under their transforms in the encoder
        // pass below. Done before the content render — both reuse `self.scene`.
        if let ScenePlan::Cached { layers, .. } = &plan {
            self.bake_cached_layers(layers);
        }
        if let Some(ops) = content_ops {
            self.scene.reset();
            // Base transform = device scale: the author's Scene is in LOGICAL
            // coordinates; scaling by the dpr makes it fill the physical-pixel
            // surface (no retina under-fill). `1.0` → identity (physical scale).
            encode_scene(ops, &mut self.scene, Affine::scale(self.scale));

            let params = RenderParams {
                base_color: Color::from_rgba8(0, 0, 0, 0),
                width: self.config.width,
                height: self.config.height,
                antialiasing_method: AaConfig::Area,
            };
            let view = if to_overlay { &self.overlay.as_ref().unwrap().1 } else { &self.target_view };
            // Route image-bearing content (a live-dragged media item lives in
            // `rest`) to the dedicated `image_renderer`. The main renderer's vello
            // image atlas is shrunk to 1×1 by image-less bakes (grid/ink); a later
            // image render resizes it back WITHOUT re-uploading the cached image →
            // the live image renders blank ("media disappears while dragging").
            // `image_renderer` only ever renders image content, so its atlas keeps
            // the upload (same protection as the cached image-layer bakes).
            let has_image = ops.iter().any(|op| matches!(op, DrawOp::Image { .. }));
            // Normally built eagerly in the constructor; retry here only if that
            // transiently failed.
            if has_image && self.image_renderer.is_none() {
                // GL path (surface None) needs single-threaded shader init.
                self.image_renderer = new_vello_renderer(&self.device, self.surface.is_none());
            }
            // Image content MUST render on a renderer whose vello image atlas was
            // never shrunk by an image-less frame, or the image renders blank/
            // black (see the comment above). The dedicated `image_renderer` is
            // that renderer. If it's unavailable, `self.renderer` may have been
            // contaminated by image-less frames (e.g. a vector-only page shown
            // before an image-bearing one) — so make `self.renderer` safe for
            // images too by forcing the cached images to RE-UPLOAD this frame
            // (bumping the encode cache's generation rebuilds their Blob, so vello
            // can't reuse a stale/evicted atlas slot).
            if has_image && self.image_renderer.is_none() {
                crate::encode::force_image_reupload();
                self.scene.reset();
                encode_scene(ops, &mut self.scene, Affine::scale(self.scale));
            }
            let renderer = match (has_image, self.image_renderer.as_mut()) {
                (true, Some(r)) => r,
                _ => &mut self.renderer,
            };
            if renderer
                .render_to_texture(&self.device, &self.queue, &self.scene, view, &params)
                .is_err()
            {
                return false;
            }
        }

        // Encoder for the target-space passes (shape backdrop / composites /
        // layers) and the final present blit. Built before acquiring the present
        // destination so the target passes are identical on both present paths —
        // they only ever touch `self.target_view`, never the surface/FBO.
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("canvas-vello-blit"),
            });
        // Instanced shape backdrop, BEFORE the layer composite and blit (it owns
        // the target clear that vello would otherwise have done). For a `Hybrid`
        // scene, follow it by compositing vello's content (in `overlay`) over the
        // backdrop. Disjoint field borrows — bind the shared refs to locals first.
        match &plan {
            ScenePlan::Vello => {}
            ScenePlan::Shapes(batches) => {
                if self.shape_pass.is_none() {
                    self.shape_pass = Some(ShapePass::new(&self.device));
                }
                let device = &self.device;
                let queue = &self.queue;
                let target_view = &self.target_view;
                let (cw, ch) = (self.config.width, self.config.height);
                let s = self.scale as f32;
                self.shape_pass.as_mut().unwrap().render(
                    device, queue, &mut encoder, target_view, batches, s, cw, ch,
                );
            }
            ScenePlan::Hybrid { prefix, .. } => {
                if self.shape_pass.is_none() {
                    self.shape_pass = Some(ShapePass::new(&self.device));
                }
                if self.overlay_compositor.is_none() {
                    self.overlay_compositor = Some(OverlayCompositor::new(&self.device));
                }
                let device = &self.device;
                let queue = &self.queue;
                let target_view = &self.target_view;
                let (cw, ch) = (self.config.width, self.config.height);
                let s = self.scale as f32;
                // 1) instanced backdrop into target_view (clears + draws it).
                self.shape_pass.as_mut().unwrap().render(
                    device, queue, &mut encoder, target_view, prefix, s, cw, ch,
                );
                // 2) vello's content (in overlay) over the backdrop, in place.
                let overlay_view = &self.overlay.as_ref().unwrap().1;
                self.overlay_compositor.as_ref().unwrap().composite(
                    device,
                    &mut encoder,
                    overlay_view,
                    target_view,
                );
            }
            ScenePlan::Cached { layers, rest } => {
                // Lazy-build the compositors (disjoint from the locals bound next).
                if self.transform_compositor.is_none() {
                    self.transform_compositor = Some(TransformCompositor::new(&self.device));
                }
                if !rest.is_empty() && self.overlay_compositor.is_none() {
                    self.overlay_compositor = Some(OverlayCompositor::new(&self.device));
                }
                let device = &self.device;
                let target_view = &self.target_view;
                let (cw, ch) = (self.config.width, self.config.height);
                let s = self.scale as f32;
                let frac = overscan_frac();
                let (ow, oh) = overscan_dims(cw, ch, frac);
                // Clear, then composite each cached layer (in order) under its
                // camera transform — one transformed quad each, no per-op work.
                crate::compose_transform::clear_to_transparent(&mut encoder, target_view);
                let tc = self.transform_compositor.as_ref().unwrap();
                for layer in layers {
                    if let Some((tex, view)) = self.cached_layers.get(&layer.id) {
                        // Skip a stale-size texture (resize race) — the app re-bakes.
                        // Compare against the OVERSCAN dims (what bake allocates).
                        if tex.width() == ow && tex.height() == oh {
                            tc.composite(
                                device, &mut encoder, view, target_view,
                                layer.transform, s, layer.alpha, cw, ch, frac,
                            );
                        }
                    }
                }
                // Live ink (`rest`, rendered into overlay) over the backdrop.
                if !rest.is_empty() {
                    let overlay_view = &self.overlay.as_ref().unwrap().1;
                    self.overlay_compositor.as_ref().unwrap().composite(
                        device,
                        &mut encoder,
                        overlay_view,
                        target_view,
                    );
                }
            }
        }
        // Texture layers (macOS): composite the camera/screen-share/… into the
        // target BEFORE the blits, so the strokes + layers are one image that
        // both the on-screen surface AND the recording IOSurface receive.
        // Disjoint field borrows — bind the shared ones first.
        {
            let device = &self.device;
            let queue = &self.queue;
            let target_view = &self.target_view;
            let (cw, ch) = (self.config.width, self.config.height);
            let s = self.scale as f32;
            if let Some(lc) = self.layer_compositor.as_mut() {
                lc.composite_layers(device, queue, &mut encoder, &self.layers, target_view, s, cw, ch);
            }
        }

        // Acquire the present destination and encode the final blit of
        // `self.target` into it. Held here (as `swap_frame`) so `.present()` runs
        // after `submit`. Two paths, same source texture, converging on-screen:
        //   * Swapchain (RawWindow): a STRAIGHT copy into the acquired surface
        //     texture. `self.target` holds already-sRGB-encoded bytes and the
        //     surface format is non-sRGB, so no re-encode (see `RenderState::new`).
        //   * GL (GTK4): a vertical-FLIP straight copy into the GLArea's
        //     framebuffer. Still NO colour transform — the FBO is `Rgba8Unorm`
        //     like `self.target`, so the bytes pass through verbatim; only the V
        //     axis inverts because `GlTarget::origin() == BottomLeft` (GTK
        //     composites the framebuffer bottom-up). This is why the GL blit is
        //     `FlipBlit`, NOT the wgpu-host's sRGB-re-encoding blit.
        //
        // Two `#[cfg]` bindings rather than one match with a cfg'd arm: off Linux
        // there is no GL path (`surface` is always `Some`), so a `None` arm there
        // would be dead code with an unreachable trailing expression. Splitting by
        // cfg keeps each platform's binding total and warning-free.
        #[cfg(target_os = "linux")]
        let swap_frame = match &self.surface {
            Some(surface) => {
                let frame = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(t)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                    // Skip the frame on timeout/occluded/outdated/lost/validation.
                    _ => return false,
                };
                let surface_view =
                    frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                self.blitter.copy(&self.device, &mut encoder, &self.target_view, &surface_view);
                Some(frame)
            }
            None => {
                // GL (GTK4): NO present-time blit here. This `render()` runs
                // OUT-OF-BAND — it's driven by a reactive effect, not GTK's
                // render signal — and GtkGLArea only composites what is drawn
                // INSIDE that signal. An out-of-band blit is therefore never
                // shown (the "canvas stuck on the clear colour" bug). So the
                // FBO flip-blit is deferred to `blit_to_fbo`, which the backend
                // runs in the render signal (registered via `GlTarget::on_render`
                // in `on_ready`). Here we only render vello into `self.target`
                // (submitted with `encoder` below) and, via `present()` at the
                // tail, schedule the render pass that runs `blit_to_fbo`.
                None
            }
        };
        #[cfg(not(target_os = "linux"))]
        let swap_frame = {
            // Off Linux the target is always a swapchain surface.
            let surface = self.surface.as_ref().expect("non-Linux graphics target is a swapchain");
            let frame = match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t)
                | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                // Skip the frame on timeout/occluded/outdated/lost/validation.
                _ => return false,
            };
            let surface_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.blitter.copy(&self.device, &mut encoder, &self.target_view, &surface_view);
            Some(frame)
        };

        // Throttle recording capture to a fixed ~60fps, DECOUPLED from the
        // on-screen render rate (which is input-driven and can run faster). The
        // screen presents every frame above; only ~60×/sec do we also pay the
        // capture blit, so a fast screen doesn't inflate the recorded fps or its
        // GPU cost. A phase accumulator (advance by a fixed interval per capture)
        // keeps the AVERAGE at 60fps even when render timing is uneven; we resync
        // if we fall a whole interval behind (e.g. after a stall).
        // TODO: make CAPTURE_INTERVAL user-configurable (capture-rate setting).
        const CAPTURE_INTERVAL: std::time::Duration = std::time::Duration::from_micros(16_667);
        let capture_now = {
            let wants = self.native_capture.as_ref().is_some_and(|nc| nc.wants())
                || self.capture.as_ref().is_some_and(|w| w.wants_cpu_frames());
            if !wants {
                self.next_capture = None;
                false
            } else {
                let now = std::time::Instant::now();
                let due = self.next_capture.map_or(true, |t| now >= t);
                if due {
                    let base = self.next_capture.unwrap_or(now);
                    let next = base + CAPTURE_INTERVAL;
                    self.next_capture = Some(if next <= now { now + CAPTURE_INTERVAL } else { next });
                }
                due
            }
        };

        // Zero-copy capture (macOS): blit the same target into the next ring
        // IOSurface in THIS encoder, so it's submitted with the frame. Disjoint
        // field borrows (device/target_view vs. native_capture) — bind the
        // shared ones first. Gated by `capture_now` (the 60fps throttle).
        let native_publish = {
            let device = &self.device;
            let target_view = &self.target_view;
            let (cw, ch) = (self.config.width, self.config.height);
            match self.native_capture.as_mut() {
                Some(nc) if nc.wants() && capture_now => {
                    nc.blit_into(device, &mut encoder, target_view, cw, ch)
                }
                _ => None,
            }
        };

        self.queue.submit([encoder.finish()]);
        match swap_frame {
            // Swapchain: hand the acquired texture to the compositor.
            Some(frame) => frame.present(),
            // GL (GTK4): tell the backend the framebuffer is ready so GTK
            // re-composites the GLArea into the widget tree (queue_render).
            None => {
                #[cfg(target_os = "linux")]
                if let Some(gl) = &self.gl {
                    gl.target.present();
                }
            }
        }

        // Publish the just-blitted IOSurface AFTER submit (the ring guarantees
        // it isn't reused until POOL frames later, so the in-flight GPU blit
        // finishes long before then — no fence needed at this cadence).
        if let Some(idx) = native_publish {
            self.native_capture.as_ref().unwrap().publish(idx);
        }

        // CPU read-back fallback only when the zero-copy path ISN'T carrying the
        // recording (non-macOS, or a CPU-only `subscribe` consumer).
        let native_active = self.native_capture.as_ref().is_some_and(|nc| nc.wants());
        let cpu_active = self.capture.as_ref().is_some_and(|w| w.wants_cpu_frames());
        if native_active || cpu_active {
            // Announce the GPU recording mode ONCE. This is the FAST path (the
            // scene is GPU-rendered by vello); the CPU-renderer fallback on
            // sim/emulator warns separately. `wants()`/`wants_cpu_frames` only
            // flip true while a recorder taps, so this fires on a recording's
            // first frame. (`log::info!` reaches logcat/console on Android/desktop;
            // it's a no-op on iOS, which is fine — iOS recording is GPU on device.)
            static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                log::info!(
                    "canvas-vello: recording on the GPU renderer ({})",
                    if native_active { "zero-copy IOSurface" } else { "GPU\u{2192}CPU read-back" }
                );
            }
        }
        if !native_active && capture_now {
            self.capture_frame();
        }
        true
    }

    /// Present the last-rendered `self.target` into the GLArea's framebuffer,
    /// **from inside GTK's `render` signal**. Registered via
    /// [`GlTarget::on_render`] in `on_ready`; the backend invokes it with the
    /// GL context current and the area's FBO bound.
    ///
    /// # The invariant this method exists to honour (CLAUDE.md §5)
    ///
    /// GtkGLArea only composites what is drawn *during its own render pass*.
    /// `render()` runs out-of-band (a reactive effect), renders vello into
    /// `self.target`, and calls `gl.target.present()` (queue_render) to
    /// SCHEDULE a pass. GTK then emits `render` and the backend runs THIS
    /// callback — the one and only place the flip-blit's result is guaranteed
    /// to reach the screen. Blitting in `render()` instead left the widget
    /// showing the pre-adoption clear colour forever (the bug this fixes).
    ///
    /// A separate one-shot encoder (not `render()`'s): this runs on a different
    /// tick, and its only job is the `self.target` → FBO copy.
    #[cfg(target_os = "linux")]
    fn blit_to_fbo(&mut self) {
        let Some(gl) = &self.gl else { return };
        // Defensive: GTK has already made the context current + bound the FBO
        // before emitting `render`, but `make_current` (→ `attach_buffers`) is
        // cheap and re-binds the FBO that `wrap_framebuffer` reads below,
        // keeping this correct regardless of what ran between passes.
        gl.target.make_current();
        // Re-wrap the GLArea's colour attachment every frame — GTK reallocates
        // it across resizes/re-realizes, so a cached wrap would target freed
        // GPU memory (`wrap_framebuffer` doc).
        let Some(fbo_tex) = wrap_framebuffer(
            &self.device,
            &gl.target,
            self.config.width,
            self.config.height,
        ) else {
            // Framebuffer not wrappable yet (mid-realize) — nothing to
            // composite this pass; a later `present()` re-runs this.
            return;
        };
        let fbo_view = fbo_tex.create_view(&wgpu::TextureViewDescriptor::default());
        debug_assert_eq!(
            gl.target.origin(),
            runtime_core::primitives::graphics::FramebufferOrigin::BottomLeft,
            "flip blit assumes a bottom-left GL framebuffer origin",
        );
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("canvas-vello-gl-present"),
        });
        gl.blit.draw(&self.device, &mut encoder, &self.target_view, &fbo_view);
        self.queue.submit([encoder.finish()]);
    }

    /// Read the just-rendered `target` texture back GPU→CPU and write it to the
    /// capture `FrameWriter` as RGBA8, feeding the app's `MediaStream`. No-op
    /// unless a `capture` sink is set AND a consumer is tapping frames
    /// (`wants_cpu_frames`), so an un-recorded canvas does zero read-back.
    ///
    /// v1 is a blocking `copy_texture_to_buffer` + `map` read-back on the render
    /// thread (correct, app-resolution, every frame). The zero-copy path
    /// (render straight into an IOSurface-backed texture + publish it as the
    /// stream's `native_source`) is the planned optimization.
    fn capture_frame(&mut self) {
        let writer = match &self.capture {
            Some(w) if w.wants_cpu_frames() => w.clone(),
            _ => return,
        };
        let (w, h) = (self.config.width, self.config.height);
        if w == 0 || h == 0 {
            return;
        }

        let unpadded_bpr = w * 4;
        let padded_bpr = unpadded_bpr.div_ceil(256) * 256;

        // (Re)create the row-padded read-back buffer to match the target size.
        // Mutate `self.readback` BEFORE taking the `&buffer` borrow below.
        let need_new = self.readback.as_ref().map(|(_, bpr)| *bpr != padded_bpr).unwrap_or(true);
        if need_new {
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("canvas-vello-capture-readback"),
                size: (padded_bpr as u64) * (h as u64),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            self.readback = Some((buf, padded_bpr));
        }
        let buffer = &self.readback.as_ref().unwrap().0;

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("canvas-vello-capture-copy"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);

        // Map + block until the GPU finishes (v1 readback; main-thread).
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());

        // Strip row padding into a tightly-packed RGBA frame.
        let data = slice.get_mapped_range();
        let mut frame = Vec::with_capacity((unpadded_bpr as usize) * (h as usize));
        for row in 0..h {
            let start = (row * padded_bpr) as usize;
            frame.extend_from_slice(&data[start..start + unpadded_bpr as usize]);
        }
        drop(data);
        buffer.unmap();

        // vello target is Rgba8Unorm, top-down, straight alpha — exactly the
        // FrameWriter contract.
        writer.write_rgba8(w, h, &frame);
    }
}

/// GL (GTK4): the adopted context must be current to DROP anything derived from
/// it — `new_external`'s adapter holds no EGL handle and never makes the context
/// current for you, so dropping the device/queue/textures off a non-current
/// context is UB. Every render/resize already re-establishes it; this covers the
/// teardown path (surface lost, canvas unmounted), where nothing else does. The
/// `make_current` runs in the Drop body, BEFORE the struct's fields (device,
/// queue, target, …) drop — Rust drops fields after `Drop::drop` returns.
/// No-op on the swapchain path (no `gl`) and off Linux.
#[cfg(target_os = "linux")]
impl Drop for RenderState {
    fn drop(&mut self) {
        if let Some(gl) = &self.gl {
            gl.target.make_current();
        }
    }
}

// ============================================================================
// GL present path (Linux/GTK4)
// ============================================================================
//
// GTK4 can't hand out a per-widget window handle, so the `graphics` primitive
// lends a live GL context (`GtkGLArea`). This block adopts that context through
// wgpu's GL backend and presents by flip-blitting `self.target` into the lent
// framebuffer. Every piece here is copied from the proven
// `host-linux-desktop::linux` / `backend-linux` adoption, with ONE deliberate
// difference: the flip-blit is a STRAIGHT copy (no sRGB re-encode), because
// canvas-vello's `self.target` already holds sRGB-encoded bytes and the FBO is
// `Rgba8Unorm` — the host rendered into an `Rgba8UnormSrgb` target and had to
// re-encode; we must NOT.

#[cfg(target_os = "linux")]
use runtime_core::primitives::graphics::GlTarget;

/// The lent GL context + the flip-blit that presents into its framebuffer.
/// `self.gl` is `Some(GlPresent)` exactly when `self.surface` is `None`.
#[cfg(target_os = "linux")]
struct GlPresent {
    target: GlTarget,
    blit: FlipBlit,
}

// --- GL constants for the framebuffer-attachment query (copied from
//     host-linux-desktop / backend-linux) -----------------------------------
#[cfg(target_os = "linux")]
const GL_FRAMEBUFFER: u32 = 0x8D40;
#[cfg(target_os = "linux")]
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
#[cfg(target_os = "linux")]
const GL_FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE: u32 = 0x8CD0;
#[cfg(target_os = "linux")]
const GL_FRAMEBUFFER_ATTACHMENT_OBJECT_NAME: u32 = 0x8CD1;
#[cfg(target_os = "linux")]
const GL_TEXTURE: i32 = 0x1702;
#[cfg(target_os = "linux")]
const GL_RENDERBUFFER: i32 = 0x8D41;

/// Format of GTK's own framebuffer attachment, and so of the flip-blit's
/// output: plain `Rgba8Unorm`, storing whatever bytes the shader writes. Same
/// format as `self.target` (`TARGET_FORMAT`), so the blit is a straight copy.
#[cfg(target_os = "linux")]
const FRAMEBUFFER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Adopt the lent GL context as a wgpu `(device, queue)` on wgpu's GL backend.
/// Copied from `host-linux-desktop::adopt` (made synchronous via `pollster`,
/// which this crate already uses). `None` if the driver can't satisfy
/// `new_external` (GL below the GLES 3.0 floor) or `request_device` is rejected.
///
/// SAFETY: `new_external`'s contract is that the GL context is current for this
/// call and for every use/drop of what it returns. The caller ran
/// `make_current()` immediately before; `render`/`resize`/`Drop` each
/// re-establish it before touching anything derived from this adapter.
#[cfg(target_os = "linux")]
fn adopt(gl: &GlTarget) -> Option<(wgpu::Device, wgpu::Queue)> {
    let exposed = unsafe {
        wgpu_hal::gles::Adapter::new_external(
            |sym| gl.get_proc_address(sym),
            wgpu_types::GlBackendOptions::default(),
        )
    }?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        flags: wgpu::InstanceFlags::empty(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    // SAFETY: as above — the adapter is built from the current context.
    let adapter = unsafe { instance.create_adapter_from_hal(exposed) };
    // Request the adapter's OWN limits, not `downlevel_defaults()`. host-linux-
    // desktop's `render-wgpu` renderer is modest enough for the downlevel floor,
    // but vello is NOT: its compute pipeline binds 5 storage buffers in one
    // stage, over `downlevel_defaults()`'s `max_storage_buffers_per_shader_stage`
    // of 4 (`create_bind_group_layout` → "Too many bindings of type
    // StorageBuffers … limit is 4, count was 5"). `using_resolution()` only lifts
    // the texture-DIMENSION limits, not the storage-buffer counts, so it wouldn't
    // help. `adapter.limits()` is exactly what this GL adapter advertises (desktop
    // Mesa GL 4.6 grants far more than the floor) — always grantable, never
    // over-asks, the same invariant the swapchain path relies on. Request NO
    // features: vello needs `SHADER_F16` only on Vulkan (naga enforces it for the
    // `flatten` shader there); GL/naga doesn't require the explicit feature, and
    // asking for it on GL would fail device creation. vello's own minimums are
    // validated by `Renderer::new` (called after bringup); too weak → None.
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("canvas-vello-gl-device"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        memory_hints: wgpu::MemoryHints::default(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        trace: wgpu::Trace::Off,
    }))
    .ok()
}

/// Wrap the lent framebuffer's colour attachment as a wgpu texture. Copied from
/// `host-linux-desktop::wrap_framebuffer`, taking the pieces it needs directly
/// rather than a `HostInner`.
///
/// Re-queried every frame rather than cached: GTK reallocates the `GtkGLArea`'s
/// buffers across resizes and re-realizes, and rendering into a stale attachment
/// draws into freed GPU memory.
#[cfg(target_os = "linux")]
fn wrap_framebuffer(
    device: &wgpu::Device,
    gl: &GlTarget,
    w: u32,
    h: u32,
) -> Option<wgpu::Texture> {
    let get: unsafe extern "C" fn(u32, u32, u32, *mut i32) =
        gl_fn(gl, "glGetFramebufferAttachmentParameteriv")?;
    let (mut kind, mut name) = (0i32, 0i32);
    // SAFETY: called with the context current and GTK's framebuffer bound
    // (`make_current` runs `gtk_gl_area_attach_buffers`).
    unsafe {
        get(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE, &mut kind);
        get(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_FRAMEBUFFER_ATTACHMENT_OBJECT_NAME, &mut name);
    }
    let name = std::num::NonZeroU32::new(u32::try_from(name).ok()?)?;

    let desc = wgpu::TextureDescriptor {
        label: Some("gtk-glarea-framebuffer"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FRAMEBUFFER_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    };
    let hal_desc = wgpu_hal::TextureDescriptor {
        label: desc.label,
        size: desc.size,
        mip_level_count: desc.mip_level_count,
        sample_count: desc.sample_count,
        dimension: desc.dimension,
        format: desc.format,
        usage: wgpu_types::TextureUses::COLOR_TARGET,
        memory_flags: wgpu_hal::MemoryFlags::empty(),
        view_formats: vec![],
    };
    // SAFETY: `name` is GTK's live colour attachment, described faithfully above.
    // The no-op `drop_callback` is load-bearing: it tells wgpu-hal the object is
    // BORROWED, so dropping this texture does not delete GTK's attachment.
    let hal_texture = unsafe {
        let dev = device.as_hal::<wgpu_hal::api::Gles>()?;
        match kind {
            GL_TEXTURE => dev.texture_from_raw(name, &hal_desc, Some(Box::new(|| {}))),
            GL_RENDERBUFFER => {
                dev.texture_from_raw_renderbuffer(name, &hal_desc, Some(Box::new(|| {})))
            }
            _ => return None,
        }
    };
    // SAFETY: `hal_texture` was just built from this device.
    Some(unsafe { device.create_texture_from_hal::<wgpu_hal::api::Gles>(hal_texture, &desc) })
}

#[cfg(target_os = "linux")]
fn gl_fn<T: Copy>(gl: &GlTarget, symbol: &str) -> Option<T> {
    let p = gl.get_proc_address(symbol);
    if p.is_null() {
        return None;
    }
    debug_assert_eq!(
        std::mem::size_of::<T>(),
        std::mem::size_of::<*const std::ffi::c_void>()
    );
    Some(unsafe { *(&p as *const *const std::ffi::c_void as *const T) })
}

/// Fullscreen blit that inverts V on the way out and copies colour STRAIGHT.
///
/// Same structure as `host-linux-desktop::FlipBlit` (a fullscreen triangle whose
/// UVs invert V), but the fragment shader is a plain `textureSample` with NO
/// sRGB re-encode. The host rendered into an `Rgba8UnormSrgb` target and had to
/// put the transfer function back when writing GTK's plain `Rgba8Unorm` buffer.
/// canvas-vello's `self.target` is `Rgba8Unorm` ALREADY holding sRGB-encoded
/// premultiplied bytes (vello writes encoded bytes; the swapchain blit is also a
/// straight copy), so re-encoding here would double-gamma and wash every colour
/// out. The vertical flip is still required: `GlTarget::origin() == BottomLeft`,
/// so a top-left-origin image lands upside down without it (proven in
/// `backend-linux/tests/gl_target_adoption.rs`).
#[cfg(target_os = "linux")]
struct FlipBlit {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
}

/// Fullscreen triangle; UVs derived from clip space with the V axis inverted
/// (`(y + 1) / 2` instead of the usual `(1 - y) / 2`). Fragment = STRAIGHT copy.
#[cfg(target_os = "linux")]
const FLIP_BLIT_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) idx: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    let xy = corners[idx];
    var out: VsOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    // V inverted: `(xy.y + 1) * 0.5` (vs the usual `(1 - xy.y) * 0.5`) is the
    // entire vertical flip — GTK composites the GLArea framebuffer bottom-up.
    out.uv = vec2<f32>((xy.x + 1.0) * 0.5, (xy.y + 1.0) * 0.5);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    // STRAIGHT COPY — no linear→sRGB re-encode. `src` is Rgba8Unorm already
    // holding sRGB-encoded premultiplied bytes and the destination FBO is
    // Rgba8Unorm too, so the bytes must pass through verbatim.
    return textureSample(src, samp, in.uv);
}
"#;

#[cfg(target_os = "linux")]
impl FlipBlit {
    fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("canvas-vello-gl-flip-blit"),
            source: wgpu::ShaderSource::Wgsl(FLIP_BLIT_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas-vello-gl-flip-blit-bgl"),
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
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("canvas-vello-gl-flip-blit-pipeline"),
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
                // GTK's framebuffer is plain Rgba8Unorm; writing the
                // already-sRGB-encoded bytes through a linear target stores them
                // verbatim, keeping colours matching the other backends.
                targets: &[Some(FRAMEBUFFER_FORMAT.into())],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        // Nearest: `self.target` and the FBO are the same (drawable) size, so the
        // blit is an exact 1:1 texel copy — no filtering needed or wanted.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("canvas-vello-gl-flip-blit-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self { pipeline, sampler, layout }
    }

    fn draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
    ) {
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
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
            label: Some("canvas-vello-gl-flip-blit-pass"),
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

/// Retry the first frame on a ~frame cadence until it presents (bounded). On
/// macOS the `CAMetalLayer`'s first drawable isn't acquirable on the deferred
/// `on_ready` tick — the surface stays at its dark clear until something
/// re-renders — so without this the canvas shows dark until the first reactive
/// repaint (e.g. the first stroke). `after_ms_detached` is reentrancy-safe and
/// parked by the runtime (no handle to hold, no `mem::forget`).
fn retry_first_frame(
    scene_cell: Rc<RefCell<CanvasScene>>,
    state_cell: Rc<RefCell<Option<RenderState>>>,
    attempts_left: u32,
) {
    if attempts_left == 0 {
        return;
    }
    runtime_core::scheduling::after_ms_detached(16, move || {
        let presented = match state_cell.borrow_mut().as_mut() {
            Some(state) => state.render(&scene_cell.borrow()),
            None => return, // surface lost / canvas dropped — stop retrying
        };
        if !presented {
            retry_first_frame(scene_cell, state_cell, attempts_left - 1);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{
        BlendMode, Color as CanvasColor, ImageSource, Paint, Path, Rect as CanvasRect,
        Scene as CanvasScene, ShapeInstance,
    };

    /// Render a canvas scene to an `S×S` RGBA8 buffer on a real GPU and
    /// return the pixels. Returns `None` when the host has no usable GPU
    /// adapter / device / vello-capable GPU, so the callers skip (don't
    /// fail) on CI without a GPU.
    fn render_to_rgba(cs: &CanvasScene, s: u32) -> Option<Vec<u8>> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("canvas-test"),
            ..Default::default()
        }))
        .ok()?;
        let mut renderer = Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: None,
                pipeline_cache: None,
            },
        )
        .ok()?;

        let mut vs = VelloScene::new();
        encode_scene(cs.ops(), &mut vs, Affine::scale(1.0));
        let (target, target_view) = make_target(&device, s, s);
        let params = RenderParams {
            base_color: Color::from_rgba8(0, 0, 0, 0),
            width: s,
            height: s,
            antialiasing_method: AaConfig::Area,
        };
        renderer
            .render_to_texture(&device, &queue, &vs, &target_view, &params)
            .ok()?;

        // bytes_per_row must be 256-aligned; pick `s` so s*4 % 256 == 0 (s=64).
        let bpr = s * 4;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bpr * s) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(s),
                },
            },
            wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let data = buffer.slice(..).get_mapped_range();
        Some(data.to_vec())
    }

    /// Index helper for an `S`-wide RGBA8 buffer.
    fn px(buf: &[u8], s: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * s + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    /// End-to-end GPU proof of the eraser: fill the whole target red, then
    /// fill the center with `BlendMode::DestinationOut`. The covered pixels
    /// must become fully transparent while pixels outside stay opaque red.
    /// A renderer that dropped the `push_layer(..DestOut..)` wrap (drawing
    /// the eraser as an opaque fill instead) would leave the center opaque
    /// and fail here.
    #[test]
    fn destination_out_erases_a_hole_on_the_gpu() {
        const S: u32 = 64;
        let mut cs = CanvasScene::new();
        cs.path().add_path(Path::rect(0.0, 0.0, S as f32, S as f32));
        cs.fill(Paint::solid(CanvasColor::new(255, 0, 0, 255)));
        cs.path().add_path(Path::rect(24.0, 24.0, 16.0, 16.0));
        cs.fill(Paint::eraser());

        let Some(buf) = render_to_rgba(&cs, S) else {
            eprintln!("skip: no GPU");
            return;
        };
        let center = px(&buf, S, 32, 32);
        assert!(center[3] < 8, "center should be erased (alpha ~0), got {center:?}");
        let corner = px(&buf, S, 2, 2);
        assert!(
            corner[0] > 200 && corner[3] > 200,
            "corner should stay opaque red, got {corner:?}"
        );
    }

    /// End-to-end GPU proof of an extended blend mode: `Difference` of
    /// (200,50,50) under (50,200,50) must give |200-50|,|50-200|,|50-50| =
    /// (150,150,0). A wrong canvas→peniko Mix mapping (e.g. falling back to
    /// Normal, or swapping a mode) produces different pixels and fails here.
    #[test]
    fn difference_blend_composites_on_the_gpu() {
        const S: u32 = 64;
        let mut cs = CanvasScene::new();
        cs.path().add_path(Path::rect(0.0, 0.0, S as f32, S as f32));
        cs.fill(Paint::solid(CanvasColor::new(200, 50, 50, 255)));
        cs.path().add_path(Path::rect(0.0, 0.0, S as f32, S as f32));
        cs.fill(Paint::solid(CanvasColor::new(50, 200, 50, 255)).blend(BlendMode::Difference));

        let Some(buf) = render_to_rgba(&cs, S) else {
            eprintln!("skip: no GPU");
            return;
        };
        let c = px(&buf, S, 32, 32);
        let near = |v: u8, t: i32| (v as i32 - t).abs() <= 6;
        assert!(
            near(c[0], 150) && near(c[1], 150) && near(c[2], 0) && c[3] > 200,
            "Difference blend should give ~(150,150,0,255), got {c:?}"
        );
    }

    /// End-to-end GPU proof of dashed strokes: an 8-on/8-off dashed horizontal
    /// line must alternate opaque (dash) and transparent (gap) pixels along its
    /// length. A renderer that ignored the dash would draw a solid line (all
    /// opaque) and fail.
    #[test]
    fn dashed_stroke_has_gaps_on_the_gpu() {
        use canvas_core::Stroke as CStroke;
        const S: u32 = 64;
        let mut cs = CanvasScene::new();
        let line = Path::new().move_to(0.0, 32.0).line_to(64.0, 32.0);
        cs.stroke_path(
            line,
            Paint::solid(CanvasColor::new(0, 0, 0, 255)),
            CStroke::width(8.0).dash(vec![8.0, 8.0], 0.0),
        );
        let Some(buf) = render_to_rgba(&cs, S) else { eprintln!("skip: no GPU"); return; };
        let (mut on, mut off) = (0u32, 0u32);
        for x in 0..S {
            if px(&buf, S, x, 32)[3] > 128 { on += 1 } else { off += 1 }
        }
        assert!(on > 4 && off > 4, "dashed line should alternate on/off, got on={on} off={off}");
    }

    /// End-to-end GPU proof of a soft mask: a full red rect (content) masked by
    /// a left-half-white / right-half-absent luminance mask. The left half must
    /// show (mask luminance 1 → opaque red); the right half must be hidden (mask
    /// luminance 0 → transparent). A renderer that ignored the mask would show
    /// red everywhere and fail.
    #[test]
    fn soft_mask_modulates_content_on_the_gpu() {
        const S: u32 = 64;
        let red = Paint::solid(CanvasColor::new(255, 0, 0, 255));
        let white = Paint::solid(CanvasColor::new(255, 255, 255, 255));
        let content = vec![canvas_core::DrawOp::Fill {
            path: Path::rect(0.0, 0.0, S as f32, S as f32),
            paint: red,
            fill_rule: canvas_core::FillRule::NonZero,
        }];
        // Mask: left half white (luminance 1), right half undrawn (luminance 0).
        let mask = vec![canvas_core::DrawOp::Fill {
            path: Path::rect(0.0, 0.0, (S / 2) as f32, S as f32),
            paint: white,
            fill_rule: canvas_core::FillRule::NonZero,
        }];
        let mut cs = CanvasScene::new();
        cs.push_op(canvas_core::DrawOp::MaskGroup {
            content,
            mask,
            luminance: true,
            alpha: 1.0,
            blend: BlendMode::Normal,
        });

        let Some(buf) = render_to_rgba(&cs, S) else {
            eprintln!("skip: no GPU");
            return;
        };
        let left = px(&buf, S, 16, 32);
        let right = px(&buf, S, 48, 32);
        assert!(left[0] > 200 && left[3] > 200, "left half should be opaque red, got {left:?}");
        assert!(right[3] < 40, "right half should be masked out (transparent), got {right:?}");
    }

    /// End-to-end GPU proof of the persistent layer: frame 1 bakes a red
    /// rect into a layer; frame 2 emits ONLY an eraser hole (`clear: false`).
    /// The red must persist from frame 1 (it is *not* re-emitted in frame 2),
    /// and the eraser must cut a permanent hole in the baked content. A
    /// renderer that didn't retain the layer across frames would show an empty
    /// surface in frame 2 and fail.
    #[test]
    fn persistent_layer_accumulates_and_erases_across_frames() {
        const S: u32 = 64;
        // Unique id so the thread-local retained scene can't collide with
        // another test sharing the render thread.
        const ID: u32 = 0xB0_0B;

        // Frame 1: bake an opaque red rect into the layer (clear to start clean).
        let mut f1 = CanvasScene::new();
        f1.layer(ID, true, |l| {
            l.path().add_path(Path::rect(0.0, 0.0, S as f32, S as f32));
            l.fill(Paint::solid(CanvasColor::new(255, 0, 0, 255)));
        });
        if render_to_rgba(&f1, S).is_none() {
            eprintln!("skip: no GPU");
            return;
        }

        // Frame 2: emit ONLY an eraser hole, accumulating onto frame 1's bake.
        let mut f2 = CanvasScene::new();
        f2.layer(ID, false, |l| {
            l.path().add_path(Path::rect(24.0, 24.0, 16.0, 16.0));
            l.fill(Paint::eraser());
        });
        let buf = render_to_rgba(&f2, S).expect("frame 2 render");

        // Center: erased by frame 2's eraser → transparent.
        let center = px(&buf, S, 32, 32);
        assert!(center[3] < 8, "center should be erased, got {center:?}");
        // Corner: red baked in frame 1 must still be there in frame 2.
        let corner = px(&buf, S, 2, 2);
        assert!(
            corner[0] > 200 && corner[3] > 200,
            "frame-1 red must persist into frame 2, got {corner:?}"
        );
    }

    /// The `DrawOp::Shapes` fallback path: `encode_scene` expands a batch into
    /// ordered per-shape fills, which vello renders. Proves a batched shape
    /// reaches the GPU and paints a filled circle — the path every mixed scene
    /// (and the web backend) takes. The instanced fast path is proven separately
    /// in `shape_pass.rs`; both must paint the same interior, which is how the
    /// optimization stays a no-op on output (CLAUDE.md §7).
    #[test]
    fn shapes_batch_renders_via_vello_fallback() {
        const S: u32 = 64;
        let mut cs = CanvasScene::new();
        cs.shapes([ShapeInstance::circle(32.0, 32.0, 20.0, CanvasColor::new(0, 200, 80, 255))]);

        let Some(buf) = render_to_rgba(&cs, S) else {
            eprintln!("skip: no GPU");
            return;
        };
        let center = px(&buf, S, 32, 32);
        assert!(
            center[1] > 180 && center[0] < 60 && center[3] > 200,
            "circle center should be opaque green, got {center:?}"
        );
        let corner = px(&buf, S, 2, 2);
        assert!(corner[3] < 8, "corner should be transparent, got {corner:?}");
    }

    /// End-to-end GPU proof of the image blit: a 2×2 image (green top-left,
    /// red bottom-right) blitted to fill the whole target. The renderer must
    /// upload the pixels and scale them to `dst` — a no-op `_ => {}` arm
    /// would leave the surface transparent and fail here.
    #[test]
    fn image_blit_paints_scaled_pixels_on_the_gpu() {
        const S: u32 = 64;
        // 2×2 straight-RGBA8: TL green, TR blue, BL blue, BR red.
        let rgba = vec![
            0, 255, 0, 255, // (0,0) green
            0, 0, 255, 255, // (1,0) blue
            0, 0, 255, 255, // (0,1) blue
            255, 0, 0, 255, // (1,1) red
        ];
        let img = ImageSource::from_rgba8(1001, 2, 2, rgba);
        let mut cs = CanvasScene::new();
        cs.draw_image(img, CanvasRect::new(0.0, 0.0, S as f32, S as f32));

        let Some(buf) = render_to_rgba(&cs, S) else {
            eprintln!("skip: no GPU");
            return;
        };
        // Top-left quadrant samples the green texel; bottom-right the red one.
        let tl = px(&buf, S, 8, 8);
        assert!(tl[1] > 200 && tl[0] < 60 && tl[3] > 200, "top-left should be green, got {tl:?}");
        let br = px(&buf, S, 56, 56);
        assert!(br[0] > 200 && br[1] < 60 && br[3] > 200, "bottom-right should be red, got {br:?}");
    }

    /// `plan_scene` classifies a scene by its LEADING ops: all-shapes → `Shapes`
    /// (the whole-frame instanced pass), leading-shapes-then-other → `Hybrid`
    /// (instanced backdrop + vello over it), anything else → `Vello`. A non-Normal
    /// blend shape batch ends the leading run (it needs vello's compositor).
    #[test]
    fn plan_scene_classifies_by_leading_ops() {
        let green = CanvasColor::new(0, 200, 80, 255);

        // Empty scene → Shapes(empty) (the pass renders a transparent clear).
        assert!(matches!(plan_scene(CanvasScene::new().ops()), ScenePlan::Shapes(b) if b.is_empty()));

        // Every op a Normal shape batch → Shapes.
        let mut all_shapes = CanvasScene::new();
        all_shapes.shapes([ShapeInstance::circle(8.0, 8.0, 4.0, green)]);
        all_shapes.shapes([ShapeInstance::rect(0.0, 0.0, 4.0, 4.0, green)]);
        assert!(matches!(plan_scene(all_shapes.ops()), ScenePlan::Shapes(b) if b.len() == 2));

        // Leading shape batch, then a fill → Hybrid (prefix = the one batch).
        let mut backdrop_then_ink = CanvasScene::new();
        backdrop_then_ink.shapes([ShapeInstance::rect(0.0, 0.0, 16.0, 16.0, green)]);
        backdrop_then_ink.path().add_path(Path::rect(2.0, 2.0, 4.0, 4.0));
        backdrop_then_ink.fill(Paint::solid(CanvasColor::new(255, 0, 0, 255)));
        assert!(matches!(
            plan_scene(backdrop_then_ink.ops()),
            ScenePlan::Hybrid { prefix, rest } if prefix.len() == 1 && rest.len() == 1
        ));

        // A fill BEFORE the shapes → no leading shape run → Vello.
        let mut ink_then_shapes = CanvasScene::new();
        ink_then_shapes.path().add_path(Path::rect(2.0, 2.0, 4.0, 4.0));
        ink_then_shapes.fill(Paint::solid(green));
        ink_then_shapes.shapes([ShapeInstance::circle(8.0, 8.0, 4.0, green)]);
        assert!(matches!(plan_scene(ink_then_shapes.ops()), ScenePlan::Vello));

        // A non-Normal blend shape batch can't lead the instanced pass → Vello.
        let mut multiply_shapes = CanvasScene::new();
        multiply_shapes
            .shapes_with([ShapeInstance::circle(8.0, 8.0, 4.0, green)], BlendMode::Multiply);
        assert!(matches!(plan_scene(multiply_shapes.ops()), ScenePlan::Vello));
    }

    /// Render a scene through the FULL plan path (instanced [`ShapePass`] backdrop
    /// + vello content + [`OverlayCompositor`]), into an `S×S` target, headless —
    /// the same drawing `render` does, minus the surface acquire/blit/capture.
    /// `None` when the host has no usable GPU (callers skip rather than fail).
    fn plan_render_to_rgba(cs: &CanvasScene, s: u32) -> Option<Vec<u8>> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("canvas-plan-test"),
            ..Default::default()
        }))
        .ok()?;
        let mut renderer = Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: None,
                pipeline_cache: None,
            },
        )
        .ok()?;
        let mut shape_pass = ShapePass::new(&device);
        let compositor = OverlayCompositor::new(&device);
        let (target, target_view) = make_target(&device, s, s);
        let (_overlay, overlay_view) = make_target(&device, s, s);
        let plan = plan_scene(cs.ops());

        // vello content: whole scene for `Vello` (into target), `rest` for
        // `Hybrid` (into overlay), nothing for `Shapes`.
        let params = RenderParams {
            base_color: Color::from_rgba8(0, 0, 0, 0),
            width: s,
            height: s,
            antialiasing_method: AaConfig::Area,
        };
        let mut vs = VelloScene::new();
        match &plan {
            ScenePlan::Vello => {
                encode_scene(cs.ops(), &mut vs, Affine::scale(1.0));
                renderer.render_to_texture(&device, &queue, &vs, &target_view, &params).ok()?;
            }
            ScenePlan::Hybrid { rest, .. } => {
                encode_scene(rest, &mut vs, Affine::scale(1.0));
                renderer.render_to_texture(&device, &queue, &vs, &overlay_view, &params).ok()?;
            }
            ScenePlan::Shapes(_) => {}
            // `Cached` scenes are exercised by `CachedHarness`, not this helper.
            ScenePlan::Cached { .. } => unreachable!("plan_render_to_rgba: not used for Cached"),
        }

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        match &plan {
            ScenePlan::Vello => {}
            ScenePlan::Shapes(batches) => {
                shape_pass.render(&device, &queue, &mut enc, &target_view, batches, 1.0, s, s);
            }
            ScenePlan::Hybrid { prefix, .. } => {
                shape_pass.render(&device, &queue, &mut enc, &target_view, prefix, 1.0, s, s);
                compositor.composite(&device, &mut enc, &overlay_view, &target_view);
            }
            ScenePlan::Cached { .. } => unreachable!("plan_render_to_rgba: not used for Cached"),
        }

        let bpr = s * 4;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bpr * s) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(s),
                },
            },
            wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let data = buffer.slice(..).get_mapped_range();
        Some(data.to_vec())
    }

    /// The §7 convergence guarantee for the HYBRID path on the GPU: an instanced
    /// shape backdrop with vello ink composited over it must match the SAME scene
    /// rendered entirely through vello (where the shape batch expands to per-shape
    /// fills via `encode_scene`). Build a blue backdrop + green dot (leading
    /// shapes) under a red ink rect, render it both ways, and compare interiors
    /// (away from AA edges, where SDF vs tessellation legitimately differ).
    #[test]
    fn hybrid_backdrop_plus_ink_matches_all_vello() {
        const S: u32 = 64;
        let build = || {
            let mut cs = CanvasScene::new();
            cs.shapes([
                ShapeInstance::rect(0.0, 0.0, S as f32, S as f32, CanvasColor::new(0, 0, 255, 255)),
                ShapeInstance::circle(16.0, 16.0, 7.0, CanvasColor::new(0, 200, 80, 255)),
            ]);
            cs.path().add_path(Path::rect(40.0, 40.0, 16.0, 16.0));
            cs.fill(Paint::solid(CanvasColor::new(255, 0, 0, 255)));
            cs
        };
        let hybrid = build();
        assert!(matches!(plan_scene(hybrid.ops()), ScenePlan::Hybrid { .. }));

        let Some(h) = plan_render_to_rgba(&hybrid, S) else {
            eprintln!("skip: no GPU");
            return;
        };
        // The reference goes all-vello: `render_to_rgba` only `encode_scene`s, so
        // the leading shape batch is expanded to fills — no instanced pass.
        let reference = build();
        let v = render_to_rgba(&reference, S).expect("reference render");

        // Backdrop blue (2,2), dot green (16,16), ink red (47,47) — all interiors.
        for &(x, y) in &[(2u32, 2u32), (16, 16), (47, 47)] {
            let a = px(&h, S, x, y);
            let b = px(&v, S, x, y);
            for c in 0..4 {
                assert!(
                    (a[c] as i32 - b[c] as i32).abs() <= 6,
                    "pixel ({x},{y}) chan {c}: hybrid {a:?} vs all-vello {b:?}"
                );
            }
        }
    }

    /// Headless harness that exercises the `ScenePlan::Cached` GPU fast path —
    /// the bake-to-texture + transform-composite logic from `RenderState::render`
    /// — without a surface, persisting the cached-layer textures across `frame`
    /// calls so retention (`dirty: false` reuse) is testable. Mirrors
    /// `plan_render_to_rgba`'s standalone-GPU approach (`RenderState::new` needs a
    /// real `GraphicsSurface`, so it can't be built in a unit test). `None` when
    /// the host has no usable GPU (callers skip rather than fail).
    struct CachedHarness {
        device: wgpu::Device,
        queue: wgpu::Queue,
        renderer: Renderer,
        tc: crate::compose_transform::TransformCompositor,
        overlay_compositor: OverlayCompositor,
        cached: std::collections::HashMap<u32, (wgpu::Texture, wgpu::TextureView)>,
        s: u32,
        target: wgpu::Texture,
        target_view: wgpu::TextureView,
        overlay_view: wgpu::TextureView,
    }

    impl CachedHarness {
        fn new(s: u32) -> Option<Self> {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::all(),
                flags: wgpu::InstanceFlags::default(),
                memory_budget_thresholds: Default::default(),
                backend_options: wgpu::BackendOptions::default(),
                display: None,
            });
            let adapter = pollster::block_on(instance.request_adapter(
                &wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                },
            ))
            .ok()?;
            let (device, queue) = pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor { label: Some("cached-test"), ..Default::default() },
            ))
            .ok()?;
            let renderer = Renderer::new(
                &device,
                RendererOptions {
                    use_cpu: false,
                    antialiasing_support: AaSupport::area_only(),
                    num_init_threads: None,
                    pipeline_cache: None,
                },
            )
            .ok()?;
            let tc = crate::compose_transform::TransformCompositor::new(&device);
            let overlay_compositor = OverlayCompositor::new(&device);
            let (target, target_view) = make_target(&device, s, s);
            let (_overlay, overlay_view) = make_target(&device, s, s);
            Some(Self {
                device,
                queue,
                renderer,
                tc,
                overlay_compositor,
                cached: std::collections::HashMap::new(),
                s,
                target,
                target_view,
                overlay_view,
            })
        }

        /// Render one frame of a `Cached`-plan scene into the target and read it
        /// back. Scale is 1.0 (no dpr) so texel↔logical is 1:1.
        fn frame(&mut self, scene: &CanvasScene) -> Vec<u8> {
            let plan = plan_scene(scene.ops());
            let params = RenderParams {
                base_color: Color::from_rgba8(0, 0, 0, 0),
                width: self.s,
                height: self.s,
                antialiasing_method: AaConfig::Area,
            };
            // Bake dirty layers; render the live ink (rest) into the overlay.
            if let ScenePlan::Cached { layers, rest } = &plan {
                for layer in layers {
                    let missing = !self.cached.contains_key(&layer.id);
                    if layer.dirty || missing {
                        if missing {
                            self.cached
                                .insert(layer.id, make_target(&self.device, self.s, self.s));
                        }
                        let mut vs = VelloScene::new();
                        encode_scene(layer.ops, &mut vs, Affine::scale(1.0));
                        let view = &self.cached.get(&layer.id).unwrap().1;
                        self.renderer
                            .render_to_texture(&self.device, &self.queue, &vs, view, &params)
                            .unwrap();
                    }
                }
                if !rest.is_empty() {
                    let mut vs = VelloScene::new();
                    encode_scene(rest, &mut vs, Affine::scale(1.0));
                    self.renderer
                        .render_to_texture(
                            &self.device,
                            &self.queue,
                            &vs,
                            &self.overlay_view,
                            &params,
                        )
                        .unwrap();
                }
            }

            let mut enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            if let ScenePlan::Cached { layers, rest } = &plan {
                crate::compose_transform::clear_to_transparent(&mut enc, &self.target_view);
                for layer in layers {
                    if let Some((_, view)) = self.cached.get(&layer.id) {
                        self.tc.composite(
                            &self.device,
                            &mut enc,
                            view,
                            &self.target_view,
                            layer.transform,
                            1.0,
                            layer.alpha,
                            self.s,
                            self.s,
                            // over_frac: no overscan in this fixed-camera test
                            // composite (added to the signature in 9b19fdd; this
                            // test call wasn't updated, breaking the test target).
                            0.0,
                        );
                    }
                }
                if !rest.is_empty() {
                    self.overlay_compositor.composite(
                        &self.device,
                        &mut enc,
                        &self.overlay_view,
                        &self.target_view,
                    );
                }
            }

            // Read the target back (s chosen so s*4 is 256-aligned: s = 64).
            let bpr = self.s * 4;
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cached-readback"),
                size: (bpr * self.s) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.target,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bpr),
                        rows_per_image: Some(self.s),
                    },
                },
                wgpu::Extent3d { width: self.s, height: self.s, depth_or_array_layers: 1 },
            );
            self.queue.submit([enc.finish()]);
            buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            let data = buffer.slice(..).get_mapped_range();
            data.to_vec()
        }
    }

    /// End-to-end GPU proof of the cached-layer fast path: frame 1 bakes a red
    /// square into a cached layer (`dirty: true`); frame 2 sends **no ops**
    /// (`dirty: false`) and only a `translate(24, 24)` transform. The square must
    /// (a) still be there in frame 2 — proving the texture is RETAINED across
    /// frames (frame 2 didn't re-emit it) — and (b) appear shifted by (24, 24) —
    /// proving the transform composite. A renderer that re-rastered each frame
    /// would show nothing in frame 2 (no ops sent), and one that ignored the
    /// transform would leave the square in place.
    #[test]
    fn cached_layer_retains_and_composites_under_transform() {
        const S: u32 = 64;
        let Some(mut h) = CachedHarness::new(S) else {
            eprintln!("skip: no GPU");
            return;
        };

        // Frame 1: bake a 16×16 red square at (8, 8) (dirty).
        let mut f1 = CanvasScene::new();
        f1.layer_cached(1, true, canvas_core::Transform::IDENTITY, |l| {
            l.fill_path(
                canvas_core::Path::rect(8.0, 8.0, 16.0, 16.0),
                CanvasColor::new(255, 0, 0, 255),
            );
        });
        let _ = h.frame(&f1);

        // Frame 2: NO ops, dirty=false, translate by (24, 24).
        let mut f2 = CanvasScene::new();
        f2.layer_cached(1, false, canvas_core::Transform::translate(24.0, 24.0), |_| {});
        let buf = h.frame(&f2);

        // The square's center was (16,16); after +(24,24) it's at (40,40).
        let moved = px(&buf, S, 40, 40);
        assert!(
            moved[0] > 200 && moved[3] > 200,
            "translated square center should be opaque red, got {moved:?}"
        );
        // Its original location is now empty (the layer moved, wasn't redrawn there).
        let vacated = px(&buf, S, 16, 16);
        assert!(vacated[3] < 8, "vacated location should be transparent, got {vacated:?}");
    }

    /// Regression: two cached layers in ONE frame composite under their OWN
    /// transforms — not all under the last one's. `TransformCompositor` builds a
    /// fresh per-call uniform buffer; the earlier shared-uniform + `write_buffer`
    /// version made every draw in the frame read the LAST transform written
    /// (last-write-wins aliasing). Layer 1 (identity) and layer 2 (translate 40,40)
    /// bake the same local square; with the fix each lands at its own place, so the
    /// identity layer's pixel is occupied. The aliased version would composite BOTH
    /// under translate(40,40), leaving layer 1's spot empty → this fails.
    #[test]
    fn cached_layers_use_independent_transforms() {
        const S: u32 = 64;
        let Some(mut h) = CachedHarness::new(S) else {
            eprintln!("skip: no GPU");
            return;
        };
        let mut sc = CanvasScene::new();
        // Both bake a 12×12 square at local (8,8); different composite transforms.
        sc.layer_cached(1, true, canvas_core::Transform::IDENTITY, |l| {
            l.fill_path(canvas_core::Path::rect(8.0, 8.0, 12.0, 12.0), CanvasColor::new(255, 0, 0, 255));
        });
        sc.layer_cached(2, true, canvas_core::Transform::translate(40.0, 40.0), |l| {
            l.fill_path(canvas_core::Path::rect(8.0, 8.0, 12.0, 12.0), CanvasColor::new(0, 200, 80, 255));
        });
        let buf = h.frame(&sc);

        // Layer 1 (identity) square center (14,14) must be opaque RED — proving it
        // composited under ITS transform, not layer 2's.
        let l1 = px(&buf, S, 14, 14);
        assert!(
            l1[0] > 200 && l1[1] < 60 && l1[3] > 200,
            "layer 1 (identity) center should be opaque red, got {l1:?} — uniform aliasing?"
        );
        // Layer 2 (translate 40,40) square center (54,54) must be opaque GREEN.
        let l2 = px(&buf, S, 54, 54);
        assert!(
            l2[1] > 150 && l2[0] < 60 && l2[3] > 200,
            "layer 2 (translate) center should be opaque green, got {l2:?}"
        );
    }

    /// §7 convergence for the cached fast path: a `Cached` scene (a cached layer
    /// under an integer translate + live ink on top) rendered through the GPU
    /// bake+composite path must match the SAME scene rendered all-vello (where
    /// `encode_scene` handles `LayerCached` via its retained-op-log + `append`).
    /// Integer translate keeps texel sampling exact, so deep interiors match
    /// tightly. Modeled on `hybrid_backdrop_plus_ink_matches_all_vello`.
    #[test]
    fn cached_fast_path_matches_all_vello() {
        const S: u32 = 64;
        // Unique id so the all-vello path's thread-local retained log can't
        // collide with the other cached test sharing the render thread.
        const ID: u32 = 0xCAC_1;
        let build = || {
            let mut cs = CanvasScene::new();
            cs.layer_cached(ID, true, canvas_core::Transform::translate(10.0, 6.0), |l| {
                l.fill_path(
                    canvas_core::Path::rect(4.0, 4.0, 20.0, 20.0),
                    CanvasColor::new(0, 120, 255, 255),
                );
            });
            // Live ink over the cached backdrop (the `rest`).
            cs.path().add_path(canvas_core::Path::rect(36.0, 36.0, 14.0, 14.0));
            cs.fill(Paint::solid(CanvasColor::new(255, 0, 0, 255)));
            cs
        };
        let scene = build();
        assert!(matches!(plan_scene(scene.ops()), ScenePlan::Cached { .. }));

        let Some(mut h) = CachedHarness::new(S) else {
            eprintln!("skip: no GPU");
            return;
        };
        let fast = h.frame(&scene);
        // All-vello reference (encode_scene's retained-op-log fallback path).
        let reference = render_to_rgba(&build(), S).expect("reference render");

        // Cached blue shifted to (10+8 .. 10+24, 6+4 .. 6+24) ⇒ interior (20,16);
        // live red ink interior (42,42); a transparent corner (2,2).
        for &(x, y) in &[(20u32, 16u32), (42, 42), (2, 2)] {
            let a = px(&fast, S, x, y);
            let b = px(&reference, S, x, y);
            for c in 0..4 {
                assert!(
                    (a[c] as i32 - b[c] as i32).abs() <= 8,
                    "pixel ({x},{y}) chan {c}: cached {a:?} vs all-vello {b:?}"
                );
            }
        }
    }

    /// Load a real system font's bytes + face index, or `None` to skip.
    #[cfg(not(target_arch = "wasm32"))]
    fn load_test_font() -> Option<(Vec<u8>, u32)> {
        for path in [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Geneva.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ] {
            if let Ok(bytes) = std::fs::read(path) {
                return Some((bytes, 0));
            }
        }
        None
    }

    /// A skrifa outline pen recording into a canvas `Path` at the requested em
    /// (y-up font units — exactly the space `DrawOp::Glyphs` affines target).
    #[cfg(not(target_arch = "wasm32"))]
    #[derive(Default)]
    struct CanvasPen(canvas_core::Path);
    #[cfg(not(target_arch = "wasm32"))]
    impl skrifa::outline::OutlinePen for CanvasPen {
        fn move_to(&mut self, x: f32, y: f32) {
            self.0.segs.push(canvas_core::PathSeg::MoveTo { x, y });
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.0.segs.push(canvas_core::PathSeg::LineTo { x, y });
        }
        fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
            self.0.segs.push(canvas_core::PathSeg::QuadTo { cx, cy, x, y });
        }
        fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
            self.0.segs.push(canvas_core::PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y });
        }
        fn close(&mut self) {
            self.0.segs.push(canvas_core::PathSeg::Close);
        }
    }

    /// End-to-end GPU proof of the glyph pipeline AND its orientation: render a
    /// glyph via a `DrawOp::Glyphs` run and, independently, via its skrifa outline
    /// as an ordinary `Fill` under the *same* transform. Both go through vello on
    /// a real GPU and must produce near-identical pixels (CLAUDE.md §7).
    ///
    /// This is the regression guard for the **mirrored-text bug**: vello's glyph
    /// pipeline applies an internal y-flip (`-font_size/upem`), so a `Glyphs` arm
    /// that doesn't cancel it draws the glyph flipped relative to the outline
    /// Fill. With an asymmetric glyph ('F', top-heavy) the flipped run diverges
    /// from the outline on most of its ink and this fails; the no-op (pre-feature
    /// `_ => {}`) arm leaves the run blank and also fails.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn glyph_run_matches_outline_on_the_gpu() {
        use canvas_core::{FontResource, Paint as CPaint, Path as CPath, PositionedGlyph, Transform};
        use skrifa::instance::{LocationRef, Size};
        use skrifa::outline::DrawSettings;
        use skrifa::{FontRef, GlyphId, MetadataProvider};

        const S: u32 = 64;
        const UPEM: f32 = 1000.0; // matches the Glyphs arm's GLYPH_UPEM

        let Some((bytes, index)) = load_test_font() else {
            eprintln!("skip: no system font");
            return;
        };
        let font_ref = FontRef::from_index(&bytes, index).expect("parse font");
        // 'F' — vertically asymmetric (bars at the top), so a flip is unmissable.
        let Some(gid) = font_ref.charmap().map('F') else {
            eprintln!("skip: font lacks 'F'");
            return;
        };

        // upem-1000, y-up outline → y-down ~48px glyph: scale by 48/1000, flip y
        // (d < 0, the page-flip a PDF carries), baseline near y≈52.
        let em = 48.0 / UPEM;
        let t = Transform { a: em, b: 0.0, c: 0.0, d: -em, e: 12.0, f: 52.0 };
        let black = CPaint::solid(CanvasColor::new(0, 0, 0, 255));

        // (1) The glyph run.
        let mut cs_run = CanvasScene::new();
        cs_run.glyphs(
            FontResource::new(0xF0, index, bytes.clone()),
            [PositionedGlyph::new(gid.to_u32(), t)],
            black.clone(),
        );

        // (2) The same glyph as an outline Fill (skrifa at upem 1000, same `t`).
        let mut pen = CanvasPen::default();
        font_ref
            .outline_glyphs()
            .get(GlyphId::new(gid.to_u32()))
            .expect("outline")
            .draw(DrawSettings::unhinted(Size::new(UPEM), LocationRef::default()), &mut pen)
            .expect("draw outline");
        let outline: CPath = pen.0;
        let mut cs_outline = CanvasScene::new();
        cs_outline.save();
        cs_outline.transform(t);
        cs_outline.add_path(outline);
        cs_outline.fill(black);
        cs_outline.restore();

        let (Some(run), Some(reference)) =
            (render_to_rgba(&cs_run, S), render_to_rgba(&cs_outline, S))
        else {
            eprintln!("skip: no GPU");
            return;
        };

        let inked = |p: [u8; 4]| p[3] > 128 && p[0] < 80 && p[1] < 80 && p[2] < 80;
        let (mut ink_run, mut ink_ref, mut mismatch) = (0u32, 0u32, 0u32);
        for y in 0..S {
            for x in 0..S {
                let (a, b) = (inked(px(&run, S, x, y)), inked(px(&reference, S, x, y)));
                ink_run += a as u32;
                ink_ref += b as u32;
                mismatch += (a != b) as u32;
            }
        }
        assert!(ink_run > 40, "glyph run drew no ink ({ink_run}) — Glyphs arm not rendering");
        assert!(ink_ref > 40, "outline reference drew no ink ({ink_ref})");
        // A correctly-oriented run overlaps the outline almost perfectly (only
        // antialiased edges differ). A flipped run would mismatch on most ink.
        assert!(
            mismatch < ink_ref / 3,
            "glyph run diverges from its outline (mismatch {mismatch} vs ink {ink_ref}) — \
             likely mirrored (vello's internal y-flip not cancelled)"
        );
    }

    /// GPU repro: a straight-RGBA image whose opaque pixels are light gray must
    /// render light gray (not black). Guards the alpha-type / format handoff to
    /// vello for partially-transparent images (a PDF's alpha-channel watermark).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn light_gray_alpha_image_renders_gray_on_gpu() {
        use canvas_core::{ImageSource, Rect as CRect};
        const S: u32 = 64;
        // 32x32: top half opaque light gray (245), bottom half transparent white.
        let (iw, ih) = (32u32, 32u32);
        let mut rgba = Vec::with_capacity((iw * ih * 4) as usize);
        for y in 0..ih {
            for _ in 0..iw {
                if y < ih / 2 { rgba.extend_from_slice(&[245, 246, 247, 255]); }
                else { rgba.extend_from_slice(&[255, 255, 255, 0]); }
            }
        }
        let img = ImageSource::from_rgba8(0xA1, iw, ih, rgba);
        let mut cs = CanvasScene::new();
        cs.draw_image(img, CRect::new(0.0, 0.0, S as f32, S as f32));
        let Some(buf) = render_to_rgba(&cs, S) else { eprintln!("skip: no GPU"); return; };
        // Sample the opaque (top) region.
        let top = px(&buf, S, 32, 16);
        eprintln!("TOP pixel (should be ~245 gray): {top:?}");
        assert!(top[0] > 200 && top[1] > 200 && top[2] > 200 && top[3] > 200,
            "opaque light-gray image rendered wrong: {top:?}");
    }

    /// GPU repro at the REAL size: a 2550×3300 light-gray image (a PDF's
    /// full-page alpha watermark) downscaled into the frame. vello packs images
    /// into a size-limited atlas; an oversized source can fail to black. If the
    /// opaque region comes out dark here, that's the watermark-renders-black bug.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn large_light_gray_image_renders_gray_on_gpu() {
        use canvas_core::{ImageSource, Rect as CRect};
        const S: u32 = 64;
        let (iw, ih) = (2550u32, 3300u32);
        // Fully opaque light gray (the swoosh interior) — simplest worst case.
        let rgba = vec![245u8; (iw * ih * 4) as usize];
        let mut rgba = rgba;
        for px in rgba.chunks_exact_mut(4) { px[3] = 255; }
        let img = ImageSource::from_rgba8(0xB2, iw, ih, rgba);
        let mut cs = CanvasScene::new();
        cs.draw_image(img, CRect::new(0.0, 0.0, S as f32, S as f32));
        let Some(buf) = render_to_rgba(&cs, S) else { eprintln!("skip: no GPU"); return; };
        let mid = px(&buf, S, 32, 32);
        eprintln!("LARGE image mid pixel (should be ~245 gray): {mid:?}");
        assert!(mid[0] > 200 && mid[1] > 200 && mid[2] > 200,
            "large light-gray image rendered dark: {mid:?}");
    }

    /// GPU repro of the REAL op structure: an image drawn INSIDE a clip layer
    /// (Save · Clip · Save · Transform · Image · Restore · Restore) — exactly how
    /// a PDF's crop-box-clipped page background is recorded. If the image goes
    /// black here but not unclipped, the bug is image-inside-clip compositing.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn image_inside_clip_renders_gray_on_gpu() {
        use canvas_core::{ImageSource, Path as CPath, Rect as CRect};
        const S: u32 = 64;
        let (iw, ih) = (256u32, 256u32);
        let mut rgba = vec![245u8; (iw * ih * 4) as usize];
        for px in rgba.chunks_exact_mut(4) { px[3] = 255; }
        let img = ImageSource::from_rgba8(0xC3, iw, ih, rgba);

        let mut cs = CanvasScene::new();
        cs.save();
        cs.add_path(CPath::rect(0.0, 0.0, S as f32, S as f32));
        cs.clip();
        cs.draw_image(img, CRect::new(0.0, 0.0, S as f32, S as f32));
        cs.restore();

        let Some(buf) = render_to_rgba(&cs, S) else { eprintln!("skip: no GPU"); return; };
        let mid = px(&buf, S, 32, 32);
        eprintln!("CLIPPED image mid pixel (should be ~245 gray): {mid:?}");
        assert!(mid[0] > 200 && mid[1] > 200 && mid[2] > 200,
            "image inside a clip rendered dark: {mid:?}");
    }

    /// A dense, representative one-page PDF: a grid of colored rects + ~60 lines
    /// of Helvetica text (→ thousands of outline-fill path ops). 816×1056 pt.
    #[cfg(not(target_arch = "wasm32"))]
    fn bench_pdf() -> Vec<u8> {
        let mut content = String::new();
        for i in 0..15 {
            let (x, y) = (40 + (i % 5) * 150, 80 + (i / 5) * 300);
            content += &format!(
                "{:.2} {:.2} {:.2} rg {x} {y} 120 90 re f\n",
                (i * 17 % 100) as f32 / 100.0,
                (i * 31 % 100) as f32 / 100.0,
                (i * 53 % 100) as f32 / 100.0,
            );
        }
        for i in 0..60 {
            let y = 1000 - i * 16;
            content += &format!(
                "BT /F1 11 Tf 0 0 0 rg 40 {y} Td \
                 (The quick brown fox jumps over the lazy dog 0123456789 \\(line {i}\\)) Tj ET\n"
            );
        }
        let objects: [String; 5] = [
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 816 1056] \
             /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
                .into(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut pdf = String::from("%PDF-1.7\n");
        let mut offs = Vec::new();
        for (i, b) in objects.iter().enumerate() {
            offs.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{b}\nendobj\n", i + 1));
        }
        let x = pdf.len();
        pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
        for o in &offs {
            pdf.push_str(&format!("{o:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF",
            objects.len() + 1
        ));
        pdf.into_bytes()
    }

    /// min / median / mean of a sample (ms).
    #[cfg(not(target_arch = "wasm32"))]
    fn stats(mut v: Vec<f64>) -> (f64, f64, f64) {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = v[0];
        let median = v[v.len() / 2];
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        (min, median, mean)
    }

    /// CPU-vs-GPU rasterization benchmark: the SAME PDF-derived `Scene`, encoded
    /// once, rasterized by vello on the GPU vs vello's CPU pipeline (`use_cpu`).
    /// Identical encode + scene; only the compute backend differs.
    ///
    /// Run: `cargo test -p canvas-vello bench_cpu_vs_gpu -- --ignored --nocapture --release`
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "benchmark; run with --ignored --nocapture --release"]
    fn bench_cpu_vs_gpu_rasterization() {
        use std::time::Instant;

        // 1. Interpret the PDF page → canvas Scene (CPU, shared cost).
        let t = Instant::now();
        let doc = pdf::Document::load(bench_pdf()).expect("load pdf");
        let page = doc.render_page(0).expect("render page");
        let t_interpret = t.elapsed().as_secs_f64() * 1000.0;
        let scene = page.scene;
        let n_ops = scene.ops().len();

        // 2. Device.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        })) else {
            eprintln!("skip: no GPU adapter");
            return;
        };
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor { label: Some("bench"), ..Default::default() }),
        )
        .expect("device");

        // 3. Two renderers — GPU (use_cpu:false) and CPU (use_cpu:true).
        let opts = |use_cpu| RendererOptions {
            use_cpu,
            antialiasing_support: AaSupport::area_only(),
            num_init_threads: None,
            pipeline_cache: None,
        };
        let mut gpu = Renderer::new(&device, opts(false)).expect("gpu renderer");
        let mut cpu = Renderer::new(&device, opts(true)).expect("cpu renderer");

        // 4. Encode ONCE (the CPU scene→vello step, shared by both renderers).
        let dpr = 2.0_f64;
        let (pw, ph) = (816u32, 1056u32);
        let (w, h) = ((pw as f64 * dpr) as u32, (ph as f64 * dpr) as u32);
        let t = Instant::now();
        let mut vs = VelloScene::new();
        encode_scene(scene.ops(), &mut vs, Affine::scale(dpr));
        let t_encode = t.elapsed().as_secs_f64() * 1000.0;
        let (_target, view) = make_target(&device, w, h);
        let params = RenderParams {
            base_color: Color::from_rgba8(255, 255, 255, 255),
            width: w,
            height: h,
            antialiasing_method: AaConfig::Area,
        };

        // Render + block until complete (so the timing captures actual raster work).
        let mut once = |r: &mut Renderer| {
            r.render_to_texture(&device, &queue, &vs, &view, &params).expect("render");
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        };

        // 5. Warmup, then time.
        for _ in 0..3 {
            once(&mut gpu);
        }
        for _ in 0..3 {
            once(&mut cpu);
        }
        let timed = |r: &mut Renderer, n: u32, once: &mut dyn FnMut(&mut Renderer)| -> Vec<f64> {
            (0..n)
                .map(|_| {
                    let t = Instant::now();
                    once(r);
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .collect()
        };
        let gpu_ms = timed(&mut gpu, 40, &mut once);
        let cpu_ms = timed(&mut cpu, 10, &mut once);

        let (g_min, g_med, g_mean) = stats(gpu_ms);
        let (c_min, c_med, c_mean) = stats(cpu_ms);

        eprintln!("\n=== PDF rasterization: CPU vs GPU (vello) ===");
        eprintln!("adapter      : {:?} / {}", info.backend, info.name);
        eprintln!("page         : {pw}x{ph} pt @ {dpr}x = {w}x{h} px");
        eprintln!("scene        : {n_ops} ops");
        eprintln!("interpret    : {t_interpret:7.2} ms  (PDF → Scene, one-time, CPU)");
        eprintln!("encode       : {t_encode:7.2} ms  (Scene → vello, one-time, CPU)");
        eprintln!("GPU raster   : min {g_min:6.2}  median {g_med:6.2}  mean {g_mean:6.2} ms/frame");
        eprintln!("CPU raster   : min {c_min:6.2}  median {c_med:6.2}  mean {c_mean:6.2} ms/frame");
        eprintln!("GPU speedup  : {:.1}x (median)\n", c_med / g_med);
    }

    /// The PUBLIC headless export (`super::render_to_rgba`). Uses a non-square
    /// size whose row stride (100*4 = 400) is NOT 256-aligned, so it exercises
    /// the read-back padding strip. Fills the whole frame and checks a center
    /// pixel + the tight buffer length.
    #[test]
    fn public_render_to_rgba_exports_unaligned_size() {
        let mut cs = CanvasScene::new();
        cs.path().add_path(Path::rect(0.0, 0.0, 100.0, 60.0));
        cs.fill(Paint::solid(CanvasColor::new(20, 130, 240, 255)));

        let Some(img) = super::render_to_rgba(&cs, 100, 60) else {
            eprintln!("skip: no GPU/software adapter");
            return;
        };
        assert_eq!((img.width, img.height), (100, 60));
        assert_eq!(img.data.len(), 100 * 60 * 4);
        let i = ((30 * 100 + 50) * 4) as usize; // center-ish, RGBA8
        let c = [img.data[i], img.data[i + 1], img.data[i + 2], img.data[i + 3]];
        let near = |v: u8, t: i32| (v as i32 - t).abs() <= 8;
        assert!(
            near(c[0], 20) && near(c[1], 130) && near(c[2], 240) && c[3] > 200,
            "center px should be ~(20,130,240,255), got {c:?}"
        );
    }

    /// Headless proof of the Linux/GTK4 GL present blit WITHOUT a live GtkGLArea:
    /// a top-half-red source flip-blitted through [`FlipBlit`] into an
    /// `Rgba8Unorm` target must land the red in the target's BOTTOM half (V
    /// inverted, matching GTK's bottom-up GLArea compositing) and pass the RGB
    /// through UNCHANGED (straight copy — no sRGB re-encode). Guards two of the
    /// three GL-path invariants documented on `FlipBlit`:
    ///
    /// - **Vertical flip** (`GlTarget::origin() == BottomLeft`): if the vertex
    ///   shader ever stopped inverting V, the red would stay in the top half.
    /// - **Straight colour copy**: byte 200 sampled from an `Rgba8Unorm` source
    ///   and written to an `Rgba8Unorm` target must come back ~200. If someone
    ///   pasted host-linux-desktop's `linear_to_srgb` fragment shader, 200 would
    ///   re-encode to ~229 and the `±6` check below would fail — the exact
    ///   double-gamma wash the invariant forbids.
    ///
    /// The live-context half (adopting a real `GtkGLArea`, the framebuffer wrap,
    /// on-screen orientation) is proven end-to-end in
    /// `backend-linux/tests/gl_target_adoption.rs`; that needs a GTK main thread,
    /// so it can't live here. Skips when there's no GPU/software adapter.
    #[cfg(target_os = "linux")]
    #[test]
    fn gl_flip_blit_inverts_v_and_copies_colour_straight() {
        const S: u32 = 64;
        let Some((device, queue)) = super::headless_device() else {
            eprintln!("skip: no GPU/software adapter");
            return;
        };
        let blit = super::FlipBlit::new(&device);

        // Source: top half (rows 0..S/2) opaque red 200, bottom half transparent.
        // Rows are top-down (wgpu's texture origin is top-left).
        let src = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("flip-blit-src"),
            size: wgpu::Extent3d { width: S, height: S, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut pixels = vec![0u8; (S * S * 4) as usize];
        for y in 0..S {
            for x in 0..S {
                let i = ((y * S + x) * 4) as usize;
                if y < S / 2 {
                    pixels[i] = 200; // R
                    pixels[i + 3] = 255; // A
                }
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &src,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(S * 4),
                rows_per_image: Some(S),
            },
            wgpu::Extent3d { width: S, height: S, depth_or_array_layers: 1 },
        );
        let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());

        // Destination: same Rgba8Unorm format + size as the GLArea framebuffer.
        let (dst, dst_view) = make_target(&device, S, S);
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        blit.draw(&device, &mut enc, &src_view, &dst_view);
        queue.submit([enc.finish()]);

        let img = read_target_rgba(&device, &queue, &dst, S, S);
        let at = |x: u32, y: u32| {
            let i = ((y * S + x) * 4) as usize;
            [img.data[i], img.data[i + 1], img.data[i + 2], img.data[i + 3]]
        };
        // Source's top-red must have flipped to the target's BOTTOM.
        let bottom = at(S / 2, S - 5);
        assert!(
            bottom[0] > 180 && bottom[3] > 180,
            "V-flip: source top-red must land in the target BOTTOM, got {bottom:?}"
        );
        // Target's top is the source's (transparent) bottom half.
        let top = at(S / 2, 4);
        assert!(top[3] < 40, "target top should be the transparent source bottom, got {top:?}");
        // Straight copy: 200 in → ~200 out. A linear→sRGB re-encode would give ~229.
        assert!(
            (bottom[0] as i32 - 200).abs() <= 6,
            "straight copy must preserve RGB (no sRGB re-encode), got {bottom:?}"
        );
    }
}




