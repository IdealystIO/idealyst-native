//! Pointer hit-testing.
//!
//! Produced as part of a render rather than recomputed on each pointer
//! move, because the mapping from pixel back to datum depends on the exact
//! domain and plot rect that render used. Recomputing it separately is how
//! tooltips end up pointing at the wrong datum one frame after a resize.

use crate::scene::{Point, Rect};
use crate::spec::Datum;

/// One datum's resolved screen position.
#[derive(Clone, Copy, PartialEq, Debug)]
struct HitPoint {
    at: Point,
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
    /// Pixel distance from the queried point.
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
    points: Vec<HitPoint>,
    plot: Rect,
}

impl HitIndex {
    pub(crate) fn new(plot: Rect) -> Self {
        Self { points: Vec::new(), plot }
    }

    pub(crate) fn push(&mut self, at: Point, series: usize, index: usize, datum: Datum) {
        self.points.push(HitPoint { at, series, index, datum });
    }

    /// The data area this index was built against.
    pub fn plot(&self) -> Rect {
        self.plot
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    fn to_result(&self, hp: &HitPoint, from: Point) -> HitResult {
        let (dx, dy) = (hp.at.x - from.x, hp.at.y - from.y);
        HitResult {
            series: hp.series,
            index: hp.index,
            datum: hp.datum,
            position: hp.at,
            distance: (dx * dx + dy * dy).sqrt(),
        }
    }

    /// Nearest datum by straight-line pixel distance, ignoring how far away
    /// it is. Use when the tooltip should always show something.
    pub fn nearest(&self, p: Point) -> Option<HitResult> {
        self.points
            .iter()
            .map(|hp| self.to_result(hp, p))
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }

    /// Nearest datum, but only within `radius` pixels. Use when the tooltip
    /// should disappear as the pointer leaves the marks.
    pub fn nearest_within(&self, p: Point, radius: f32) -> Option<HitResult> {
        self.nearest(p).filter(|r| r.distance <= radius)
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
            .points
            .iter()
            .min_by(|a, b| (a.at.x - p.x).abs().total_cmp(&(b.at.x - p.x).abs()))
            .map(|hp| hp.datum.x)
        else {
            return Vec::new();
        };
        let mut out: Vec<HitResult> = self
            .points
            .iter()
            .filter(|hp| hp.datum.x == target)
            .map(|hp| self.to_result(hp, p))
            .collect();
        out.sort_by_key(|r| (r.series, r.index));
        out
    }
}
