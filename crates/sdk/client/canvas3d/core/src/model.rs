//! Drawable 3D content: geometry ([`MeshData`]), surfaces ([`Material`],
//! [`Texture`]) and the [`Model`] that groups them — built in code or loaded
//! from glTF 2.0.
//!
//! Every heavy resource carries a process-unique `id`. Renderers key their GPU
//! uploads on it, so a model drawn every frame uploads once; cloning a
//! [`Model`] is a refcount bump that keeps the id.

use crate::color::Color;
use glam::{Mat4, Vec3};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A fresh process-unique resource id.
pub(crate) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// An axis-aligned bounding box. [`Aabb::EMPTY`] contains nothing and is the
/// identity for [`union`](Aabb::union).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: Vec3::INFINITY, max: Vec3::NEG_INFINITY };

    pub fn new(min: Vec3, max: Vec3) -> Aabb {
        Aabb { min, max }
    }

    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Aabb {
        points.into_iter().fold(Aabb::EMPTY, |b, p| b.extend(p))
    }

    pub fn is_empty(&self) -> bool {
        self.min.cmpgt(self.max).any()
    }

    pub fn extend(self, p: Vec3) -> Aabb {
        Aabb { min: self.min.min(p), max: self.max.max(p) }
    }

    pub fn union(self, other: Aabb) -> Aabb {
        Aabb { min: self.min.min(other.min), max: self.max.max(other.max) }
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    pub fn corners(&self) -> [Vec3; 8] {
        let (a, b) = (self.min, self.max);
        [
            Vec3::new(a.x, a.y, a.z),
            Vec3::new(b.x, a.y, a.z),
            Vec3::new(a.x, b.y, a.z),
            Vec3::new(b.x, b.y, a.z),
            Vec3::new(a.x, a.y, b.z),
            Vec3::new(b.x, a.y, b.z),
            Vec3::new(a.x, b.y, b.z),
            Vec3::new(b.x, b.y, b.z),
        ]
    }

    /// The box enclosing this one after `m` (its eight corners transformed).
    pub fn transformed(&self, m: Mat4) -> Aabb {
        if self.is_empty() {
            return *self;
        }
        Aabb::from_points(self.corners().into_iter().map(|c| m.transform_point3(c)))
    }
}

/// An indexed triangle list. `normals` and `uvs` are per-vertex and the same
/// length as `positions`. Winding is counter-clockwise for front faces (glTF).
#[derive(Debug)]
pub struct MeshData {
    pub id: u64,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub bounds: Aabb,
}

impl MeshData {
    /// Build a mesh. Missing `normals` are computed (area-weighted smooth),
    /// missing `uvs` are zero, missing `indices` mean "every three vertices".
    ///
    /// Panics if the per-vertex arrays disagree in length or an index is out
    /// of range — a malformed mesh is the caller's bug, and drawing it would
    /// read out of bounds on the GPU.
    pub fn new(
        positions: Vec<[f32; 3]>,
        normals: Option<Vec<[f32; 3]>>,
        uvs: Option<Vec<[f32; 2]>>,
        indices: Option<Vec<u32>>,
    ) -> MeshData {
        let n = positions.len();
        let indices = indices.unwrap_or_else(|| (0..n as u32).collect());
        assert!(indices.len() % 3 == 0, "triangle list index count {} is not a multiple of 3", indices.len());
        assert!(indices.iter().all(|&i| (i as usize) < n), "mesh index out of range");
        let normals = normals.unwrap_or_else(|| smooth_normals(&positions, &indices));
        assert_eq!(normals.len(), n, "normals length");
        let uvs = uvs.unwrap_or_else(|| vec![[0.0, 0.0]; n]);
        assert_eq!(uvs.len(), n, "uvs length");
        let bounds = Aabb::from_points(positions.iter().map(|p| Vec3::from(*p)));
        MeshData { id: next_id(), positions, normals, uvs, indices, bounds }
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Triangle `t`'s corners in mesh space.
    pub fn triangle(&self, t: usize) -> [Vec3; 3] {
        let i = &self.indices[t * 3..t * 3 + 3];
        [
            Vec3::from(self.positions[i[0] as usize]),
            Vec3::from(self.positions[i[1] as usize]),
            Vec3::from(self.positions[i[2] as usize]),
        ]
    }

    /// An axis-aligned cube of edge `size`, centred on the origin, with flat
    /// per-face normals and a full 0..1 UV square on every face.
    pub fn cube(size: f32) -> MeshData {
        let h = size * 0.5;
        // (normal, u axis, v axis) per face; corners = n·h ± u·h ± v·h.
        let faces: [(Vec3, Vec3, Vec3); 6] = [
            (Vec3::X, -Vec3::Z, Vec3::Y),
            (-Vec3::X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::X, -Vec3::Z),
            (-Vec3::Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (-Vec3::Z, -Vec3::X, Vec3::Y),
        ];
        let (mut p, mut n, mut uv, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (normal, u, v) in faces {
            let base = p.len() as u32;
            for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                p.push(((normal + u * su + v * sv) * h).to_array());
                n.push(normal.to_array());
                uv.push([(su + 1.0) * 0.5, (1.0 - sv) * 0.5]);
            }
            idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        MeshData::new(p, Some(n), Some(uv), Some(idx))
    }

    /// A `width`×`depth` rectangle in the XZ plane facing +Y, centred on the
    /// origin.
    pub fn plane(width: f32, depth: f32) -> MeshData {
        let (w, d) = (width * 0.5, depth * 0.5);
        let p = vec![[-w, 0.0, d], [w, 0.0, d], [w, 0.0, -d], [-w, 0.0, -d]];
        let n = vec![[0.0, 1.0, 0.0]; 4];
        let uv = vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        MeshData::new(p, Some(n), Some(uv), Some(vec![0, 1, 2, 0, 2, 3]))
    }

    /// A UV sphere of `radius` with `segments` around and `rings` top to
    /// bottom (each at least 3 / 2).
    pub fn uv_sphere(radius: f32, segments: u32, rings: u32) -> MeshData {
        let (segments, rings) = (segments.max(3), rings.max(2));
        let (mut p, mut n, mut uv) = (Vec::new(), Vec::new(), Vec::new());
        for r in 0..=rings {
            let v = r as f32 / rings as f32;
            let phi = v * std::f32::consts::PI;
            for s in 0..=segments {
                let u = s as f32 / segments as f32;
                let theta = u * std::f32::consts::TAU;
                let dir = Vec3::new(phi.sin() * theta.sin(), phi.cos(), phi.sin() * theta.cos());
                p.push((dir * radius).to_array());
                n.push(dir.to_array());
                uv.push([u, v]);
            }
        }
        let row = segments + 1;
        let mut idx = Vec::new();
        for r in 0..rings {
            for s in 0..segments {
                let (a, b) = (r * row + s, (r + 1) * row + s);
                idx.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        MeshData::new(p, Some(n), Some(uv), Some(idx))
    }
}

/// Area-weighted vertex normals (unnormalised face normals summed per vertex).
fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![Vec3::ZERO; positions.len()];
    for tri in indices.chunks_exact(3) {
        let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| Vec3::from(positions[i as usize]));
        let face = (b - a).cross(c - a);
        for &i in tri {
            acc[i as usize] += face;
        }
    }
    acc.into_iter().map(|n| n.normalize_or(Vec3::Y).to_array()).collect()
}

/// Decoded straight-alpha RGBA8 pixels. Which colour space they're in is the
/// material slot's business (base colour / emissive are sRGB; the rest linear).
#[derive(Debug)]
pub struct Texture {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Texture {
    /// `rgba.len()` must be `width * height * 4`.
    pub fn from_rgba8(width: u32, height: u32, rgba: Vec<u8>) -> Arc<Texture> {
        assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4, "texture byte length");
        Arc::new(Texture { id: next_id(), width, height, rgba })
    }

    /// Decode PNG or JPEG bytes.
    pub fn decode(bytes: &[u8]) -> Result<Arc<Texture>, image::ImageError> {
        let img = image::load_from_memory(bytes)?.into_rgba8();
        let (w, h) = img.dimensions();
        Ok(Texture::from_rgba8(w, h, img.into_raw()))
    }
}

/// How a material's alpha is used (glTF `alphaMode`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    Opaque,
    /// Fragments with alpha below `cutoff` are discarded; the rest are opaque.
    Mask { cutoff: f32 },
    /// Alpha-blended over what's behind.
    Blend,
}

/// A glTF metallic-roughness surface. Colour factors are **linear** (glTF's
/// convention); build from an author [`Color`] with [`Material::color`].
#[derive(Clone, Debug)]
pub struct Material {
    pub base_color: [f32; 4],
    pub base_color_texture: Option<Arc<Texture>>,
    pub metallic: f32,
    pub roughness: f32,
    /// Blue = metallic, green = roughness (glTF).
    pub metallic_roughness_texture: Option<Arc<Texture>>,
    pub normal_texture: Option<Arc<Texture>>,
    pub normal_scale: f32,
    pub occlusion_texture: Option<Arc<Texture>>,
    pub occlusion_strength: f32,
    pub emissive: [f32; 3],
    pub emissive_texture: Option<Arc<Texture>>,
    pub alpha_mode: AlphaMode,
    pub double_sided: bool,
    /// Ignore lights: the surface shows its base colour as-is.
    pub unlit: bool,
}

impl Default for Material {
    /// A light-gray dielectric — what code-built geometry gets unless it says
    /// otherwise.
    fn default() -> Self {
        Material::color(Color::rgb(0.8, 0.8, 0.8))
    }
}

impl Material {
    /// A plain dielectric surface of `color` (sRGB).
    pub fn color(color: Color) -> Material {
        Material {
            base_color: color.to_linear(),
            base_color_texture: None,
            metallic: 0.0,
            roughness: 0.6,
            metallic_roughness_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            occlusion_texture: None,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            emissive_texture: None,
            alpha_mode: if color.a < 1.0 { AlphaMode::Blend } else { AlphaMode::Opaque },
            double_sided: false,
            unlit: false,
        }
    }

    pub fn metallic(mut self, metallic: f32) -> Material {
        self.metallic = metallic.clamp(0.0, 1.0);
        self
    }

    pub fn roughness(mut self, roughness: f32) -> Material {
        self.roughness = roughness.clamp(0.0, 1.0);
        self
    }

    pub fn unlit(mut self) -> Material {
        self.unlit = true;
        self
    }

    pub fn double_sided(mut self) -> Material {
        self.double_sided = true;
        self
    }

    /// The glTF specification's default material (used for primitives that
    /// reference none): white, fully metallic, fully rough.
    pub fn gltf_default() -> Material {
        Material { base_color: [1.0; 4], metallic: 1.0, roughness: 1.0, ..Material::color(Color::WHITE) }
    }

    /// Every texture the material samples, for renderers that upload eagerly.
    pub fn textures(&self) -> impl Iterator<Item = &Arc<Texture>> {
        [
            &self.base_color_texture,
            &self.metallic_roughness_texture,
            &self.normal_texture,
            &self.occlusion_texture,
            &self.emissive_texture,
        ]
        .into_iter()
        .flatten()
    }
}

/// One drawable piece of a model: a mesh, the index of its material in
/// [`Model::materials`], and its transform relative to the model.
#[derive(Clone, Debug)]
pub struct Part {
    pub mesh: Arc<MeshData>,
    pub material: usize,
    pub transform: Mat4,
}

#[derive(Debug)]
struct ModelData {
    parts: Vec<Part>,
    materials: Vec<Material>,
    bounds: Aabb,
}

/// A drawable group of meshes and materials. Cheap to clone; clones share the
/// data and the id renderers cache uploads on.
#[derive(Clone, Debug)]
pub struct Model {
    id: u64,
    data: Arc<ModelData>,
}

/// Why a glTF couldn't be loaded.
#[derive(Debug)]
pub enum ModelError {
    Gltf(gltf::Error),
    /// The asset references an external file by URI. Load the referenced
    /// bytes yourself and use a `.glb` (or embed them as `data:` URIs).
    ExternalUri(String),
    /// A `data:` URI that isn't base64.
    BadDataUri,
    Image(image::ImageError),
    /// A primitive without `POSITION`.
    MissingPositions,
    /// An image's buffer view runs past the end of its buffer.
    BadBufferView,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::Gltf(e) => write!(f, "glTF: {e}"),
            ModelError::ExternalUri(u) => write!(f, "glTF references an external file ({u}); use a .glb"),
            ModelError::BadDataUri => f.write_str("glTF data: URI is not base64"),
            ModelError::Image(e) => write!(f, "glTF image: {e}"),
            ModelError::MissingPositions => f.write_str("glTF primitive has no POSITION attribute"),
            ModelError::BadBufferView => f.write_str("glTF image buffer view is out of range"),
        }
    }
}

impl std::error::Error for ModelError {}

impl From<gltf::Error> for ModelError {
    fn from(e: gltf::Error) -> Self {
        ModelError::Gltf(e)
    }
}

impl From<image::ImageError> for ModelError {
    fn from(e: image::ImageError) -> Self {
        ModelError::Image(e)
    }
}

impl Model {
    /// A model of one mesh with one material.
    pub fn from_mesh(mesh: MeshData, material: Material) -> Model {
        Model::from_parts(
            vec![Part { mesh: Arc::new(mesh), material: 0, transform: Mat4::IDENTITY }],
            vec![material],
        )
    }

    /// A model from explicit parts. Each part's `material` must index
    /// `materials`.
    pub fn from_parts(parts: Vec<Part>, materials: Vec<Material>) -> Model {
        assert!(
            parts.iter().all(|p| p.material < materials.len()),
            "part material index out of range"
        );
        let bounds = parts
            .iter()
            .fold(Aabb::EMPTY, |b, p| b.union(p.mesh.bounds.transformed(p.transform)));
        Model { id: next_id(), data: Arc::new(ModelData { parts, materials, bounds }) }
    }

    /// Load a glTF 2.0 asset from bytes: binary `.glb`, or `.gltf` JSON whose
    /// buffers and images are embedded `data:` URIs. Draws the asset's default
    /// scene (or its first, or every root node when it declares none) with
    /// each node's transform applied.
    ///
    /// Supported: triangle primitives with positions, optional normals,
    /// `TEXCOORD_0` and indices; metallic-roughness materials with their five
    /// textures (all read through `TEXCOORD_0`); alpha modes; double-sided;
    /// `KHR_materials_unlit`. Not loaded (in this version): skins, morph
    /// targets, animations, cameras, lights, non-triangle primitives.
    pub fn from_gltf(bytes: &[u8]) -> Result<Model, ModelError> {
        crate::gltf_load::load(bytes)
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn parts(&self) -> &[Part] {
        &self.data.parts
    }

    pub fn materials(&self) -> &[Material] {
        &self.data.materials
    }

    /// Model-space bounds of every part (with part transforms applied).
    pub fn bounds(&self) -> Aabb {
        self.data.bounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_has_six_outward_faces() {
        let m = MeshData::cube(2.0);
        assert_eq!(m.triangle_count(), 12);
        assert_eq!(m.bounds, Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0)));
        // Every triangle winds counter-clockwise seen from outside: its
        // geometric normal points away from the centre.
        for t in 0..m.triangle_count() {
            let [a, b, c] = m.triangle(t);
            let n = (b - a).cross(c - a);
            let centroid = (a + b + c) / 3.0;
            assert!(n.dot(centroid) > 0.0, "triangle {t} faces inward");
        }
    }

    #[test]
    fn plane_faces_up() {
        let m = MeshData::plane(2.0, 2.0);
        let [a, b, c] = m.triangle(0);
        assert!((b - a).cross(c - a).y > 0.0);
    }

    #[test]
    fn sphere_normals_point_outward_and_triangles_wind_outward() {
        let m = MeshData::uv_sphere(1.5, 12, 8);
        for (p, n) in m.positions.iter().zip(&m.normals) {
            assert!((Vec3::from(*p).normalize() - Vec3::from(*n)).length() < 1e-4);
        }
        for t in 0..m.triangle_count() {
            let [a, b, c] = m.triangle(t);
            let n = (b - a).cross(c - a);
            if n.length() > 1e-6 {
                assert!(n.dot((a + b + c) / 3.0) > 0.0, "triangle {t} faces inward");
            }
        }
    }

    #[test]
    fn missing_normals_are_computed_smooth() {
        let m = MeshData::new(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], None, None, None);
        for n in &m.normals {
            assert_eq!(*n, [0.0, 0.0, 1.0]);
        }
    }

    #[test]
    fn model_bounds_include_part_transforms() {
        let parts = vec![
            Part { mesh: Arc::new(MeshData::cube(1.0)), material: 0, transform: Mat4::IDENTITY },
            Part {
                mesh: Arc::new(MeshData::cube(1.0)),
                material: 0,
                transform: Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)),
            },
        ];
        let m = Model::from_parts(parts, vec![Material::default()]);
        assert_eq!(m.bounds(), Aabb::new(Vec3::new(-0.5, -0.5, -0.5), Vec3::new(5.5, 0.5, 0.5)));
    }

    #[test]
    fn clones_share_their_id_and_new_models_do_not() {
        let a = Model::from_mesh(MeshData::cube(1.0), Material::default());
        let b = Model::from_mesh(MeshData::cube(1.0), Material::default());
        assert_eq!(a.id(), a.clone().id());
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn aabb_transform_encloses_rotated_box() {
        let b = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        let r = b.transformed(Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4));
        let s2 = std::f32::consts::SQRT_2;
        assert!((r.max.x - s2).abs() < 1e-5 && (r.max.z - s2).abs() < 1e-5);
        assert!(Aabb::EMPTY.is_empty() && Aabb::EMPTY.transformed(Mat4::IDENTITY).is_empty());
    }
}
