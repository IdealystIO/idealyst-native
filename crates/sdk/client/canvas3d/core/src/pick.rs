//! CPU picking: which model surface does a world ray hit first.
//!
//! Exact against the same triangles the renderer draws (meshes keep their
//! positions on the CPU), with a per-part bounding-box rejection first. Rays
//! are tested in each part's local space, so no geometry is transformed.

use crate::camera::Ray;
use crate::model::Aabb;
use crate::scene::Scene3d;
use glam::{Mat4, Vec3};

/// The nearest pickable surface under a pick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickHit {
    /// The `pick_id` the model was drawn with.
    pub pick_id: u32,
    /// World-space hit point.
    pub world_pos: Vec3,
    /// Distance from the ray origin (the near plane) to the hit.
    pub distance: f32,
}

/// Möller–Trumbore ray/triangle test. Returns the ray parameter `t > 0` of
/// the hit. With `cull_back`, triangles whose counter-clockwise front faces
/// away from the ray are missed. `dir` need not be unit length; `t` is in
/// units of `dir`.
pub fn ray_triangle(origin: Vec3, dir: Vec3, [a, b, c]: [Vec3; 3], cull_back: bool) -> Option<f32> {
    const EPS: f32 = 1e-7;
    let e1 = b - a;
    let e2 = c - a;
    let p = dir.cross(e2);
    let det = e1.dot(p);
    // det > 0: the ray sees the counter-clockwise (front) side.
    if cull_back && det < EPS {
        return None;
    }
    if det.abs() < EPS {
        return None; // parallel
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > EPS).then_some(t)
}

/// Slab test: the ray parameter range `[t_near, t_far]` inside `bounds`, or
/// `None` if the ray misses it (or it lies entirely behind the origin).
pub fn ray_aabb(origin: Vec3, dir: Vec3, bounds: Aabb) -> Option<(f32, f32)> {
    if bounds.is_empty() {
        return None;
    }
    let inv = dir.recip();
    let t0 = (bounds.min - origin) * inv;
    let t1 = (bounds.max - origin) * inv;
    let near = t0.min(t1).max_element();
    let far = t0.max(t1).min_element();
    (far >= near.max(0.0)).then_some((near, far))
}

/// The nearest hit of `ray` (unit `dir`) on the pickable models of `scene`.
pub fn pick(scene: &Scene3d, ray: Ray) -> Option<PickHit> {
    let mut best: Option<(f32, u32)> = None;
    for item in scene.models() {
        let Some(id) = item.pick_id else { continue };
        for part in item.model.parts() {
            let to_world = item.transform * part.transform;
            let Some(t) = nearest_in_part(ray, to_world, part, &item.model) else { continue };
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, id));
            }
        }
    }
    best.map(|(t, id)| PickHit { pick_id: id, world_pos: ray.at(t), distance: t })
}

fn nearest_in_part(ray: Ray, to_world: Mat4, part: &crate::model::Part, model: &crate::model::Model) -> Option<f32> {
    // A degenerate (non-invertible) transform collapses the part to nothing
    // pickable — and draws nothing either.
    if to_world.determinant().abs() < f32::EPSILON {
        return None;
    }
    let to_local = to_world.inverse();
    // Affine map: local hit `o' + t·d'` is the image of world `o + t·d`, so
    // `t` stays the WORLD distance along the unit world ray.
    let o = to_local.transform_point3(ray.origin);
    let d = to_local.transform_vector3(ray.dir);
    ray_aabb(o, d, part.mesh.bounds)?;
    // A mirroring transform flips which side is the front face.
    let mirrored = to_world.determinant() < 0.0;
    let cull_back = !model.materials()[part.material].double_sided;
    let mesh = &part.mesh;
    let mut nearest: Option<f32> = None;
    for tri in 0..mesh.triangle_count() {
        let mut corners = mesh.triangle(tri);
        if mirrored {
            corners.swap(1, 2);
        }
        if let Some(t) = ray_triangle(o, d, corners, cull_back) {
            if nearest.is_none_or(|n| t < n) {
                nearest = Some(t);
            }
        }
    }
    nearest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use crate::model::{Material, MeshData, Model};
    use glam::Vec2;

    const SIZE: (f32, f32) = (400.0, 300.0);
    const CENTRE: Vec2 = Vec2::new(200.0, 150.0);

    fn scene_looking_down_z() -> Scene3d {
        let mut s = Scene3d::with_size(SIZE.0, SIZE.1);
        s.camera(Camera::look_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO));
        s
    }

    fn cube() -> Model {
        Model::from_mesh(MeshData::cube(1.0), Material::default())
    }

    #[test]
    fn hit_reports_the_front_face_and_its_id() {
        let mut s = scene_looking_down_z();
        s.model(&cube(), Mat4::IDENTITY).pick_id(7);
        let hit = s.pick(CENTRE).expect("the cube is in the middle of the view");
        assert_eq!(hit.pick_id, 7);
        assert!((hit.world_pos.z - 0.5).abs() < 1e-3, "front face at z = 0.5: {}", hit.world_pos);
    }

    #[test]
    fn miss_returns_none() {
        let mut s = scene_looking_down_z();
        s.model(&cube(), Mat4::IDENTITY).pick_id(1);
        assert_eq!(s.pick(Vec2::new(5.0, 5.0)), None);
    }

    #[test]
    fn unpickable_models_are_ignored() {
        let mut s = scene_looking_down_z();
        s.model(&cube(), Mat4::IDENTITY);
        assert_eq!(s.pick(CENTRE), None);
    }

    #[test]
    fn nearest_of_two_wins_regardless_of_draw_order() {
        let mut s = scene_looking_down_z();
        s.model(&cube(), Mat4::from_translation(Vec3::new(0.0, 0.0, -3.0))).pick_id(1);
        s.model(&cube(), Mat4::from_translation(Vec3::new(0.0, 0.0, 2.0))).pick_id(2);
        assert_eq!(s.pick(CENTRE).unwrap().pick_id, 2);
    }

    #[test]
    fn single_sided_back_faces_are_not_hit_but_double_sided_are() {
        // A plane facing +Y, seen from below: only its back face is visible.
        let mut s = Scene3d::with_size(SIZE.0, SIZE.1);
        s.camera(Camera::look_at(Vec3::new(0.0, -5.0, 0.01), Vec3::ZERO));
        let one_sided = Model::from_mesh(MeshData::plane(4.0, 4.0), Material::default());
        s.model(&one_sided, Mat4::IDENTITY).pick_id(1);
        assert_eq!(s.pick(CENTRE), None, "back face of a single-sided plane");

        let mut s2 = s.clone();
        let two_sided = Model::from_mesh(MeshData::plane(4.0, 4.0), Material::default().double_sided());
        s2.model(&two_sided, Mat4::IDENTITY).pick_id(2);
        assert_eq!(s2.pick(CENTRE).map(|h| h.pick_id), Some(2));
    }

    #[test]
    fn transformed_model_is_hit_where_it_was_moved() {
        let mut s = scene_looking_down_z();
        let xf = Mat4::from_scale_rotation_translation(
            Vec3::splat(2.0),
            glam::Quat::from_rotation_y(0.6),
            Vec3::new(1.5, 0.0, 0.0),
        );
        s.model(&cube(), xf).pick_id(3);
        assert_eq!(s.pick(CENTRE), None, "moved off-centre");
        let p = s.project(Vec3::new(1.5, 0.0, 0.0)).unwrap();
        let hit = s.pick(p).expect("hit where the model now is");
        assert_eq!(hit.pick_id, 3);
        // Distance is in world units along the ray.
        assert!((hit.distance - (Vec3::new(0.0, 0.0, 10.0) - hit.world_pos).length()).abs() < 0.2);
    }

    #[test]
    fn mirrored_transform_keeps_outward_faces_pickable() {
        let mut s = scene_looking_down_z();
        s.model(&cube(), Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0))).pick_id(4);
        assert_eq!(s.pick(CENTRE).map(|h| h.pick_id), Some(4));
    }

    #[test]
    fn ray_aabb_rejects_boxes_behind_the_origin() {
        let b = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        assert!(ray_aabb(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z, b).is_some());
        assert!(ray_aabb(Vec3::new(0.0, 0.0, 5.0), Vec3::Z, b).is_none());
    }
}
