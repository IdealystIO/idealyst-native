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

use crate::encode::encode_scene;
use crate::native_capture::{LayerCompositor, NativeCapture};
use crate::shape_pass::ShapePass;
use canvas_core::{
    paint_scene, BlendMode, CanvasProps, DrawOp, Scene as CanvasScene, ShapeInstance, TextureLayer,
};
use media_stream::FrameWriter;
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::graphics::{OnReadyEvent, OnResizeEvent};
use runtime_core::{Backend, Effect, RegisterExternal};

use std::cell::RefCell;
use std::rc::Rc;

use vello::kurbo::Affine;
use vello::peniko::Color;
use vello::{AaConfig, AaSupport, Renderer, RendererOptions, RenderParams, Scene as VelloScene};

/// vello renders into a storage texture of this format; the blitter copies
/// it to the surface (whatever the surface's own format is).
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

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
    let _effect = Effect::new({
        let props = props.clone();
        let scene_cell = scene_cell.clone();
        let state_cell = state_cell.clone();
        move || {
            *scene_cell.borrow_mut() = paint_scene(&props);
            if let Some(state) = state_cell.borrow_mut().as_mut() {
                state.render(&scene_cell.borrow());
            }
        }
    });

    let on_ready = {
        let scene_cell = scene_cell.clone();
        let state_cell = state_cell.clone();
        // Self-capture sink (app's MediaStream producer half), if the canvas
        // was built with `capture: Some(writer)`. `FrameWriter` is Clone.
        let capture = props.capture.clone();
        // Texture layers composited into the canvas (Clone: MediaStream + Rc).
        let layers = props.layers.clone();
        move |ev: OnReadyEvent| {
            if let Some(mut state) =
                RenderState::new(ev.surface, ev.size, ev.scale, capture.clone(), layers.clone())
            {
                let presented = state.render(&scene_cell.borrow());
                *state_cell.borrow_mut() = Some(state);
                // First drawable often isn't acquirable on the deferred on_ready
                // tick (macOS CAMetalLayer) — retry until the initial scene lands
                // so the canvas doesn't show dark until the first repaint.
                if !presented {
                    retry_first_frame(scene_cell.clone(), state_cell.clone(), 120);
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
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
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
    /// Texture layers (camera, screen share, …) composited over the scene each
    /// frame (macOS), so the strokes + layers are one image — on-screen and in
    /// the recording. Empty when the canvas has no `layers`.
    layers: Vec<TextureLayer>,
    layer_compositor: Option<LayerCompositor>,
    /// Instanced analytic-shape pass, built lazily the first time a PURE-shape
    /// scene is rendered (a canvas that never uses `DrawOp::Shapes` pays
    /// nothing). Draws a rounded-box-SDF grid in one instanced draw instead of
    /// one tessellated fill per shape. See [`ShapePass`].
    shape_pass: Option<ShapePass>,
}

fn make_target(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
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

/// If every op in `ops` is a [`DrawOp::Shapes`] batch with [`BlendMode::Normal`],
/// return the per-batch shape slices (in op order) for the instanced fast path;
/// `None` if the scene contains any other op (vector fill/stroke/image/layer/
/// transform) or a non-Normal batch blend — those need vello's compositor, so
/// they go through `encode_scene` (which expands shapes to ordered fills,
/// matching this pass's output, CLAUDE.md §7). An empty op list returns
/// `Some(vec![])`, which the pass renders as a transparent clear.
///
/// The predicate is deliberately strict: a single non-shape op disables the fast
/// path for the whole frame. That keeps z-order and blending exact (vello can't
/// cheaply layer a custom pass between its own draws) at the cost of not
/// accelerating mixed scenes — the common bulk case (a grid/scatter of shapes,
/// nothing else) is the one that wins, and it wins completely.
fn pure_shape_batches(ops: &[DrawOp]) -> Option<Vec<&[ShapeInstance]>> {
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        match op {
            DrawOp::Shapes { shapes, blend } if *blend == BlendMode::Normal => {
                out.push(shapes.as_slice())
            }
            _ => return None,
        }
    }
    Some(out)
}

impl RenderState {
    fn new(
        surface_target: runtime_core::primitives::graphics::GraphicsSurface,
        size: (u32, u32),
        scale: f32,
        capture: Option<FrameWriter>,
        layers: Vec<TextureLayer>,
    ) -> Option<Self> {
        let (w, h) = (size.0.max(1), size.1.max(1));

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
        let surface = instance.create_surface(surface_target).ok()?;

        // Deferred to a runloop turn by the backend, so block_on is safe.
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
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
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("canvas-vello-device"),
            required_features: f16,
            required_limits: adapter.limits(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        }))
        .ok()?;

        let caps = surface.get_capabilities(&adapter);
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
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w,
            height: h,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

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
            config,
            renderer,
            scene: VelloScene::new(),
            target,
            target_view,
            blitter,
            // `1.0` is the historical "no dpr reported" behavior (render at
            // physical scale); a backend reporting the real factor (macOS
            // backingScaleFactor) makes the logical scene fill the surface.
            scale: if scale > 0.0 { scale as f64 } else { 1.0 },
            native_capture: capture.clone().map(NativeCapture::new),
            layer_compositor,
            layers,
            capture,
            readback: None,
            shape_pass: None,
        })
    }

    fn resize(&mut self, size: (u32, u32)) {
        self.config.width = size.0.max(1);
        self.config.height = size.1.max(1);
        self.surface.configure(&self.device, &self.config);
        // Read-back buffer is sized to the target; invalidate so render()
        // recreates it for the new dimensions.
        self.readback = None;
        let (target, target_view) =
            make_target(&self.device, self.config.width, self.config.height);
        self.target = target;
        self.target_view = target_view;
    }

    /// Render the scene and present a frame. Returns `true` iff a frame was
    /// actually presented; `false` when the swapchain texture couldn't be
    /// acquired (drawable not ready yet — common for the very first frame on a
    /// freshly-created `CAMetalLayer`) or vello's encode failed. `on_ready`
    /// uses the return to retry the FIRST frame until it lands, so the initial
    /// scene (e.g. the canvas's white background) isn't lost to a dark surface.
    fn render(&mut self, canvas_scene: &CanvasScene) -> bool {
        // Fast path: a PURE-shape scene (every op a Normal-blend `Shapes` batch)
        // is drawn by the instanced SDF pass below instead of vello — one draw
        // for the whole grid. Any other op (vector fill/stroke/image/layer/
        // transform, or a non-Normal batch blend) falls through to vello, which
        // expands shapes to ordered per-shape fills (encode.rs) so the pixels are
        // identical (CLAUDE.md §7) — the fast path is an optimization, not a fork.
        let fast = pure_shape_batches(canvas_scene.ops());

        if fast.is_none() {
            self.scene.reset();
            // Base transform = device scale: the author's Scene is in LOGICAL
            // coordinates; scaling by the dpr makes it fill the physical-pixel
            // surface (no retina under-fill). `1.0` → identity (physical scale).
            encode_scene(canvas_scene.ops(), &mut self.scene, Affine::scale(self.scale));

            let params = RenderParams {
                base_color: Color::from_rgba8(0, 0, 0, 0),
                width: self.config.width,
                height: self.config.height,
                antialiasing_method: AaConfig::Area,
            };
            if self
                .renderer
                .render_to_texture(
                    &self.device,
                    &self.queue,
                    &self.scene,
                    &self.target_view,
                    &params,
                )
                .is_err()
            {
                return false;
            }
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            // Skip the frame on timeout/occluded/outdated/lost/validation.
            _ => return false,
        };
        let surface_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("canvas-vello-blit"),
            });
        // Instanced shape fast path: clear the target and draw the whole shape
        // grid in one instanced pass, BEFORE the layer composite and blit (it
        // owns the target clear that vello would otherwise have done). Disjoint
        // field borrows — bind the shared refs to locals first.
        if let Some(batches) = fast.as_ref() {
            if self.shape_pass.is_none() {
                self.shape_pass = Some(ShapePass::new(&self.device));
            }
            let device = &self.device;
            let queue = &self.queue;
            let target_view = &self.target_view;
            let (cw, ch) = (self.config.width, self.config.height);
            let s = self.scale as f32;
            self.shape_pass.as_mut().unwrap().render(
                device,
                queue,
                &mut encoder,
                target_view,
                batches,
                s,
                cw,
                ch,
            );
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

        self.blitter.copy(&self.device, &mut encoder, &self.target_view, &surface_view);

        // Zero-copy capture (macOS): blit the same target into the next ring
        // IOSurface in THIS encoder, so it's submitted with the frame. Disjoint
        // field borrows (device/target_view vs. native_capture) — bind the
        // shared ones first.
        let native_publish = {
            let device = &self.device;
            let target_view = &self.target_view;
            let (cw, ch) = (self.config.width, self.config.height);
            match self.native_capture.as_mut() {
                Some(nc) if nc.wants() => {
                    nc.blit_into(device, &mut encoder, target_view, cw, ch)
                }
                _ => None,
            }
        };

        self.queue.submit([encoder.finish()]);
        frame.present();

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
        if !native_active {
            self.capture_frame();
        }
        true
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
        Color as CanvasColor, ImageSource, Paint, Path, Rect as CanvasRect, Scene as CanvasScene,
        ShapeInstance,
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
}
