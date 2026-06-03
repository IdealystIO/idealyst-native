//! Native vello renderer: translates a `canvas_core::Scene` into a
//! `vello::Scene` and paints it onto the framework's `graphics` surface
//! via `wgpu`.
//!
//! # Coordinate space (known limitation)
//!
//! The `graphics` primitive's `on_ready`/`on_resize` report the drawable
//! size in **physical pixels** only — there is no device-scale (dpr) on the
//! event. This renderer therefore paints the author's `Scene` in *surface
//! pixel* coordinates (base transform = identity). On a retina surface that
//! makes a logical-pixel scene under-fill; matching the native renderers'
//! logical-coordinate behavior needs `OnReadyEvent` to carry the device
//! scale — a small follow-up to the graphics primitive + backends. Tracked
//! here, not silently divergent (CLAUDE.md §7).

use canvas_core::{
    paint_scene, CanvasProps, Color as CanvasColor, DrawOp, FillRule, GradientStop, LineCap,
    LineJoin, Paint, PaintKind, Path, PathSeg, Scene as CanvasScene, Stroke as CanvasStroke,
};
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
        move |ev: OnReadyEvent| {
            if let Some(mut state) = RenderState::new(ev.surface, ev.size) {
                state.render(&scene_cell.borrow());
                *state_cell.borrow_mut() = Some(state);
            }
        }
    };

    let on_resize = {
        let scene_cell = scene_cell.clone();
        let state_cell = state_cell.clone();
        move |ev: OnResizeEvent| {
            if let Some(state) = state_cell.borrow_mut().as_mut() {
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
    /// surface each frame.
    target_view: wgpu::TextureView,
    blitter: wgpu::util::TextureBlitter,
}

fn make_target(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("canvas-vello-target"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

impl RenderState {
    fn new(
        surface_target: runtime_core::primitives::graphics::GraphicsSurface,
        size: (u32, u32),
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
        let format = caps.formats[0];
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

        let target_view = make_target(&device, w, h);
        let blitter = wgpu::util::TextureBlitter::new(&device, format);

        Some(Self {
            device,
            queue,
            surface,
            config,
            renderer,
            scene: VelloScene::new(),
            target_view,
            blitter,
        })
    }

    fn resize(&mut self, size: (u32, u32)) {
        self.config.width = size.0.max(1);
        self.config.height = size.1.max(1);
        self.surface.configure(&self.device, &self.config);
        self.target_view = make_target(&self.device, self.config.width, self.config.height);
    }

    fn render(&mut self, canvas_scene: &CanvasScene) {
        self.scene.reset();
        encode_scene(canvas_scene, &mut self.scene, Affine::IDENTITY);

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
            return;
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            // Skip the frame on timeout/occluded/outdated/lost/validation.
            _ => return,
        };
        let surface_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("canvas-vello-blit"),
            });
        self.blitter.copy(&self.device, &mut encoder, &self.target_view, &surface_view);
        self.queue.submit([encoder.finish()]);
        frame.present();
    }
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
