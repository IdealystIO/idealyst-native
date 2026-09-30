//! Dashed / dotted borders — WHAT to paint, decided without GDI+.
//!
//! [`plan`] turns a view's border sides into explicit marks (one
//! polyline per dash, one circle per dot); `scene::paint_border` draws
//! them with a flat-capped solid pen and a solid fill. All geometry —
//! dash lengths, gap fitting, the centreline loop, where each mark sits
//! — comes from [`runtime_shared::border_dash`], the source every native
//! backend draws from (CLAUDE.md §7).
//!
//! ## Why explicit marks, not `GdipSetPenDashArray`
//!
//! GDI+ has a dashed pen, but two of its semantics can't be pinned down
//! to the shared geometry without a real Windows box to measure on:
//!
//! - **Dots.** The toolkit convention (`Dash::toolkit_array`) is a
//!   ZERO-length dash with a round cap. `GdipSetPenDashArray([0, 4])`
//!   returns `InvalidParameter` under Wine's gdiplus (probed while
//!   writing this; `[0.001, 4]` is accepted), and Microsoft does not
//!   document what native GDI+ does with a 0 element. The fallback, a
//!   tiny positive on-length with `DashCapRound`, relies on how GDI+
//!   sizes round dash caps relative to the dash, which is not documented
//!   precisely enough to promise a dot exactly one border-width across.
//! - **Phase.** `GdipSetPenDashOffset` is documented only as "the
//!   distance from the start of the line to the beginning of the dash
//!   pattern", in pen widths — its sign convention against Cairo/Skia's
//!   is not stated, and the uniform loop needs the first dash CENTRED on
//!   the path start (`Dash::toolkit_phase`) to land where every other
//!   backend lands it.
//!
//! Explicit marks sidestep both: the output depends only on
//! `GdipDrawLines` with flat caps and `GdipFillEllipse`, whose geometry
//! is unambiguous, and the mark positions are `border_dash`'s own. The
//! cost is one draw call per mark — a few dozen on a typical box.

use runtime_shared::border_dash::{self, Dash, LoopPath};
use runtime_shared::color::Rgba;
use runtime_shared::BorderStyle;

use crate::{uniform_border, BorderSide};

/// A dash that bends round a rounded corner is drawn as a polyline with
/// a vertex at least this often (px), so it stays on the curve.
const ARC_STEP: f32 = 1.0;

/// Below this width a side draws nothing — the same threshold
/// `scene::paint_side_borders` uses for solid per-side bars.
const MIN_SIDE_WIDTH: f32 = 0.5;

/// One mark of a patterned border, in the box's own space.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BorderMark {
    /// One dash: stroke `points` as an open polyline, flat caps, `width`
    /// wide. A straight dash is two points.
    Dash { points: Vec<(f32, f32)>, width: f32, color: Rgba },
    /// One dot: a filled circle of `radius` (half the border width).
    Dot { cx: f32, cy: f32, radius: f32, color: Rgba },
}

/// The marks for a patterned border on a `w × h` box with outer corner
/// radii `radii` (`[tl, tr, br, bl]`), or `None` when the style is solid
/// — the caller then draws the existing solid path, unchanged.
///
/// Same uniform / asymmetric split as the solid path: a uniform border
/// ([`uniform_border`]) is one fitted closed loop that follows the
/// corners; otherwise each side is one straight fitted line with its own
/// width and colour (a side without a colour falls back to the first
/// side that has one, as for solid bars).
pub(crate) fn plan(
    sides: &[BorderSide; 4],
    radii: [f32; 4],
    style: BorderStyle,
    w: f32,
    h: f32,
) -> Option<Vec<BorderMark>> {
    if style == BorderStyle::Solid {
        return None;
    }
    let mut marks = Vec::new();
    if !(w > 0.0 && h > 0.0) {
        return Some(marks);
    }
    if let Some((width, color)) = uniform_border(sides) {
        let Some(dash) = border_dash::base(style, width) else { return Some(marks) };
        let path = LoopPath::new(w, h, radii, width);
        let len = path.length();
        if !(len > 0.0) {
            return Some(marks);
        }
        let dash = border_dash::fit_closed(dash, len);
        for centre in loop_centres(dash, len) {
            if dash.round {
                let (cx, cy, _) = path.point_at(centre);
                marks.push(BorderMark::Dot { cx, cy, radius: width / 2.0, color });
            } else {
                // `point_at` wraps, so the first dash — which straddles the
                // start — is one continuous polyline rather than the two
                // halves `dash_runs` reports for it.
                let half = dash.on / 2.0;
                let points = loop_polyline(&path, centre - half, centre + half);
                marks.push(BorderMark::Dash { points, width, color });
            }
        }
        return Some(marks);
    }
    let fallback = sides.iter().find_map(|s| s.color);
    for (idx, side) in sides.iter().enumerate() {
        if side.width <= MIN_SIDE_WIDTH {
            continue;
        }
        let Some(color) = side.color.or(fallback).filter(|c| c.a > 0) else { continue };
        let Some(dash) = border_dash::base(style, side.width) else { continue };
        let (x0, y0, x1, y1) = border_dash::side_line(w, h, idx, side.width);
        let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
        if !(len > 0.0) {
            continue;
        }
        let dash = border_dash::fit_open(dash, len);
        let at = |s: f32| (x0 + (x1 - x0) * s / len, y0 + (y1 - y0) * s / len);
        for run in border_dash::dash_runs(dash, len, false) {
            if dash.round {
                let (cx, cy) = at(run.mid());
                marks.push(BorderMark::Dot { cx, cy, radius: side.width / 2.0, color });
            } else {
                let points = vec![at(run.start), at(run.end)];
                marks.push(BorderMark::Dash { points, width: side.width, color });
            }
        }
    }
    Some(marks)
}

/// A polyline through `path` from `start` to `end` (distances along it,
/// `start` may be negative — `point_at` wraps) with a vertex every
/// [`ARC_STEP`] AND at every corner break the span crosses.
///
/// `LoopPath::polyline` samples at even steps only, so a dash across a
/// SQUARE corner would get a chord from just before the corner to just
/// after it: a chamfered, shortened dash instead of an L. Adding the
/// breaks (where each straight edge meets its arc — the same point
/// twice on a square corner) keeps the corner vertex exact.
fn loop_polyline(path: &LoopPath, start: f32, end: f32) -> Vec<(f32, f32)> {
    let len = path.length();
    let steps = (((end - start) / ARC_STEP).ceil() as usize).max(1);
    let mut ts: Vec<f32> =
        (0..=steps).map(|i| start + (end - start) * i as f32 / steps as f32).collect();
    for b in corner_breaks(path) {
        for t in [b - len, b, b + len] {
            if t > start && t < end {
                ts.push(t);
            }
        }
    }
    ts.sort_by(|a, b| a.total_cmp(b));
    ts.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    ts.into_iter()
        .map(|t| {
            let (x, y, _) = path.point_at(t);
            (x, y)
        })
        .collect()
}

/// Distance along `path` of each place a straight edge meets a corner
/// arc, in `LoopPath::point_at`'s walk order (top edge first, clockwise).
fn corner_breaks(p: &LoopPath) -> [f32; 8] {
    use std::f32::consts::FRAC_PI_2;
    let [tl, tr, br, bl] = p.radii;
    let pieces = [
        (p.w - tl - tr).max(0.0),
        tr * FRAC_PI_2,
        (p.h - tr - br).max(0.0),
        br * FRAC_PI_2,
        (p.w - br - bl).max(0.0),
        bl * FRAC_PI_2,
        (p.h - bl - tl).max(0.0),
        tl * FRAC_PI_2,
    ];
    let mut out = [0.0; 8];
    let mut acc = 0.0;
    for (i, piece) in pieces.iter().enumerate() {
        acc += piece;
        out[i] = acc;
    }
    out
}

/// Distances along a closed loop at which a fitted pattern's marks are
/// centred: `i · period` for whole periods (`fit_closed` made the period
/// divide `len`) — the same marks `border_dash::dash_runs` reports with
/// `closed = true`.
fn loop_centres(dash: Dash, len: f32) -> impl Iterator<Item = f32> {
    let n = (len / dash.period()).round().max(1.0) as usize;
    (0..n).map(move |i| i as f32 * dash.period())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red() -> Rgba {
        Rgba::new(255, 0, 0, 255)
    }
    fn blue() -> Rgba {
        Rgba::new(0, 0, 255, 255)
    }
    fn side(width: f32, color: Option<Rgba>) -> BorderSide {
        BorderSide { width, color }
    }
    fn uniform(width: f32) -> [BorderSide; 4] {
        [side(width, Some(red())); 4]
    }
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }
    fn dash_len(points: &[(f32, f32)]) -> f32 {
        points.windows(2).map(|p| ((p[1].0 - p[0].0).powi(2) + (p[1].1 - p[0].1).powi(2)).sqrt()).sum()
    }

    #[test]
    fn solid_style_has_no_plan() {
        assert_eq!(plan(&uniform(2.0), [0.0; 4], BorderStyle::Solid, 50.0, 20.0), None);
    }

    #[test]
    fn uniform_dashed_border_marks_match_the_shared_dash_runs() {
        let (w, h, width) = (60.0, 30.0, 2.0);
        let marks = plan(&uniform(width), [0.0; 4], BorderStyle::Dashed, w, h).unwrap();
        let path = LoopPath::new(w, h, [0.0; 4], width);
        let fitted = border_dash::fit_closed(border_dash::base(BorderStyle::Dashed, width).unwrap(), path.length());
        // `dash_runs` splits the first (wrapping) dash in two; we draw it whole.
        let runs = border_dash::dash_runs(fitted, path.length(), true);
        assert_eq!(marks.len(), runs.len() - 1);
        for mark in &marks {
            let BorderMark::Dash { points, width: pw, color } = mark else { panic!("{mark:?}") };
            assert_eq!((*pw, *color), (width, red()));
            assert!(close(dash_len(points), 6.0), "every dash is 3 widths: {points:?}");
        }
        // The first dash is centred on the loop's start (1, 1) — the left
        // end of the top edge on a square box.
        let BorderMark::Dash { points, .. } = &marks[0] else { panic!() };
        let mid = points[points.len() / 2];
        assert!(close(mid.0, 1.0) && close(mid.1, 1.0), "first dash centred on the start: {points:?}");
    }

    #[test]
    fn uniform_dash_bends_round_a_rounded_corner() {
        let marks = plan(&uniform(1.0), [12.0; 4], BorderStyle::Dashed, 80.0, 40.0).unwrap();
        let path = LoopPath::new(80.0, 40.0, [12.0; 4], 1.0);
        for mark in &marks {
            let BorderMark::Dash { points, .. } = mark else { panic!() };
            assert!(points.len() >= 3, "a dash is sampled along the curve: {points:?}");
            // Every vertex sits on the centreline: within the arc's radius
            // of a corner centre or on a straight edge.
            for (x, y) in points {
                let on_edge = close(*y, path.y) || close(*y, path.y + path.h) || close(*x, path.x) || close(*x, path.x + path.w);
                let r = path.radii[0];
                let corners = [
                    (path.x + r, path.y + r),
                    (path.x + path.w - r, path.y + r),
                    (path.x + path.w - r, path.y + path.h - r),
                    (path.x + r, path.y + path.h - r),
                ];
                let on_arc = corners.iter().any(|(cx, cy)| close(((x - cx).powi(2) + (y - cy).powi(2)).sqrt(), r));
                assert!(on_edge || on_arc, "({x}, {y}) is off the centreline");
            }
        }
    }

    #[test]
    fn uniform_dotted_border_is_one_width_dots_at_whole_periods() {
        let marks = plan(&uniform(2.0), [0.0; 4], BorderStyle::Dotted, 50.0, 30.0).unwrap();
        // Loop 2·(48 + 28) = 152, dot period 4 → 38 dots.
        assert_eq!(marks.len(), 38);
        assert_eq!(marks[0], BorderMark::Dot { cx: 1.0, cy: 1.0, radius: 1.0, color: red() });
    }

    #[test]
    fn asymmetric_dashed_side_starts_and_ends_on_a_dash() {
        // A bottom-only divider.
        let sides = [side(0.0, None), side(0.0, None), side(1.0, Some(blue())), side(0.0, None)];
        let marks = plan(&sides, [0.0; 4], BorderStyle::Dashed, 40.0, 10.0).unwrap();
        let BorderMark::Dash { points: first, width, color } = &marks[0] else { panic!() };
        assert_eq!((*width, *color), (1.0, blue()));
        assert_eq!(first[0], (40.0, 9.5), "bottom runs right → left, clockwise");
        let BorderMark::Dash { points: last, .. } = marks.last().unwrap() else { panic!() };
        assert!(close(last[1].0, 0.0) && close(last[1].1, 9.5), "{last:?}");
        assert!(marks.iter().all(|m| matches!(m, BorderMark::Dash { points, .. } if close(dash_len(points), 3.0))));
    }

    #[test]
    fn asymmetric_dotted_sides_keep_their_own_width_and_colour() {
        let sides = [side(2.0, Some(red())), side(0.0, None), side(0.0, None), side(4.0, Some(blue()))];
        let marks = plan(&sides, [0.0; 4], BorderStyle::Dotted, 40.0, 20.0).unwrap();
        let top: Vec<_> = marks.iter().filter(|m| matches!(m, BorderMark::Dot { color, .. } if *color == red())).collect();
        let left: Vec<_> = marks.iter().filter(|m| matches!(m, BorderMark::Dot { color, .. } if *color == blue())).collect();
        assert_eq!(top.len() + left.len(), marks.len());
        assert_eq!(*top[0], BorderMark::Dot { cx: 1.0, cy: 1.0, radius: 1.0, color: red() });
        let BorderMark::Dot { cx, radius, .. } = top.last().unwrap() else { panic!() };
        assert!(close(*cx, 39.0), "last dot ends the side: {cx}");
        let BorderMark::Dot { radius: lr, .. } = left[0] else { panic!() };
        assert_eq!((*radius, *lr), (1.0, 2.0));
    }

    #[test]
    fn asymmetric_side_without_colour_falls_back_like_solid_bars() {
        let sides = [side(1.0, Some(red())), side(0.0, None), side(2.0, None), side(0.0, None)];
        let marks = plan(&sides, [0.0; 4], BorderStyle::Dashed, 40.0, 20.0).unwrap();
        assert!(marks.iter().all(|m| matches!(m, BorderMark::Dash { color, .. } if *color == red())));
        assert!(marks.iter().any(|m| matches!(m, BorderMark::Dash { width, .. } if *width == 2.0)));
    }
}
