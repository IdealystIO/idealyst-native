//! Rounded-rect render pipeline.
//!
//! One instanced draw per frame: 6 vertices × N instances, where N
//! is the number of painted nodes. Each instance carries the rect's
//! screen-space `[x, y, w, h]`, background color, per-corner radius,
//! and a uniform border (color + width) for the MVP. The fragment
//! shader (`shaders/rect.wgsl`) does the SDF + AA.
//!
//! The instance buffer is grown on demand and reused across frames
//! — no per-frame allocation in the steady state. A small uniform
//! buffer carries the viewport size so the vertex shader can map
//! pixel coords → NDC.

use bytemuck::{Pod, Zeroable};
use std::num::NonZeroU64;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Instance {
    pub rect: [f32; 4],          // x, y, w, h in px (top-left origin)
    pub bg: [f32; 4],
    pub corner_radius: [f32; 4], // tl, tr, br, bl
    pub border_color: [f32; 4],
    pub border_width: f32,
    /// Rotation around the rect's center, in radians. The vertex
    /// shader rotates the quad's screen-space corners; the SDF
    /// fragment math stays in the rect's axis-aligned local
    /// frame so corner radii / borders still work.
    pub rotation: f32,
    pub _pad: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Globals {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

pub struct RectPipeline {
    pipeline: wgpu::RenderPipeline,
    // Kept so we can rebuild the bind group on device-lost without
    // re-declaring its layout.
    #[allow(dead_code)]
    bind_group_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    bind_group: wgpu::BindGroup,
}

impl RectPipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rect-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/rect.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rect-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Globals>() as u64),
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rect-pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rect-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[Self::instance_layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
            multiview: None,
            cache: None,
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rect-globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let instance_capacity = 256;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rect-instances"),
            size: instance_capacity * std::mem::size_of::<Instance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rect-bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            bind_group_layout,
            globals,
            instance_buffer,
            instance_capacity,
            bind_group,
        }
    }

    fn instance_layout<'a>() -> wgpu::VertexBufferLayout<'a> {
        // Manually-declared; the derive-via-bytemuck dance isn't
        // worth the dep when the layout is this short.
        const STRIDE: u64 = std::mem::size_of::<Instance>() as u64;
        wgpu::VertexBufferLayout {
            array_stride: STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute { offset: 0,  shader_location: 0, format: wgpu::VertexFormat::Float32x4 },
                wgpu::VertexAttribute { offset: 16, shader_location: 1, format: wgpu::VertexFormat::Float32x4 },
                wgpu::VertexAttribute { offset: 32, shader_location: 2, format: wgpu::VertexFormat::Float32x4 },
                wgpu::VertexAttribute { offset: 48, shader_location: 3, format: wgpu::VertexFormat::Float32x4 },
                wgpu::VertexAttribute { offset: 64, shader_location: 4, format: wgpu::VertexFormat::Float32 },
                wgpu::VertexAttribute { offset: 68, shader_location: 5, format: wgpu::VertexFormat::Float32 },
                wgpu::VertexAttribute { offset: 72, shader_location: 6, format: wgpu::VertexFormat::Float32x2 },
            ],
        }
    }

    /// Upload `instances` and the current viewport size, then encode
    /// the draw. Caller is responsible for the render pass's color
    /// attachment + clear.
    pub fn render<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        viewport: [f32; 2],
        instances: &[Instance],
    ) {
        if instances.is_empty() {
            return;
        }

        // Grow if needed (round to next power of two).
        let needed = instances.len() as u64;
        if needed > self.instance_capacity {
            let mut new_cap = self.instance_capacity;
            while new_cap < needed {
                new_cap *= 2;
            }
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rect-instances"),
                size: new_cap * std::mem::size_of::<Instance>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_capacity = new_cap;
        }

        queue.write_buffer(
            &self.globals,
            0,
            bytemuck::bytes_of(&Globals { viewport, _pad: [0.0; 2] }),
        );
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        pass.draw(0..6, 0..instances.len() as u32);
    }
}

/// Suppress unused-field warnings — `bind_group_layout` is held so
/// future re-bind paths (resize, device-lost) can rebuild without
/// re-creating the layout. Same for `_pad` on the instance struct.
const _: () = {
    let _ = std::mem::size_of::<Instance>();
};
