//! Animated Mandelbrot demo for the surface-provider Graphics
//! primitive.
//!
//! The framework hands us a `GraphicsSurface` (a raw-window-handle
//! provider) in `on_ready`. We do everything else ourselves:
//! `wgpu::Instance`, `Surface`, adapter/device, pipeline, and a
//! `requestAnimationFrame` render loop. Same author code would work
//! on Android — wgpu's the abstraction layer; the framework just
//! provides the canvas.
//!
//! # The animation
//!
//! Every 12-second cycle: zoom log-linearly from a wide view of the
//! whole set down to the seahorse-valley target
//! `(-0.743643887037151, 0.131825904205330)` over ~10s, hold for
//! 2s, then snap back to the wide view. `max_iter` scales with
//! zoom so we stay sharp deep into the set without wasting work
//! at the wide view.

use framework_core::primitives::graphics::{
    GraphicsSurface, OnReadyEvent, OnResizeEvent,
};
use framework_core::{ui, Primitive};
use std::cell::RefCell;
use std::rc::Rc;

/// All state that exists *after* the surface is up. Wrapped in
/// `Rc<RefCell<Option<...>>>` so the four closures (on_ready,
/// on_resize, on_lost, rAF) can all reach it. The `Option` flips
/// to `None` on `on_lost` and back to `Some` on the next `on_ready`.
struct RendererState {
    /// Hold the surface alive — `wgpu::Surface<'static>` borrows
    /// from a window handle, and the GraphicsSurface keeps the
    /// underlying canvas alive.
    #[allow(dead_code)]
    framework_surface: GraphicsSurface,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    start_ms: f64,
    /// Per-frame render loop. Holding the handle keeps the loop
    /// alive; dropping it cancels any pending frame *and* stops
    /// the auto-rearm. `None` between construction and the kickoff
    /// inside `on_ready`.
    raf_loop: Option<framework_core::RafLoop>,
}

/// Uniform layout. std140-aligned by hand: `vec2`s are 8-byte
/// aligned, scalars come after, padding to 16-byte stride at the
/// end. `bytemuck` ensures the struct's byte layout matches the
/// shader's expectation.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    /// Mandelbrot center in complex-plane coordinates (real, imag).
    center: [f32; 2],
    /// Drawable size in physical pixels.
    resolution: [f32; 2],
    /// Zoom multiplier. `1.0` shows roughly the whole set;
    /// larger = deeper. We cap at ~1e5 because f32 precision
    /// breaks down beyond that.
    zoom: f32,
    /// Maximum iteration count. Scaled with zoom so we resolve
    /// the boundary cleanly at depth without wasting compute when
    /// zoomed out.
    max_iter: u32,
    /// Width / height of the canvas. The shader uses this to
    /// avoid stretching the set vertically when the canvas is
    /// wider than tall.
    aspect: f32,
    _pad: f32,
}

const SHADER_SRC: &str = r#"
struct Uniforms {
    center: vec2<f32>,
    resolution: vec2<f32>,
    zoom: f32,
    max_iter: u32,
    aspect: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Full-screen triangle pair.
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
    );
    let p = positions[vid];
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    // UV in [-1, 1], which we'll then scale into complex coords.
    out.uv = p;
    return out;
}

/// Smooth multi-stop palette for Mandelbrot escape times. Built
/// from a handful of brand-friendly stops; `t` should be in [0, 1].
fn palette(t: f32) -> vec3<f32> {
    // Stops: deep navy → magenta → orange → cream → black (in-set).
    // Black-on-set is handled by the caller; this function maps
    // [0, 1) for escaping pixels only.
    let t2 = clamp(t, 0.0, 0.999);
    // Build a simple cosine palette in HSV-ish space. The
    // coefficients below are tuned so the result reads as
    // navy→pink→orange→pale yellow as t goes 0→1.
    let a = vec3<f32>(0.50, 0.50, 0.50);
    let b = vec3<f32>(0.50, 0.50, 0.50);
    let c = vec3<f32>(1.00, 1.00, 1.00);
    let d = vec3<f32>(0.00, 0.10, 0.20);
    return a + b * cos(6.28318 * (c * t2 + d));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Convert UV in [-1, 1] to a point in the complex plane,
    // centered on `u.center` and scaled by `u.zoom`. Aspect
    // correction keeps the set from squashing when the canvas
    // isn't square.
    //
    // The "natural" Mandelbrot view is roughly real ∈ [-2.5, 1],
    // imag ∈ [-1, 1]. We normalize that to a 3.0-wide window at
    // zoom = 1, so the in-set region fits with margin.
    let scale = 1.5 / u.zoom;
    let c = u.center + vec2<f32>(in.uv.x * scale * u.aspect, in.uv.y * scale);

    // Mandelbrot iteration: z_{n+1} = z_n^2 + c, starting at z_0 = 0.
    // Loop until |z| > 2 (escape) or hit max_iter (assumed in set).
    var z = vec2<f32>(0.0, 0.0);
    var i: u32 = 0u;
    let bail = 256.0;  // |z|^2 threshold; larger gives smoother coloring
    var z2 = 0.0;
    loop {
        if (i >= u.max_iter) { break; }
        // Complex square + add: (a+bi)^2 = (a^2 - b^2) + (2ab)i
        let new_z = vec2<f32>(z.x * z.x - z.y * z.y + c.x, 2.0 * z.x * z.y + c.y);
        z = new_z;
        z2 = z.x * z.x + z.y * z.y;
        if (z2 > bail) { break; }
        i = i + 1u;
    }

    // In-set pixel: render black.
    if (i >= u.max_iter) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    // Smooth (continuous) escape time:
    //   nu = i + 1 - log2(log2(|z|))
    // Standard trick for getting band-free coloring.
    let log_zn = log(z2) * 0.5;            // log(|z|) = 0.5 * log(|z|^2)
    let nu = log(log_zn / log(2.0)) / log(2.0);
    let smooth_i = f32(i) + 1.0 - nu;

    // Map to palette. Multiply by a small factor so the gradient
    // sweeps several times across deep regions, giving the classic
    // "glowing tendrils" look.
    let t = sqrt(smooth_i / f32(u.max_iter));
    let col = palette(t * 4.0 + u.zoom * 0.001);
    return vec4<f32>(col, 1.0);
}
"#;

// ---------------------------------------------------------------------------
// Animation parameters
// ---------------------------------------------------------------------------
//
// Idle-wallpaper style. Two superimposed motions, both continuous
// and out-of-phase so the trajectory never quite repeats:
//
//   1. Anchor drift — every PAN_SEGMENT_SECS the camera slides to
//      a new boundary-rich tour point.
//   2. Orbit — around whichever anchor is current, the camera
//      traces a small circle whose radius scales with the
//      current view size, so you always see the local structure
//      "from sliding angles" rather than a straight zoom-in.
//
// The zoom also breathes mildly so you get a sense of depth
// without ever fully losing context.

/// Anchor points the camera orbits around, in order. Picked from
/// the Mandelbrot boundary where there's always interesting
/// detail: spiral seahorses, lightning-fork tendrils, mini-set
/// neighborhoods.
const TOUR: &[[f32; 2]] = &[
    [-0.7436, 0.1318],   // seahorse valley
    [-0.1011, 0.9563],   // upper antenna, curls
    [0.2820, 0.5300],    // top-right boundary near a cardioid bulb
    [-1.4007, 0.0000],   // needle / mini-set neighborhood
    [-0.7269, 0.1889],   // Julia-island spirals
    [-0.5500, 0.5600],   // upper-left boundary, light tendrils
];
/// Seconds before drifting from one tour anchor to the next.
const PAN_SEGMENT_SECS: f32 = 10.0;
/// One full revolution of the orbit motion.
const ORBIT_PERIOD_SECS: f32 = 7.0;
/// Orbit radius as a fraction of the current view's half-width.
/// At 0.20 the camera traces a small circle so the anchor stays
/// well within frame — the motion reads as "drifting around the
/// detail" rather than "swinging across it".
const ORBIT_RADIUS_FRAC: f32 = 0.20;
/// Period of the zoom breath. Picked coprime-ish with the orbit
/// period so the (orbit, zoom) phase combination doesn't loop
/// exactly each revolution.
const ZOOM_BREATH_SECS: f32 = 17.0;
/// Min/max zoom of the breath. The wide end (~3×) is roughly
/// "you can see the whole set" — preserves orientation and lets
/// the viewer recognize what they're looking at. The deep end
/// (~12×) is just enough to push into a tendril and resolve
/// boundary detail. Subtle on purpose: this is meant to read as
/// "the GPU is doing real work at a steady framerate", not a
/// deep-zoom showpiece.
const ZOOM_MIN: f32 = 3.0;
const ZOOM_MAX: f32 = 12.0;
/// Constant iteration budget. At our zoom range the boundary is
/// resolved sharply by ~200 iters; we give it a little headroom.
const ITER_COUNT: u32 = 256;

/// Cosine ease — input and output in [0, 1], C¹-continuous at the
/// endpoints. Smoother than smoothstep at segment joins.
fn ease(t: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::PI * t).cos()
}

/// Compute the per-frame uniforms from elapsed seconds.
fn uniforms_for_time(elapsed: f32, resolution: (u32, u32)) -> Uniforms {
    // Anchor drift — cosine-interpolate between consecutive tour
    // points so velocity is C¹-continuous at the joins.
    let pan_phase = elapsed / PAN_SEGMENT_SECS;
    let segment = pan_phase.floor() as usize;
    let frac = pan_phase - pan_phase.floor();
    let a = TOUR[segment % TOUR.len()];
    let b = TOUR[(segment + 1) % TOUR.len()];
    let m = ease(frac);
    let anchor = [a[0] + (b[0] - a[0]) * m, a[1] + (b[1] - a[1]) * m];

    // Zoom: log-space sine wave between ZOOM_MIN and ZOOM_MAX.
    // Working in log-space means a 2× zoom feels the same whether
    // we're at 80× or 400×.
    let z_phase = elapsed / ZOOM_BREATH_SECS * std::f32::consts::TAU;
    let z_norm = 0.5 - 0.5 * z_phase.cos();
    let log_min = ZOOM_MIN.ln();
    let log_max = ZOOM_MAX.ln();
    let zoom = (log_min + (log_max - log_min) * z_norm).exp();

    // Orbit — small circular offset added to the anchor. Radius
    // scales with the current view half-width (1.5 / zoom in
    // complex units, see the shader) so the orbit is always the
    // same fraction of the visible area regardless of zoom level.
    let view_half_width = 1.5 / zoom;
    let orbit_radius = view_half_width * ORBIT_RADIUS_FRAC;
    let orbit_phase = elapsed / ORBIT_PERIOD_SECS * std::f32::consts::TAU;
    let center = [
        anchor[0] + orbit_radius * orbit_phase.cos(),
        anchor[1] + orbit_radius * orbit_phase.sin(),
    ];

    let (w, h) = (resolution.0.max(1) as f32, resolution.1.max(1) as f32);
    Uniforms {
        center,
        resolution: [w, h],
        zoom,
        max_iter: ITER_COUNT,
        aspect: w / h,
        _pad: 0.0,
    }
}

pub fn gradient_canvas() -> Primitive {
    // Shared between every callback. `None` until on_ready
    // completes; reset to `None` on on_lost.
    let state: Rc<RefCell<Option<RendererState>>> = Rc::new(RefCell::new(None));

    let state_for_ready = state.clone();
    let state_for_resize = state.clone();
    let state_for_lost = state;

    ui! {
        Graphics(
            on_ready = move |event: OnReadyEvent| {
                let state_slot = state_for_ready.clone();
                // wgpu init is async on web (request_adapter,
                // request_device). spawn_local kicks it onto the
                // microtask queue without blocking.
                wasm_bindgen_futures::spawn_local(async move {
                    if let Some(mut s) = build_renderer(event.surface, event.size).await {
                        // Kick the render loop. `RafLoop` owns the
                        // per-frame closure + the browser handle;
                        // dropping it on teardown cancels the next
                        // frame *and* destroys the closure in the
                        // right order so wasm-bindgen never sees a
                        // pending-but-destroyed Closure.
                        let weak_state = std::rc::Rc::downgrade(&state_slot);
                        s.raf_loop = Some(framework_core::raf_loop(move || {
                            if let Some(state) = weak_state.upgrade() {
                                paint_one_frame(&state);
                            }
                        }));
                        *state_slot.borrow_mut() = Some(s);
                    }
                });
            },
            on_resize = move |event: OnResizeEvent| {
                let mut slot = state_for_resize.borrow_mut();
                let Some(s) = slot.as_mut() else { return };
                s.config.width = event.size.0;
                s.config.height = event.size.1;
                s.surface.configure(&s.device, &s.config);
            },
            on_lost = move || {
                // Drop everything wgpu-related. The framework will
                // call on_ready again with a fresh surface if/when
                // the canvas comes back.
                *state_for_lost.borrow_mut() = None;
            }
        ).with_style(crate::gradient_canvas_style())
    }
}

/// Build the full renderer. Async because adapter + device requests
/// are async. Returns `None` on any failure (logged to console).
async fn build_renderer(
    framework_surface: GraphicsSurface,
    size: (u32, u32),
) -> Option<RendererState> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
    // Match the Android demo: turn off DEBUG/VALIDATION flags. They
    // try to install browser-side validation paths that aren't
    // universally supported and don't add value for an end-user
    // demo. Same `InstanceFlags::empty()` keeps debug-builds and
    // release-builds behaving identically in the browser.
    desc.flags = wgpu::InstanceFlags::empty();
    let instance = wgpu::util::new_instance_with_webgpu_detection(desc).await;

    // The framework surface implements HasWindowHandle +
    // HasDisplayHandle, so we can hand it straight to wgpu.
    let surface: wgpu::Surface<'static> = match instance.create_surface(framework_surface.clone()) {
        Ok(s) => s,
        Err(e) => {
            web_sys::console::error_1(
                &format!("[gradient] create_surface failed: {e:?}").into(),
            );
            return None;
        }
    };

    let adapter = match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
    {
        Ok(a) => a,
        Err(e) => {
            web_sys::console::error_1(
                &format!("[gradient] request_adapter failed: {e:?}").into(),
            );
            return None;
        }
    };

    let (device, queue) = match adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("gradient-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                .using_resolution(adapter.limits()),
            ..Default::default()
        })
        .await
    {
        Ok(p) => p,
        Err(e) => {
            web_sys::console::error_1(
                &format!("[gradient] request_device failed: {e:?}").into(),
            );
            return None;
        }
    };

    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or_else(|| caps.formats[0]);

    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.0,
        height: size.1,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
    };
    surface.configure(&device, &config);

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("gradient-shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("gradient-bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("gradient-pl"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("gradient-pipeline"),
        layout: Some(&layout),
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
                blend: Some(wgpu::BlendState::REPLACE),
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
    let initial = uniforms_for_time(0.0, size);
    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gradient-uniforms"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&initial));
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("gradient-bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });

    Some(RendererState {
        framework_surface,
        surface,
        device,
        queue,
        config,
        pipeline,
        uniform_buf,
        bind_group,
        start_ms: js_sys::Date::now(),
        raf_loop: None,
    })
}

/// Render exactly one frame. The auto-rearm is handled by the
/// `RafLoop` that wraps this — `paint_one_frame` itself just
/// reads the uniform, encodes a draw, and submits.
fn paint_one_frame(state: &Rc<RefCell<Option<RendererState>>>) {
    let frame = {
        let slot = state.borrow();
        let Some(s) = slot.as_ref() else { return };
        match s.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            other => {
                web_sys::console::warn_1(
                    &format!("[gradient] get_current_texture: {other:?}").into(),
                );
                return;
            }
        }
    };
    let view = frame.texture.create_view(&Default::default());
    {
        let slot = state.borrow();
        let Some(s) = slot.as_ref() else { return };
        let now = js_sys::Date::now();
        let elapsed = ((now - s.start_ms) / 1000.0) as f32;
        let u = uniforms_for_time(elapsed, (s.config.width, s.config.height));
        s.queue.write_buffer(&s.uniform_buf, 0, bytemuck::bytes_of(&u));

        let mut encoder = s.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gradient-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gradient-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&s.pipeline);
            pass.set_bind_group(0, &s.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        s.queue.submit(std::iter::once(encoder.finish()));
    }
    frame.present();
    // Next frame is auto-scheduled by the surrounding `RafLoop`.
}
