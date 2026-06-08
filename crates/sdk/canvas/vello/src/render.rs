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
    paint_scene, CanvasProps, DrawOp, Scene as CanvasScene, ShapeInstance, TextureLayer,
};
use media_stream::FrameWriter;
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::graphics::{OnReadyEvent, OnResizeEvent};
use runtime_core::{Backend, Effect, RegisterExternal};

use std::cell::RefCell;
use std::collections::HashMap;
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
    /// Transformed-quad compositor for `cached_layers`, built lazily the first
    /// time a cached-layer scene is rendered. See [`TransformCompositor`].
    transform_compositor: Option<TransformCompositor>,
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

// Scene classification (`ScenePlan` / `plan_scene`) lives in `crate::plan`, shared
// with the web renderer.

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
            overlay: None,
            overlay_compositor: None,
            cached_layers: HashMap::new(),
            transform_compositor: None,
        })
    }

    fn resize(&mut self, size: (u32, u32)) {
        self.config.width = size.0.max(1);
        self.config.height = size.1.max(1);
        self.surface.configure(&self.device, &self.config);
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
        for layer in layers {
            let missing = !self.cached_layers.contains_key(&layer.id);
            if !(layer.dirty || missing) {
                continue;
            }
            if missing {
                self.cached_layers.insert(layer.id, make_target(&self.device, w, h));
            }
            // Encode the layer's ops at the dpr base into a fresh vello scene,
            // then compute-render into the layer texture (transparent base).
            self.scene.reset();
            encode_scene(layer.ops, &mut self.scene, Affine::scale(self.scale));
            let params = RenderParams {
                base_color: Color::from_rgba8(0, 0, 0, 0),
                width: w,
                height: h,
                antialiasing_method: AaConfig::Area,
            };
            let view = &self.cached_layers.get(&layer.id).unwrap().1;
            let _ = self
                .renderer
                .render_to_texture(&self.device, &self.queue, &self.scene, view, &params);
        }
    }

    /// Render the scene and present a frame. Returns `true` iff a frame was
    /// actually presented; `false` when the swapchain texture couldn't be
    /// acquired (drawable not ready yet — common for the very first frame on a
    /// freshly-created `CAMetalLayer`) or vello's encode failed. `on_ready`
    /// uses the return to retry the FIRST frame until it lands, so the initial
    /// scene (e.g. the canvas's white background) isn't lost to a dark surface.
    fn render(&mut self, canvas_scene: &CanvasScene) -> bool {
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
            ScenePlan::Cached { rest, .. } => {
                // Cached layers form the backdrop (composited from their textures
                // below); the live ink (`rest`) renders through vello into the
                // overlay so the backdrop survives underneath, exactly like Hybrid.
                if rest.is_empty() {
                    (None, false)
                } else {
                    if self.overlay.is_none() {
                        self.overlay =
                            Some(make_target(&self.device, self.config.width, self.config.height));
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
            if self
                .renderer
                .render_to_texture(&self.device, &self.queue, &self.scene, view, &params)
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
                // Clear, then composite each cached layer (in order) under its
                // camera transform — one transformed quad each, no per-op work.
                crate::compose_transform::clear_to_transparent(&mut encoder, target_view);
                let tc = self.transform_compositor.as_ref().unwrap();
                for layer in layers {
                    if let Some((tex, view)) = self.cached_layers.get(&layer.id) {
                        // Skip a stale-size texture (resize race) — the app re-bakes.
                        if tex.width() == cw && tex.height() == ch {
                            tc.composite(
                                device, &mut encoder, view, target_view,
                                layer.transform, s, layer.alpha, cw, ch,
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
}
