//! glTF 2.0 → [`Model`]. See [`Model::from_gltf`] for what's supported.

use crate::anim::{Animation, Channel, ChannelValues, Interpolation, Node, Skin, SkinWeights, Transform};
use crate::model::{AlphaMode, Material, MeshData, Model, ModelDesc, ModelError, Part, Texture};
use base64::Engine as _;
use glam::{Mat4, Quat, Vec3};
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) fn load(bytes: &[u8]) -> Result<Model, ModelError> {
    let gltf = gltf::Gltf::from_slice(bytes)?;
    let buffers = gltf
        .buffers()
        .map(|b| match b.source() {
            gltf::buffer::Source::Bin => Ok(gltf.blob.clone().unwrap_or_default()),
            gltf::buffer::Source::Uri(uri) => data_uri(uri),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let read = |b: gltf::Buffer| buffers.get(b.index()).map(|v| v.as_slice());

    let mut loader = Loader { buffers: &buffers, images: HashMap::new() };

    let mut materials = gltf
        .materials()
        .map(|m| loader.material(&m))
        .collect::<Result<Vec<_>, _>>()?;
    // Primitives without a material use the spec default, appended once.
    let default_material = materials.len();
    materials.push(Material::gltf_default());

    // Every node, with its parent, so animations and skins can address any
    // of them by glTF index.
    let mut nodes: Vec<Node> = gltf
        .nodes()
        .map(|n| {
            let (t, r, s) = n.transform().decomposed();
            Node {
                name: n.name().map(str::to_owned),
                parent: None,
                rest: Transform::new(Vec3::from(t), Quat::from_array(r), Vec3::from(s)),
            }
        })
        .collect();
    for n in gltf.nodes() {
        for c in n.children() {
            nodes[c.index()].parent = Some(n.index());
        }
    }

    let skins = gltf
        .skins()
        .map(|skin| {
            let joints: Vec<usize> = skin.joints().map(|j| j.index()).collect();
            // Absent inverse-bind matrices are identities (glTF 2.0 §5.28).
            let inverse_bind = match skin.reader(read).read_inverse_bind_matrices() {
                Some(m) => m.map(|c| Mat4::from_cols_array_2d(&c)).collect(),
                None => vec![Mat4::IDENTITY; joints.len()],
            };
            Skin { joints, inverse_bind }
        })
        .collect();

    let mut meshes: HashMap<(usize, usize), Arc<MeshData>> = HashMap::new();
    let mut parts = Vec::new();
    let roots: Vec<gltf::Node> = match gltf.default_scene().or_else(|| gltf.scenes().next()) {
        Some(scene) => scene.nodes().collect(),
        // No scenes declared: every node no other node lists as a child.
        None => gltf.nodes().filter(|n| nodes[n.index()].parent.is_none()).collect(),
    };
    let mut stack: Vec<gltf::Node> = roots;
    while let Some(node) = stack.pop() {
        if let Some(mesh) = node.mesh() {
            for prim in mesh.primitives() {
                if prim.mode() != gltf::mesh::Mode::Triangles {
                    // Lines/points/strips aren't drawn in this version.
                    continue;
                }
                let key = (mesh.index(), prim.index());
                let data = match meshes.get(&key) {
                    Some(d) => d.clone(),
                    None => {
                        let d = Arc::new(loader.primitive(&prim)?);
                        meshes.insert(key, d.clone());
                        d
                    }
                };
                let material = prim.material().index().unwrap_or(default_material);
                let part = Part::new(data, material, Mat4::IDENTITY).on_node(node.index());
                parts.push(match node.skin() {
                    Some(skin) => part.skinned(skin.index()),
                    None => part,
                });
            }
        }
        stack.extend(node.children());
    }

    let animations = gltf
        .animations()
        .map(|a| {
            let channels = a.channels().filter_map(|c| channel(&c, read)).collect::<Vec<_>>();
            if let Some(problem) = Animation::problem(&channels) {
                return Err(ModelError::Invalid(problem));
            }
            Ok(Animation::new_unchecked(a.name().map(str::to_owned), channels))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Model::try_from_desc(ModelDesc { parts, materials, nodes, skins, animations }).map_err(ModelError::Invalid)
}

/// One animation channel, or `None` for what this version doesn't animate
/// (morph-target weights) and for channels whose accessors are missing.
fn channel<'a, 's, F>(c: &gltf::animation::Channel<'a>, read: F) -> Option<Channel>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    use gltf::animation::util::ReadOutputs;
    let reader = c.reader(read);
    let times: Vec<f32> = reader.read_inputs()?.collect();
    let values = match reader.read_outputs()? {
        ReadOutputs::Translations(v) => ChannelValues::Translation(v.map(Vec3::from).collect()),
        ReadOutputs::Scales(v) => ChannelValues::Scale(v.map(Vec3::from).collect()),
        ReadOutputs::Rotations(v) => ChannelValues::Rotation(v.into_f32().map(Quat::from_array).collect()),
        ReadOutputs::MorphTargetWeights(_) => return None,
    };
    let interpolation = match c.sampler().interpolation() {
        gltf::animation::Interpolation::Step => Interpolation::Step,
        gltf::animation::Interpolation::Linear => Interpolation::Linear,
        gltf::animation::Interpolation::CubicSpline => Interpolation::CubicSpline,
    };
    Some(Channel { node: c.target().node().index(), interpolation, times, values })
}

/// Decode a `data:[mime];base64,<payload>` URI. Anything else is an external
/// reference this loader doesn't follow.
fn data_uri(uri: &str) -> Result<Vec<u8>, ModelError> {
    let Some(rest) = uri.strip_prefix("data:") else {
        return Err(ModelError::ExternalUri(uri.to_string()));
    };
    let (meta, payload) = rest.split_once(',').ok_or(ModelError::BadDataUri)?;
    if !meta.ends_with(";base64") {
        return Err(ModelError::BadDataUri);
    }
    base64::engine::general_purpose::STANDARD.decode(payload).map_err(|_| ModelError::BadDataUri)
}

struct Loader<'a> {
    buffers: &'a [Vec<u8>],
    /// Decoded images by glTF image index (a texture may be shared by many
    /// materials; decode it once).
    images: HashMap<usize, Arc<Texture>>,
}

impl Loader<'_> {
    fn image(&mut self, image: gltf::Image) -> Result<Arc<Texture>, ModelError> {
        if let Some(t) = self.images.get(&image.index()) {
            return Ok(t.clone());
        }
        let bytes: Vec<u8> = match image.source() {
            gltf::image::Source::View { view, .. } => {
                let buf = &self.buffers[view.buffer().index()];
                buf.get(view.offset()..view.offset() + view.length())
                    .ok_or(ModelError::BadBufferView)?
                    .to_vec()
            }
            gltf::image::Source::Uri { uri, .. } => data_uri(uri)?,
        };
        let tex = Texture::decode(&bytes)?;
        self.images.insert(image.index(), tex.clone());
        Ok(tex)
    }

    fn texture(&mut self, tex: gltf::Texture) -> Result<Arc<Texture>, ModelError> {
        self.image(tex.source())
    }

    fn material(&mut self, m: &gltf::Material) -> Result<Material, ModelError> {
        let pbr = m.pbr_metallic_roughness();
        let base_color_texture = pbr.base_color_texture().map(|i| self.texture(i.texture())).transpose()?;
        let metallic_roughness_texture =
            pbr.metallic_roughness_texture().map(|i| self.texture(i.texture())).transpose()?;
        let (normal_texture, normal_scale) = match m.normal_texture() {
            Some(n) => (Some(self.texture(n.texture())?), n.scale()),
            None => (None, 1.0),
        };
        let (occlusion_texture, occlusion_strength) = match m.occlusion_texture() {
            Some(o) => (Some(self.texture(o.texture())?), o.strength()),
            None => (None, 1.0),
        };
        let emissive_texture = m.emissive_texture().map(|i| self.texture(i.texture())).transpose()?;
        Ok(Material {
            base_color: pbr.base_color_factor(),
            base_color_texture,
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            metallic_roughness_texture,
            normal_texture,
            normal_scale,
            occlusion_texture,
            occlusion_strength,
            emissive: m.emissive_factor(),
            emissive_texture,
            alpha_mode: match m.alpha_mode() {
                gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                gltf::material::AlphaMode::Mask => AlphaMode::Mask { cutoff: m.alpha_cutoff().unwrap_or(0.5) },
                gltf::material::AlphaMode::Blend => AlphaMode::Blend,
            },
            double_sided: m.double_sided(),
            unlit: m.unlit(),
        })
    }

    fn primitive(&self, prim: &gltf::Primitive) -> Result<MeshData, ModelError> {
        let reader = prim.reader(|b| self.buffers.get(b.index()).map(|v| v.as_slice()));
        let positions: Vec<[f32; 3]> = reader.read_positions().ok_or(ModelError::MissingPositions)?.collect();
        let normals = reader.read_normals().map(|n| n.collect());
        let uvs = reader.read_tex_coords(0).map(|t| t.into_f32().collect());
        let indices = reader.read_indices().map(|i| i.into_u32().collect());
        let mesh = MeshData::new(positions, normals, uvs, indices);
        let skin = reader.read_joints(0).zip(reader.read_weights(0)).map(|(j, w)| SkinWeights {
            joints: j.into_u16().collect(),
            weights: w.into_f32().collect(),
        });
        Ok(match skin {
            Some(s) if s.joints.len() == mesh.positions.len() && s.weights.len() == mesh.positions.len() => {
                mesh.with_skin(s)
            }
            Some(_) => return Err(ModelError::Invalid("skin influences don't match the vertex count")),
            None => mesh,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use glam::{Mat4, Vec3};

    /// Build a minimal binary glTF (`.glb`) in memory: one triangle (positions
    /// + indices), one material, one node translated by `offset`. Lets the
    /// loader be tested without shipping a fixture file.
    pub(crate) fn tiny_glb(offset: [f32; 3], base_color: [f32; 4]) -> Vec<u8> {
        let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let indices: [u16; 3] = [0, 1, 2];
        let mut bin = Vec::new();
        for p in positions {
            for c in p {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let idx_offset = bin.len();
        for i in indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let json = format!(
            r#"{{
  "asset": {{"version": "2.0"}},
  "scene": 0,
  "scenes": [{{"nodes": [0]}}],
  "nodes": [{{"mesh": 0, "translation": [{}, {}, {}]}}],
  "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0}}, "indices": 1, "material": 0}}]}}],
  "materials": [{{"pbrMetallicRoughness": {{"baseColorFactor": [{}, {}, {}, {}], "metallicFactor": 0.0}}, "doubleSided": true}}],
  "buffers": [{{"byteLength": {}}}],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": 36}},
    {{"buffer": 0, "byteOffset": {}, "byteLength": 6}}
  ],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,0]}},
    {{"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"}}
  ]
}}"#,
            offset[0], offset[1], offset[2],
            base_color[0], base_color[1], base_color[2], base_color[3],
            bin.len(),
            idx_offset
        );
        let mut json = json.into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut glb = Vec::with_capacity(total);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bin);
        glb
    }

    #[test]
    fn loads_a_glb_with_node_transform_and_material() {
        let model = Model::from_gltf(&tiny_glb([2.0, 0.0, -1.0], [0.25, 0.5, 1.0, 1.0])).unwrap();
        assert_eq!(model.parts().len(), 1);
        let part = &model.parts()[0];
        assert_eq!(part.mesh.triangle_count(), 1);
        // Normals weren't in the file: computed, facing +Z for this winding.
        assert_eq!(part.mesh.normals[0], [0.0, 0.0, 1.0]);
        // The node's translation is the part transform and is in the bounds.
        assert_eq!(part.transform.transform_point3(Vec3::ZERO), Vec3::new(2.0, 0.0, -1.0));
        assert_eq!(model.bounds().min, Vec3::new(2.0, 0.0, -1.0));
        assert_eq!(model.bounds().max, Vec3::new(3.0, 1.0, -1.0));
        let mat = &model.materials()[part.material];
        assert_eq!(mat.base_color, [0.25, 0.5, 1.0, 1.0], "glTF factors are linear, kept as-is");
        assert_eq!(mat.metallic, 0.0);
        assert!(mat.double_sided);
    }

    /// Accessor-by-accessor GLB builder for tests that need more than one
    /// triangle: each `add` appends a tightly packed buffer view + accessor
    /// and returns the accessor index for the JSON.
    #[derive(Default)]
    pub(crate) struct Glb {
        bin: Vec<u8>,
        views: Vec<String>,
        accessors: Vec<String>,
    }

    impl Glb {
        /// `component`: 5126 f32, 5123 u16. `ty`: "SCALAR", "VEC3", "VEC4", "MAT4".
        pub(crate) fn add(&mut self, bytes: &[u8], count: usize, component: u32, ty: &str, extra: &str) -> usize {
            while self.bin.len() % 4 != 0 {
                self.bin.push(0);
            }
            let offset = self.bin.len();
            self.bin.extend_from_slice(bytes);
            self.views.push(format!(r#"{{"buffer":0,"byteOffset":{offset},"byteLength":{}}}"#, bytes.len()));
            let view = self.views.len() - 1;
            self.accessors.push(format!(
                r#"{{"bufferView":{view},"componentType":{component},"count":{count},"type":"{ty}"{extra}}}"#
            ));
            self.accessors.len() - 1
        }

        pub(crate) fn f32s(&mut self, v: &[f32], count: usize, ty: &str, extra: &str) -> usize {
            let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            self.add(&bytes, count, 5126, ty, extra)
        }

        pub(crate) fn u16s(&mut self, v: &[u16], count: usize, ty: &str) -> usize {
            let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            self.add(&bytes, count, 5123, ty, "")
        }

        /// Assemble with `body` — the JSON members after `asset`, `buffers`,
        /// `bufferViews` and `accessors` (which the builder writes).
        pub(crate) fn finish(mut self, body: &str) -> Vec<u8> {
            while self.bin.len() % 4 != 0 {
                self.bin.push(0);
            }
            let mut json = format!(
                r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"byteLength":{}}}],"bufferViews":[{}],"accessors":[{}],{body}}}"#,
                self.bin.len(),
                self.views.join(","),
                self.accessors.join(",")
            )
            .into_bytes();
            while json.len() % 4 != 0 {
                json.push(b' ');
            }
            let total = 12 + 8 + json.len() + 8 + self.bin.len();
            let mut glb = Vec::with_capacity(total);
            glb.extend_from_slice(b"glTF");
            glb.extend_from_slice(&2u32.to_le_bytes());
            glb.extend_from_slice(&(total as u32).to_le_bytes());
            glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
            glb.extend_from_slice(b"JSON");
            glb.extend_from_slice(&json);
            glb.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
            glb.extend_from_slice(b"BIN\0");
            glb.extend_from_slice(&self.bin);
            glb
        }
    }

    /// A skinned, animated arm: joints `root` (node 0) and `elbow` (node 1,
    /// one unit up), a mesh node (2) skinned to them, and a clip "Bend" that
    /// rotates the elbow 90° about Z over one second (linear).
    pub(crate) fn skinned_arm_glb() -> Vec<u8> {
        let mut g = Glb::default();
        let pos = g.f32s(
            &[-0.25, 0.0, 0.0, 0.25, 0.0, 0.0, 0.0, 2.0, 0.0],
            3,
            "VEC3",
            r#","min":[-0.25,0,0],"max":[0.25,2,0]"#,
        );
        let idx = g.u16s(&[0, 1, 2], 3, "SCALAR");
        let joints = g.u16s(&[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0], 3, "VEC4");
        let weights = g.f32s(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0], 3, "VEC4", "");
        let mut ibm = Mat4::IDENTITY.to_cols_array().to_vec();
        ibm.extend(Mat4::from_translation(-Vec3::Y).to_cols_array());
        let ibm = g.f32s(&ibm, 2, "MAT4", "");
        let times = g.f32s(&[0.0, 1.0], 2, "SCALAR", r#","min":[0],"max":[1]"#);
        let q = glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let rots = g.f32s(&[0.0, 0.0, 0.0, 1.0, q.x, q.y, q.z, q.w], 2, "VEC4", "");
        g.finish(&format!(
            r#""scene":0,"scenes":[{{"nodes":[0,2]}}],
"nodes":[{{"name":"root","children":[1]}},{{"name":"elbow","translation":[0,1,0]}},{{"name":"arm","mesh":0,"skin":0}}],
"meshes":[{{"primitives":[{{"attributes":{{"POSITION":{pos},"JOINTS_0":{joints},"WEIGHTS_0":{weights}}},"indices":{idx}}}]}}],
"skins":[{{"joints":[0,1],"inverseBindMatrices":{ibm}}}],
"animations":[{{"name":"Bend","samplers":[{{"input":{times},"output":{rots},"interpolation":"LINEAR"}}],"channels":[{{"sampler":0,"target":{{"node":1,"path":"rotation"}}}}]}}]"#
        ))
    }

    #[test]
    fn loads_skin_hierarchy_and_animation_and_plays_them() {
        let model = Model::from_gltf(&skinned_arm_glb()).unwrap();
        assert_eq!(model.nodes().len(), 3);
        assert_eq!(model.nodes()[1].parent, Some(0));
        assert_eq!(model.node("elbow"), Some(1));
        assert_eq!(model.skins()[0].joints, vec![0, 1]);
        let part = &model.parts()[0];
        assert_eq!((part.node, part.skin), (Some(2), Some(0)));
        assert!(part.mesh.skin.is_some());

        let bend = model.animation("Bend").expect("named clip");
        assert_eq!(bend.duration(), 1.0);
        let at = |t: f32| {
            let pose = crate::Pose::rest(&model).sampled(bend, t);
            let globals = model.globals(Some(&pose));
            crate::skinned_positions(&part.mesh, &model.joint_matrices(0, &globals))[2]
        };
        assert!((at(0.0) - Vec3::new(0.0, 2.0, 0.0)).length() < 1e-4, "rest: tip straight up");
        assert!((at(1.0) - Vec3::new(-1.0, 1.0, 0.0)).length() < 1e-4, "bent: tip swung to −X");
        let half = at(0.5);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert!((half - Vec3::new(-s, 1.0 + s, 0.0)).length() < 1e-4, "halfway along the arc: {half}");
        // The rest bounds were measured skinned.
        assert!((model.bounds().max.y - 2.0).abs() < 1e-4);
    }

    #[test]
    fn a_channel_whose_values_dont_match_its_keyframes_is_an_error() {
        let mut g = Glb::default();
        let times = g.f32s(&[0.0, 1.0], 2, "SCALAR", r#","min":[0],"max":[1]"#);
        let one = g.f32s(&[0.0, 0.0, 0.0], 1, "VEC3", "");
        let bytes = g.finish(&format!(
            r#""nodes":[{{}}],"animations":[{{"samplers":[{{"input":{times},"output":{one}}}],"channels":[{{"sampler":0,"target":{{"node":0,"path":"translation"}}}}]}}]"#
        ));
        assert!(matches!(Model::from_gltf(&bytes), Err(ModelError::Invalid(_))));
    }

    #[test]
    fn external_uris_are_reported_not_fetched() {
        let json = br#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4,"uri":"mesh.bin"}]}"#;
        match Model::from_gltf(json) {
            Err(ModelError::ExternalUri(u)) => assert_eq!(u, "mesh.bin"),
            other => panic!("expected ExternalUri, got {other:?}"),
        }
    }

    #[test]
    fn data_uri_buffers_load() {
        assert_eq!(data_uri("data:application/octet-stream;base64,AQID").unwrap(), vec![1, 2, 3]);
        assert!(matches!(data_uri("data:text/plain,abc"), Err(ModelError::BadDataUri)));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(Model::from_gltf(b"not a gltf").is_err());
    }
}
