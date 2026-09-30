//! Dash geometry for [`BorderStyle::Dashed`] / [`BorderStyle::Dotted`]
//! borders — the one source of truth for every backend that strokes a
//! border itself.
//!
//! Web, SSR and email hand `border-style` to the browser. Every other
//! backend draws the pattern, and if each picked its own dash length the
//! same author tree would show fat dashes on Android and fine ones on
//! iOS — the per-platform drift CLAUDE.md §7 rules out. So the lengths,
//! the fitting, and the path walk all live here and are unit-tested
//! once on the host.
//!
//! ## Proportions
//!
//! A dash is `3w` long with a `3w` gap and a dot is one border-width
//! round with a one-width gap, where `w` is the border width. That is
//! close to what the desktop browsers draw for the common 1–2px border,
//! so a web build and a native build of the same screen read alike.
//! Lengths scale with the width: a fixed-pixel dash would be a smear on
//! a 4px border and invisible on a hairline.
//!
//! ## Fitting
//!
//! A raw `3w/3w` pattern almost never divides a box's perimeter evenly,
//! which leaves a stubby half-dash or a doubled gap wherever the loop
//! closes (and, per side, a clipped dash at each corner). Browsers
//! stretch the GAP so the pattern fits; so does [`fit_closed`] (whole
//! periods around a loop) and [`fit_open`] (dash at both ends of a
//! side). The dash itself keeps its length — for a dot that length is
//! the border width, which must not change.
//!
//! ## Two shapes
//!
//! Backends already split borders into a *uniform* case (all sides the
//! same width and colour — one stroke that follows the corner radius)
//! and an *asymmetric* case (straight per-side bars). A patterned border
//! follows the same split:
//!
//! * uniform → one closed loop, [`LoopPath`], stroked along the border's
//!   centreline (inset `w/2`, radii shrunk by `w/2` — the CSS geometry);
//! * asymmetric → one open line per side with a width, fitted per side
//!   by [`fit_open`] and walked by [`side_line`].
//!
//! Toolkits with a native dashed stroke (CoreAnimation, Android `Paint`,
//! Cairo/GSK, GDI+) take [`Dash::toolkit_array`] and the path. Renderers
//! that draw only rects and circles (the GPU engine, the CPU rasterizer)
//! take [`dash_runs`] and sample the path with [`LoopPath::point_at`].
//!
//! [`BorderStyle::Dashed`]: crate::BorderStyle::Dashed
//! [`BorderStyle::Dotted`]: crate::BorderStyle::Dotted

use crate::BorderStyle;

/// Dash length, in multiples of the border width.
pub const DASH_ON: f32 = 3.0;
/// Gap after a dash, in multiples of the border width (before fitting).
pub const DASH_OFF: f32 = 3.0;
/// Dot diameter, in multiples of the border width.
pub const DOT_ON: f32 = 1.0;
/// Gap between dots, in multiples of the border width (before fitting).
pub const DOT_OFF: f32 = 1.0;

/// A dash pattern in *visual* terms: `on` is how long each mark looks
/// on screen (a dot's `on` is its diameter), `off` the clear space
/// between marks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dash {
    pub on: f32,
    pub off: f32,
    /// Dots are round: toolkits draw them as zero-length dashes with a
    /// round cap. Dashes are square-ended (butt caps).
    pub round: bool,
}

impl Dash {
    /// `on + off`.
    pub fn period(&self) -> f32 {
        self.on + self.off
    }

    /// The `[on, off]` array to hand a toolkit's dashed stroke, and how
    /// far to shift it ([`Dash::toolkit_phase`]).
    ///
    /// A round cap grows each dash by half the line width at both ends,
    /// so a dot is expressed as a ZERO-length dash and its gap as the
    /// full period — otherwise every dot would come out as a pill `2w`
    /// long. CoreAnimation, Skia (Android) and Cairo all draw a
    /// zero-length round-capped dash as a dot.
    pub fn toolkit_array(&self) -> [f32; 2] {
        if self.round {
            [0.0, self.period()]
        } else {
            [self.on, self.off]
        }
    }

    /// Dash phase that puts the centre of the first mark at the path's
    /// start point, matching [`dash_runs`]. With a round cap the zero-length
    /// dash is already centred, so no shift is needed.
    pub fn toolkit_phase(&self) -> f32 {
        if self.round {
            0.0
        } else {
            self.on / 2.0
        }
    }

    /// Dash phase for an OPEN line (a per-side border), matching
    /// [`dash_runs`]' open placement: the first mark STARTS at the line's
    /// start point instead of straddling it. A dash already starts the
    /// pattern, so no shift. A dot is a zero-length dash drawn at the
    /// pattern start, but its centre belongs `on / 2` in — so the pattern
    /// is shifted back by a period less that half-dot.
    pub fn open_toolkit_phase(&self) -> f32 {
        if self.round {
            self.period() - self.on / 2.0
        } else {
            0.0
        }
    }
}

/// The unfitted pattern for a style at a border width, or `None` for a
/// solid border (and for a zero or negative width, which draws nothing).
pub fn base(style: BorderStyle, width: f32) -> Option<Dash> {
    if !(width > 0.0) {
        return None;
    }
    match style {
        BorderStyle::Solid => None,
        BorderStyle::Dashed => {
            Some(Dash { on: DASH_ON * width, off: DASH_OFF * width, round: false })
        }
        BorderStyle::Dotted => {
            Some(Dash { on: DOT_ON * width, off: DOT_OFF * width, round: true })
        }
    }
}

/// Stretch the gap so a CLOSED loop of `length` holds a whole number of
/// periods. The loop then closes seamlessly wherever the toolkit starts
/// the path.
pub fn fit_closed(dash: Dash, length: f32) -> Dash {
    if !(length > 0.0) {
        return dash;
    }
    let n = (length / dash.period()).round().max(1.0);
    let off = (length / n - dash.on).max(0.0);
    Dash { off, ..dash }
}

/// Stretch the gap so an OPEN line of `length` starts and ends on a
/// dash: `n` dashes and `n - 1` gaps exactly fill it. A side too short
/// for two dashes is one mark centred on it.
pub fn fit_open(dash: Dash, length: f32) -> Dash {
    if !(length > 0.0) {
        return dash;
    }
    // n·on + (n-1)·off = length  →  n = (length + off) / period.
    let mut n = ((length + dash.off) / dash.period()).round().max(1.0);
    while n > 1.0 && n * dash.on > length {
        n -= 1.0;
    }
    if n <= 1.0 {
        return Dash { off: length, ..dash };
    }
    let off = (length - n * dash.on) / (n - 1.0);
    Dash { off, ..dash }
}

/// One visible mark, as a span of distance along the path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DashRun {
    pub start: f32,
    pub end: f32,
}

impl DashRun {
    /// Distance of the mark's centre along the path — where a dot is
    /// drawn.
    pub fn mid(&self) -> f32 {
        (self.start + self.end) / 2.0
    }
}

/// Every mark a fitted `dash` draws along a path of `length`.
///
/// Mark `i` is centred at `i · period` (open lines shift by `on / 2` so
/// the first mark starts at 0). On a closed loop the first mark straddles
/// the start point, so its leading half is reported as a run at the END
/// of the loop — callers never see a negative distance.
pub fn dash_runs(dash: Dash, length: f32, closed: bool) -> Vec<DashRun> {
    let mut out = Vec::new();
    if !(length > 0.0) || !(dash.period() > 0.0) {
        return out;
    }
    let half = dash.on / 2.0;
    let offset = if closed { 0.0 } else { half };
    let period = dash.period();
    // `+ 0.5` absorbs float error in a fitted period that divides
    // `length` exactly.
    let count = ((length - offset + half) / period + 0.5).floor().max(0.0) as usize + 1;
    for i in 0..count {
        let centre = offset + i as f32 * period;
        let start = centre - half;
        let end = centre + half;
        if closed {
            if centre >= length - period * 1e-3 {
                break;
            }
            if start < 0.0 {
                // Wraps: [length + start, length) and [0, end).
                out.push(DashRun { start: 0.0, end });
                out.push(DashRun { start: length + start, end: length });
            } else {
                out.push(DashRun { start, end: end.min(length) });
            }
        } else {
            if start >= length {
                break;
            }
            out.push(DashRun { start: start.max(0.0), end: end.min(length) });
        }
    }
    out
}

/// A border's centreline as a closed rounded rectangle, walked clockwise
/// from the LEFT end of the top edge (just past the top-left corner).
///
/// Built from the border box, not the centreline: [`LoopPath::new`]
/// insets by half the stroke and shrinks each radius to match, which is
/// the curve a CSS border's middle follows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopPath {
    /// Centreline rect origin + size.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Centreline corner radii, `[top_left, top_right, bottom_right, bottom_left]`.
    pub radii: [f32; 4],
}

impl LoopPath {
    /// The centreline of a `stroke`-wide border drawn inside a
    /// `box_w × box_h` box whose OUTER corner radii are `radii`
    /// (`[tl, tr, br, bl]`).
    pub fn new(box_w: f32, box_h: f32, radii: [f32; 4], stroke: f32) -> Self {
        let inset = stroke / 2.0;
        let w = (box_w - stroke).max(0.0);
        let h = (box_h - stroke).max(0.0);
        let cap = w.min(h) / 2.0;
        let radii = radii.map(|r| (r - inset).max(0.0).min(cap));
        Self { x: inset, y: inset, w, h, radii }
    }

    /// Total length of the loop.
    pub fn length(&self) -> f32 {
        let [tl, tr, br, bl] = self.radii;
        let straight = (self.w - tl - tr)
            + (self.h - tr - br)
            + (self.w - br - bl)
            + (self.h - bl - tl);
        let arcs = (tl + tr + br + bl) * std::f32::consts::FRAC_PI_2;
        straight.max(0.0) + arcs
    }

    /// The point at distance `s` along the loop (wrapped into range) and
    /// the direction of travel there, in radians (0 = +x, clockwise on a
    /// y-down screen is positive).
    pub fn point_at(&self, s: f32) -> (f32, f32, f32) {
        use std::f32::consts::{FRAC_PI_2, PI};
        let len = self.length();
        let mut s = if len > 0.0 { s.rem_euclid(len) } else { 0.0 };
        let [tl, tr, br, bl] = self.radii;
        let (x0, y0, x1, y1) = (self.x, self.y, self.x + self.w, self.y + self.h);

        // Each piece: (length, point-at-local-t).
        // 1. top edge, left → right
        let top = (self.w - tl - tr).max(0.0);
        if s <= top {
            return (x0 + tl + s, y0, 0.0);
        }
        s -= top;
        // 2. top-right arc, from angle -π/2 to 0 around its centre
        let a = tr * FRAC_PI_2;
        if s <= a {
            let t = if tr > 0.0 { s / tr } else { 0.0 };
            let ang = -FRAC_PI_2 + t;
            return (x1 - tr + tr * ang.cos(), y0 + tr + tr * ang.sin(), ang + FRAC_PI_2);
        }
        s -= a;
        // 3. right edge, top → bottom
        let right = (self.h - tr - br).max(0.0);
        if s <= right {
            return (x1, y0 + tr + s, FRAC_PI_2);
        }
        s -= right;
        // 4. bottom-right arc, 0 → π/2
        let a = br * FRAC_PI_2;
        if s <= a {
            let t = if br > 0.0 { s / br } else { 0.0 };
            return (x1 - br + br * t.cos(), y1 - br + br * t.sin(), t + FRAC_PI_2);
        }
        s -= a;
        // 5. bottom edge, right → left
        let bottom = (self.w - br - bl).max(0.0);
        if s <= bottom {
            return (x1 - br - s, y1, PI);
        }
        s -= bottom;
        // 6. bottom-left arc, π/2 → π
        let a = bl * FRAC_PI_2;
        if s <= a {
            let t = if bl > 0.0 { s / bl } else { 0.0 };
            let ang = FRAC_PI_2 + t;
            return (x0 + bl + bl * ang.cos(), y1 - bl + bl * ang.sin(), ang + FRAC_PI_2);
        }
        s -= a;
        // 7. left edge, bottom → top
        let left = (self.h - bl - tl).max(0.0);
        if s <= left {
            return (x0, y1 - bl - s, -FRAC_PI_2);
        }
        s -= left;
        // 8. top-left arc, π → 3π/2
        let t = if tl > 0.0 { (s / tl).min(FRAC_PI_2) } else { 0.0 };
        let ang = PI + t;
        (x0 + tl + tl * ang.cos(), y0 + tl + tl * ang.sin(), ang + FRAC_PI_2)
    }

    /// A polyline through the loop from `start` to `end` (distances along
    /// it), with a vertex at least every `max_step` px so a mark that
    /// bends round a corner stays on the curve. Straight stretches come
    /// out as their two end points plus the steps.
    pub fn polyline(&self, start: f32, end: f32, max_step: f32) -> Vec<(f32, f32)> {
        let span = (end - start).max(0.0);
        let steps = ((span / max_step.max(0.25)).ceil() as usize).max(1);
        (0..=steps)
            .map(|i| {
                let (x, y, _) = self.point_at(start + span * i as f32 / steps as f32);
                (x, y)
            })
            .collect()
    }
}

/// The centreline of one side's border in an asymmetric (per-side)
/// border: a straight line across the full side, inset by half that
/// side's width. `side` is `0..4` = top, right, bottom, left. Returns
/// `(x0, y0, x1, y1)` running clockwise, matching [`LoopPath`].
pub fn side_line(box_w: f32, box_h: f32, side: usize, width: f32) -> (f32, f32, f32, f32) {
    let i = width / 2.0;
    match side {
        0 => (0.0, i, box_w, i),
        1 => (box_w - i, 0.0, box_w - i, box_h),
        2 => (box_w, box_h - i, 0.0, box_h - i),
        _ => (i, box_h, i, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn solid_and_zero_width_have_no_pattern() {
        assert_eq!(base(BorderStyle::Solid, 2.0), None);
        assert_eq!(base(BorderStyle::Dashed, 0.0), None);
        assert_eq!(base(BorderStyle::Dotted, -1.0), None);
    }

    #[test]
    fn pattern_scales_with_width() {
        let d = base(BorderStyle::Dashed, 2.0).unwrap();
        assert_eq!((d.on, d.off, d.round), (6.0, 6.0, false));
        let d = base(BorderStyle::Dotted, 2.0).unwrap();
        assert_eq!((d.on, d.off, d.round), (2.0, 2.0, true));
    }

    #[test]
    fn toolkit_array_expresses_dots_as_zero_length_round_dashes() {
        // A round cap adds w/2 at each end; a dot passed as `[w, w]`
        // would render as a 2w pill.
        let dot = base(BorderStyle::Dotted, 2.0).unwrap();
        assert_eq!(dot.toolkit_array(), [0.0, 4.0]);
        assert_eq!(dot.toolkit_phase(), 0.0);
        let dash = base(BorderStyle::Dashed, 1.0).unwrap();
        assert_eq!(dash.toolkit_array(), [3.0, 3.0]);
        assert_eq!(dash.toolkit_phase(), 1.5);
    }

    /// Simulate a toolkit dasher (`[on, off]` array, pattern shifted by
    /// `phase`) on an open line and check its marks land where
    /// `dash_runs` says: the first mark starts the side rather than
    /// straddling its corner.
    #[test]
    fn open_toolkit_phase_matches_dash_runs() {
        for style in [BorderStyle::Dashed, BorderStyle::Dotted] {
            let len = 50.0;
            let d = fit_open(base(style, 2.0).unwrap(), len);
            let [on, off] = d.toolkit_array();
            let period = on + off;
            let phase = d.open_toolkit_phase();
            // The toolkit draws pattern mark j at j·period − phase; the
            // ones before the line's start are clipped away.
            let drawn: Vec<f32> = (0..64)
                .map(|j| j as f32 * period - phase + on / 2.0)
                .filter(|c| *c >= -1e-3 && *c <= len + 1e-3)
                .collect();
            let expected: Vec<f32> = dash_runs(d, len, false).iter().map(|r| r.mid()).collect();
            assert_eq!(drawn.len(), expected.len(), "{style:?}: {drawn:?} vs {expected:?}");
            for (k, (got, want)) in drawn.iter().zip(&expected).enumerate() {
                assert!(close(*got, *want), "{style:?} mark {k}: {got} vs {want}");
            }
        }
    }

    #[test]
    fn closed_fit_divides_the_loop_into_whole_periods() {
        let d = base(BorderStyle::Dashed, 1.0).unwrap(); // period 6
        let fitted = fit_closed(d, 100.0);
        assert_eq!(fitted.on, 3.0, "the dash keeps its length; only the gap stretches");
        let n = 100.0 / fitted.period();
        assert!(close(n, n.round()), "100 / {} is not whole", fitted.period());
        assert_eq!(n.round(), 17.0);
    }

    #[test]
    fn open_fit_starts_and_ends_on_a_dash() {
        let d = base(BorderStyle::Dashed, 1.0).unwrap();
        let fitted = fit_open(d, 40.0);
        let runs = dash_runs(fitted, 40.0, false);
        assert!(close(runs.first().unwrap().start, 0.0));
        assert!(close(runs.last().unwrap().end, 40.0), "last run {:?}", runs.last());
        for r in &runs {
            assert!(close(r.end - r.start, 3.0), "every dash is full length: {r:?}");
        }
    }

    #[test]
    fn open_fit_on_a_side_too_short_for_two_dashes_is_one_mark() {
        let d = base(BorderStyle::Dashed, 1.0).unwrap();
        let fitted = fit_open(d, 4.0);
        let runs = dash_runs(fitted, 4.0, false);
        assert_eq!(runs.len(), 1);
    }

    #[test]
    fn closed_runs_cover_the_right_fraction_of_the_loop() {
        let len = 120.0;
        let fitted = fit_closed(base(BorderStyle::Dashed, 1.0).unwrap(), len);
        let runs = dash_runs(fitted, len, true);
        let inked: f32 = runs.iter().map(|r| r.end - r.start).sum();
        let periods = (len / fitted.period()).round();
        assert!(close(inked, periods * fitted.on), "inked {inked}");
        assert!(runs.iter().all(|r| r.start >= 0.0 && r.end <= len + 1e-3));
    }

    #[test]
    fn closed_first_mark_wraps_across_the_start() {
        let len = 60.0;
        let fitted = fit_closed(base(BorderStyle::Dashed, 2.0).unwrap(), len);
        let runs = dash_runs(fitted, len, true);
        assert_eq!(runs[0], DashRun { start: 0.0, end: 3.0 });
        assert_eq!(runs[1], DashRun { start: 57.0, end: 60.0 });
    }

    #[test]
    fn loop_length_of_a_square_box_is_its_inset_perimeter() {
        let p = LoopPath::new(20.0, 10.0, [0.0; 4], 2.0);
        assert!(close(p.length(), 2.0 * (18.0 + 8.0)));
    }

    #[test]
    fn loop_length_of_a_pill_counts_its_arcs() {
        // 100×22 pill with radius 11, 1px border: centreline radius 10.5,
        // straight top/bottom of 99 - 21 = 78, two half circles.
        let p = LoopPath::new(100.0, 22.0, [11.0; 4], 1.0);
        let expected = 2.0 * 78.0 + 2.0 * std::f32::consts::PI * 10.5;
        assert!(close(p.length(), expected), "{} vs {expected}", p.length());
    }

    #[test]
    fn point_at_walks_clockwise_from_the_top_left() {
        let p = LoopPath::new(20.0, 10.0, [0.0; 4], 0.0);
        assert_eq!(p.point_at(0.0), (0.0, 0.0, 0.0));
        let (x, y, _) = p.point_at(20.0);
        assert!(close(x, 20.0) && close(y, 0.0));
        let (x, y, dir) = p.point_at(25.0);
        assert!(close(x, 20.0) && close(y, 5.0) && close(dir, std::f32::consts::FRAC_PI_2));
        let (x, y, dir) = p.point_at(40.0);
        assert!(close(x, 10.0) && close(y, 10.0) && close(dir, std::f32::consts::PI));
        let (x, y, _) = p.point_at(50.0);
        assert!(close(x, 0.0) && close(y, 10.0));
        let (x, y, _) = p.point_at(60.0);
        assert!(close(x, 0.0) && close(y, 0.0), "wraps to the start");
    }

    #[test]
    fn point_at_follows_a_rounded_corner() {
        let p = LoopPath::new(40.0, 40.0, [10.0; 4], 0.0);
        let top = 20.0;
        let quarter = 10.0 * std::f32::consts::FRAC_PI_2;
        // Halfway round the top-right arc sits on the 45° diagonal.
        let (x, y, _) = p.point_at(top + quarter / 2.0);
        let d = 10.0 * std::f32::consts::FRAC_1_SQRT_2;
        assert!(close(x, 30.0 + d) && close(y, 10.0 - d), "({x}, {y})");
        // The end of the arc is the top of the right edge.
        let (x, y, _) = p.point_at(top + quarter);
        assert!(close(x, 40.0) && close(y, 10.0));
    }

    #[test]
    fn radii_clamp_to_the_centreline_box() {
        let p = LoopPath::new(10.0, 10.0, [100.0; 4], 2.0);
        assert!(p.radii.iter().all(|r| close(*r, 4.0)));
    }

    #[test]
    fn side_lines_sit_half_a_width_inside_each_edge() {
        assert_eq!(side_line(10.0, 6.0, 0, 2.0), (0.0, 1.0, 10.0, 1.0));
        assert_eq!(side_line(10.0, 6.0, 1, 2.0), (9.0, 0.0, 9.0, 6.0));
        assert_eq!(side_line(10.0, 6.0, 2, 2.0), (10.0, 5.0, 0.0, 5.0));
        assert_eq!(side_line(10.0, 6.0, 3, 2.0), (1.0, 6.0, 1.0, 0.0));
    }
}
