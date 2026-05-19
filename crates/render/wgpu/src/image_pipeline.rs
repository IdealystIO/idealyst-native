//! Textured-quad pipeline for the `Image` primitive.
//!
//! One pipeline instance per renderer. Each draw uses a
//! per-image `wgpu::BindGroup` (texture + sampler) — small UI
//! pages have a handful of images, so the binding switch cost
//! per frame is negligible. A future texture-atlas would let
//! every image share one bind group and collapse into a single
//! instanced draw; the shader already takes per-instance UV
//! sub-rect coordinates to support that without modification.
//!
//! Sibling of [`crate::pipeline::RectPipeline`]; both write
//! into the same render pass.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};

/// Per-image draw instance. Lives in a per-frame vertex buffer
/// alongside its sibling `RectInstance`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ImageInstance {
    pub rect: [f32; 4],
    pub uv_rect: [f32; 4],
    pub tint: [f32; 4],
    pub rotation: f32,
    pub opacity: f32,
    pub _pad: [f32; 2],
}

impl ImageInstance {
    pub fn solid(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            rect: [x, y, w, h],
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0, 1.0, 1.0, 1.0],
            rotation: 0.0,
            opacity: 1.0,
            _pad: [0.0; 2],
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Globals {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

pub struct ImagePipeline {
    pipeline: wgpu::RenderPipeline,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    /// Bind-group layout for per-image `(texture, sampler)`.
    /// Held so image-cache entries can build their own bind
    /// groups against it.
    pub texture_bgl: wgpu::BindGroupLayout,
    /// Shared bilinear sampler — no anisotropic filtering, no
    /// mipmaps. Good enough for UI-scale images; mipmaps land
    /// when we start downscaling thumbnails.
    pub sampler: wgpu::Sampler,
    /// Per-frame instance buffer — holds every image instance
    /// the batch wants to draw, written once. Each draw call
    /// indexes into it via `pass.draw(0..6, i..i+1)` so two
    /// draws within one submit don't clobber each other's
    /// vertex data the way the rect pipeline's single-call
    /// scratch buffer does.
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
}

impl ImagePipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/image.wgsl").into()),
        });

        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image-globals-bgl"),
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

        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image-texture-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("image-pl"),
            bind_group_layouts: &[&globals_bgl, &texture_bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("image-pipeline"),
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

        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image-globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image-globals-bg"),
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let instance_capacity: u64 = 32;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image-instance"),
            size: instance_capacity * std::mem::size_of::<ImageInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            globals_buffer,
            globals_bind_group,
            texture_bgl,
            sampler,
            instance_buffer,
            instance_capacity,
        }
    }

    fn instance_layout<'a>() -> wgpu::VertexBufferLayout<'a> {
        const STRIDE: u64 = std::mem::size_of::<ImageInstance>() as u64;
        wgpu::VertexBufferLayout {
            array_stride: STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute { offset: 0,  shader_location: 0, format: wgpu::VertexFormat::Float32x4 },
                wgpu::VertexAttribute { offset: 16, shader_location: 1, format: wgpu::VertexFormat::Float32x4 },
                wgpu::VertexAttribute { offset: 32, shader_location: 2, format: wgpu::VertexFormat::Float32x4 },
                wgpu::VertexAttribute { offset: 48, shader_location: 3, format: wgpu::VertexFormat::Float32 },
                wgpu::VertexAttribute { offset: 52, shader_location: 4, format: wgpu::VertexFormat::Float32 },
                wgpu::VertexAttribute { offset: 56, shader_location: 5, format: wgpu::VertexFormat::Float32x2 },
            ],
        }
    }

    /// Render a batch of image draws inside an open render
    /// pass. All instances are written to the per-frame buffer
    /// once, then we iterate them with bind-group switches
    /// between draws — `pass.draw(0..6, i..i+1)` selects each
    /// instance by index, so two calls within one submit never
    /// conflict on the same buffer range.
    pub fn render<'pass>(
        &'pass mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'pass>,
        viewport: [f32; 2],
        items: &'pass [ImageDraw],
    ) {
        if items.is_empty() {
            return;
        }
        // Grow the instance buffer to fit this frame's batch.
        let needed = items.len() as u64;
        if needed > self.instance_capacity {
            let mut new_cap = self.instance_capacity.max(1);
            while new_cap < needed {
                new_cap *= 2;
            }
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("image-instance"),
                size: new_cap * std::mem::size_of::<ImageInstance>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_capacity = new_cap;
        }
        // Pack every instance contiguously.
        let instances: Vec<ImageInstance> = items.iter().map(|i| i.instance).collect();
        queue.write_buffer(
            &self.globals_buffer,
            0,
            bytemuck::bytes_of(&Globals { viewport, _pad: [0.0; 2] }),
        );
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instances));

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        for (i, item) in items.iter().enumerate() {
            pass.set_bind_group(1, item.bind_group, &[]);
            let idx = i as u32;
            pass.draw(0..6, idx..idx + 1);
        }
    }
}

/// One queued image draw. Pairs an instance (rect + uv + tint)
/// with the bind group that contains its texture + sampler.
/// Borrowed lifetimes — the bind group lives in the renderer's
/// image cache and must outlive the render pass.
pub struct ImageDraw<'a> {
    pub instance: ImageInstance,
    pub bind_group: &'a wgpu::BindGroup,
}
