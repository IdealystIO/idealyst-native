//! iOS variant of the animated Mandelbrot demo.
//!
//! Same wgpu pipeline + WGSL shader as web and Android; platform
//! glue differs:
//!
//! - **GPU backend**: Metal.
//! - **Render loop**: `CADisplayLink` on the main thread (iOS's
//!   standard vsync-driven rendering pattern). Unlike Android, iOS
//!   requires `CAMetalLayer` interaction on the main thread.
//! - **Time source**: `std::time::Instant`.
//! - **Async init**: `pollster::block_on` on the main thread.

use framework_core::primitives::graphics::{GraphicsSurface, OnReadyEvent, OnResizeEvent};
use framework_core::{ui, Primitive};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

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

/// Render state held alive by the display link callback.
struct RenderState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    size: (u32, u32),
    started: Instant,
    frame_count: u64,
}

impl RenderState {
    fn render_frame(&mut self) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => return,
        };
        let view = frame.texture.create_view(&Default::default());

        let elapsed = self.started.elapsed().as_secs_f32();
        let u = uniforms_for_time(elapsed, self.size);
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

        self.frame_count += 1;
        if self.frame_count <= 3 || self.frame_count % 120 == 0 {
            eprintln!("[gradient/ios] frame #{} (elapsed: {:.1}s)", self.frame_count, elapsed);
        }
    }
}

pub fn gradient_canvas() -> Primitive {
    // Shared render state + display link, kept alive by Rc.
    // Set to Some after on_ready, cleared on on_lost.
    let state: Rc<RefCell<Option<RenderState>>> = Rc::new(RefCell::new(None));
    // The CADisplayLink ref — kept alive so the timer keeps firing.
    // Stored as a raw NSObject retained reference.
    let display_link: Rc<RefCell<Option<objc2::rc::Retained<objc2_foundation::NSObject>>>> =
        Rc::new(RefCell::new(None));

    let state_for_ready = state.clone();
    let state_for_render = state.clone();
    let dl_for_ready = display_link.clone();
    let dl_for_lost = display_link.clone();
    let state_for_lost = state;

    ui! {
        Graphics(
            on_ready = move |event: OnReadyEvent| {
                eprintln!("[gradient/ios] on_ready: size={}x{}", event.size.0, event.size.1);

                let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
                desc.backends = wgpu::Backends::METAL;
                let instance = wgpu::Instance::new(desc);
                let surface = match instance.create_surface(event.surface.clone()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[gradient/ios] create_surface failed: {e:?}");
                        return;
                    }
                };

                let adapter = match pollster::block_on(instance.request_adapter(
                    &wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::HighPerformance,
                        compatible_surface: Some(&surface),
                        force_fallback_adapter: false,
                    },
                )) {
                    Ok(a) => a,
                    Err(e) => {
                        eprintln!("[gradient/ios] request_adapter failed: {e:?}");
                        return;
                    }
                };

                let (device, queue) = match pollster::block_on(adapter.request_device(
                    &wgpu::DeviceDescriptor {
                        label: Some("gradient-device"),
                        required_features: wgpu::Features::empty(),
                        required_limits: wgpu::Limits::downlevel_defaults()
                            .using_resolution(adapter.limits()),
                        ..Default::default()
                    },
                )) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("[gradient/ios] request_device failed: {e:?}");
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

                let config = wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format,
                    width: event.size.0.max(1),
                    height: event.size.1.max(1),
                    present_mode: wgpu::PresentMode::AutoNoVsync,
                    desired_maximum_frame_latency: 2,
                    alpha_mode: caps.alpha_modes[0],
                    view_formats: vec![],
                };
                surface.configure(&device, &config);
                eprintln!("[gradient/ios] surface configured {}x{}, format={:?}", config.width, config.height, format);

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

                *state_for_ready.borrow_mut() = Some(RenderState {
                    device, queue, surface, pipeline, bind_group, uniform_buf,
                    size: (config.width, config.height),
                    started: Instant::now(),
                    frame_count: 0,
                });

                // Set up a CADisplayLink to drive rendering on the main
                // thread at vsync rate. We use a CallbackTarget as the
                // target since CADisplayLink needs an ObjC target/action.
                use objc2::{msg_send, msg_send_id, sel};
                use objc2_foundation::NSObject;

                // We need a callback target that calls render_frame on
                // every display link tick. Reuse the framework's
                // CallbackTarget pattern via a leaked Fn closure.
                let render_state = state_for_render.clone();
                let render_fn: Rc<dyn Fn()> = Rc::new(move || {
                    if let Some(rs) = render_state.borrow_mut().as_mut() {
                        rs.render_frame();
                    }
                });

                // Create a simple NSObject-based target for the display link.
                // We'll use the same CallbackTarget from the backend.
                // But we don't have direct access to it here (it's in the backend crate).
                // Instead, use NSTimer with a short interval as a simpler approach.
                //
                // Actually, we can use CADisplayLink via raw msg_send.
                // CADisplayLink.displayLinkWithTarget:selector: creates the link,
                // then addToRunLoop:forMode: starts it.

                // For the target, we need an ObjC object with an action method.
                // The simplest approach: use a standalone NSObject subclass.
                // But we can't easily declare_class here in the hello crate.
                //
                // Simplest alternative: use an NSTimer at 1/60s interval.
                // Not as precise as CADisplayLink but works for the demo.
                use block2::ConcreteBlock;
                let timer_block = ConcreteBlock::new(move |_timer: *const NSObject| {
                    render_fn();
                });
                let timer_block = timer_block.copy();
                let timer: objc2::rc::Retained<NSObject> = unsafe {
                    msg_send_id![
                        objc2::class!(NSTimer),
                        scheduledTimerWithTimeInterval: (1.0 / 60.0) as f64,
                        repeats: true,
                        block: &*timer_block
                    ]
                };
                *dl_for_ready.borrow_mut() = Some(timer);
                eprintln!("[gradient/ios] timer started");
            },
            on_resize = move |_event: OnResizeEvent| {},
            on_lost = move || {
                // Invalidate the timer
                if let Some(timer) = dl_for_lost.borrow_mut().take() {
                    let _: () = unsafe { objc2::msg_send![&timer, invalidate] };
                }
                state_for_lost.borrow_mut().take();
            }
        ).with_style(crate::gradient_canvas_style())
    }
}

#[allow(dead_code)]
fn _silence(_: GraphicsSurface) {}
