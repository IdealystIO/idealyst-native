//! glTF 2.0 → [`Model`]. See [`Model::from_gltf`] for what's supported.

use crate::model::{AlphaMode, Material, MeshData, Model, ModelError, Part, Texture};
use base64::Engine as _;
use glam::Mat4;
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

    let mut loader = Loader { buffers: &buffers, images: HashMap::new() };

    let mut materials = gltf
        .materials()
        .map(|m| loader.material(&m))
        .collect::<Result<Vec<_>, _>>()?;
    // Primitives without a material use the spec default, appended once.
    let default_material = materials.len();
    materials.push(Material::gltf_default());

    let mut meshes: HashMap<(usize, usize), Arc<MeshData>> = HashMap::new();
    let mut parts = Vec::new();
    let roots: Vec<gltf::Node> = match gltf.default_scene().or_else(|| gltf.scenes().next()) {
        Some(scene) => scene.nodes().collect(),
        // No scenes declared: every node no other node lists as a child.
        None => {
            let children: std::collections::HashSet<usize> =
                gltf.nodes().flat_map(|n| n.children().map(|c| c.index())).collect();
            gltf.nodes().filter(|n| !children.contains(&n.index())).collect()
        }
    };
    let mut stack: Vec<(gltf::Node, Mat4)> = roots.into_iter().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
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
                parts.push(Part { mesh: data, material, transform: world });
            }
        }
        stack.extend(node.children().map(|c| (c, world)));
    }
    Ok(Model::from_parts(parts, materials))
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
        Ok(MeshData::new(positions, normals, uvs, indices))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use glam::Vec3;

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
