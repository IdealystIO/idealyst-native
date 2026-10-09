//! Real-world glTF through the loader: the demo's Khronos WaterBottle
//! (embedded PNG textures for all five material slots, a node hierarchy) and
//! Fox (a 24-joint skin, three clips). The assets are fetched, not committed —
//! these skip with a note until `./scripts/fetch-canvas3d-demo-model.sh` has
//! run.

use canvas3d_core::{skinned_positions, AlphaMode, Model, Pose};

const ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/canvas3d-demo/assets/WaterBottle.glb");
const FOX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/canvas3d-demo/assets/Fox.glb");

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

#[test]
fn khronos_fox_loads_its_skin_and_clips_and_moves_when_sampled() {
    let Ok(bytes) = std::fs::read(FOX) else {
        eprintln!("skipping: {FOX} not fetched (run ./scripts/fetch-canvas3d-demo-model.sh)");
        return;
    };
    let fox = Model::from_gltf(&bytes).expect("Fox.glb loads");
    let mut names: Vec<_> = fox.animations().iter().filter_map(|a| a.name()).collect();
    names.sort();
    assert_eq!(names, ["Run", "Survey", "Walk"]);
    assert_eq!(fox.skins().len(), 1);
    assert_eq!(fox.skins()[0].joints.len(), 24);
    let part = fox.parts().iter().find(|p| p.skin.is_some()).expect("a skinned part");

    let walk = fox.animation("Walk").unwrap();
    assert!(walk.duration() > 0.5, "a walk cycle: {}", walk.duration());
    let at = |t: f32| {
        let pose = Pose::rest(&fox).sampled(walk, t);
        skinned_positions(&part.mesh, &fox.joint_matrices(0, &fox.globals(Some(&pose))))
    };
    let (a, b) = (at(0.0), at(walk.duration() * 0.5));
    assert!(a.iter().chain(&b).all(|p| p.is_finite()));
    let moved = a.iter().zip(&b).filter(|(p, q)| p.distance(**q) > 1.0).count();
    assert!(moved > part.mesh.positions.len() / 10, "half a stride later the legs have moved ({moved} vertices)");
}
