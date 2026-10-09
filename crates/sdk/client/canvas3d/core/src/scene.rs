//! The retained 3D scene a `Canvas3d` painter fills each run.

use crate::camera::{Camera, Ray};
use crate::color::Color;
use crate::model::{Aabb, Model};
use crate::pick::{pick, PickHit};
use glam::{Mat4, Vec2, Vec3};
use std::sync::Arc;

/// Lights that illuminate lit materials (in addition to the ambient term).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Light {
    /// Parallel light travelling along `direction` (world space; need not be
    /// normalised) — a sun.
    Directional { direction: Vec3, color: Color, intensity: f32 },
}

impl Light {
    pub fn directional(direction: Vec3, color: Color, intensity: f32) -> Light {
        Light::Directional { direction, color, intensity }
    }
}

/// How world-space lines interact with the scene's depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineDepth {
    /// Hidden behind nearer geometry (a ground grid).
    Tested,
    /// Always drawn over the scene (a selection outline you need to see
    /// through other objects).
    OnTop,
}

/// A set of world-space line segments. Cheap to clone (shared), so a static
/// set — a grid — can be built once and drawn every frame.
#[derive(Clone, Debug)]
pub struct Lines {
    segments: Arc<Vec<[Vec3; 2]>>,
}

impl Lines {
    pub fn new(segments: Vec<[Vec3; 2]>) -> Lines {
        Lines { segments: Arc::new(segments) }
    }

    /// A square grid on the XZ plane (y = 0) reaching `half_extent` from the
    /// origin on each side, with a line every `step`.
    pub fn grid(half_extent: f32, step: f32) -> Lines {
        let step = step.max(f32::EPSILON);
        let n = (half_extent / step).floor() as i32;
        let e = n as f32 * step;
        let mut segments = Vec::with_capacity(((2 * n + 1) * 2) as usize);
        for i in -n..=n {
            let c = i as f32 * step;
            segments.push([Vec3::new(c, 0.0, -e), Vec3::new(c, 0.0, e)]);
            segments.push([Vec3::new(-e, 0.0, c), Vec3::new(e, 0.0, c)]);
        }
        Lines::new(segments)
    }

    /// The 12 edges of `bounds` after `transform` — a selection box.
    pub fn box_edges(bounds: Aabb, transform: Mat4) -> Lines {
        if bounds.is_empty() {
            return Lines::new(Vec::new());
        }
        let c = bounds.corners().map(|p| transform.transform_point3(p));
        // Corner index bits: x = 1, y = 2, z = 4 (see `Aabb::corners`).
        const EDGES: [(usize, usize); 12] = [
            (0, 1), (2, 3), (4, 5), (6, 7), // along x
            (0, 2), (1, 3), (4, 6), (5, 7), // along y
            (0, 4), (1, 5), (2, 6), (3, 7), // along z
        ];
        Lines::new(EDGES.iter().map(|&(a, b)| [c[a], c[b]]).collect())
    }

    pub fn segments(&self) -> &[[Vec3; 2]] {
        &self.segments
    }
}

/// One drawn model.
#[derive(Clone, Debug)]
pub struct ModelItem {
    pub model: Model,
    pub transform: Mat4,
    /// Set → the model is pickable and [`Scene3d::pick`] reports this id.
    pub pick_id: Option<u32>,
}

/// One batch of lines.
#[derive(Clone, Debug)]
pub struct LineBatch {
    pub lines: Lines,
    pub color: Color,
    pub depth: LineDepth,
}

/// What a 3D view shows this frame. Built fresh by the author's painter each
/// time it runs; heavy content ([`Model`]s, [`Lines`]) is shared, not copied.
#[derive(Clone, Debug)]
pub struct Scene3d {
    size: (f32, f32),
    camera: Camera,
    clear: Color,
    ambient: Color,
    ambient_intensity: f32,
    lights: Vec<Light>,
    models: Vec<ModelItem>,
    lines: Vec<LineBatch>,
}

impl Default for Scene3d {
    fn default() -> Self {
        Scene3d::new()
    }
}

/// Default ambient intensity: enough that unlit sides read as shape, not black.
const DEFAULT_AMBIENT_INTENSITY: f32 = 0.3;

impl Scene3d {
    pub fn new() -> Scene3d {
        Scene3d::with_size(0.0, 0.0)
    }

    /// A scene for a view of `width`×`height` logical units.
    pub fn with_size(width: f32, height: f32) -> Scene3d {
        Scene3d {
            size: (width, height),
            camera: Camera::default(),
            clear: Color::TRANSPARENT,
            ambient: Color::WHITE,
            ambient_intensity: DEFAULT_AMBIENT_INTENSITY,
            lights: Vec::new(),
            models: Vec::new(),
            lines: Vec::new(),
        }
    }

    /// The view's laid-out logical size. Reading it in a painter re-runs the
    /// painter on resize. `(0, 0)` before the first layout.
    pub fn size(&self) -> (f32, f32) {
        self.size
    }

    pub fn camera(&mut self, camera: Camera) -> &mut Self {
        self.camera = camera;
        self
    }

    /// Background colour behind everything (default transparent: the UI
    /// behind the view shows through).
    pub fn clear(&mut self, color: Color) -> &mut Self {
        self.clear = color;
        self
    }

    /// Uniform light reaching every surface from all directions.
    pub fn ambient(&mut self, color: Color, intensity: f32) -> &mut Self {
        self.ambient = color;
        self.ambient_intensity = intensity.max(0.0);
        self
    }

    pub fn light(&mut self, light: Light) -> &mut Self {
        self.lights.push(light);
        self
    }

    /// Draw `model` placed by `transform`. Returns the item to set a pick id
    /// on: `s.model(&m, xf).pick_id(1);`.
    pub fn model(&mut self, model: &Model, transform: Mat4) -> &mut ModelItem {
        self.models.push(ModelItem { model: model.clone(), transform, pick_id: None });
        self.models.last_mut().expect("just pushed")
    }

    pub fn lines(&mut self, lines: &Lines, color: Color, depth: LineDepth) -> &mut Self {
        self.lines.push(LineBatch { lines: lines.clone(), color, depth });
        self
    }

    pub fn get_camera(&self) -> &Camera {
        &self.camera
    }

    pub fn clear_color(&self) -> Color {
        self.clear
    }

    pub fn ambient_light(&self) -> (Color, f32) {
        (self.ambient, self.ambient_intensity)
    }

    pub fn get_lights(&self) -> &[Light] {
        &self.lights
    }

    pub fn models(&self) -> &[ModelItem] {
        &self.models
    }

    pub fn line_batches(&self) -> &[LineBatch] {
        &self.lines
    }

    /// Where `world` appears in this view (logical units, top-left origin).
    pub fn project(&self, world: Vec3) -> Option<Vec2> {
        self.camera.project(world, self.size)
    }

    /// The world ray under `point` (logical units, top-left origin).
    pub fn ray(&self, point: Vec2) -> Ray {
        self.camera.ray(point, self.size)
    }

    /// The nearest pickable model surface under `point` (logical units,
    /// top-left origin), if any. Back faces of single-sided materials aren't
    /// hit — they aren't drawn either.
    pub fn pick(&self, point: Vec2) -> Option<PickHit> {
        pick(self, self.ray(point))
    }
}

impl ModelItem {
    pub fn pick_id(&mut self, id: u32) -> &mut Self {
        self.pick_id = Some(id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_spans_the_extent_on_both_axes() {
        let g = Lines::grid(2.0, 1.0);
        assert_eq!(g.segments().len(), 10);
        for [a, b] in g.segments() {
            assert_eq!(a.y, 0.0);
            assert_eq!((*b - *a).length(), 4.0);
        }
    }

    #[test]
    fn box_edges_are_the_twelve_unit_edges() {
        let e = Lines::box_edges(Aabb::new(Vec3::ZERO, Vec3::ONE), Mat4::IDENTITY);
        assert_eq!(e.segments().len(), 12);
        for [a, b] in e.segments() {
            assert!(((*b - *a).length() - 1.0).abs() < 1e-6);
        }
        assert!(Lines::box_edges(Aabb::EMPTY, Mat4::IDENTITY).segments().is_empty());
    }
}
