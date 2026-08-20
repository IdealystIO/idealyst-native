//! Pointer hit-testing.
//!
//! Produced as part of a render rather than recomputed on each pointer
//! move, because the mapping from pixel back to datum depends on the exact
//! domain and plot rect that render used. Recomputing it separately is how
//! tooltips end up pointing at the wrong datum one frame after a resize.

use std::f32::consts::TAU;

use crate::scene::{Point, Rect};
use crate::spec::Datum;

/// The region of the plot that resolves to one datum.
///
/// Shapes rather than points because the marks are shapes, and a
/// nearest-point index silently disagrees with what the user can see. A bar
/// indexed by its top-centre stops responding near its base; a pie slice
/// indexed by its centroid resolves to a neighbour over most of its own
/// area. Both bugs vanish once the index stores what was actually drawn.
#[derive(Clone, Copy, PartialEq, Debug)]
enum HitShape {
    /// A marker: line/area/scatter points. Matched by proximity, since a
    /// 3px dot is smaller than anyone can reliably aim at.
    Point,
    /// A bar. Matched by containment over its whole body.
    Rect(Rect),
    /// A pie or radial-bar slice. `start` is clockwise from twelve o'clock
    /// in radians; `sweep` is non-negative.
    Wedge { center: Point, r0: f32, r1: f32, start: f32, sweep: f32 },
}

/// Angle of `p` about `center`, clockwise from twelve o'clock, in `0..TAU`.
///
/// `atan2(dx, -dy)` rather than the usual `atan2(dy, dx)`: screen y grows
/// downward, and charts measure a slice from the top. Doing the conversion
/// here — once — is what keeps every other piece of polar math free of sign
/// juggling.
fn angle_at(center: Point, p: Point) -> f32 {
    let (dx, dy) = (p.x - center.x, p.y - center.y);
    dx.atan2(-dy).rem_euclid(TAU)
}

impl HitShape {
    fn contains(&self, p: Point, anchor: Point) -> bool {
        match *self {
            // A point mark has no area; containment is meaningless and
            // proximity is the only sensible test. `nearest_within` is the
            // query for those.
            HitShape::Point => false,
            HitShape::Rect(r) => r.contains(p),
            HitShape::Wedge { center, r0, r1, start, sweep } => {
                let (dx, dy) = (p.x - center.x, p.y - center.y);
                let d = (dx * dx + dy * dy).sqrt();
                if d < r0 || d > r1 {
                    return false;
                }
                // A full circle has no angular boundary to test, and the
                // modular comparison below would reject exactly one ray of
                // it. Ring gauges are drawn as one full-sweep wedge, so
                // this is the common case, not an edge case.
                if sweep >= TAU - f32::EPSILON {
                    return true;
                }
                let _ = anchor;
                (angle_at(center, p) - start).rem_euclid(TAU) <= sweep
            }
        }
    }

    /// Distance from `p` to the shape: zero inside, otherwise a measure that
    /// grows as the pointer moves away.
    fn distance(&self, p: Point, anchor: Point) -> f32 {
        match *self {
            HitShape::Point => dist(p, anchor),
            HitShape::Rect(r) => {
                let dx = (r.x - p.x).max(0.0).max(p.x - r.right());
                let dy = (r.y - p.y).max(0.0).max(p.y - r.bottom());
                (dx * dx + dy * dy).sqrt()
            }
            HitShape::Wedge { .. } => {
                if self.contains(p, anchor) {
                    0.0
                } else {
                    // Falling back to the centroid is deliberate: a wedge is
                    // meant to be queried by containment, and an exact
                    // distance-to-wedge is a lot of trigonometry for a
                    // tie-break nobody looks at.
                    dist(p, anchor)
                }
            }
        }
    }
}

fn dist(a: Point, b: Point) -> f32 {
    let (dx, dy) = (a.x - b.x, a.y - b.y);
    (dx * dx + dy * dy).sqrt()
}

/// One datum's resolved screen region.
#[derive(Clone, Copy, PartialEq, Debug)]
struct HitEntry {
    shape: HitShape,
    /// Where a tooltip should point. For a marker this is the marker; for a
    /// bar, its outer end; for a wedge, its centroid.
    anchor: Point,
    series: usize,
    index: usize,
    datum: Datum,
}

/// A matched datum.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HitResult {
    /// Index into [`ChartSpec::series`](crate::spec::ChartSpec::series) —
    /// the original list, including hidden series, so callers can look up
    /// the name and color without re-filtering.
    pub series: usize,
    /// Index into that series' `data`.
    pub index: usize,
    pub datum: Datum,
    /// Where the datum was drawn, for anchoring a tooltip or crosshair.
    pub position: Point,
    /// Pixel distance from the queried point; zero when the pointer is
    /// inside the mark.
    pub distance: f32,
}

/// Spatial index over every plotted datum, in pixel space.
///
/// Deliberately a flat vector with linear scans. Charts that fit on a
/// screen have at most a few thousand *visible* marks, a scan of which is
/// well under a frame; a tree would add build cost to every render — which
/// happens far more often than a hover — to save time on the rarer
/// operation. If a scatter plot with 100k points ever needs this, the fix
/// is to bucket by x, not to reach for a general spatial structure.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct HitIndex {
    entries: Vec<HitEntry>,
    plot: Rect,
}

impl HitIndex {
    pub(crate) fn new(plot: Rect) -> Self {
        Self { entries: Vec::new(), plot }
    }

    /// Index a point marker at `at`.
    pub(crate) fn push(&mut self, at: Point, series: usize, index: usize, datum: Datum) {
        self.entries.push(HitEntry {
            shape: HitShape::Point,
            anchor: at,
            series,
            index,
            datum,
        });
    }

    /// Index a bar over its whole body, anchoring the tooltip at `anchor` —
    /// the outer end, where a pointer approaching from outside the column
    /// meets it first.
    pub(crate) fn push_rect(
        &mut self,
        rect: Rect,
        anchor: Point,
        series: usize,
        index: usize,
        datum: Datum,
    ) {
        self.entries.push(HitEntry {
            shape: HitShape::Rect(rect),
            anchor,
            series,
            index,
            datum,
        });
    }

    /// Index a wedge. `start` is clockwise from twelve o'clock, in radians.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_wedge(
        &mut self,
        center: Point,
        r0: f32,
        r1: f32,
        start: f32,
        sweep: f32,
        anchor: Point,
        series: usize,
        index: usize,
        datum: Datum,
    ) {
        self.entries.push(HitEntry {
            shape: HitShape::Wedge { center, r0, r1, start, sweep },
            anchor,
            series,
            index,
            datum,
        });
    }

    /// The data area this index was built against.
    pub fn plot(&self) -> Rect {
        self.plot
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn to_result(&self, e: &HitEntry, from: Point) -> HitResult {
        HitResult {
            series: e.series,
            index: e.index,
            datum: e.datum,
            position: e.anchor,
            distance: e.shape.distance(from, e.anchor),
        }
    }

    /// The datum whose mark actually covers `p`, or `None`.
    ///
    /// Later entries win, because they were painted later and are therefore
    /// what the user sees on top. This is the right query for bars, pie
    /// slices, and heatmap cells — anything with area. Point markers never
    /// match; use [`nearest_within`](Self::nearest_within) for those.
    pub fn contains(&self, p: Point) -> Option<HitResult> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.shape.contains(p, e.anchor))
            .map(|e| self.to_result(e, p))
    }

    /// Nearest datum by pixel distance, ignoring how far away it is. Use
    /// when the tooltip should always show something.
    pub fn nearest(&self, p: Point) -> Option<HitResult> {
        self.entries
            .iter()
            .map(|e| self.to_result(e, p))
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }

    /// Nearest datum, but only within `radius` pixels. Use when the tooltip
    /// should disappear as the pointer leaves the marks.
    pub fn nearest_within(&self, p: Point, radius: f32) -> Option<HitResult> {
        self.nearest(p).filter(|r| r.distance <= radius)
    }

    /// Whatever covers `p`, else the nearest mark within `radius`.
    ///
    /// The query a general tooltip wants: exact for marks with area,
    /// forgiving for the ones too small to aim at.
    pub fn pick(&self, p: Point, radius: f32) -> Option<HitResult> {
        self.contains(p).or_else(|| self.nearest_within(p, radius))
    }

    /// Every series' datum at the x nearest the pointer, ordered by series.
    ///
    /// This — not [`nearest`](Self::nearest) — is what a multi-series
    /// tooltip wants: hovering anywhere in a column should list all series
    /// at that x, not just the one whose mark happens to be closest
    /// vertically.
    ///
    /// Grouping is by DATA x, not pixel x. Grouped bars are the case that
    /// forces it: two series in the same category are deliberately drawn
    /// side by side, so their pixel positions differ by design, and a
    /// pixel-proximity grouping silently returns only the nearer bar — the
    /// tooltip then shows one series while the pointer visibly sits over a
    /// group of them.
    pub fn column_at(&self, p: Point) -> Vec<HitResult> {
        // Nearest in pixel space picks the column; its data x defines the
        // column's membership.
        let Some(target) = self
            .entries
            .iter()
            .min_by(|a, b| (a.anchor.x - p.x).abs().total_cmp(&(b.anchor.x - p.x).abs()))
            .map(|e| e.datum.x)
        else {
            return Vec::new();
        };
        let mut out: Vec<HitResult> = self
            .entries
            .iter()
            .filter(|e| e.datum.x == target)
            .map(|e| self.to_result(e, p))
            .collect();
        out.sort_by_key(|r| (r.series, r.index));
        out
    }
}
