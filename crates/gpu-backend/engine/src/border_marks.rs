//! Dashed / dotted borders for the rect pipeline.
//!
//! The rect shader paints a border as a uniform SDF ring and its vertex
//! attribute budget is already at WebGPU's portable max, so a dash
//! pattern cannot be taught to the shader without dropping something
//! else. Instead a patterned border is drawn as extra `RectInstance`s:
//! the node's own rect is staged fill-only and every mark becomes a
//! small rect using the instance's existing `rotation` field.
//!
//! All pattern maths (lengths, fitting, the path walk) comes from
//! `runtime_shared::border_dash` so the GPU engine draws the same dashes
//! every other backend does (CLAUDE.md §7). This module only turns runs
//! along the path into oriented rects.
//!
//! ## Shapes
//!
//! * **Uniform** border (all four widths AND colours equal) → one closed
//!   [`LoopPath`] fitted with `fit_closed`, following the corner radii.
//! * **Asymmetric** → one open line per side (`side_line` + `fit_open`),
//!   each with its own width and colour; radii are not followed, which
//!   is the shape every other backend's per-side case draws.
//!
//!   Note the SOLID asymmetric border on this engine is still the
//!   shader's uniform ring in side 0's width/colour (the pre-existing
//!   limitation of `rect.wgsl`); a patterned asymmetric border is drawn
//!   per side, i.e. correctly, because the marks are plain rects.
//!
//! ## Corners
//!
//! A dash that crosses a rounded corner is split at the arc's ends; the
//! arc part becomes chord rects (see [`CHORD_ANGLE`]). A dash that
//! crosses a SHARP corner is two straight rects meeting at the corner
//! point: the incoming one is extended by `w/2` and the outgoing one
//! trimmed by `w/2`, so the outer corner square is inked exactly once
//! (no notch, and no double-blended overlap for translucent colours).

use runtime_shared::border_dash::{self, DashRun, LoopPath};
use runtime_shared::BorderStyle;

use crate::pipeline::Instance as RectInstance;

/// Maximum angle (radians) one chord rect subtends on a rounded corner.
///
/// Two errors come from approximating an arc of centreline radius `r`
/// with straight rects `w` thick:
///
/// * sagitta (the chord cutting inside the curve) = `r·θ²/8`;
/// * a wedge-shaped notch on the outer side of each joint ≈ `w·θ/2`.
///
/// At θ = 0.2 rad the sagitta is `r/200` (under a pixel for any radius
/// below 200px) and the notch is `w/10` (0.2px for a 2px border), both
/// inside the shader's 1px anti-aliased edge. Chords are NOT lengthened
/// to close the notch: the overlap on the inner side would double-blend
/// a translucent border colour, which is more visible than a 0.1w notch.
pub const CHORD_ANGLE: f32 = 0.2;

/// One mark in box-local px (origin at the node rect's top-left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    /// Centre.
    pub cx: f32,
    pub cy: f32,
    /// Extent along the path (a dot: its diameter).
    pub len: f32,
    /// Extent across the path (the border width).
    pub thick: f32,
    /// Direction of the path at the mark, radians (y-down, clockwise +),
    /// the same convention as `RectInstance::rotation`.
    pub angle: f32,
    /// A dot — rendered with `corner_radius = thick / 2`.
    pub round: bool,
    /// Which side's colour it takes: `0..4` = top, right, bottom, left.
    /// Uniform borders use side 0 for every mark.
    pub side: usize,
}

/// Every mark of a patterned border on a `box_w × box_h` box, or an
/// empty vec for a solid border (and for no visible width).
///
/// `radii` are the OUTER corner radii `[tl, tr, br, bl]`; `colours` is
/// only compared for uniformity (any representation works).
pub fn border_marks(
    box_w: f32,
    box_h: f32,
    widths: [f32; 4],
    colours: [[f32; 4]; 4],
    radii: [f32; 4],
    style: BorderStyle,
) -> Vec<Mark> {
    if style == BorderStyle::Solid || !(box_w > 0.0 && box_h > 0.0) {
        return Vec::new();
    }
    let uniform = widths.iter().all(|w| *w == widths[0]) && colours.iter().all(|c| *c == colours[0]);
    if uniform {
        loop_marks(box_w, box_h, widths[0], radii, style)
    } else {
        (0..4).flat_map(|side| side_marks(box_w, box_h, side, widths[side], style)).collect()
    }
}

fn loop_marks(box_w: f32, box_h: f32, w: f32, radii: [f32; 4], style: BorderStyle) -> Vec<Mark> {
    let Some(base) = border_dash::base(style, w) else { return Vec::new() };
    let path = LoopPath::new(box_w, box_h, radii, w);
    let length = path.length();
    if !(length > 0.0) {
        return Vec::new();
    }
    let dash = border_dash::fit_closed(base, length);
    let runs = merge_wrapped(border_dash::dash_runs(dash, length, true), length);
    let mut out = Vec::new();
    for run in runs {
        if dash.round {
            let (cx, cy, _) = path.point_at(run.mid());
            out.push(Mark { cx, cy, len: w, thick: w, angle: 0.0, round: true, side: 0 });
        } else {
            dash_along_loop(&path, run, w, &mut out);
        }
    }
    out
}

/// `dash_runs` reports the mark straddling the loop's start as two
/// consecutive runs, `[0, e)` then `[len + s, len)`. Rejoin them into one
/// run that ends past `length` (`point_at` wraps), so the mark is walked
/// as one piece — otherwise a dot would be drawn twice at the wrong
/// centres and a dash straddling a sharp top-left corner would lose its
/// corner square.
fn merge_wrapped(mut runs: Vec<DashRun>, length: f32) -> Vec<DashRun> {
    if runs.len() >= 2 {
        let (head, tail) = (runs[0], runs[1]);
        if head.start <= 0.0 && tail.end >= length - 1e-3 && tail.start > head.end {
            runs[0] = DashRun { start: tail.start, end: length + head.end };
            runs.remove(1);
        }
    }
    runs
}

/// The loop's pieces in walk order as `(length, arc radius or None)`:
/// top, tr arc, right, br arc, bottom, bl arc, left, tl arc — the order
/// `LoopPath::point_at` walks.
fn pieces(path: &LoopPath) -> [(f32, Option<f32>); 8] {
    use std::f32::consts::FRAC_PI_2;
    let [tl, tr, br, bl] = path.radii;
    [
        ((path.w - tl - tr).max(0.0), None),
        (tr * FRAC_PI_2, Some(tr)),
        ((path.h - tr - br).max(0.0), None),
        (br * FRAC_PI_2, Some(br)),
        ((path.w - br - bl).max(0.0), None),
        (bl * FRAC_PI_2, Some(bl)),
        ((path.h - bl - tl).max(0.0), None),
        (tl * FRAC_PI_2, Some(tl)),
    ]
}

fn dash_along_loop(path: &LoopPath, run: DashRun, w: f32, out: &mut Vec<Mark>) {
    let pieces = pieces(path);
    let half = w / 2.0;
    // Walk two laps: a merged wrap-around run ends past `length`.
    let mut p0 = 0.0;
    for lap_i in 0..16 {
        let i = lap_i % 8;
        let (plen, arc) = pieces[i];
        let p1 = p0 + plen;
        let lo = run.start.max(p0);
        let hi = run.end.min(p1);
        if hi - lo > 1e-4 {
            match arc {
                Some(r) if r > 0.0 => {
                    let step = (CHORD_ANGLE * r).max(0.25);
                    let pts = path.polyline(lo, hi, step);
                    for pair in pts.windows(2) {
                        push_segment(pair[0], pair[1], w, out);
                    }
                }
                _ => {
                    // Straight piece. Its arc neighbours have radius
                    // exactly 0 when the corner is sharp.
                    let prev_sharp = pieces[(i + 7) % 8].1 == Some(0.0);
                    let next_sharp = pieces[(i + 1) % 8].1 == Some(0.0);
                    // The run continues from / into the neighbouring
                    // edge → it crosses that sharp corner.
                    let trim_start = if prev_sharp && run.start < p0 - 1e-4 { half } else { 0.0 };
                    let extend_end = if next_sharp && run.end > p1 + 1e-4 { half } else { 0.0 };
                    let (ax, ay, _) = path.point_at(lo);
                    let (bx, by, _) = path.point_at(hi);
                    // `point_at` at a piece boundary may report the
                    // neighbouring piece — same point, other direction —
                    // so take the direction from the piece's middle.
                    let (_, _, dir) = path.point_at((lo + hi) / 2.0);
                    push_segment_dir((ax, ay), (bx, by), dir, w, -trim_start, extend_end, out);
                }
            }
        }
        p0 = p1;
        if p0 >= run.end {
            break;
        }
    }
}

/// A rect from `a` to `b` (direction from the points), `w` thick.
fn push_segment(a: (f32, f32), b: (f32, f32), w: f32, out: &mut Vec<Mark>) {
    let dir = (b.1 - a.1).atan2(b.0 - a.0);
    push_segment_dir(a, b, dir, w, 0.0, 0.0, out);
}

/// A rect from `a` to `b` along `dir`, `w` thick, with its start moved
/// back by `grow_start` and its end forward by `grow_end` (negative =
/// shrink).
fn push_segment_dir(
    a: (f32, f32),
    b: (f32, f32),
    dir: f32,
    w: f32,
    grow_start: f32,
    grow_end: f32,
    out: &mut Vec<Mark>,
) {
    let (ux, uy) = (dir.cos(), dir.sin());
    let along = (b.0 - a.0) * ux + (b.1 - a.1) * uy;
    let s0 = -grow_start;
    let s1 = along + grow_end;
    let len = s1 - s0;
    if !(len > 1e-4) {
        return;
    }
    let mid = (s0 + s1) / 2.0;
    out.push(Mark {
        cx: a.0 + ux * mid,
        cy: a.1 + uy * mid,
        len,
        thick: w,
        angle: dir,
        round: false,
        side: 0,
    });
}

fn side_marks(box_w: f32, box_h: f32, side: usize, w: f32, style: BorderStyle) -> Vec<Mark> {
    let Some(base) = border_dash::base(style, w) else { return Vec::new() };
    let (x0, y0, x1, y1) = border_dash::side_line(box_w, box_h, side, w);
    let length = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
    if !(length > 0.0) {
        return Vec::new();
    }
    let (ux, uy) = ((x1 - x0) / length, (y1 - y0) / length);
    let angle = uy.atan2(ux);
    let dash = border_dash::fit_open(base, length);
    border_dash::dash_runs(dash, length, false)
        .into_iter()
        .map(|run| {
            let m = run.mid();
            let len = if dash.round { w } else { run.end - run.start };
            Mark {
                cx: x0 + ux * m,
                cy: y0 + uy * m,
                len,
                thick: w,
                angle: if dash.round { 0.0 } else { angle },
                round: dash.round,
                side,
            }
        })
        .collect()
}

/// Stage `marks` for a node drawn at `rect = [x, y, w, h]` and rotated by
/// `rotation` around its centre (the same rotation the node's own rect
/// instance gets). `colours` are the per-side FINAL instance colours —
/// already tween-sampled, opacity-multiplied and linearised by the
/// caller, exactly as the fill's `bg` is.
///
/// A rect instance rotates about its OWN centre, so a mark's centre is
/// first rotated about the node's centre and the node's rotation added
/// to the mark's own angle — the composition that keeps marks on the
/// ring of a rotated node.
pub fn mark_instances(
    marks: &[Mark],
    rect: [f32; 4],
    rotation: f32,
    colours: [[f32; 4]; 4],
) -> Vec<RectInstance> {
    let [x, y, w, h] = rect;
    let (ncx, ncy) = (x + w / 2.0, y + h / 2.0);
    let (c, s) = (rotation.cos(), rotation.sin());
    marks
        .iter()
        .filter(|m| colours[m.side][3] > 0.0)
        .map(|m| {
            let (dx, dy) = (m.cx - w / 2.0, m.cy - h / 2.0);
            let sx = ncx + c * dx - s * dy;
            let sy = ncy + s * dx + c * dy;
            let r = if m.round { m.thick / 2.0 } else { 0.0 };
            RectInstance {
                rect: [sx - m.len / 2.0, sy - m.thick / 2.0, m.len, m.thick],
                bg: colours[m.side],
                corner_radius: [r; 4],
                border_color: [0.0; 4],
                border_width: 0.0,
                rotation: rotation + m.angle,
                shadow_blur: 0.0,
                ..bytemuck::Zeroable::zeroed()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    const WHITE: [f32; 4] = [1.0; 4];

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn uniform(w: f32, h: f32, bw: f32, radius: f32, style: BorderStyle) -> Vec<Mark> {
        border_marks(w, h, [bw; 4], [WHITE; 4], [radius; 4], style)
    }

    /// Bug: the GPU engine ignored `border_style` and drew every border
    /// as the shader's unbroken SDF ring. A dashed border must come out
    /// as separate marks that ink only part of the ring (the renderer
    /// stages the node's own rect ring-less whenever the style is
    /// patterned).
    #[test]
    fn regression_gpu_dashed_border_not_painted_as_solid_ring() {
        let marks = uniform(100.0, 40.0, 2.0, 0.0, BorderStyle::Dashed);
        assert!(marks.len() > 4, "dashed border is broken into marks: {}", marks.len());
        let path = LoopPath::new(100.0, 40.0, [0.0; 4], 2.0);
        let inked: f32 = marks.iter().map(|m| m.len).sum();
        assert!(inked < path.length() * 0.75, "inked {inked} of {}", path.length());
        let dotted = uniform(100.0, 40.0, 2.0, 0.0, BorderStyle::Dotted);
        assert!(dotted.len() > 4);
        // And the staging helper actually turns them into instances.
        let inst = mark_instances(&marks, [0.0, 0.0, 100.0, 40.0], 0.0, [WHITE; 4]);
        assert_eq!(inst.len(), marks.len());
        assert!(inst.iter().all(|i| i.border_width == 0.0 && i.bg == WHITE));
    }

    #[test]
    fn solid_border_adds_no_instances() {
        assert!(uniform(100.0, 40.0, 2.0, 8.0, BorderStyle::Solid).is_empty());
        assert!(border_marks(
            100.0,
            40.0,
            [1.0, 2.0, 3.0, 4.0],
            [WHITE; 4],
            [0.0; 4],
            BorderStyle::Solid
        )
        .is_empty());
    }

    #[test]
    fn zero_width_patterned_border_adds_nothing() {
        assert!(uniform(100.0, 40.0, 0.0, 0.0, BorderStyle::Dashed).is_empty());
    }

    /// Square 2px dashed box, 62×42: centreline 60×40, perimeter 200.
    /// Base period 12 → 17 periods (fit_closed rounds 16.67); the first
    /// dash is centred on the top-left corner.
    #[test]
    fn square_dashed_box_marks_follow_the_fitted_pattern() {
        let marks = uniform(62.0, 42.0, 2.0, 0.0, BorderStyle::Dashed);
        let path = LoopPath::new(62.0, 42.0, [0.0; 4], 2.0);
        assert!(close(path.length(), 200.0));
        let period = 200.0 / 17.0;
        // Every mark lies on the centreline and is border-thick.
        for m in &marks {
            assert!(close(m.thick, 2.0));
            let on_h = close(m.cy, 1.0) || close(m.cy, 41.0);
            let on_v = close(m.cx, 1.0) || close(m.cx, 61.0);
            assert!(on_h || on_v, "mark off the centreline: {m:?}");
        }
        // Total ink = 17 dashes × 6px, however they split at corners
        // (a corner's +w/2 and -w/2 cancel).
        let inked: f32 = marks.iter().map(|m| m.len).sum();
        assert!(close(inked, 17.0 * 6.0), "inked {inked}");
        // The second dash sits wholly on the top edge, centred one
        // period in, running left→right.
        let second = marks
            .iter()
            .find(|m| close(m.cx, 1.0 + period) && close(m.cy, 1.0))
            .unwrap_or_else(|| panic!("no dash at {}: {marks:?}", 1.0 + period));
        assert!(close(second.len, 6.0) && close(second.angle, 0.0));
    }

    /// The dash straddling a sharp corner is two rects that ink the
    /// outer corner square exactly once: incoming grows by w/2, outgoing
    /// shrinks by w/2.
    #[test]
    fn dash_across_a_sharp_corner_fills_the_corner_once() {
        let marks = uniform(62.0, 42.0, 2.0, 0.0, BorderStyle::Dashed);
        // The first (wrapped) dash straddles the top-left corner at
        // (1, 1): 3px up the left edge, 3px along the top.
        let left = marks
            .iter()
            .find(|m| close(m.cx, 1.0) && m.cy < 5.0)
            .expect("left-edge half of the corner dash");
        let top = marks
            .iter()
            .find(|m| close(m.cy, 1.0) && m.cx < 5.0)
            .expect("top-edge half of the corner dash");
        // Left half is incoming (runs upward, angle -π/2): 3 + 1 long,
        // reaching the outer edge y = 0.
        assert!(close(left.angle, -FRAC_PI_2));
        assert!(close(left.len, 4.0), "{left:?}");
        assert!(close(left.cy - left.len / 2.0, 0.0), "{left:?}");
        // Top half is outgoing: 3 - 1 long, starting at x = 2 (the
        // left rect's right side).
        assert!(close(top.len, 2.0), "{top:?}");
        assert!(close(top.cx - top.len / 2.0, 2.0), "{top:?}");
    }

    #[test]
    fn dots_are_width_discs_centred_on_the_path() {
        let marks = uniform(62.0, 42.0, 2.0, 0.0, BorderStyle::Dotted);
        // Base period 4 → 50 dots on a 200px loop.
        assert_eq!(marks.len(), 50);
        for m in &marks {
            assert!(m.round && close(m.len, 2.0) && close(m.thick, 2.0));
        }
        // The first dot is centred on the loop's start point (the
        // top-left corner of the centreline), drawn once, not twice.
        let at_start = marks.iter().filter(|m| close(m.cx, 1.0) && close(m.cy, 1.0)).count();
        assert_eq!(at_start, 1);
        // Instances carry the round corner.
        let inst = mark_instances(&marks[..1], [0.0, 0.0, 62.0, 42.0], 0.0, [WHITE; 4]);
        assert_eq!(inst[0].corner_radius, [1.0; 4]);
        assert_eq!(inst[0].rect, [0.0, 0.0, 2.0, 2.0]);
    }

    /// A dash across a rounded corner is split into chords that stay on
    /// the arc (every chord end on the centreline circle).
    #[test]
    fn dash_across_a_rounded_corner_follows_the_arc() {
        // Pill 100×22, radius 11, 1px border → centreline radius 10.5.
        let marks = uniform(100.0, 22.0, 1.0, 11.0, BorderStyle::Dashed);
        let (rcx, rcy, r) = (89.0, 11.0, 10.5);
        let on_arc: Vec<&Mark> = marks.iter().filter(|m| m.cx > rcx + 0.01).collect();
        assert!(!on_arc.is_empty(), "some marks sit on the right-hand arc");
        for m in on_arc {
            let (ux, uy) = (m.angle.cos(), m.angle.sin());
            for end in [-0.5, 0.5] {
                let (ex, ey) = (m.cx + ux * m.len * end, m.cy + uy * m.len * end);
                let d = ((ex - rcx).powi(2) + (ey - rcy).powi(2)).sqrt();
                assert!((d - r).abs() < 0.01, "chord end off the arc by {}: {m:?}", d - r);
            }
            assert!(m.len <= CHORD_ANGLE * r + 1e-3, "chord longer than the step: {m:?}");
        }
    }

    #[test]
    fn asymmetric_border_is_dashed_per_side_with_each_sides_width() {
        let marks = border_marks(
            60.0,
            30.0,
            [2.0, 0.0, 4.0, 0.0],
            [WHITE; 4],
            [0.0; 4],
            BorderStyle::Dashed,
        );
        assert!(marks.iter().all(|m| m.side == 0 || m.side == 2));
        let top: Vec<_> = marks.iter().filter(|m| m.side == 0).collect();
        let bottom: Vec<_> = marks.iter().filter(|m| m.side == 2).collect();
        assert!(!top.is_empty() && !bottom.is_empty());
        assert!(top.iter().all(|m| close(m.cy, 1.0) && close(m.len, 6.0) && close(m.angle, 0.0)));
        assert!(bottom.iter().all(|m| close(m.cy, 28.0) && close(m.len, 12.0) && close(m.angle, PI)));
        // Open fit: the side starts and ends on a dash.
        let first = top.iter().map(|m| m.cx - m.len / 2.0).fold(f32::MAX, f32::min);
        let last = top.iter().map(|m| m.cx + m.len / 2.0).fold(f32::MIN, f32::max);
        assert!(close(first, 0.0) && close(last, 60.0));
    }

    /// Node rotation: marks orbit the node's centre and turn with it.
    #[test]
    fn marks_rotate_with_the_node_about_its_centre() {
        let m = Mark { cx: 10.0, cy: 0.0, len: 4.0, thick: 2.0, angle: 0.0, round: false, side: 0 };
        // Node 20×20 at (100, 100); centre (110, 110). The mark sits at
        // local (10, 0) = 10 px above centre. Rotating 90° clockwise
        // (y-down) moves it 10 px right of centre, running downward.
        let inst = mark_instances(&[m], [100.0, 100.0, 20.0, 20.0], FRAC_PI_2, [WHITE; 4]);
        let [x, y, w, h] = inst[0].rect;
        assert!(close(x + w / 2.0, 120.0) && close(y + h / 2.0, 110.0), "{:?}", inst[0].rect);
        assert!(close(inst[0].rotation, FRAC_PI_2));
    }

    #[test]
    fn transparent_side_stages_no_instances() {
        let marks = uniform(62.0, 42.0, 2.0, 0.0, BorderStyle::Dashed);
        assert!(mark_instances(&marks, [0.0, 0.0, 62.0, 42.0], 0.0, [[0.0; 4]; 4]).is_empty());
    }
}
