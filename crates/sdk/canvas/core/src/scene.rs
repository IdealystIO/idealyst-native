//! The retained 2D scene model — the renderer-agnostic abstraction.
//!
//! A [`Scene`] is a flat, serializable list of [`DrawOp`]s. Authors
//! build one with pure-Rust calls (zero FFI); a renderer then replays
//! the ops onto its surface. The same `Scene` drives every renderer
//! (`canvas-native`'s per-platform engines, `canvas-vello`'s GPU
//! pipeline), which is what makes them interchangeable *and* makes
//! benchmarking one against the other apples-to-apples: feed the
//! identical scene to each and time the flush.
//!
//! # Why retained, not immediate-mode
//!
//! An immediate-mode API (`path.move_to()` calling straight through to
//! the backend) would issue one FFI/JNI crossing per segment — fine for
//! CoreGraphics (a C API) but pathological on Android, where every
//! `Path.lineTo` is a JVM round-trip. A retained `Scene` decouples
//! authoring (pure Rust) from the flush (one batched crossing per
//! frame), so the boundary cost is `O(frames)`, not `O(draw calls)`.
//!
//! # Coordinate space
//!
//! Logical pixels, origin top-left, x right / y down — the Canvas2D /
//! CoreGraphics / `android.graphics` convention. Renderers scale to the
//! surface's device-pixel ratio at flush time.

use runtime_core::color::Rgba;
use serde::{Deserialize, Serialize};

/// A drawing color. Re-exported from the framework's canonical
/// [`Rgba`](runtime_core::color::Rgba) so canvas colors interoperate
/// with theme colors and backend color packing without a second color
/// type. Parse a CSS string with [`color`].
pub type Color = Rgba;

/// Parse a CSS color string (`#rgb`, `#rrggbb`, `#rrggbbaa`,
/// `rgb(...)`, `rgba(...)`) into a [`Color`], falling back to opaque
/// black on an unparseable input. Delegates to the framework's
/// canonical color parser — canvas does not ship its own.
pub fn color(s: &str) -> Color {
    runtime_core::color::parse_or(s, Rgba::BLACK)
}

// ============================================================================
// Path geometry
// ============================================================================

/// One segment of a [`Path`]. Coordinates are absolute logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PathSeg {
    /// Start a new subpath at `(x, y)`.
    MoveTo {
        /// Subpath start x.
        x: f32,
        /// Subpath start y.
        y: f32,
    },
    /// Straight line from the current point to `(x, y)`.
    LineTo {
        /// Endpoint x.
        x: f32,
        /// Endpoint y.
        y: f32,
    },
    /// Quadratic Bézier from the current point to `(x, y)` with control
    /// point `(cx, cy)`.
    QuadTo {
        /// Control x.
        cx: f32,
        /// Control y.
        cy: f32,
        /// Endpoint x.
        x: f32,
        /// Endpoint y.
        y: f32,
    },
    /// Cubic Bézier from the current point to `(x, y)` with control
    /// points `(c1x, c1y)` and `(c2x, c2y)`.
    CubicTo {
        /// First control x.
        c1x: f32,
        /// First control y.
        c1y: f32,
        /// Second control x.
        c2x: f32,
        /// Second control y.
        c2y: f32,
        /// Endpoint x.
        x: f32,
        /// Endpoint y.
        y: f32,
    },
    /// Close the current subpath back to its start point.
    Close,
}

/// An ordered list of [`PathSeg`]s. Build one inline on a [`Scene`]
/// (`scene.path().move_to(..).line_to(..)`) or standalone with the
/// shape constructors ([`Path::rect`], [`Path::circle`], …) for reuse
/// across multiple fills/strokes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Path {
    /// The segments, in draw order.
    pub segs: Vec<PathSeg>,
}

impl Path {
    /// An empty path.
    pub fn new() -> Self {
        Self { segs: Vec::new() }
    }

    /// Append a `MoveTo`.
    pub fn move_to(mut self, x: f32, y: f32) -> Self {
        self.segs.push(PathSeg::MoveTo { x, y });
        self
    }

    /// Append a `LineTo`.
    pub fn line_to(mut self, x: f32, y: f32) -> Self {
        self.segs.push(PathSeg::LineTo { x, y });
        self
    }

    /// Append a `QuadTo`.
    pub fn quad_to(mut self, cx: f32, cy: f32, x: f32, y: f32) -> Self {
        self.segs.push(PathSeg::QuadTo { cx, cy, x, y });
        self
    }

    /// Append a `CubicTo`.
    pub fn cubic_to(mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) -> Self {
        self.segs.push(PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y });
        self
    }

    /// Append a `Close`.
    pub fn close(mut self) -> Self {
        self.segs.push(PathSeg::Close);
        self
    }

    /// An axis-aligned rectangle subpath.
    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self::new()
            .move_to(x, y)
            .line_to(x + w, y)
            .line_to(x + w, y + h)
            .line_to(x, y + h)
            .close()
    }

    /// A rounded rectangle subpath with uniform corner radius `r`
    /// (clamped to half the shorter side).
    pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Self {
        let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
        // 4 quadratic corners; control points at the box corners.
        Self::new()
            .move_to(x + r, y)
            .line_to(x + w - r, y)
            .quad_to(x + w, y, x + w, y + r)
            .line_to(x + w, y + h - r)
            .quad_to(x + w, y + h, x + w - r, y + h)
            .line_to(x + r, y + h)
            .quad_to(x, y + h, x, y + h - r)
            .line_to(x, y + r)
            .quad_to(x, y, x + r, y)
            .close()
    }

    /// A circle subpath, approximated with four cubic Béziers.
    pub fn circle(cx: f32, cy: f32, r: f32) -> Self {
        Self::ellipse(cx, cy, r, r)
    }

    /// An axis-aligned ellipse subpath, approximated with four cubic
    /// Béziers. `K` is the standard circle-to-cubic magic constant.
    pub fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Self {
        const K: f32 = 0.552_284_8; // 4/3 * (sqrt(2) - 1)
        let (ox, oy) = (rx * K, ry * K);
        Self::new()
            .move_to(cx + rx, cy)
            .cubic_to(cx + rx, cy + oy, cx + ox, cy + ry, cx, cy + ry)
            .cubic_to(cx - ox, cy + ry, cx - rx, cy + oy, cx - rx, cy)
            .cubic_to(cx - rx, cy - oy, cx - ox, cy - ry, cx, cy - ry)
            .cubic_to(cx + ox, cy - ry, cx + rx, cy - oy, cx + rx, cy)
            .close()
    }

    /// `true` if the path has no segments.
    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }
}

// ============================================================================
// Paint
// ============================================================================

/// How a fill or stroke is colored: a flat color or a gradient.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Paint {
    /// The paint source.
    pub kind: PaintKind,
}

/// The paint source variants. `#[non_exhaustive]` so image/pattern
/// paints can be added later without breaking match arms in renderers
/// they don't yet support (those fall back to transparent).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PaintKind {
    /// A single flat color.
    Solid(#[serde(with = "rgba_argb")] Color),
    /// A linear gradient between two points.
    Linear(LinearGradient),
    /// A radial gradient.
    Radial(RadialGradient),
}

impl Paint {
    /// A flat-color paint.
    pub fn solid(c: impl Into<Color>) -> Self {
        Self { kind: PaintKind::Solid(c.into()) }
    }

    /// A linear-gradient paint from `(x0, y0)` to `(x1, y1)`.
    pub fn linear(x0: f32, y0: f32, x1: f32, y1: f32, stops: Vec<GradientStop>) -> Self {
        Self { kind: PaintKind::Linear(LinearGradient { x0, y0, x1, y1, stops }) }
    }

    /// A radial-gradient paint centered at `(cx, cy)` with radius `r`.
    pub fn radial(cx: f32, cy: f32, r: f32, stops: Vec<GradientStop>) -> Self {
        Self { kind: PaintKind::Radial(RadialGradient { cx, cy, r, stops }) }
    }
}

impl From<Color> for Paint {
    fn from(c: Color) -> Self {
        Paint::solid(c)
    }
}

/// One color stop in a gradient.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    /// Position along the gradient axis, `0.0..=1.0`.
    pub offset: f32,
    /// Color at this stop.
    #[serde(with = "rgba_argb")]
    pub color: Color,
}

impl GradientStop {
    /// A stop at `offset` with `color`.
    pub fn new(offset: f32, color: impl Into<Color>) -> Self {
        Self { offset, color: color.into() }
    }
}

/// A linear gradient between `(x0, y0)` and `(x1, y1)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinearGradient {
    /// Axis start x.
    pub x0: f32,
    /// Axis start y.
    pub y0: f32,
    /// Axis end x.
    pub x1: f32,
    /// Axis end y.
    pub y1: f32,
    /// Color stops, expected sorted by `offset`.
    pub stops: Vec<GradientStop>,
}

/// A radial gradient centered at `(cx, cy)` with radius `r`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RadialGradient {
    /// Center x.
    pub cx: f32,
    /// Center y.
    pub cy: f32,
    /// Outer radius.
    pub r: f32,
    /// Color stops, expected sorted by `offset`.
    pub stops: Vec<GradientStop>,
}

// ============================================================================
// Stroke style
// ============================================================================

/// How a path's outline is stroked.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    /// Line width in logical pixels.
    pub width: f32,
    /// End-cap style for open subpaths.
    pub cap: LineCap,
    /// Join style between segments.
    pub join: LineJoin,
    /// Miter limit (ratio) before a miter join falls back to bevel.
    pub miter_limit: f32,
}

impl Stroke {
    /// A stroke of the given `width` with default butt caps / miter
    /// joins.
    pub fn width(width: f32) -> Self {
        Self { width, cap: LineCap::Butt, join: LineJoin::Miter, miter_limit: 4.0 }
    }

    /// Set the end-cap style.
    pub fn cap(mut self, cap: LineCap) -> Self {
        self.cap = cap;
        self
    }

    /// Set the join style.
    pub fn join(mut self, join: LineJoin) -> Self {
        self.join = join;
        self
    }

    /// Set the miter limit.
    pub fn miter_limit(mut self, limit: f32) -> Self {
        self.miter_limit = limit;
        self
    }
}

impl Default for Stroke {
    fn default() -> Self {
        Self::width(1.0)
    }
}

/// Line end-cap style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineCap {
    /// Squared off at the endpoint.
    #[default]
    Butt,
    /// Rounded past the endpoint by half the width.
    Round,
    /// Squared off past the endpoint by half the width.
    Square,
}

/// Line join style between two segments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineJoin {
    /// Sharp corner, limited by [`Stroke::miter_limit`].
    #[default]
    Miter,
    /// Rounded corner.
    Round,
    /// Flattened corner.
    Bevel,
}

/// Fill winding rule for self-intersecting paths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillRule {
    /// Non-zero winding (the common default).
    #[default]
    NonZero,
    /// Even-odd winding.
    EvenOdd,
}

// ============================================================================
// Transform
// ============================================================================

/// A 2×3 affine transform, Canvas2D `setTransform(a, b, c, d, e, f)`
/// order. Maps `(x, y)` to `(a·x + c·y + e, b·x + d·y + f)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Row-0 scale-x component.
    pub a: f32,
    /// Row-1 shear-x (skew-y) component.
    pub b: f32,
    /// Row-0 shear-y (skew-x) component.
    pub c: f32,
    /// Row-1 scale-y component.
    pub d: f32,
    /// Translate-x.
    pub e: f32,
    /// Translate-y.
    pub f: f32,
}

impl Transform {
    /// The identity transform.
    pub const IDENTITY: Transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// A translation by `(dx, dy)`.
    pub fn translate(dx: f32, dy: f32) -> Self {
        Self { e: dx, f: dy, ..Self::IDENTITY }
    }

    /// A scale by `(sx, sy)`.
    pub fn scale(sx: f32, sy: f32) -> Self {
        Self { a: sx, d: sy, ..Self::IDENTITY }
    }

    /// A rotation by `radians` (clockwise in the y-down space).
    pub fn rotate(radians: f32) -> Self {
        let (s, co) = radians.sin_cos();
        Self { a: co, b: s, c: -s, d: co, e: 0.0, f: 0.0 }
    }

    /// Compose: apply `self` first, then `next`. Equivalent to the
    /// matrix product `next · self`.
    pub fn then(self, next: Transform) -> Transform {
        let (m, n) = (self, next);
        Transform {
            a: n.a * m.a + n.c * m.b,
            b: n.b * m.a + n.d * m.b,
            c: n.a * m.c + n.c * m.d,
            d: n.b * m.c + n.d * m.d,
            e: n.a * m.e + n.c * m.f + n.e,
            f: n.b * m.e + n.d * m.f + n.f,
        }
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

// ============================================================================
// Draw ops + Scene
// ============================================================================

/// A single retained drawing instruction. A [`Scene`] is a `Vec` of
/// these; renderers replay them in order. `#[non_exhaustive]` so new
/// ops (text, image blit) can be added without breaking renderer match
/// arms.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DrawOp {
    /// Fill `path` with `paint` using `fill_rule`.
    Fill {
        /// Geometry to fill.
        path: Path,
        /// Fill paint.
        paint: Paint,
        /// Winding rule.
        fill_rule: FillRule,
    },
    /// Stroke `path`'s outline with `paint` and `stroke` style.
    Stroke {
        /// Geometry to stroke.
        path: Path,
        /// Stroke paint.
        paint: Paint,
        /// Stroke style.
        stroke: Stroke,
    },
    /// Push the current transform/clip state (Canvas2D `save()`).
    Save,
    /// Pop the last saved state (Canvas2D `restore()`).
    Restore,
    /// Concatenate `Transform` onto the current transform.
    Transform(Transform),
    /// Intersect the clip region with `path`.
    Clip {
        /// Clip geometry.
        path: Path,
        /// Winding rule for the clip.
        fill_rule: FillRule,
    },
}

/// A retained list of draw operations — the renderer-agnostic unit a
/// canvas renderer consumes. Build it imperatively; the current-path
/// cursor mirrors Canvas2D semantics (`path()` begins a path,
/// `fill()`/`stroke()` consume the current path).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Scene {
    /// The recorded ops, in draw order.
    ops: Vec<DrawOp>,
    /// The in-progress path that `fill`/`stroke`/`clip` consume. Not
    /// serialized — it's a build-time cursor, fully captured into ops
    /// by the time a scene crosses the wire.
    #[serde(skip)]
    current: Path,
}

impl Scene {
    /// An empty scene.
    pub fn new() -> Self {
        Self::default()
    }

    /// The recorded ops, for a renderer to replay.
    pub fn ops(&self) -> &[DrawOp] {
        &self.ops
    }

    /// `true` if nothing has been drawn yet.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Begin a fresh current path, discarding any in-progress one.
    /// Chain the `*_to` builders, then [`fill`](Self::fill) /
    /// [`stroke`](Self::stroke) to consume it.
    pub fn path(&mut self) -> &mut Self {
        self.current = Path::new();
        self
    }

    /// Move the path cursor to `(x, y)`, starting a new subpath.
    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.current.segs.push(PathSeg::MoveTo { x, y });
        self
    }

    /// Line from the cursor to `(x, y)`.
    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.current.segs.push(PathSeg::LineTo { x, y });
        self
    }

    /// Quadratic Bézier from the cursor to `(x, y)` via `(cx, cy)`.
    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) -> &mut Self {
        self.current.segs.push(PathSeg::QuadTo { cx, cy, x, y });
        self
    }

    /// Cubic Bézier from the cursor to `(x, y)` via two control points.
    pub fn cubic_to(
        &mut self,
        c1x: f32,
        c1y: f32,
        c2x: f32,
        c2y: f32,
        x: f32,
        y: f32,
    ) -> &mut Self {
        self.current.segs.push(PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y });
        self
    }

    /// Close the current subpath.
    pub fn close(&mut self) -> &mut Self {
        self.current.segs.push(PathSeg::Close);
        self
    }

    /// Replace the current path with a prebuilt [`Path`] (e.g. from
    /// [`Path::rect`]).
    pub fn add_path(&mut self, path: Path) -> &mut Self {
        self.current.segs.extend(path.segs);
        self
    }

    /// Fill the current path with `paint` (non-zero winding).
    pub fn fill(&mut self, paint: impl Into<Paint>) -> &mut Self {
        self.ops.push(DrawOp::Fill {
            path: self.current.clone(),
            paint: paint.into(),
            fill_rule: FillRule::NonZero,
        });
        self
    }

    /// Fill the current path with an explicit winding rule.
    pub fn fill_rule(&mut self, paint: impl Into<Paint>, fill_rule: FillRule) -> &mut Self {
        self.ops.push(DrawOp::Fill { path: self.current.clone(), paint: paint.into(), fill_rule });
        self
    }

    /// Stroke the current path's outline.
    pub fn stroke(&mut self, paint: impl Into<Paint>, stroke: Stroke) -> &mut Self {
        self.ops.push(DrawOp::Stroke { path: self.current.clone(), paint: paint.into(), stroke });
        self
    }

    /// Fill a freestanding path without disturbing the current path.
    pub fn fill_path(&mut self, path: Path, paint: impl Into<Paint>) -> &mut Self {
        self.ops.push(DrawOp::Fill { path, paint: paint.into(), fill_rule: FillRule::NonZero });
        self
    }

    /// Stroke a freestanding path without disturbing the current path.
    pub fn stroke_path(
        &mut self,
        path: Path,
        paint: impl Into<Paint>,
        stroke: Stroke,
    ) -> &mut Self {
        self.ops.push(DrawOp::Stroke { path, paint: paint.into(), stroke });
        self
    }

    /// Push the current transform/clip state.
    pub fn save(&mut self) -> &mut Self {
        self.ops.push(DrawOp::Save);
        self
    }

    /// Pop the last saved transform/clip state.
    pub fn restore(&mut self) -> &mut Self {
        self.ops.push(DrawOp::Restore);
        self
    }

    /// Concatenate an arbitrary affine transform.
    pub fn transform(&mut self, t: Transform) -> &mut Self {
        self.ops.push(DrawOp::Transform(t));
        self
    }

    /// Translate the coordinate system by `(dx, dy)`.
    pub fn translate(&mut self, dx: f32, dy: f32) -> &mut Self {
        self.transform(Transform::translate(dx, dy))
    }

    /// Scale the coordinate system by `(sx, sy)`.
    pub fn scale(&mut self, sx: f32, sy: f32) -> &mut Self {
        self.transform(Transform::scale(sx, sy))
    }

    /// Rotate the coordinate system by `radians`.
    pub fn rotate(&mut self, radians: f32) -> &mut Self {
        self.transform(Transform::rotate(radians))
    }

    /// Intersect the clip region with the current path.
    pub fn clip(&mut self) -> &mut Self {
        self.ops.push(DrawOp::Clip {
            path: self.current.clone(),
            fill_rule: FillRule::NonZero,
        });
        self
    }
}

// ============================================================================
// Serde shim: pack Rgba as a single ARGB u32 on the wire.
// ============================================================================

mod rgba_argb {
    use super::Color;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(c: &Color, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(c.to_argb_u32())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color, D::Error> {
        Ok(Color::from_argb_u32(u32::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_consumes_current_path() {
        let mut s = Scene::new();
        s.path().move_to(0.0, 0.0).line_to(10.0, 0.0).line_to(10.0, 10.0).close();
        s.fill(Paint::solid(Color::new(255, 0, 0, 255)));
        assert_eq!(s.ops().len(), 1);
        match &s.ops()[0] {
            DrawOp::Fill { path, paint, fill_rule } => {
                assert_eq!(path.segs.len(), 4);
                assert_eq!(*fill_rule, FillRule::NonZero);
                assert_eq!(paint.kind, PaintKind::Solid(Color::new(255, 0, 0, 255)));
            }
            other => panic!("expected Fill, got {other:?}"),
        }
    }

    #[test]
    fn fill_then_stroke_reuse_same_current_path() {
        let mut s = Scene::new();
        s.path().add_path(Path::rect(0.0, 0.0, 5.0, 5.0));
        s.fill(Color::new(0, 0, 0, 255));
        s.stroke(Color::new(255, 255, 255, 255), Stroke::width(2.0));
        // Both ops carry the same geometry — current path is not cleared
        // by fill/stroke, only by path().
        assert_eq!(s.ops().len(), 2);
        let geom = |op: &DrawOp| match op {
            DrawOp::Fill { path, .. } | DrawOp::Stroke { path, .. } => path.segs.len(),
            _ => 0,
        };
        assert_eq!(geom(&s.ops()[0]), geom(&s.ops()[1]));
        assert!(geom(&s.ops()[0]) > 0);
    }

    #[test]
    fn color_into_paint_coercion() {
        let mut s = Scene::new();
        s.path().add_path(Path::circle(5.0, 5.0, 5.0)).fill(Color::new(1, 2, 3, 4));
        assert!(matches!(s.ops()[0], DrawOp::Fill { .. }));
    }

    #[test]
    fn scene_serde_round_trips_through_json() {
        let mut s = Scene::new();
        s.save();
        s.translate(10.0, 20.0);
        s.path().add_path(Path::rounded_rect(0.0, 0.0, 40.0, 30.0, 6.0));
        s.fill(Paint::linear(
            0.0,
            0.0,
            40.0,
            0.0,
            vec![
                GradientStop::new(0.0, Color::new(255, 0, 0, 255)),
                GradientStop::new(1.0, Color::new(0, 0, 255, 255)),
            ],
        ));
        s.stroke(Color::new(0, 0, 0, 255), Stroke::width(2.0).cap(LineCap::Round));
        s.restore();

        let bytes = serde_json::to_vec(&s).expect("serialize");
        let back: Scene = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(s.ops(), back.ops());
    }

    #[test]
    fn solid_color_packs_as_argb_u32() {
        // Verifies the rgba_argb serde shim preserves channels exactly.
        let c = Color::new(0x12, 0x34, 0x56, 0x78);
        let p = Paint::solid(c);
        let json = serde_json::to_string(&p).unwrap();
        let back: Paint = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, PaintKind::Solid(c));
    }

    #[test]
    fn transform_then_matches_manual_point_map() {
        // self = translate(10,0), next = scale(2,2).
        // Apply self first: (1,1) -> (11,1) -> scale -> (22,2).
        let m = Transform::translate(10.0, 0.0).then(Transform::scale(2.0, 2.0));
        let (x, y) = (1.0_f32, 1.0_f32);
        let px = m.a * x + m.c * y + m.e;
        let py = m.b * x + m.d * y + m.f;
        assert_eq!((px, py), (22.0, 2.0));
    }
}
