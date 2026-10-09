//! A real-world glTF through the loader: the demo's Khronos WaterBottle
//! (embedded PNG textures for all five material slots, a node hierarchy).
//! The asset is fetched, not committed — this skips with a note until
//! `./scripts/fetch-canvas3d-demo-model.sh` has run.

use canvas3d_core::{AlphaMode, Model};

const ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/canvas3d-demo/assets/WaterBottle.glb");

#[test]
fn khronos_water_bottle_loads_with_every_material_slot() {
    let Ok(bytes) = std::fs::read(ASSET) else {
        eprintln!("skipping: {ASSET} not fetched (run ./scripts/fetch-canvas3d-demo-model.sh)");
        return;
    };
    let model = Model::from_gltf(&bytes).expect("WaterBottle.glb loads");
    assert!(!model.parts().is_empty());
    let tris: usize = model.parts().iter().map(|p| p.mesh.triangle_count()).sum();
    assert!(tris > 1000, "a real mesh: {tris} triangles");

    let m = &model.materials()[model.parts()[0].material];
    assert_eq!(m.alpha_mode, AlphaMode::Opaque);
    for (slot, tex) in [
        ("base colour", &m.base_color_texture),
        ("metallic-roughness", &m.metallic_roughness_texture),
        ("normal", &m.normal_texture),
        ("occlusion", &m.occlusion_texture),
        ("emissive", &m.emissive_texture),
    ] {
        let t = tex.as_ref().unwrap_or_else(|| panic!("{slot} texture decoded"));
        assert!(t.width >= 256 && t.height >= 256, "{slot}: {}×{}", t.width, t.height);
        assert_eq!(t.rgba.len(), (t.width * t.height * 4) as usize, "{slot}");
    }

    // A bottle: taller than it is wide, a few tenths of a unit in size.
    let b = model.bounds();
    assert!(!b.is_empty());
    assert!(b.size().y > b.size().x && b.size().y < 1.0, "{:?}", b.size());
}
