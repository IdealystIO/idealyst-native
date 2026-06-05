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

use canvas_core::{
    paint_scene, CanvasProps, Color as CanvasColor, DrawOp, FillRule, GradientStop, LineCap,
    LineJoin, Paint, PaintKind, Path, PathSeg, Scene as CanvasScene, Stroke as CanvasStroke,
};
use crate::native_capture::{CameraComposite, NativeCapture};
use canvas_core::CameraLayer;
use media_stream::FrameWriter;
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::graphics::{OnReadyEvent, OnResizeEvent};
use runtime_core::{Backend, Effect, RegisterExternal};

use std::cell::RefCell;
use std::rc::Rc;

use vello::kurbo::{Affine, BezPath, Cap, Join, Point, Stroke as KurboStroke};
use vello::peniko::color::DynamicColor;
use vello::peniko::{Brush, Color, ColorStop, Fill, Gradient, Mix};
use vello::{AaConfig, AaSupport, Renderer, RendererOptions, RenderParams, Scene as VelloScene};

/// vello renders into a storage texture of this format; the blitter copies
/// it to the surface (whatever the surface's own format is).
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Register the vello canvas renderer. Generic over any backend that
/// supports externals + graphics surfaces — the surface is obtained from
/// `Backend::create_graphics`, so there's no per-platform code.
pub fn register<B: RegisterExternal>(backend: &mut B) {
    canvas_core::ensure_wire_serde();
    backend.register_external::<CanvasProps, _>(|props, b| build_canvas(props, b));
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
        // Camera layer composited into the canvas (Clone: MediaStream + Rc).
        let camera = props.camera.clone();
        move |ev: OnReadyEvent| {
            if let Some(mut state) =
                RenderState::new(ev.surface, ev.size, ev.scale, capture.clone(), camera.clone())
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
    /// A live camera (or any `MediaStream`) composited into the target each
    /// frame (macOS), so the strokes + camera are one image — on-screen and in
    /// the recording. `None` when the canvas has no `camera` layer.
    camera_layer: Option<CameraLayer>,
    camera_composite: Option<CameraComposite>,
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

impl RenderState {
    fn new(
        surface_target: runtime_core::primitives::graphics::GraphicsSurface,
        size: (u32, u32),
        scale: f32,
        capture: Option<FrameWriter>,
        camera: Option<CameraLayer>,
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

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("canvas-vello-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
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
        // Built once if the canvas has a camera layer (needs `device` before it's
        // moved into the struct).
        let camera_composite = camera.as_ref().map(|_| CameraComposite::new(&device));

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
            camera_composite,
            camera_layer: camera,
            capture,
            readback: None,
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
        self.scene.reset();
        // Base transform = device scale: the author's Scene is in LOGICAL
        // coordinates; scaling by the dpr makes it fill the physical-pixel
        // surface (no retina under-fill). `1.0` → identity (physical scale).
        encode_scene(canvas_scene, &mut self.scene, Affine::scale(self.scale));

        let params = RenderParams {
            base_color: Color::from_rgba8(0, 0, 0, 0),
            width: self.config.width,
            height: self.config.height,
            antialiasing_method: AaConfig::Area,
        };
        if self
            .renderer
            .render_to_texture(&self.device, &self.queue, &self.scene, &self.target_view, &params)
            .is_err()
        {
            return false;
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
        // Camera-as-texture (macOS): composite the live camera into the target
        // BEFORE the blits, so the strokes + camera are one image that both the
        // on-screen surface AND the recording IOSurface receive. Disjoint field
        // borrows — bind the shared ones first.
        {
            let device = &self.device;
            let queue = &self.queue;
            let target_view = &self.target_view;
            let (cw, ch) = (self.config.width, self.config.height);
            let s = self.scale as f32;
            if let (Some(cam), Some(cc)) = (&self.camera_layer, self.camera_composite.as_mut()) {
                if let Some(stream) = (cam.source)() {
                    let (lx, ly, lw, lh) = (cam.rect)();
                    cc.composite(
                        device,
                        queue,
                        &mut encoder,
                        &stream,
                        target_view,
                        (lx * s, ly * s, lw * s, lh * s),
                        cw,
                        ch,
                    );
                }
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

// ============================================================================
// Scene → vello translation
// ============================================================================

/// Walk the canvas op list into `vs`, maintaining a transform stack
/// (Save/Restore + Transform) and clip layers (Clip → push_layer).
fn encode_scene(canvas: &CanvasScene, vs: &mut VelloScene, base: Affine) {
    let mut cur = base;
    // (saved transform, number of clip layers pushed inside this save scope)
    let mut stack: Vec<(Affine, u32)> = Vec::new();
    // Clips pushed outside any save scope (popped at the end).
    let mut root_clips: u32 = 0;

    for op in canvas.ops() {
        match op {
            DrawOp::Save => stack.push((cur, 0)),
            DrawOp::Restore => {
                if let Some((saved, n_clips)) = stack.pop() {
                    for _ in 0..n_clips {
                        vs.pop_layer();
                    }
                    cur = saved;
                }
            }
            DrawOp::Transform(t) => {
                cur *= affine_of(t);
            }
            DrawOp::Clip { path, fill_rule } => {
                let shape = bez_of(path);
                // A clip layer: clip to the path interior (its fill rule),
                // Normal blend, full alpha. Popped at the matching Restore.
                vs.push_layer(fill_of(*fill_rule), Mix::Normal, 1.0, cur, &shape);
                match stack.last_mut() {
                    Some(top) => top.1 += 1,
                    None => root_clips += 1,
                }
            }
            DrawOp::Fill { path, paint, fill_rule } => {
                let shape = bez_of(path);
                let brush = brush_of(paint);
                vs.fill(fill_of(*fill_rule), cur, &brush, None, &shape);
            }
            DrawOp::Stroke { path, paint, stroke } => {
                let shape = bez_of(path);
                let brush = brush_of(paint);
                vs.stroke(&kurbo_stroke(stroke), cur, &brush, None, &shape);
            }
            _ => {}
        }
    }

    // Pop any still-open clip layers (unbalanced restore, or root clips).
    for (_, n_clips) in stack.drain(..) {
        for _ in 0..n_clips {
            vs.pop_layer();
        }
    }
    for _ in 0..root_clips {
        vs.pop_layer();
    }
}

fn bez_of(path: &Path) -> BezPath {
    let mut bp = BezPath::new();
    for seg in &path.segs {
        match seg {
            PathSeg::MoveTo { x, y } => bp.move_to(pt(*x, *y)),
            PathSeg::LineTo { x, y } => bp.line_to(pt(*x, *y)),
            PathSeg::QuadTo { cx, cy, x, y } => bp.quad_to(pt(*cx, *cy), pt(*x, *y)),
            PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y } => {
                bp.curve_to(pt(*c1x, *c1y), pt(*c2x, *c2y), pt(*x, *y))
            }
            PathSeg::Close => bp.close_path(),
        }
    }
    bp
}

fn pt(x: f32, y: f32) -> Point {
    Point::new(x as f64, y as f64)
}

fn affine_of(t: &canvas_core::Transform) -> Affine {
    // Canvas Transform (a,b,c,d,e,f) maps to kurbo's [a,b,c,d,e,f] coeffs.
    Affine::new([t.a as f64, t.b as f64, t.c as f64, t.d as f64, t.e as f64, t.f as f64])
}

fn fill_of(rule: FillRule) -> Fill {
    match rule {
        FillRule::NonZero => Fill::NonZero,
        FillRule::EvenOdd => Fill::EvenOdd,
    }
}

fn color_of(c: CanvasColor) -> Color {
    Color::from_rgba8(c.r, c.g, c.b, c.a)
}

fn brush_of(paint: &Paint) -> Brush {
    match &paint.kind {
        PaintKind::Solid(c) => Brush::Solid(color_of(*c)),
        PaintKind::Linear(g) => Brush::Gradient(
            Gradient::new_linear(pt(g.x0, g.y0), pt(g.x1, g.y1))
                .with_stops(stops_of(&g.stops).as_slice()),
        ),
        PaintKind::Radial(g) => Brush::Gradient(
            Gradient::new_radial(pt(g.cx, g.cy), g.r).with_stops(stops_of(&g.stops).as_slice()),
        ),
        _ => Brush::Solid(Color::from_rgba8(0, 0, 0, 0)),
    }
}

fn stops_of(stops: &[GradientStop]) -> Vec<ColorStop> {
    stops
        .iter()
        .map(|s| ColorStop {
            offset: s.offset,
            color: DynamicColor::from_alpha_color(color_of(s.color)),
        })
        .collect()
}

fn kurbo_stroke(s: &CanvasStroke) -> KurboStroke {
    KurboStroke::new(s.width as f64)
        .with_caps(match s.cap {
            LineCap::Butt => Cap::Butt,
            LineCap::Round => Cap::Round,
            LineCap::Square => Cap::Square,
        })
        .with_join(match s.join {
            LineJoin::Miter => Join::Miter,
            LineJoin::Round => Join::Round,
            LineJoin::Bevel => Join::Bevel,
        })
        .with_miter_limit(s.miter_limit as f64)
}
