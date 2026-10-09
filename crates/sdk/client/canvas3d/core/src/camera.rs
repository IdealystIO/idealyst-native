//! Cameras, and the mapping between the 3D world and the view's 2D
//! (logical-pixel, top-left-origin) coordinates.
//!
//! Matrices follow wgpu's conventions: right-handed world, clip-space depth
//! `0..=1` with Y up — glam's `camera::rh::proj::directx` constructors.

use glam::camera::rh::{proj::directx, view::look_at_mat4};
use glam::{Mat4, Vec2, Vec3, Vec4Swizzles};

/// How the camera projects onto the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// `fov_y` in radians.
    Perspective { fov_y: f32, near: f32, far: f32 },
    /// `height` is the world-space height the view shows.
    Orthographic { height: f32, near: f32, far: f32 },
}

impl Default for Projection {
    fn default() -> Self {
        Projection::Perspective { fov_y: 45f32.to_radians(), near: 0.05, far: 500.0 }
    }
}

/// A look-at camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub projection: Projection,
}

impl Default for Camera {
    /// Looking at the origin from +Z (and a little above).
    fn default() -> Self {
        Camera {
            eye: Vec3::new(0.0, 1.0, 5.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            projection: Projection::default(),
        }
    }
}

/// A half-line in world space. `dir` is unit length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }
}

impl Camera {
    pub fn look_at(eye: Vec3, target: Vec3) -> Camera {
        Camera { eye, target, ..Camera::default() }
    }

    pub fn with_projection(self, projection: Projection) -> Camera {
        Camera { projection, ..self }
    }

    /// World → camera space.
    pub fn view(&self) -> Mat4 {
        look_at_mat4(self.eye, self.target, self.up)
    }

    /// Camera → clip space for a view of `aspect` (width / height).
    pub fn proj(&self, aspect: f32) -> Mat4 {
        let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
        match self.projection {
            Projection::Perspective { fov_y, near, far } => {
                directx::perspective(fov_y, aspect, near, far)
            }
            Projection::Orthographic { height, near, far } => {
                let h = height * 0.5;
                let w = h * aspect;
                directx::orthographic(-w, w, -h, h, near, far)
            }
        }
    }

    /// World → clip space for a view of `size` (any units; only the aspect
    /// ratio matters).
    pub fn view_proj(&self, size: (f32, f32)) -> Mat4 {
        self.proj(aspect(size)) * self.view()
    }

    /// Where `world` lands in the view, in `size` units with a top-left
    /// origin (the coordinates a 2D overlay draws in). `None` when the point
    /// is behind the camera.
    pub fn project(&self, world: Vec3, size: (f32, f32)) -> Option<Vec2> {
        let clip = self.view_proj(size) * world.extend(1.0);
        if clip.w <= f32::EPSILON {
            return None;
        }
        let ndc = clip.xyz() / clip.w;
        Some(Vec2::new((ndc.x * 0.5 + 0.5) * size.0, (0.5 - ndc.y * 0.5) * size.1))
    }

    /// The world-space ray through `point` (in `size` units, top-left
    /// origin) — what picking casts. Starts on the near plane.
    pub fn ray(&self, point: Vec2, size: (f32, f32)) -> Ray {
        let ndc_x = 2.0 * point.x / size.0.max(f32::EPSILON) - 1.0;
        let ndc_y = 1.0 - 2.0 * point.y / size.1.max(f32::EPSILON);
        let inv = self.view_proj(size).inverse();
        let near = inv.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
        let far = inv.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
        Ray { origin: near, dir: (far - near).normalize_or(self.forward()) }
    }

    /// Unit vector the camera looks along.
    pub fn forward(&self) -> Vec3 {
        (self.target - self.eye).normalize_or(-Vec3::Z)
    }
}

fn aspect(size: (f32, f32)) -> f32 {
    if size.1 > 0.0 {
        size.0 / size.1
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (f32, f32) = (800.0, 600.0);

    fn near(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-2
    }

    #[test]
    fn target_projects_to_the_view_centre() {
        let cam = Camera::look_at(Vec3::new(3.0, 2.0, 4.0), Vec3::new(0.5, 0.0, 0.0));
        let p = cam.project(cam.target, SIZE).unwrap();
        assert!(near(p, Vec2::new(400.0, 300.0)), "{p}");
    }

    #[test]
    fn up_is_up_and_right_is_right_on_screen() {
        let cam = Camera::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
        let up = cam.project(Vec3::new(0.0, 1.0, 0.0), SIZE).unwrap();
        let right = cam.project(Vec3::new(1.0, 0.0, 0.0), SIZE).unwrap();
        assert!(up.y < 300.0, "world +Y is towards the top of the view: {up}");
        assert!(right.x > 400.0, "world +X is towards the right of the view: {right}");
    }

    #[test]
    fn points_behind_the_camera_do_not_project() {
        let cam = Camera::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
        assert_eq!(cam.project(Vec3::new(0.0, 0.0, 10.0), SIZE), None);
    }

    #[test]
    fn ray_through_a_projected_point_passes_through_the_point() {
        for cam in [
            Camera::look_at(Vec3::new(2.0, 3.0, 6.0), Vec3::new(0.0, 0.5, 0.0)),
            Camera::look_at(Vec3::new(2.0, 3.0, 6.0), Vec3::ZERO)
                .with_projection(Projection::Orthographic { height: 4.0, near: 0.1, far: 50.0 }),
        ] {
            let world = Vec3::new(0.7, -0.3, 1.1);
            let screen = cam.project(world, SIZE).unwrap();
            let ray = cam.ray(screen, SIZE);
            // Distance from `world` to the ray.
            let t = (world - ray.origin).dot(ray.dir);
            let closest = ray.at(t);
            assert!((closest - world).length() < 1e-3, "{:?}: {closest} vs {world}", cam.projection);
            assert!((ray.dir.length() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn centre_ray_points_at_the_target() {
        let cam = Camera::look_at(Vec3::new(0.0, 4.0, 4.0), Vec3::ZERO);
        let ray = cam.ray(Vec2::new(400.0, 300.0), SIZE);
        assert!((ray.dir - cam.forward()).length() < 1e-4);
    }
}
