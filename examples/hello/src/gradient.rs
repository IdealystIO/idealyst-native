//! Animated Mandelbrot demo using the bare `Graphics` surface
//! primitive + `framework_core::driver` (`render_loop` +
//! `spawn_async`).
//!
//! Before the driver primitives existed, this was three files
//! (~1450 lines total) duplicating the same wgpu init + render loop
//! per platform. Now it's one file: the wgpu code runs unmodified on
//! web/Android/iOS because wgpu is already cross-platform — what
//! differed across platforms was only the *driver* (frame ticker +
//! async runtime), which `framework-core::driver` hides.
//!
//! The flow:
//!
//! 1. `Graphics(on_ready, on_resize, on_lost, ...)` gets handed a
//!    `raw_window_handle`-shaped surface from the framework.
//! 2. `spawn_async` drives our `async fn build_renderer` — on web
//!    it's `spawn_local`, on native it's `pollster::block_on`. Same
//!    `.await` shape either way.
//! 3. `render_loop(|elapsed| ...)` ticks per frame — `rAF` on web,
//!    `CADisplayLink`-substitute on iOS, dedicated thread on Android.

use framework_core::driver::{render_loop, RenderLoop};
use framework_core::primitives::graphics::{
    GraphicsSurface, OnReadyEvent, OnResizeEvent,
};
use framework_core::{ui, Primitive};

// Per-target shared-state container. `wgpu::Surface<'static>` (and
// the other wgpu handles) are `!Send` on wasm and `Send` on native;
// the render-loop closure runs on a worker thread on Android and on
// the calling thread on web/iOS. So the cheapest shape that works
// everywhere is "Rc/RefCell when single-threaded, Arc/Mutex when not."
#[cfg(any(target_arch = "wasm32", target_os = "ios"))]
mod shared {
    use std::cell::RefCell;
    use std::rc::Rc;
    pub type Slot<T> = Rc<RefCell<Option<T>>>;
    pub fn new<T>() -> Slot<T> { Rc::new(RefCell::new(None)) }
    pub fn with_mut<T>(slot: &Slot<T>, f: impl FnOnce(&mut Option<T>)) {
        f(&mut slot.borrow_mut())
    }
    /// Take the current value out, leaving `None`. Used by `on_lost`
    /// so the heavy `Drop` (which on Android joins a render thread)
    /// runs OUTSIDE the lock — see the `on_lost` callback in
    /// `gradient_canvas` for the deadlock this avoids.
    pub fn take<T>(slot: &Slot<T>) -> Option<T> {
        slot.borrow_mut().take()
    }
}

#[cfg(target_os = "android")]
mod shared {
    use std::sync::{Arc, Mutex};
    pub type Slot<T> = Arc<Mutex<Option<T>>>;
    pub fn new<T>() -> Slot<T> { Arc::new(Mutex::new(None)) }
    pub fn with_mut<T>(slot: &Slot<T>, f: impl FnOnce(&mut Option<T>)) {
        if let Ok(mut guard) = slot.lock() {
            f(&mut guard);
        }
    }
    /// Take the current value out, leaving `None`. The returned
    /// `Option<T>` is dropped by the caller AFTER this function
    /// returns — so the heavy `Drop` (which joins the render
    /// thread, and the render thread itself acquires this mutex
    /// every frame) runs OUTSIDE the lock. Dropping inside the lock
    /// would deadlock: the render thread would block on the mutex
    /// while we wait for it to exit. See `on_lost` in
    /// `gradient_canvas` for the call site.
    pub fn take<T>(slot: &Slot<T>) -> Option<T> {
        slot.lock().ok().and_then(|mut g| g.take())
    }
}

// Per-target async-init driver. wgpu's `request_adapter` /
// `request_device` / shader compilation can take seconds on real
// hardware. Running them on the platform's UI thread blocks input
// handling (on Android, Choreographer logs "Skipped N frames" and
// the activity feels frozen).
//
//   - **Android**: `spawn_async_on_worker` ships the future to a
//     dedicated `std::thread` so the UI thread stays responsive.
//     The renderer state lives in `Arc<Mutex<…>>` and the render
//     loop runs on its own thread (driven by wgpu's `Fifo` present
//     mode).
//   - **iOS**: `spawn_async` is fine — the Metal driver wants
//     CAMetalLayer touched from the main thread, and wgpu's Metal
//     init isn't blocking-slow the way Vulkan's Android validation
//     setup is.
//   - **Web**: `spawn_async` = `wasm_bindgen_futures::spawn_local`;
//     no thread option without web workers, and JS is non-blocking
//     for the parts that matter anyway (async wgpu init returns to
//     the event loop between `.await`s).
#[cfg(target_os = "android")]
use framework_core::driver::spawn_async_on_worker as spawn_init;
#[cfg(not(target_os = "android"))]
use framework_core::driver::spawn_async as spawn_init;

// ---------------------------------------------------------------------------
// Uniforms + shader. Identical to what shipped before — see
// gradient.rs in the git history for the full annotated version.
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    center: [f32; 2],
    resolution: [f32; 2],
    zoom: f32,
    max_iter: u32,
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
    out.uv = p;
    return out;
}

fn palette(t: f32) -> vec3<f32> {
    let t2 = clamp(t, 0.0, 0.999);
    let a = vec3<f32>(0.50, 0.50, 0.50);
    let b = vec3<f32>(0.50, 0.50, 0.50);
    let c = vec3<f32>(1.00, 1.00, 1.00);
    let d = vec3<f32>(0.00, 0.10, 0.20);
    return a + b * cos(6.28318 * (c * t2 + d));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let scale = 1.5 / u.zoom;
    let c = u.center + vec2<f32>(in.uv.x * scale * u.aspect, in.uv.y * scale);

    var z = vec2<f32>(0.0, 0.0);
    var i: u32 = 0u;
    let bail = 256.0;
    var z2 = 0.0;
    loop {
        if (i >= u.max_iter) { break; }
        let new_z = vec2<f32>(z.x * z.x - z.y * z.y + c.x, 2.0 * z.x * z.y + c.y);
        z = new_z;
        z2 = z.x * z.x + z.y * z.y;
        if (z2 > bail) { break; }
        i = i + 1u;
    }

    if (i >= u.max_iter) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    let log_zn = log(z2) * 0.5;
    let nu = log(log_zn / log(2.0)) / log(2.0);
    let smooth_i = f32(i) + 1.0 - nu;

    let t = sqrt(smooth_i / f32(u.max_iter));
    let col = palette(t * 4.0 + u.zoom * 0.001);
    return vec4<f32>(col, 1.0);
}
"#;

// ---------------------------------------------------------------------------
// Animation params + uniform synthesis. Pure logic; runs identically
// on every platform.
// ---------------------------------------------------------------------------

const TOUR: &[[f32; 2]] = &[
    [-0.7436, 0.1318],
    [-0.1011, 0.9563],
    [0.2820, 0.5300],
    [-1.4007, 0.0000],
    [-0.7269, 0.1889],
    [-0.5500, 0.5600],
];
const PAN_SEGMENT_SECS: f32 = 10.0;
const ORBIT_PERIOD_SECS: f32 = 7.0;
const ORBIT_RADIUS_FRAC: f32 = 0.20;
const ZOOM_BREATH_SECS: f32 = 17.0;
const ZOOM_MIN: f32 = 3.0;
const ZOOM_MAX: f32 = 12.0;
const ITER_COUNT: u32 = 256;

fn ease(t: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::PI * t).cos()
}

fn uniforms_for_time(elapsed: f32, resolution: (u32, u32)) -> Uniforms {
    let pan_phase = elapsed / PAN_SEGMENT_SECS;
    let segment = pan_phase.floor() as usize;
    let frac = pan_phase - pan_phase.floor();
    let a = TOUR[segment % TOUR.len()];
    let b = TOUR[(segment + 1) % TOUR.len()];
    let m = ease(frac);
    let anchor = [a[0] + (b[0] - a[0]) * m, a[1] + (b[1] - a[1]) * m];

    let z_phase = elapsed / ZOOM_BREATH_SECS * std::f32::consts::TAU;
    let z_norm = 0.5 - 0.5 * z_phase.cos();
    let log_min = ZOOM_MIN.ln();
    let log_max = ZOOM_MAX.ln();
    let zoom = (log_min + (log_max - log_min) * z_norm).exp();

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

// ---------------------------------------------------------------------------
// Renderer state. Held inside the shared slot so `on_resize` and the
// per-frame loop can both reach it. `Arc<Mutex<...>>` is the cheapest
// shape that works on both web (single-threaded; mutex is uncontended)
// and Android (render loop fires on a dedicated thread).
// ---------------------------------------------------------------------------

struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl Renderer {
    fn render(&mut self, elapsed: f32) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&Default::default());
        let u = uniforms_for_time(elapsed, (self.config.width, self.config.height));
        self.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
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
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }

    fn resize(&mut self, size: (u32, u32)) {
        self.config.width = size.0.max(1);
        self.config.height = size.1.max(1);
        self.surface.configure(&self.device, &self.config);
    }
}

/// Per-target wgpu instance backend selection. Same wgpu surface API
/// past this point.
fn instance_backends() -> wgpu::Backends {
    #[cfg(target_arch = "wasm32")]
    { wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL }
    #[cfg(target_os = "android")]
    { wgpu::Backends::VULKAN | wgpu::Backends::GL }
    #[cfg(target_os = "ios")]
    { wgpu::Backends::METAL }
    #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
    { wgpu::Backends::PRIMARY }
}

/// iOS's Metal driver wants `AutoNoVsync`; everyone else gets `Fifo`
/// for proper vsync-paced presentation. wgpu picks the right swap-
/// chain shape for either.
fn preferred_present_mode() -> wgpu::PresentMode {
    #[cfg(target_os = "ios")]
    { wgpu::PresentMode::AutoNoVsync }
    #[cfg(not(target_os = "ios"))]
    { wgpu::PresentMode::Fifo }
}

/// Async wgpu init. Identical across platforms — `spawn_async`
/// drives it on each target's natural runtime.
async fn build_renderer(
    surface_handle: GraphicsSurface,
    size: (u32, u32),
) -> Option<Renderer> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = instance_backends();
    // Same conservative-flags story from the original demo:
    // Android's Vulkan loader can null-pointer crash inside
    // vkSetDebugUtilsObjectNameEXT when DEBUG/VALIDATION ask for
    // labels the device didn't actually expose. Web's WebGPU
    // validation paths aren't universally supported either. Empty
    // flags work everywhere.
    desc.flags = wgpu::InstanceFlags::empty();

    #[cfg(target_arch = "wasm32")]
    let instance = wgpu::util::new_instance_with_webgpu_detection(desc).await;
    #[cfg(not(target_arch = "wasm32"))]
    let instance = wgpu::Instance::new(desc);

    let surface = instance.create_surface(surface_handle).ok()?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
        .ok()?;

    let limits = if cfg!(target_arch = "wasm32") {
        wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
    } else {
        wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits())
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("gradient-device"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            ..Default::default()
        })
        .await
        .ok()?;

    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(caps.formats[0]);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.0.max(1),
        height: size.1.max(1),
        present_mode: preferred_present_mode(),
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

    Some(Renderer {
        surface,
        device,
        queue,
        config,
        pipeline,
        uniform_buf,
        bind_group,
    })
}

/// What lives across the three lifecycle callbacks: the renderer
/// itself, and the live render loop handle (whose `Drop` stops it).
struct RendererState {
    renderer: Renderer,
    // Holding the handle keeps the loop alive; dropping it cancels.
    // Kept here so `on_lost` can drop the whole slot atomically.
    _loop: RenderLoop,
}

pub fn gradient_canvas() -> Primitive {
    let slot: shared::Slot<RendererState> = shared::new();
    let slot_ready = slot.clone();
    let slot_resize = slot.clone();
    let slot_lost = slot;

    ui! {
        Graphics(
            on_ready = move |event: OnReadyEvent| {
                let slot = slot_ready.clone();
                spawn_init(async move {
                    let Some(renderer) = build_renderer(event.surface, event.size).await
                    else { return };

                    // The render loop closure pulls from `slot` so
                    // on_lost can drop the renderer mid-frame safely
                    // (the closure just returns when the slot is
                    // empty). The closure must be `Send` on Android
                    // and `!Send`-OK on web/iOS — `shared::Slot`
                    // resolves to the right Arc/Rc per target.
                    let slot_for_frame = slot.clone();
                    let render_loop_handle = render_loop(move |elapsed| {
                        shared::with_mut(&slot_for_frame, |state| {
                            if let Some(s) = state {
                                s.renderer.render(elapsed);
                            }
                        });
                    });

                    // Take any pre-existing state out of the slot
                    // FIRST (normally None — on_lost fires before
                    // on_ready in well-behaved sequences) so its
                    // `Drop` runs outside the lock. Then install
                    // the fresh state. Same anti-deadlock pattern
                    // as the `on_lost` callback below.
                    let stale = shared::take(&slot);
                    drop(stale);
                    shared::with_mut(&slot, |state| {
                        *state = Some(RendererState {
                            renderer,
                            _loop: render_loop_handle,
                        });
                    });
                });
            },
            on_resize = move |event: OnResizeEvent| {
                shared::with_mut(&slot_resize, |state| {
                    if let Some(s) = state {
                        s.renderer.resize(event.size);
                    }
                });
            },
            on_lost = move || {
                // Take the state OUT of the slot first, then let it
                // drop AFTER the mutex guard releases. Critical on
                // Android: dropping `RendererState` drops `_loop:
                // RenderLoop` which `join()`s the render thread.
                // The render thread acquires the same mutex on every
                // frame; if we let `RendererState` drop while still
                // holding the guard, the render thread blocks on the
                // lock, we block on `join`, deadlock — and since
                // on_lost fires on the UI thread on Android, that
                // freezes the activity → ANR.
                let stale = shared::take(&slot_lost);
                drop(stale);
            }
        ).with_style(crate::gradient_canvas_style())
    }
}
