//! Animated Mandelbrot demo rendered into an embedded
//! `Graphics` node via `render_wgpu::graphics_with_drawer`.
//!
//! Shader + uniform math lifted verbatim from
//! `examples/hello-world/src/gradient.rs`; the lifecycle wrapper
//! is simpler here because the wgpu preview backend hands us a
//! ready-made device/queue/view each frame — no async init, no
//! surface lifecycle, no platform-specific spawn-on-worker.

use std::time::Duration;

use render_wgpu::{graphics_with_drawer, GraphicsFrame};

// ---------------------------------------------------------------------------
// Uniforms + shader (verbatim from hello-world/gradient.rs).
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
// Animation params (verbatim from hello-world/gradient.rs).
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
const ITER_COUNT: u32 = 128;

fn ease(t: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::PI * t).cos()
}

fn uniforms_for(elapsed: Duration, size: (u32, u32)) -> Uniforms {
    let elapsed = elapsed.as_secs_f32();
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

    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
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
// Lazy renderer state.
//
// The drawer closure is invoked the first time the framework
// hands us a device/queue (i.e. on the first frame the Graphics
// node renders). Pipeline + bind-group setup is one-shot;
// subsequent calls only write fresh uniforms + encode a render
// pass into the texture view.
// ---------------------------------------------------------------------------

struct State {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl State {
    fn build(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mandelbrot-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mandelbrot-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mandelbrot-pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mandelbrot-pipeline"),
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
                // Matches the renderer's `graphics_cache` texture
                // format (`Rgba8UnormSrgb`). Authors writing to
                // their own swapchain pick the swapchain's
                // format; here the host owns the texture.
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mandelbrot-uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mandelbrot-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        Self { pipeline, uniform_buf, bind_group }
    }

    fn render(&mut self, frame: &mut GraphicsFrame) {
        let u = uniforms_for(frame.elapsed, frame.size);
        frame.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));
        let mut pass = frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mandelbrot-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.view,
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
        pass.set_bind_group(0, Some(&self.bind_group), &[]);
        pass.draw(0..6, 0..1);
    }
}

/// Build a `Bound<GraphicsHandle>` that draws the animated
/// Mandelbrot tour on each frame. Drop into a child list:
///
/// ```ignore
/// view(vec![mandelbrot_demo().into(), …])
/// ```
pub fn mandelbrot_demo() -> framework_core::Bound<framework_core::primitives::graphics::GraphicsHandle>
{
    // The drawer closure owns its renderer state via Option;
    // first call materializes the pipeline + bind group from
    // the live device, every subsequent call just writes new
    // uniforms and encodes a render pass.
    let mut state: Option<State> = None;
    graphics_with_drawer(move |frame: &mut GraphicsFrame| {
        let s = state.get_or_insert_with(|| State::build(frame.device));
        s.render(frame);
        // Keep ticking — the host's animator tick currently
        // doesn't know about Graphics nodes, so without an
        // explicit `request_redraw` the animation would freeze
        // on the next vsync.
        render_wgpu::request_redraw();
    })
}
