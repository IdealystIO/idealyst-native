//! The 3D pass: draws a [`Scene3d`] into a frame-sized target on a device the
//! caller owns (so it runs on a live surface and on a headless test device
//! alike).
//!
//! # Draw order
//!
//! 1. opaque + alpha-masked parts (depth write on),
//! 2. depth-tested lines,
//! 3. alpha-blended parts, back to front (depth test on, write off),
//! 4. on-top lines.
//!
//! # Resource caching
//!
//! Meshes and textures upload once and stay resident while the author still
//! holds them: the cache keeps a `Weak` to each `Arc<MeshData>` /
//! `Arc<Texture>` and drops the GPU copy when it no longer upgrades. Material
//! bind groups are cheap and keyed by (model, material); they're dropped when
//! a frame doesn't use them.

use crate::textures;
use bytemuck::{Pod, Zeroable};
use canvas3d_core::{AlphaMode, Color, Light, LineDepth, Material, MeshData, Scene3d, Texture};
use glam::{Mat4, Vec3};
use gpu_surface::TARGET_FORMAT;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
/// Directional lights the shader evaluates (`MAX_LIGHTS` in `mesh.wgsl`).
pub(crate) const MAX_LIGHTS: usize = 4;
/// Per-object uniform stride: `min_uniform_buffer_offset_alignment` is at
/// most 256 on every backend (WebGL2 included), so one stride fits all.
const UNIFORM_STRIDE: u64 = 256;
const VERTEX_STRIDE: u64 = 32; // position(12) + normal(12) + uv(8)

/// The most samples this adapter can resolve for both the colour and depth
/// formats, among 4 and 1. Anti-aliasing quality is the same request on
/// every backend; a GPU that can't multisample these formats renders aliased
/// rather than failing.
pub fn choose_sample_count(adapter: &wgpu::Adapter) -> u32 {
    let color = adapter.get_texture_format_features(TARGET_FORMAT).flags;
    let depth = adapter.get_texture_format_features(DEPTH_FORMAT).flags;
    let x4 = wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4;
    if color.contains(x4 | wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE) && depth.contains(x4) {
        4
    } else {
        1
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniforms {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    ambient: [f32; 4],
    light_dir: [[f32; 4]; MAX_LIGHTS],
    light_color: [[f32; 4]; MAX_LIGHTS],
    counts: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ObjectUniforms {
    model: [[f32; 4]; 4],
    normal: [[f32; 4]; 4],
    base_color: [f32; 4],
    emissive: [f32; 4],
    params: [f32; 4],
    params2: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LineUniforms {
    view_proj: [[f32; 4]; 4],
    color: [f32; 4],
}

/// An sRGB author colour as the frame target stores it: encoded, premultiplied.
fn encoded_premultiplied(c: Color) -> [f32; 4] {
    [c.r * c.a, c.g * c.a, c.b * c.a, c.a]
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PipelineKey {
    blend: bool,
    cull: bool,
    /// Front faces wind clockwise (a mirroring model transform).
    cw: bool,
}

struct MeshGpu {
    source: Weak<MeshData>,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

struct TextureGpu {
    source: Weak<Texture>,
    view: wgpu::TextureView,
}

/// One part to draw this frame.
struct Draw {
    mesh: u64,
    material: (u64, usize),
    key: PipelineKey,
    /// Index into the frame's object-uniform array.
    slot: u32,
    /// Squared distance from the eye (blend sorting).
    depth: f32,
}

pub struct MeshRenderer {
    sample_count: u32,
    object_bgl: wgpu::BindGroupLayout,
    material_bgl: wgpu::BindGroupLayout,
    line_bgl: wgpu::BindGroupLayout,
    mesh_layout: wgpu::PipelineLayout,
    mesh_shader: wgpu::ShaderModule,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    line_pipelines: HashMap<LineDepthKey, wgpu::RenderPipeline>,
    line_layout: wgpu::PipelineLayout,
    line_shader: wgpu::ShaderModule,
    frame_buf: wgpu::Buffer,
    frame_bg: wgpu::BindGroup,
    object_buf: Option<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    line_uniform_buf: Option<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    line_vertex_buf: Option<(wgpu::Buffer, u64)>,
    sampler: wgpu::Sampler,
    default_white_srgb: wgpu::TextureView,
    default_white: wgpu::TextureView,
    default_normal: wgpu::TextureView,
    meshes: HashMap<u64, MeshGpu>,
    textures: HashMap<(u64, bool), TextureGpu>,
    materials: HashMap<(u64, usize), wgpu::BindGroup>,
    /// Frame-sized attachments, rebuilt when the size changes.
    attachments: Option<Attachments>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum LineDepthKey {
    Tested,
    OnTop,
}

struct Attachments {
    size: (u32, u32),
    depth: wgpu::TextureView,
    /// Multisampled colour (resolved into the frame target); `None` at 1×.
    msaa: Option<wgpu::TextureView>,
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages, dynamic: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

impl MeshRenderer {
    /// Build pipelines for `sample_count`× rendering (see
    /// [`choose_sample_count`]).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, sample_count: u32) -> MeshRenderer {
        let vs_fs = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let frame_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas3d-frame-bgl"),
            entries: &[uniform_entry(0, vs_fs, false)],
        });
        let object_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas3d-object-bgl"),
            entries: &[uniform_entry(0, vs_fs, true)],
        });
        let material_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas3d-material-bgl"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let line_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas3d-line-bgl"),
            entries: &[uniform_entry(0, vs_fs, true)],
        });
        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas3d-mesh-pl"),
            bind_group_layouts: &[Some(&frame_bgl), Some(&object_bgl), Some(&material_bgl)],
            immediate_size: 0,
        });
        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas3d-line-pl"),
            bind_group_layouts: &[Some(&line_bgl)],
            immediate_size: 0,
        });
        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("canvas3d-mesh"),
            source: wgpu::ShaderSource::Wgsl(MESH_WGSL.into()),
        });
        let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("canvas3d-line"),
            source: wgpu::ShaderSource::Wgsl(LINE_WGSL.into()),
        });
        let frame_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("canvas3d-frame-uniforms"),
            size: std::mem::size_of::<FrameUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas3d-frame-bg"),
            layout: &frame_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buf.as_entire_binding() }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("canvas3d-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        MeshRenderer {
            sample_count,
            default_white_srgb: textures::solid(device, queue, [255; 4], true),
            default_white: textures::solid(device, queue, [255; 4], false),
            default_normal: textures::solid(device, queue, [128, 128, 255, 255], false),
            object_bgl,
            material_bgl,
            line_bgl,
            mesh_layout,
            mesh_shader,
            pipelines: HashMap::new(),
            line_pipelines: HashMap::new(),
            line_layout,
            line_shader,
            frame_buf,
            frame_bg,
            object_buf: None,
            line_uniform_buf: None,
            line_vertex_buf: None,
            sampler,
            meshes: HashMap::new(),
            textures: HashMap::new(),
            materials: HashMap::new(),
            attachments: None,
        }
    }

    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    /// Meshes currently resident on the GPU.
    #[cfg(test)]
    pub(crate) fn cached_meshes(&self) -> usize {
        self.meshes.len()
    }

    /// Drop frame-sized attachments (after a resize; rebuilt on demand).
    pub fn resized(&mut self) {
        self.attachments = None;
    }

    fn depth_stencil(write: bool, compare: wgpu::CompareFunction) -> Option<wgpu::DepthStencilState> {
        Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        })
    }

    fn pipeline(&mut self, device: &wgpu::Device, key: PipelineKey) -> &wgpu::RenderPipeline {
        let sample_count = self.sample_count;
        let (layout, shader) = (&self.mesh_layout, &self.mesh_shader);
        self.pipelines.entry(key).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("canvas3d-mesh-pipeline"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: VERTEX_STRIDE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TARGET_FORMAT,
                        blend: key.blend.then_some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: if key.cw { wgpu::FrontFace::Cw } else { wgpu::FrontFace::Ccw },
                    cull_mode: key.cull.then_some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: if key.blend {
                    Self::depth_stencil(false, wgpu::CompareFunction::LessEqual)
                } else {
                    Self::depth_stencil(true, wgpu::CompareFunction::Less)
                },
                multisample: wgpu::MultisampleState { count: sample_count, ..Default::default() },
                multiview_mask: None,
                cache: None,
            })
        })
    }

    fn line_pipeline(&mut self, device: &wgpu::Device, key: LineDepthKey) -> &wgpu::RenderPipeline {
        let sample_count = self.sample_count;
        let (layout, shader) = (&self.line_layout, &self.line_shader);
        self.line_pipelines.entry(key).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("canvas3d-line-pipeline"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TARGET_FORMAT,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    ..Default::default()
                },
                depth_stencil: Self::depth_stencil(
                    false,
                    match key {
                        LineDepthKey::Tested => wgpu::CompareFunction::LessEqual,
                        LineDepthKey::OnTop => wgpu::CompareFunction::Always,
                    },
                ),
                multisample: wgpu::MultisampleState { count: sample_count, ..Default::default() },
                multiview_mask: None,
                cache: None,
            })
        })
    }

    fn attachments(&mut self, device: &wgpu::Device, size: (u32, u32)) -> &Attachments {
        if self.attachments.as_ref().is_none_or(|a| a.size != size) {
            let extent = wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 };
            let make = |format, label| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: extent,
                        mip_level_count: 1,
                        sample_count: self.sample_count,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            };
            self.attachments = Some(Attachments {
                size,
                depth: make(DEPTH_FORMAT, "canvas3d-depth"),
                msaa: (self.sample_count > 1).then(|| make(TARGET_FORMAT, "canvas3d-msaa")),
            });
        }
        self.attachments.as_ref().expect("just built")
    }

    fn mesh_gpu(&mut self, device: &wgpu::Device, mesh: &Arc<MeshData>) {
        if self.meshes.contains_key(&mesh.id) {
            return;
        }
        let mut verts: Vec<f32> = Vec::with_capacity(mesh.positions.len() * 8);
        for i in 0..mesh.positions.len() {
            verts.extend_from_slice(&mesh.positions[i]);
            verts.extend_from_slice(&mesh.normals[i]);
            verts.extend_from_slice(&mesh.uvs[i]);
        }
        let vertices = buffer_init(device, "canvas3d-vertices", bytemuck::cast_slice(&verts), wgpu::BufferUsages::VERTEX);
        let indices = buffer_init(device, "canvas3d-indices", bytemuck::cast_slice(&mesh.indices), wgpu::BufferUsages::INDEX);
        self.meshes.insert(
            mesh.id,
            MeshGpu { source: Arc::downgrade(mesh), vertices, indices, index_count: mesh.indices.len() as u32 },
        );
    }

    fn texture_view(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, tex: &Arc<Texture>, srgb: bool) -> wgpu::TextureView {
        self.textures
            .entry((tex.id, srgb))
            .or_insert_with(|| TextureGpu {
                source: Arc::downgrade(tex),
                view: textures::upload(device, queue, tex, srgb),
            })
            .view
            .clone()
    }

    fn material_bind_group(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: (u64, usize),
        material: &Material,
    ) {
        if self.materials.contains_key(&key) {
            return;
        }
        let (white_srgb, white, flat) =
            (self.default_white_srgb.clone(), self.default_white.clone(), self.default_normal.clone());
        let mut slot = |tex: &Option<Arc<Texture>>, srgb: bool, default: &wgpu::TextureView| match tex {
            Some(t) => self.texture_view(device, queue, t, srgb),
            None => default.clone(),
        };
        let views = [
            slot(&material.base_color_texture, true, &white_srgb),
            slot(&material.metallic_roughness_texture, false, &white),
            slot(&material.normal_texture, false, &flat),
            slot(&material.occlusion_texture, false, &white),
            slot(&material.emissive_texture, true, &white_srgb),
        ];
        let entries: Vec<wgpu::BindGroupEntry> = views
            .iter()
            .enumerate()
            .map(|(i, v)| wgpu::BindGroupEntry { binding: i as u32, resource: wgpu::BindingResource::TextureView(v) })
            .chain(std::iter::once(wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            }))
            .collect();
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas3d-material-bg"),
            layout: &self.material_bgl,
            entries: &entries,
        });
        self.materials.insert(key, bg);
    }

    /// A uniform buffer of `count` 256-byte slots + its dynamic-offset bind
    /// group, grown (doubling) as needed.
    fn ensure_slots(
        device: &wgpu::Device,
        slot: &mut Option<(wgpu::Buffer, wgpu::BindGroup, u64)>,
        layout: &wgpu::BindGroupLayout,
        count: usize,
        binding_size: u64,
        label: &str,
    ) {
        let need = (count.max(1) as u64) * UNIFORM_STRIDE;
        if slot.as_ref().is_some_and(|(_, _, cap)| *cap >= need) {
            return;
        }
        let cap = need.next_power_of_two();
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: cap,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(binding_size),
                }),
            }],
        });
        *slot = Some((buf, bg, cap));
    }

    /// Record the 3D pass for `scene` into `encoder`, rendering into
    /// `target` (a `size`-sized [`TARGET_FORMAT`] view). Uniform and buffer
    /// writes go through `queue` and land before the encoder's submission.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene3d,
        target: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        let aspect_size = (size.0.max(1) as f32, size.1.max(1) as f32);
        let camera = *scene.get_camera();
        let view_proj = camera.view_proj(aspect_size);

        // --- frame uniforms ---
        let (ambient, ambient_intensity) = scene.ambient_light();
        let a = ambient.to_linear();
        let mut frame = FrameUniforms {
            view_proj: view_proj.to_cols_array_2d(),
            camera_pos: camera.eye.extend(1.0).to_array(),
            ambient: [a[0] * ambient_intensity, a[1] * ambient_intensity, a[2] * ambient_intensity, 0.0],
            light_dir: [[0.0; 4]; MAX_LIGHTS],
            light_color: [[0.0; 4]; MAX_LIGHTS],
            counts: [0; 4],
        };
        let mut n = 0usize;
        for light in scene.get_lights() {
            if n == MAX_LIGHTS {
                break;
            }
            let Light::Directional { direction, color, intensity } = *light else { continue };
            let c = color.to_linear();
            frame.light_dir[n] = direction.normalize_or(Vec3::NEG_Y).extend(0.0).to_array();
            frame.light_color[n] = [c[0] * intensity, c[1] * intensity, c[2] * intensity, 0.0];
            n += 1;
        }
        frame.counts[0] = n as u32;
        queue.write_buffer(&self.frame_buf, 0, bytemuck::bytes_of(&frame));

        // --- gather draws + object uniforms ---
        let mut draws: Vec<Draw> = Vec::new();
        let mut objects: Vec<ObjectUniforms> = Vec::new();
        let mut used_materials: HashSet<(u64, usize)> = HashSet::new();
        for item in scene.models() {
            for part in item.model.parts() {
                let model = item.transform * part.transform;
                let det = model.determinant();
                if det.abs() < f32::EPSILON || part.mesh.indices.is_empty() {
                    continue; // collapsed or empty: nothing to draw
                }
                let material = &item.model.materials()[part.material];
                let mat_key = (item.model.id(), part.material);
                self.mesh_gpu(device, &part.mesh);
                self.material_bind_group(device, queue, mat_key, material);
                used_materials.insert(mat_key);
                let blend = material.alpha_mode == AlphaMode::Blend;
                let center = model.transform_point3(part.mesh.bounds.center());
                draws.push(Draw {
                    mesh: part.mesh.id,
                    material: mat_key,
                    key: PipelineKey { blend, cull: !material.double_sided, cw: det < 0.0 },
                    slot: objects.len() as u32,
                    depth: center.distance_squared(camera.eye),
                });
                objects.push(object_uniforms(model, material));
            }
        }
        Self::ensure_slots(
            device,
            &mut self.object_buf,
            &self.object_bgl,
            objects.len(),
            std::mem::size_of::<ObjectUniforms>() as u64,
            "canvas3d-object-uniforms",
        );
        if let Some((buf, _, _)) = &self.object_buf {
            let mut bytes = vec![0u8; objects.len() * UNIFORM_STRIDE as usize];
            for (i, o) in objects.iter().enumerate() {
                let at = i * UNIFORM_STRIDE as usize;
                bytes[at..at + std::mem::size_of::<ObjectUniforms>()].copy_from_slice(bytemuck::bytes_of(o));
            }
            if !bytes.is_empty() {
                queue.write_buffer(buf, 0, &bytes);
            }
        }

        // --- lines: one vertex buffer, one uniform slot per batch ---
        let batches = scene.line_batches();
        let mut line_verts: Vec<[f32; 3]> = Vec::new();
        let mut line_ranges: Vec<(u32, u32, LineDepthKey)> = Vec::new();
        let mut line_uniforms: Vec<LineUniforms> = Vec::new();
        for b in batches {
            let start = line_verts.len() as u32;
            for [p, q] in b.lines.segments() {
                line_verts.push(p.to_array());
                line_verts.push(q.to_array());
            }
            let end = line_verts.len() as u32;
            if end > start {
                let key = match b.depth {
                    LineDepth::Tested => LineDepthKey::Tested,
                    LineDepth::OnTop => LineDepthKey::OnTop,
                };
                line_ranges.push((start, end, key));
                line_uniforms.push(LineUniforms {
                    view_proj: view_proj.to_cols_array_2d(),
                    color: encoded_premultiplied(b.color),
                });
            }
        }
        if !line_ranges.is_empty() {
            Self::ensure_slots(
                device,
                &mut self.line_uniform_buf,
                &self.line_bgl,
                line_uniforms.len(),
                std::mem::size_of::<LineUniforms>() as u64,
                "canvas3d-line-uniforms",
            );
            let mut bytes = vec![0u8; line_uniforms.len() * UNIFORM_STRIDE as usize];
            for (i, u) in line_uniforms.iter().enumerate() {
                let at = i * UNIFORM_STRIDE as usize;
                bytes[at..at + std::mem::size_of::<LineUniforms>()].copy_from_slice(bytemuck::bytes_of(u));
            }
            queue.write_buffer(&self.line_uniform_buf.as_ref().expect("ensured").0, 0, &bytes);
            let need = (line_verts.len() * 12) as u64;
            if self.line_vertex_buf.as_ref().is_none_or(|(_, cap)| *cap < need) {
                let cap = need.next_power_of_two();
                let buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("canvas3d-line-vertices"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.line_vertex_buf = Some((buf, cap));
            }
            queue.write_buffer(&self.line_vertex_buf.as_ref().expect("ensured").0, 0, bytemuck::cast_slice(&line_verts));
        }

        // Pipelines needed this frame (created before the pass borrows self).
        for d in &draws {
            self.pipeline(device, d.key);
        }
        for (_, _, key) in &line_ranges {
            self.line_pipeline(device, *key);
        }
        // Drop cached GPU copies of resources the author released, and
        // material bind groups this frame didn't use.
        self.meshes.retain(|_, m| m.source.strong_count() > 0);
        self.textures.retain(|_, t| t.source.strong_count() > 0);
        self.materials.retain(|k, _| used_materials.contains(k));

        // Opaque first, blended back to front.
        let (mut opaque, mut blended): (Vec<&Draw>, Vec<&Draw>) = draws.iter().partition(|d| !d.key.blend);
        opaque.sort_by_key(|d| (d.key.cull, d.key.cw)); // fewer pipeline switches
        blended.sort_by(|a, b| b.depth.total_cmp(&a.depth));

        let clear = encoded_premultiplied(scene.clear_color());
        self.attachments(device, size);
        let att = self.attachments.as_ref().expect("built");
        let (color_view, resolve) = match &att.msaa {
            Some(msaa) => (msaa, Some(target)),
            None => (target, None),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("canvas3d-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0] as f64,
                        g: clear[1] as f64,
                        b: clear[2] as f64,
                        a: clear[3] as f64,
                    }),
                    // The multisampled buffer is only needed until it resolves.
                    store: if resolve.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &att.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.frame_bg, &[]);

        let draw_parts = |pass: &mut wgpu::RenderPass, list: &[&Draw]| {
            let Some((_, object_bg, _)) = &self.object_buf else { return };
            for d in list {
                let mesh = &self.meshes[&d.mesh];
                pass.set_pipeline(&self.pipelines[&d.key]);
                pass.set_bind_group(1, object_bg, &[d.slot * UNIFORM_STRIDE as u32]);
                pass.set_bind_group(2, &self.materials[&d.material], &[]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        };
        let draw_lines = |pass: &mut wgpu::RenderPass, which: LineDepthKey| {
            let (Some((_, line_bg, _)), Some((vbuf, _))) = (&self.line_uniform_buf, &self.line_vertex_buf) else {
                return;
            };
            for (i, (start, end, key)) in line_ranges.iter().enumerate() {
                if *key != which {
                    continue;
                }
                pass.set_pipeline(&self.line_pipelines[key]);
                pass.set_bind_group(0, line_bg, &[i as u32 * UNIFORM_STRIDE as u32]);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.draw(*start..*end, 0..1);
            }
        };

        draw_parts(&mut pass, &opaque);
        draw_lines(&mut pass, LineDepthKey::Tested);
        // Lines rebind group 0; restore the frame uniforms for meshes.
        pass.set_bind_group(0, &self.frame_bg, &[]);
        draw_parts(&mut pass, &blended);
        draw_lines(&mut pass, LineDepthKey::OnTop);
    }
}

fn object_uniforms(model: Mat4, m: &Material) -> ObjectUniforms {
    let (cutoff, mode) = match m.alpha_mode {
        AlphaMode::Opaque => (0.0, 0.0),
        AlphaMode::Mask { cutoff } => (cutoff, 1.0),
        AlphaMode::Blend => (0.0, 2.0),
    };
    ObjectUniforms {
        model: model.to_cols_array_2d(),
        normal: model.inverse().transpose().to_cols_array_2d(),
        base_color: m.base_color,
        emissive: [m.emissive[0], m.emissive[1], m.emissive[2], 0.0],
        params: [m.metallic, m.roughness, m.normal_scale, m.occlusion_strength],
        params2: [
            cutoff,
            mode,
            if m.unlit { 1.0 } else { 0.0 },
            if m.normal_texture.is_some() { 1.0 } else { 0.0 },
        ],
    }
}

fn buffer_init(device: &wgpu::Device, label: &str, bytes: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    // A zero-length buffer is invalid; empty meshes are skipped before here,
    // but keep the call total.
    let padded;
    let contents = if bytes.is_empty() {
        padded = [0u8; 4];
        &padded[..]
    } else {
        bytes
    };
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
}

pub(crate) const MESH_WGSL: &str = include_str!("mesh.wgsl");
pub(crate) const LINE_WGSL: &str = include_str!("line.wgsl");
