//! Dashed / dotted borders — WHAT to paint, decided without GTK.
//!
//! [`plan`] turns a [`BorderPaint`] and the box it paints on into a list
//! of [`Mark`]s; `view::paint_box` hands them to GSK. The geometry (dash
//! lengths, gap fitting, the centreline loop) comes from
//! [`runtime_shared::border_dash`], the one source every native backend
//! draws from, so a dashed border here lands its marks where the GPU,
//! CPU, Apple and Android backends land theirs (CLAUDE.md §7).
//!
//! ## Why dashes stroke but dots fill
//!
//! Dashes go through GSK's own dashed stroke (`GskStroke::set_dash` +
//! `set_dash_offset`, GTK ≥ 4.14 — this crate's floor). Its dash offset
//! is documented as Cairo's is — "the offset into the dash pattern at
//! which the stroke starts" — so `Dash::toolkit_phase` (`on / 2`) centres
//! the first dash on the path's start, exactly where
//! `border_dash::dash_runs` puts it.
//!
//! Dots do NOT use the zero-length-round-capped-dash trick
//! (`Dash::toolkit_array`): Cairo documents that a zero-length dash
//! with a round cap draws a dot, but GSK's stroker makes no such promise
//! — a zero-length dash segment may be dropped as degenerate, which
//! would paint a dotted border as nothing at all. So each dot is a
//! filled circle, one border-width across, at the centre the shared
//! geometry gives. That is also what the GPU and CPU renderers draw.

use runtime_shared::border_dash::{self, Dash, LoopPath};
use runtime_shared::BorderStyle;

use crate::view::BorderPaint;

/// One thing to draw for a patterned border, in the box's own space
/// (origin at the box's top-left).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Mark {
    /// Uniform border: a dashed stroke along the closed centreline.
    DashedLoop {
        path: LoopPath,
        /// `[on, off]` for `GskStroke::set_dash`.
        array: [f32; 2],
        /// For `GskStroke::set_dash_offset`.
        phase: f32,
        width: f32,
        color: [f32; 4],
    },
    /// Asymmetric border: a dashed stroke along one side's straight
    /// centreline. `fit_open` fits the pattern so a dash STARTS at the
    /// line's start and ends at its end; `phase` is
    /// `Dash::open_toolkit_phase` (the loop's `toolkit_phase` would
    /// centre the first dash on the corner instead).
    DashedLine {
        from: (f32, f32),
        to: (f32, f32),
        array: [f32; 2],
        phase: f32,
        width: f32,
        color: [f32; 4],
    },
    /// Filled round dots, `radius` = half the border width.
    Dots { centres: Vec<(f32, f32)>, radius: f32, color: [f32; 4] },
}

/// The marks for a patterned border on a `w × h` box whose (already
/// clamped) outer corner radii are `radius` (`[tl, tr, br, bl]`), or
/// `None` when the border is solid — the caller keeps drawing that with
/// `append_border`, unchanged.
///
/// Uniform (all four widths and colours equal) → one closed loop that
/// follows the corner radii; anything else → one straight line per
/// side that has a width, the same uniform/asymmetric split the other
/// backends use.
pub(crate) fn plan(b: &BorderPaint, w: f32, h: f32, radius: [f32; 4]) -> Option<Vec<Mark>> {
    if b.style == BorderStyle::Solid || !(w > 0.0 && h > 0.0) {
        return None;
    }
    let uniform = b.widths.iter().all(|x| *x == b.widths[0]) && b.colors.iter().all(|c| *c == b.colors[0]);
    let mut marks = Vec::new();
    if uniform {
        let width = b.widths[0];
        let dash = border_dash::base(b.style, width)?;
        let path = LoopPath::new(w, h, radius, width);
        let len = path.length();
        if !(len > 0.0) {
            return Some(marks);
        }
        let dash = border_dash::fit_closed(dash, len);
        let color = b.colors[0];
        if dash.round {
            let centres = loop_centres(dash, len).into_iter().map(|s| {
                let (x, y, _) = path.point_at(s);
                (x, y)
            });
            marks.push(Mark::Dots { centres: centres.collect(), radius: width / 2.0, color });
        } else {
            marks.push(Mark::DashedLoop {
                path,
                array: dash.toolkit_array(),
                phase: dash.toolkit_phase(),
                width,
                color,
            });
        }
        return Some(marks);
    }
    for side in 0..4 {
        let width = b.widths[side];
        let Some(dash) = border_dash::base(b.style, width) else { continue };
        let (x0, y0, x1, y1) = border_dash::side_line(w, h, side, width);
        let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
        if !(len > 0.0) {
            continue;
        }
        let dash = border_dash::fit_open(dash, len);
        let color = b.colors[side];
        if dash.round {
            let centres = border_dash::dash_runs(dash, len, false).into_iter().map(|r| {
                let t = r.mid() / len;
                (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t)
            });
            marks.push(Mark::Dots { centres: centres.collect(), radius: width / 2.0, color });
        } else {
            marks.push(Mark::DashedLine {
                from: (x0, y0),
                to: (x1, y1),
                array: dash.toolkit_array(),
                phase: dash.open_toolkit_phase(),
                width,
                color,
            });
        }
    }
    Some(marks)
}

/// Distances along a closed loop of `len` at which a fitted pattern's
/// marks are centred: `i · period`, whole periods only (`fit_closed`
/// made the period divide `len`), matching `border_dash::dash_runs`
/// with `closed = true`.
fn loop_centres(dash: Dash, len: f32) -> Vec<f32> {
    let n = (len / dash.period()).round().max(1.0) as usize;
    (0..n).map(|i| i as f32 * dash.period()).collect()
}

/// The CSS keyword for the border [`plan`] would paint — what the
/// introspection reader reports as `border_style`. `"solid"` whenever
/// the plan falls back to `append_border`, so a dashed style on a box
/// the pattern can't be drawn on is not reported as dashed.
#[cfg(any(feature = "robot", test))]
pub(crate) fn painted_keyword(b: &BorderPaint, w: f32, h: f32, radius: [f32; 4]) -> &'static str {
    match plan(b, w, h, radius) {
        None => "solid",
        Some(_) => match b.style {
            BorderStyle::Solid => "solid",
            BorderStyle::Dashed => "dashed",
            BorderStyle::Dotted => "dotted",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

    fn uniform(style: BorderStyle, width: f32) -> BorderPaint {
        BorderPaint { widths: [width; 4], colors: [RED; 4], style }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn solid_border_has_no_plan_so_append_border_still_draws_it() {
        assert_eq!(plan(&uniform(BorderStyle::Solid, 2.0), 100.0, 40.0, [0.0; 4]), None);
        assert_eq!(painted_keyword(&uniform(BorderStyle::Solid, 2.0), 100.0, 40.0, [0.0; 4]), "solid");
    }

    #[test]
    fn uniform_dashed_border_is_one_fitted_loop_centred_on_the_start() {
        let radius = [8.0; 4];
        let marks = plan(&uniform(BorderStyle::Dashed, 2.0), 100.0, 40.0, radius).unwrap();
        let [Mark::DashedLoop { path, array, phase, width, color }] = marks.as_slice() else {
            panic!("expected one dashed loop, got {marks:?}");
        };
        assert_eq!(*path, LoopPath::new(100.0, 40.0, radius, 2.0), "the CSS centreline");
        assert_eq!(array[0], 6.0, "a dash is 3 widths and keeps its length");
        let periods = path.length() / (array[0] + array[1]);
        assert!(close(periods, periods.round()), "whole periods round the loop: {periods}");
        assert_eq!(*phase, 3.0, "first dash centred on the path start");
        assert_eq!((*width, *color), (2.0, RED));
    }

    #[test]
    fn uniform_dotted_border_is_one_width_dots_along_the_loop() {
        let marks = plan(&uniform(BorderStyle::Dotted, 2.0), 50.0, 30.0, [0.0; 4]).unwrap();
        let [Mark::Dots { centres, radius, color }] = marks.as_slice() else {
            panic!("expected one dot run, got {marks:?}");
        };
        assert_eq!((*radius, *color), (1.0, RED), "a dot is one border-width across");
        let path = LoopPath::new(50.0, 30.0, [0.0; 4], 2.0);
        // Loop 2·(48 + 28) = 152, period 4 → 38 dots, first on the start.
        assert_eq!(centres.len(), 38);
        assert_eq!(centres[0], (1.0, 1.0));
        let spacing = path.length() / centres.len() as f32;
        let (x, y, _) = path.point_at(spacing);
        assert!(close(centres[1].0, x) && close(centres[1].1, y));
    }

    #[test]
    fn asymmetric_dashed_border_strokes_only_the_sides_with_a_width() {
        // A bottom-only divider.
        let b = BorderPaint { widths: [0.0, 0.0, 1.0, 0.0], colors: [BLUE; 4], style: BorderStyle::Dashed };
        let marks = plan(&b, 40.0, 10.0, [0.0; 4]).unwrap();
        let [Mark::DashedLine { from, to, array, phase, width, color }] = marks.as_slice() else {
            panic!("expected one side line, got {marks:?}");
        };
        assert_eq!(*phase, 0.0, "the first dash STARTS the side, not centred on its corner");
        assert_eq!((*from, *to), ((40.0, 9.5), (0.0, 9.5)), "bottom centreline, clockwise");
        assert_eq!((*width, *color), (1.0, BLUE));
        // Fitted to start AND end on a dash: n dashes + (n - 1) gaps = 40.
        let n = (40.0 + array[1]) / (array[0] + array[1]);
        assert!(close(n, n.round()), "n = {n}");
    }

    #[test]
    fn asymmetric_dotted_border_puts_a_dot_at_each_end_of_a_side() {
        let b = BorderPaint {
            widths: [2.0, 0.0, 0.0, 4.0],
            colors: [RED, RED, RED, BLUE],
            style: BorderStyle::Dotted,
        };
        let marks = plan(&b, 40.0, 20.0, [0.0; 4]).unwrap();
        assert_eq!(marks.len(), 2, "top + left only: {marks:?}");
        let Mark::Dots { centres, radius, color } = &marks[0] else { panic!() };
        assert_eq!((*radius, *color), (1.0, RED));
        assert_eq!(centres.first(), Some(&(1.0, 1.0)));
        assert!(close(centres.last().unwrap().0, 39.0));
        let Mark::Dots { radius, color, .. } = &marks[1] else { panic!() };
        assert_eq!((*radius, *color), (2.0, BLUE), "each side keeps its own width + colour");
    }

    #[test]
    fn differing_colours_are_asymmetric_even_with_equal_widths() {
        let mut b = uniform(BorderStyle::Dashed, 1.0);
        b.colors[2] = BLUE;
        let marks = plan(&b, 30.0, 30.0, [0.0; 4]).unwrap();
        assert_eq!(marks.len(), 4);
        assert!(marks.iter().all(|m| matches!(m, Mark::DashedLine { .. })));
    }

    #[test]
    fn keyword_reports_what_is_painted() {
        assert_eq!(painted_keyword(&uniform(BorderStyle::Dashed, 1.0), 30.0, 30.0, [0.0; 4]), "dashed");
        assert_eq!(painted_keyword(&uniform(BorderStyle::Dotted, 1.0), 30.0, 30.0, [0.0; 4]), "dotted");
        // An empty box paints nothing patterned.
        assert_eq!(painted_keyword(&uniform(BorderStyle::Dotted, 1.0), 0.0, 30.0, [0.0; 4]), "solid");
    }
}
