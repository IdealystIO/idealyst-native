//! Instanced analytic-shape pass for canvas-vello.
//!
//! Draws a batch of [`ShapeInstance`] rounded boxes (rect / rounded-rect /
//! circle / pill) in a SINGLE instanced draw call, rasterizing each
//! analytically with an SDF fragment shader (`sd_round_box` — the same SDF the
//! [layer compositor](crate::native_capture) uses) instead of tessellating one
//! Bézier path per shape. This is the throughput fast path [`render`](crate::render)
//! takes for a PURE-shape scene (every op a [`DrawOp::Shapes`](canvas_core::DrawOp)
//! batch with Normal blend): thousands of shapes cost one pipeline bind + one
//! draw, not one path flatten/bin per shape.
//!
//! Mixed scenes (shapes interleaved with vector fills/strokes/images/transforms,
//! or a non-Normal batch blend) fall back to the encoder's expand-to-fills path
//! ([`encode`](crate::encode)), which encodes the per-shape fill an author would
//! write by hand (CLAUDE.md §7) — so the instanced pass is a pure optimization,
//! never a behavioral fork. The fast-path predicate lives in `render.rs`
//! (`pure_shape_batches`); this module only knows how to draw a flat list of
//! instances into a target.
//!
//! # Coordinate space & color
//!
//! Instances are in LOGICAL canvas coords (top-left origin, y-down). The vertex
//! shader applies the device-pixel ratio (`scale`) and maps to NDC, matching the
//! base transform `render.rs` hands vello (`Affine::scale(scale)`). Colors are
//! straight-alpha sRGB bytes scaled to `0..1` and written into the linear
//! `Rgba8Unorm` target verbatim — the same convention vello uses (the target is
//! deliberately non-sRGB so stored bytes aren't re-gamma-encoded; see
//! `render.rs`). The pipeline alpha-blends, so overlapping instances within a
//! batch composite in draw order, matching the per-shape fill fallback.

use canvas_core::ShapeInstance;

/// Bytes per instance in the vertex buffer: `center.xy`, `half.xy`,
/// `(radius, _pad)`, `color.rgba` = 2+2+2+4 floats = 40 bytes.
const INSTANCE_SIZE: u64 = 40;

/// Initial instance-buffer capacity (in instances). Grows on demand — no cap.
const INITIAL_INSTANCES: u64 = 1024;

/// The vello target is `Rgba8Unorm`; the pass draws into it directly.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const SHAPE_WGSL: &str = r#"
// Viewport: x = target width (physical px), y = height, z = dpr (physical/logical).
struct Vp { dims: vec4<f32> };
@group(0) @binding(0) var<uniform> vp: Vp;

// AA margin (logical px) the quad is inflated by, so the SDF's smoothstep band
// is never clipped at the geometric edge.
const MARGIN: f32 = 2.0;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,  // offset from center (logical px)
    @location(1) hsize: vec2<f32>,  // half-extents (logical px)
    @location(2) radius: f32,       // corner radius (logical px)
    @location(3) color: vec4<f32>,
};

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) center: vec2<f32>,
    @location(1) hsize: vec2<f32>,
    @location(2) shape: vec2<f32>,  // (radius, _pad)
    @location(3) color: vec4<f32>,
) -> VsOut {
    // Triangle-strip unit quad corners.
    var corners = array<vec2<f32>, 4>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0), vec2(1.0, 1.0),
    );
    let c = corners[vi];
    let ext = hsize + vec2<f32>(MARGIN);
    let local = c * ext;             // logical offset from center
    let world = center + local;      // logical canvas coords
    let phys = world * vp.dims.z;    // physical pixels
    // Physical px → NDC, top-left origin (flip y).
    let ndc = vec2<f32>(
        phys.x / vp.dims.x * 2.0 - 1.0,
        1.0 - phys.y / vp.dims.y * 2.0,
    );
    var out: VsOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.local = local;
    out.hsize = hsize;
    out.radius = shape.x;
    out.color = color;
    return out;
}

// Signed distance to a rounded box (negative inside). Identical formula to the
// layer compositor's mask, so corner rounding matches across the codebase.
fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let r = clamp(in.radius, 0.0, min(in.hsize.x, in.hsize.y));
    let d_logical = sd_round_box(in.local, in.hsize, r);
    let d_phys = d_logical * vp.dims.z;   // distance in physical px
    // ~1px anti-aliased coverage band centered on the edge.
    let aa = 1.0 - smoothstep(-0.5, 0.5, d_phys);
    return vec4<f32>(in.color.rgb, in.color.a * aa);
}
"#;

pub(crate) struct ShapePass {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    /// `vec4` viewport uniform (target w, h, dpr, _).
    viewport: wgpu::Buffer,
    /// Per-instance vertex buffer, grown on demand. `capacity` is in bytes.
    instances: wgpu::Buffer,
    capacity: u64,
}

impl ShapePass {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shape-instanced-shader"),
            source: wgpu::ShaderSource::Wgsl(SHAPE_WGSL.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shape-instanced-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(16),
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shape-instanced-pl"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: INSTANCE_SIZE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 8,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 16,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 24,
                    shader_location: 3,
                },
            ],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shape-instanced-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[instance_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                // Match the vello target; straight-alpha over-compositing so
                // overlapping instances blend in draw order.
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let viewport = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shape-instanced-viewport"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let capacity = INITIAL_INSTANCES * INSTANCE_SIZE;
        let instances = make_instance_buffer(device, capacity);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shape-instanced-bind-group"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport.as_entire_binding(),
            }],
        });
        Self { pipeline, bind_group, viewport, instances, capacity }
    }

    /// Clear `target_view` to transparent and draw every instance in `batches`
    /// (in order) in one instanced pass. `scale` is the device-pixel ratio;
    /// `target_w`/`target_h` are the target's physical-pixel size. The pass owns
    /// the clear (it replaces vello's clear on the fast path), so a pure-shape
    /// scene with zero shapes still wipes the target to transparent.
    pub(crate) fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        batches: &[&[ShapeInstance]],
        scale: f32,
        target_w: u32,
        target_h: u32,
    ) {
        let total: usize = batches.iter().map(|b| b.len()).sum();

        // Pack all instances, in op order.
        let mut bytes: Vec<u8> = Vec::with_capacity(total * INSTANCE_SIZE as usize);
        for batch in batches {
            for sh in *batch {
                push_instance(&mut bytes, sh);
            }
        }

        // Viewport uniform: target physical size + dpr.
        let vp = [target_w as f32, target_h as f32, scale.max(0.0), 0.0];
        let mut vpb = [0u8; 16];
        for (i, f) in vp.iter().enumerate() {
            vpb[i * 4..i * 4 + 4].copy_from_slice(&f.to_ne_bytes());
        }
        queue.write_buffer(&self.viewport, 0, &vpb);

        // Grow the instance buffer if this frame needs more than it holds.
        if !bytes.is_empty() {
            let need = bytes.len() as u64;
            if need > self.capacity {
                let cap = need.next_power_of_two();
                self.instances = make_instance_buffer(device, cap);
                self.capacity = cap;
            }
            queue.write_buffer(&self.instances, 0, &bytes);
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shape-instanced-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
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
        if total > 0 {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            // 4-vert triangle-strip quad per instance.
            pass.draw(0..4, 0..total as u32);
        }
    }
}

fn make_instance_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shape-instances"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Append one instance's 40 bytes (10 little-endian f32s) to `out`. Uses the
/// clamped [`ShapeInstance::effective_radius`] so the GPU geometry matches the
/// CPU fill expansion exactly.
fn push_instance(out: &mut Vec<u8>, sh: &ShapeInstance) {
    let r = sh.effective_radius();
    let fields = [
        sh.cx,
        sh.cy,
        sh.hw,
        sh.hh,
        r,
        0.0, // pad
        sh.color.r as f32 / 255.0,
        sh.color.g as f32 / 255.0,
        sh.color.b as f32 / 255.0,
        sh.color.a as f32 / 255.0,
    ];
    for f in fields {
        out.extend_from_slice(&f.to_ne_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::Color as CanvasColor;

    /// Render `batches` through a fresh [`ShapePass`] into an `S×S` Rgba8Unorm
    /// target at dpr 1.0 and read the pixels back. `None` when the host has no
    /// usable GPU (CI without a GPU skips rather than fails), mirroring
    /// `render.rs`'s `render_to_rgba`.
    fn render_shapes(batches: &[&[ShapeInstance]], s: u32) -> Option<Vec<u8>> {
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
            label: Some("shape-pass-test"),
            ..Default::default()
        }))
        .ok()?;

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shape-pass-test-target"),
            size: wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let mut pass = ShapePass::new(&device);
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        pass.render(&device, &queue, &mut encoder, &view, batches, 1.0, s, s);

        // bytes_per_row must be 256-aligned; s=64 → 256, no padding.
        let bpr = s * 4;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bpr * s) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
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
        queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let data = buffer.slice(..).get_mapped_range();
        Some(data.to_vec())
    }

    fn px(buf: &[u8], s: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * s + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    /// A red circle in the center and a blue sharp rect in the top-left corner,
    /// drawn in one instanced pass. Proves: the SDF fills the circle interior,
    /// the rect interior, the gap between them is transparent (the pass cleared
    /// it), and instance colors land where their geometry is.
    #[test]
    fn instanced_circle_and_rect_paint_their_interiors() {
        const S: u32 = 64;
        let shapes = [
            ShapeInstance::circle(32.0, 32.0, 14.0, CanvasColor::new(255, 0, 0, 255)),
            ShapeInstance::rect(4.0, 4.0, 12.0, 12.0, CanvasColor::new(0, 0, 255, 255)),
        ];
        let batches: [&[ShapeInstance]; 1] = [&shapes];
        let Some(buf) = render_shapes(&batches, S) else {
            eprintln!("skip: no GPU");
            return;
        };
        // Circle center: opaque red.
        let center = px(&buf, S, 32, 32);
        assert!(
            center[0] > 200 && center[1] < 60 && center[2] < 60 && center[3] > 200,
            "circle center should be opaque red, got {center:?}"
        );
        // Rect interior (10,10): opaque blue.
        let rect = px(&buf, S, 10, 10);
        assert!(
            rect[2] > 200 && rect[0] < 60 && rect[3] > 200,
            "rect interior should be opaque blue, got {rect:?}"
        );
        // A gap far from both shapes: transparent (the pass cleared the target).
        let gap = px(&buf, S, 60, 4);
        assert!(gap[3] < 8, "gap should be transparent, got {gap:?}");
        // Outside the circle radius but inside its bounding quad: transparent
        // (the SDF, not the quad, defines coverage). Corner of the circle's quad.
        let circle_quad_corner = px(&buf, S, 20, 20);
        assert!(
            circle_quad_corner[3] < 40,
            "circle quad corner must be ~transparent (SDF coverage, not the quad), got {circle_quad_corner:?}"
        );
    }

    /// Zero shapes must still clear the target to transparent (the fast path
    /// owns the clear for an empty pure-shape scene).
    #[test]
    fn empty_batch_clears_target() {
        const S: u32 = 64;
        let empty: [&[ShapeInstance]; 0] = [];
        let Some(buf) = render_shapes(&empty, S) else {
            eprintln!("skip: no GPU");
            return;
        };
        let p = px(&buf, S, 32, 32);
        assert!(p[3] < 8, "empty scene should clear to transparent, got {p:?}");
    }

    /// Growing past the initial capacity must not panic or drop instances: draw
    /// more circles than `INITIAL_INSTANCES` and confirm a late one painted.
    #[test]
    fn grows_instance_buffer_past_initial_capacity() {
        const S: u32 = 64;
        // One more than the initial capacity, all tiny, tiled across the target;
        // then a big red one LAST so it's on top at center — proving the buffer
        // held every instance through the grow.
        let mut shapes: Vec<ShapeInstance> = (0..INITIAL_INSTANCES + 1)
            .map(|i| {
                let x = (i % 64) as f32;
                let y = ((i / 64) % 64) as f32;
                ShapeInstance::circle(x, y, 0.5, CanvasColor::new(0, 255, 0, 255))
            })
            .collect();
        shapes.push(ShapeInstance::circle(32.0, 32.0, 12.0, CanvasColor::new(255, 0, 0, 255)));
        let batches: [&[ShapeInstance]; 1] = [&shapes];
        let Some(buf) = render_shapes(&batches, S) else {
            eprintln!("skip: no GPU");
            return;
        };
        let center = px(&buf, S, 32, 32);
        assert!(
            center[0] > 200 && center[3] > 200,
            "last (big red) instance must paint after a buffer grow, got {center:?}"
        );
    }
}
