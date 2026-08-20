//! Shared polar machinery: arc geometry, and the output type the polar
//! renderers return.
//!
//! # Why polar charts are not `ChartSpec` variants
//!
//! [`ChartSpec`](crate::spec::ChartSpec) is cartesian to the bone — two
//! axes, a domain per axis, tick selection, gridlines, gutters, bar slot
//! math, column hit-testing. A pie inherits none of it. Folding polar in as
//! a `Coord::Polar` variant would leave half the struct meaningless in half
//! its states and put a "does this apply?" branch in every render path.
//!
//! What the two families genuinely share is everything below the spec: the
//! mark IR, the hit index, the label-placement protocol, and therefore every
//! renderer and every host. So they share those, and nothing else.
//!
//! # Angles
//!
//! One convention, everywhere in this crate: **radians, clockwise, zero at
//! twelve o'clock**. That is where a reader expects a chart to start and
//! which way they expect it to go, and screen y grows downward — so the
//! usual math convention would need a sign flip at every call site. The
//! conversion lives in [`point_on`] and in `hit::angle_at`, and nowhere
//! else. Author-facing fields are in DEGREES, converted once on entry.

use std::f32::consts::{PI, TAU};

use crate::hit::HitIndex;
use crate::scene::{pt, ChartScene, Color, Path, Point};

/// The result of a polar render.
///
/// No axes: there are none. That is the whole reason this is a separate type
/// rather than a [`ChartOutput`](crate::render::ChartOutput) with two fields
/// nobody can meaningfully read.
#[derive(Clone, PartialEq, Debug)]
pub struct PolarOutput {
    pub scene: ChartScene,
    pub hit: HitIndex,
    /// Center of the ring, in pixel space. Hosts anchor center labels and
    /// pointer math to it.
    pub center: Point,
    /// Outer radius actually used, after label reservation.
    pub radius: f32,
}

/// Which slices are emphasised, and how the rest respond.
///
/// The polar counterpart of [`Highlight`](crate::spec::Highlight), and
/// deliberately its own type: there is no column to hover and no `(series,
/// index)` pair to name, just an index into one flat list.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SliceHighlight {
    pub hovered: Option<usize>,
    pub selected: Vec<usize>,
    /// Fade slices that are neither hovered nor selected.
    pub dim_others: bool,
    /// Opacity multiplier applied by `dim_others`, 0..=1.
    pub dim_opacity: f32,
}

impl SliceHighlight {
    pub fn hovered(i: usize) -> Self {
        Self { hovered: Some(i), dim_opacity: 0.35, ..Default::default() }
    }

    pub fn selected(indices: Vec<usize>) -> Self {
        Self { selected: indices, dim_opacity: 0.35, ..Default::default() }
    }

    pub fn dim_others(mut self, on: bool) -> Self {
        self.dim_others = on;
        if self.dim_opacity <= 0.0 {
            self.dim_opacity = 0.35;
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.hovered.is_none() && self.selected.is_empty()
    }

    pub fn of(&self, index: usize) -> crate::spec::Emphasis {
        use crate::spec::Emphasis;
        if self.selected.contains(&index) {
            Emphasis::Selected
        } else if self.hovered == Some(index) {
            Emphasis::Hovered
        } else {
            Emphasis::None
        }
    }

    /// The opacity multiplier for a slice, folding in `dim_others`.
    pub fn dim_for(&self, index: usize) -> f32 {
        if self.dim_others && !self.is_empty() && self.of(index) == crate::spec::Emphasis::None {
            self.dim_opacity.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

/// A point on the circle of radius `r` about `center`, at `angle` radians
/// clockwise from twelve o'clock.
pub fn point_on(center: Point, r: f32, angle: f32) -> Point {
    pt(center.x + r * angle.sin(), center.y - r * angle.cos())
}

/// Append a circular arc to a path whose current point is already at
/// `(center, r, start)`. A negative `sweep` runs counter-clockwise, which is
/// what the inner edge of a donut segment needs.
///
/// Each quarter-turn or less becomes one cubic with control handles of
/// length `4/3 · tan(Δ/4) · r` along the tangent. That is the standard
/// bezier circle approximation; its worst-case radial error at a quarter
/// turn is under 0.03% of the radius, far below a pixel at any chart size —
/// and unlike flattening to line segments it keeps the resolution a GPU
/// renderer can use when the chart is scaled up.
pub fn arc_to(mut p: Path, center: Point, r: f32, start: f32, sweep: f32) -> Path {
    if r <= 0.0 || sweep.abs() < f32::EPSILON {
        return p;
    }
    let n = (sweep.abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let delta = sweep / n as f32;
    let k = (4.0 / 3.0) * (delta / 4.0).tan() * r;
    for i in 0..n {
        let a0 = start + delta * i as f32;
        let a1 = a0 + delta;
        let p0 = point_on(center, r, a0);
        let p1 = point_on(center, r, a1);
        // d/da of `point_on` is (r·cos a, r·sin a): the clockwise tangent.
        let (t0x, t0y) = (a0.cos(), a0.sin());
        let (t1x, t1y) = (a1.cos(), a1.sin());
        p = p.cubic_to(
            p0.x + k * t0x,
            p0.y + k * t0y,
            p1.x - k * t1x,
            p1.y - k * t1y,
            p1.x,
            p1.y,
        );
    }
    p
}

/// A filled wedge: the region between radii `r0..r1` over `start..start+sweep`.
///
/// `r0 <= 0` produces a pie slice (a wedge meeting at the center); a positive
/// `r0` produces a donut segment. A sweep of a full turn produces a closed
/// ring.
///
/// Always fill this with [`FillRule::EvenOdd`](crate::scene::FillRule) — see
/// [`WEDGE_FILL_RULE`]. A partial wedge is a simple closed contour, where the
/// two rules agree; a full ring is two same-wound circles, where only
/// even-odd punches the hole.
pub fn wedge_path(center: Point, r0: f32, r1: f32, start: f32, sweep: f32) -> Path {
    let r1 = r1.max(0.0);
    let r0 = r0.clamp(0.0, r1);
    if r1 <= 0.0 {
        return Path::new();
    }
    if sweep.abs() >= TAU - 1e-4 {
        let outer = Path::circle(center, r1);
        if r0 <= 0.0 {
            return outer;
        }
        let mut segs = outer.segs;
        segs.extend(Path::circle(center, r0).segs);
        return Path { segs };
    }

    let a1 = start + sweep;
    if r0 <= 0.0 {
        let s = point_on(center, r1, start);
        let p = Path::new().move_to(center.x, center.y).line_to(s.x, s.y);
        return arc_to(p, center, r1, start, sweep).close();
    }
    let outer_start = point_on(center, r1, start);
    let inner_start = point_on(center, r0, start);
    let p = Path::new().move_to(inner_start.x, inner_start.y).line_to(outer_start.x, outer_start.y);
    let p = arc_to(p, center, r1, start, sweep);
    let inner_end = point_on(center, r0, a1);
    let p = p.line_to(inner_end.x, inner_end.y);
    // Back along the inner edge, so the contour closes without crossing
    // itself — a self-crossing contour renders as a bowtie under either
    // fill rule.
    arc_to(p, center, r0, a1, -sweep).close()
}

/// The fill rule every wedge must be painted with. See [`wedge_path`].
pub const WEDGE_FILL_RULE: crate::scene::FillRule = crate::scene::FillRule::EvenOdd;

/// Degrees to radians.
pub fn rad(degrees: f32) -> f32 {
    degrees * PI / 180.0
}

/// Scale a color's alpha, for the `dim_others` fade.
pub(crate) fn fade(c: Color, factor: f32) -> Color {
    if factor >= 1.0 {
        return c;
    }
    Color { a: (c.a as f32 * factor.clamp(0.0, 1.0)).round() as u8, ..c }
}
