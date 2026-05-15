//! Android variant of the animated Mandelbrot demo.
//!
//! The same wgpu pipeline + WGSL shader that runs on web; the
//! platform glue differs:
//!
//! - **Render loop driver**: instead of `requestAnimationFrame` we
//!   spawn a dedicated render thread that pumps frames at the
//!   surface's vsync (FIFO present mode does the actual pacing).
//! - **Time source**: `std::time::Instant` instead of
//!   `js_sys::Date::now()`.
//! - **Async init**: `pollster::block_on` instead of
//!   `wasm_bindgen_futures::spawn_local`.
//! - **Lifecycle**: Android destroys + recreates the surface on
//!   backgrounding (`on_lost` → `on_ready` again). Each `on_ready`
//!   spins up a fresh render thread with a fresh wgpu surface; the
//!   previous thread is signalled to exit on `on_lost`.
//!
//! The framework surface itself is `!Send` (it wraps a non-Send
//! per-backend handle internally). We sidestep that by doing
//! `Instance::create_surface(framework_surface)` on the UI thread
//! immediately on `on_ready` — `wgpu::Surface<'static>` IS `Send`
//! on native — and shipping that to the render thread, along with
//! the wgpu instance.

use framework_core::primitives::graphics::{GraphicsSurface, OnReadyEvent, OnResizeEvent};
use framework_core::{ui, Primitive};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{channel, Sender};
use std::thread;
use std::time::Instant;

/// Messages sent from the UI thread to the render thread. The
/// thread blocks on the channel between frames; `Resize` updates
/// the surface config + triggers a redraw, `Stop` exits the loop.
enum RenderMsg {
    Resize(u32, u32),
    Stop,
}

/// State the UI thread holds onto so it can send messages to the
/// render thread (resize, stop) and tear it down on `on_lost`.
struct RenderHandle {
    tx: Sender<RenderMsg>,
    /// Joined on drop so we don't leak threads if the Activity
    /// finishes mid-render.
    join: Option<thread::JoinHandle<()>>,
}

impl Drop for RenderHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(RenderMsg::Stop);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

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

// Animation parameters — kept identical to gradient_web so the
// experience matches across platforms. See that file for full
// notes on the orbit + breath shape.
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

pub fn gradient_canvas() -> Primitive {
    // Shared between on_ready / on_resize / on_lost. Holds the
    // active render thread's handle (None until on_ready completes,
    // None again after on_lost).
    let render_slot: Rc<RefCell<Option<RenderHandle>>> = Rc::new(RefCell::new(None));

    let slot_for_ready = render_slot.clone();
    let slot_for_resize = render_slot.clone();
    let slot_for_lost = render_slot;

    ui! {
        Graphics(
            on_ready = move |event: OnReadyEvent| {
                // Build wgpu instance + surface here on the UI
                // thread (synchronous), then ship the surface to a
                // dedicated render thread along with the size.
                // The framework surface is `!Send`; the wgpu surface
                // it produces IS `Send` on native.
                let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
                desc.backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
                // Don't ask wgpu to enable VK_EXT_debug_utils labels.
                // The default `InstanceFlags::from_build_config()` turns
                // DEBUG + VALIDATION on for debug builds, which on
                // Android crashes inside `vkSetDebugUtilsObjectNameEXT`
                // when the device's Vulkan loader didn't actually
                // expose the extension (null fn pointer SIGSEGV during
                // request_device). Forcing `empty()` keeps the demo
                // running on stock devices regardless of debug/release
                // build profile.
                desc.flags = wgpu::InstanceFlags::empty();
                let instance = wgpu::Instance::new(desc);
                let surface = match instance.create_surface(event.surface.clone()) {
                    Ok(s) => s,
                    Err(e) => {
                        log::error!("[gradient/android] create_surface failed: {e:?}");
                        return;
                    }
                };

                let (tx, rx) = channel::<RenderMsg>();
                let initial_size = event.size;
                let join = thread::Builder::new()
                    .name("gradient-renderer".into())
                    .spawn(move || {
                        run_render_thread(instance, surface, initial_size, rx);
                    })
                    .expect("failed to spawn renderer thread");

                let handle = RenderHandle { tx, join: Some(join) };
                *slot_for_ready.borrow_mut() = Some(handle);
            },
            on_resize = move |event: OnResizeEvent| {
                if let Some(h) = slot_for_resize.borrow().as_ref() {
                    let _ = h.tx.send(RenderMsg::Resize(event.size.0, event.size.1));
                }
            },
            on_lost = move || {
                // Drop the RenderHandle — its Drop sends Stop and
                // joins. The next on_ready (when the surface
                // returns) builds a fresh thread.
                slot_for_lost.borrow_mut().take();
            }
        ).with_style(crate::gradient_canvas_style())
    }
}

/// Runs on the dedicated render thread. Init wgpu device/queue/
/// pipeline, then loop drawing frames until a `Stop` message
/// arrives. `present_mode: Fifo` blocks `present()` until the
/// next vsync, which paces the loop without us needing to sleep.
fn run_render_thread(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    initial_size: (u32, u32),
    rx: std::sync::mpsc::Receiver<RenderMsg>,
) {
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    })) {
        Ok(a) => a,
        Err(e) => {
            log::error!("[gradient/android] request_adapter failed: {e:?}");
            return;
        }
    };

    let (device, queue) = match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gradient-device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    })) {
        Ok(p) => p,
        Err(e) => {
            log::error!("[gradient/android] request_device failed: {e:?}");
            return;
        }
    };

    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or_else(|| caps.formats[0]);

    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: initial_size.0.max(1),
        height: initial_size.1.max(1),
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
    let initial = uniforms_for_time(0.0, (config.width, config.height));
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

    let started = Instant::now();
    loop {
        // Drain pending messages without blocking — we only block
        // on present(). Stop messages cause an immediate return.
        loop {
            match rx.try_recv() {
                Ok(RenderMsg::Resize(w, h)) => {
                    config.width = w.max(1);
                    config.height = h.max(1);
                    surface.configure(&device, &config);
                }
                Ok(RenderMsg::Stop) => return,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }

        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated => {
                surface.configure(&device, &config);
                continue;
            }
            other => {
                log::warn!("[gradient/android] get_current_texture: {other:?}");
                // Brief backoff so we don't spin if the surface is
                // perma-broken (e.g. backgrounded but not yet
                // surfaceDestroyed'd).
                thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }
        };
        let view = frame.texture.create_view(&Default::default());

        let elapsed = started.elapsed().as_secs_f32();
        let u = uniforms_for_time(elapsed, (config.width, config.height));
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&u));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
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
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}

// Surface field is unused on this side directly, but the framework
// surface is held alive by the wgpu Surface internally. Silence the
// warning by referencing it.
#[allow(dead_code)]
fn _silence(_: GraphicsSurface) {}
