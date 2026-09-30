//! Dashed / dotted border geometry for the Android border drawable —
//! the Rust half of `RustBorderDrawable.kt`'s patterned stroke.
//!
//! Un-gated (compiles on any host) so the tests run from any platform,
//! same pattern as `transform_transition_policy` / `layout_policy`. The
//! JNI exports that hand these numbers to Kotlin live in
//! `imp/jni_exports.rs`.
//!
//! ## Why the fitting runs in Rust, called FROM the drawable's draw
//!
//! A patterned border's gap is stretched so the pattern fits the path
//! exactly ([`border_dash::fit_closed`] / [`border_dash::fit_open`]),
//! and the path is the drawable's ACTUAL bounds — which only the
//! drawable knows, and which change on every layout without a style
//! re-apply (and on every tick of a border-width animation, which
//! drives `RustBorderDrawable.update` from Kotlin). So the fit cannot
//! be computed at style-apply time and pushed down.
//!
//! The alternative — mirroring `border_dash`'s ratios and fitting maths
//! in Kotlin, as `RustUnderlineSpan.kt` mirrors `underline_geometry` —
//! duplicates LOGIC, not just four constants, and every other backend
//! reads the Rust module directly. Instead the drawable calls
//! `nativeLoopDash` / `nativeSideDash` when its geometry is dirty
//! (bounds / widths / radii / style changed) and draws exactly the
//! numbers returned here. Kotlin holds no dash ratio at all, so there
//! is nothing to drift. The cost is one JNI call per dirty draw — never
//! per frame for a static border.
//!
//! All lengths are device pixels: the drawable's bounds, the border
//! widths (`dp_to_px`'d by `imp/style.rs`) and the corner radii
//! (likewise) all arrive in px, and `border_dash` is unit-agnostic
//! because every length in it is a multiple of the border width.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use runtime_shared::border_dash::{self, Dash, LoopPath};
use runtime_shared::BorderStyle;

/// JNI wire codes for [`BorderStyle`], passed to
/// `RustBorderDrawable.setBorderStyle(int)`. MIRRORED by the
/// `STYLE_*` constants in `RustBorderDrawable.kt`'s companion object;
/// `kotlin_style_codes_match_rust` fails if the two drift.
pub(crate) const STYLE_SOLID: i32 = 0;
pub(crate) const STYLE_DASHED: i32 = 1;
pub(crate) const STYLE_DOTTED: i32 = 2;

/// Map the style rule onto the wire code. `None` is the solid border
/// every app had before `border_style` existed.
pub(crate) fn style_code(style: Option<BorderStyle>) -> i32 {
    match style.unwrap_or_default() {
        BorderStyle::Solid => STYLE_SOLID,
        BorderStyle::Dashed => STYLE_DASHED,
        BorderStyle::Dotted => STYLE_DOTTED,
    }
}

/// Inverse of [`style_code`]. An unknown code draws solid — the
/// drawable's pre-`border_style` behaviour — rather than guessing a
/// pattern.
pub(crate) fn style_from_code(code: i32) -> BorderStyle {
    match code {
        STYLE_DASHED => BorderStyle::Dashed,
        STYLE_DOTTED => BorderStyle::Dotted,
        _ => BorderStyle::Solid,
    }
}

/// Must this apply run the drawable path (`apply_drawable_path`)?
///
/// Besides the obvious triggers, a view whose border drawable is still
/// installed must go through it even when the new rules carry no border
/// at all: that path is the only one that detaches the foreground. Before
/// this, a style re-apply that dropped every `border_*_width` (with no
/// radius / gradient) took the plain `setBackgroundColor` branch and the
/// old border — solid, dashed or dotted — kept drawing.
pub(crate) fn needs_drawable_path(
    has_border: bool,
    has_radius: bool,
    has_gradient: bool,
    border_drawable_installed: bool,
) -> bool {
    has_border || has_radius || has_gradient || border_drawable_installed
}

/// `DashPathEffect(intervals, phase)` + cap for a fitted pattern.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stroke {
    pub intervals: [f32; 2],
    pub phase: f32,
    /// `Paint.Cap.ROUND` (dots) vs `Paint.Cap.BUTT` (dashes).
    pub round: bool,
}

fn round_flag(round: bool) -> f32 {
    if round {
        1.0
    } else {
        0.0
    }
}

/// Uniform border (one width + colour on every side): the centreline
/// loop and the pattern fitted to its length.
///
/// Returns `[x, y, w, h, r_tl, r_tr, r_br, r_bl, on, off, phase, round]`
/// — the `LoopPath` rect + radii Kotlin builds its `Path` from (in the
/// same clockwise-from-top-left order `LoopPath::point_at` walks, so the
/// phase lands where every other backend puts the first mark), then the
/// `DashPathEffect` intervals + phase, then `1.0` for a round cap. `None`
/// for a solid style or a zero width: the caller draws solid.
pub(crate) fn loop_geometry(
    style: BorderStyle,
    width: f32,
    box_w: f32,
    box_h: f32,
    radii: [f32; 4],
) -> Option<[f32; 12]> {
    let base = border_dash::base(style, width)?;
    let path = LoopPath::new(box_w, box_h, radii, width);
    let dash = border_dash::fit_closed(base, path.length());
    let [on, off] = dash.toolkit_array();
    let [tl, tr, br, bl] = path.radii;
    Some([
        path.x,
        path.y,
        path.w,
        path.h,
        tl,
        tr,
        br,
        bl,
        on,
        off,
        dash.toolkit_phase(),
        round_flag(dash.round),
    ])
}

/// The stroke packed in one [`loop_geometry`] — a test view of the
/// flat array the JNI export returns.
#[cfg(test)]
pub(crate) fn loop_stroke(g: &[f32; 12]) -> Stroke {
    Stroke { intervals: [g[8], g[9]], phase: g[10], round: g[11] != 0.0 }
}

/// One side (`0..4` = top, right, bottom, left) of an asymmetric
/// border: its centreline and the pattern fitted to it.
///
/// Returns `[x0, y0, x1, y1, on, off, phase, round]`, `None` for a solid
/// style or a zero width.
pub(crate) fn side_geometry(
    style: BorderStyle,
    side: usize,
    width: f32,
    box_w: f32,
    box_h: f32,
) -> Option<[f32; 8]> {
    let base = border_dash::base(style, width)?;
    let (x0, y0, x1, y1) = border_dash::side_line(box_w, box_h, side, width);
    let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
    let dash = border_dash::fit_open(base, len);
    let [on, off] = dash.toolkit_array();
    Some([x0, y0, x1, y1, on, off, dash.open_toolkit_phase(), round_flag(dash.round)])
}

/// The stroke for one [`side_geometry`].
#[cfg(test)]
pub(crate) fn side_stroke(g: &[f32; 8]) -> Stroke {
    Stroke { intervals: [g[4], g[5]], phase: g[6], round: g[7] != 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::border_dash::dash_runs;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// Before `border_style` reached the Android apply path the border
    /// drawable was only ever told widths + colours, so a dashed border
    /// drew as a solid one. The style must map to a non-solid wire code
    /// AND yield a pattern for the drawable to stroke with.
    #[test]
    fn regression_android_dashed_border_not_drawn_solid() {
        let code = style_code(Some(BorderStyle::Dashed));
        assert_ne!(code, STYLE_SOLID);
        let style = style_from_code(code);
        assert_eq!(style, BorderStyle::Dashed);
        let g = loop_geometry(style, 2.0, 100.0, 40.0, [0.0; 4])
            .expect("a dashed border must produce a dash pattern");
        let s = loop_stroke(&g);
        assert!(s.intervals[0] > 0.0 && s.intervals[1] > 0.0, "{s:?}");
        assert!(!s.round);

        let code = style_code(Some(BorderStyle::Dotted));
        let g = loop_geometry(style_from_code(code), 2.0, 100.0, 40.0, [0.0; 4]).unwrap();
        assert!(loop_stroke(&g).round, "dots are round-capped");
    }

    #[test]
    fn solid_and_unset_styles_draw_the_existing_solid_border() {
        assert_eq!(style_code(None), STYLE_SOLID);
        assert_eq!(style_code(Some(BorderStyle::Solid)), STYLE_SOLID);
        assert_eq!(loop_geometry(BorderStyle::Solid, 2.0, 100.0, 40.0, [4.0; 4]), None);
        assert_eq!(side_geometry(BorderStyle::Solid, 0, 2.0, 100.0, 40.0), None);
        assert_eq!(style_from_code(99), BorderStyle::Solid, "unknown code draws solid");
    }

    #[test]
    fn zero_width_draws_nothing_patterned() {
        assert_eq!(loop_geometry(BorderStyle::Dashed, 0.0, 100.0, 40.0, [0.0; 4]), None);
        assert_eq!(side_geometry(BorderStyle::Dotted, 2, 0.0, 100.0, 40.0), None);
    }

    #[test]
    fn loop_pattern_is_fitted_to_the_centreline_of_the_actual_box() {
        let (w, h, stroke) = (120.0, 48.0, 3.0);
        let radii = [12.0, 12.0, 0.0, 6.0];
        let g = loop_geometry(BorderStyle::Dashed, stroke, w, h, radii).unwrap();
        let path = LoopPath::new(w, h, radii, stroke);
        assert_eq!([g[0], g[1], g[2], g[3]], [path.x, path.y, path.w, path.h]);
        assert_eq!([g[4], g[5], g[6], g[7]], path.radii);
        let s = loop_stroke(&g);
        assert_eq!(s.intervals[0], 3.0 * stroke, "the dash keeps its shared length");
        let periods = path.length() / (s.intervals[0] + s.intervals[1]);
        assert!(close(periods, periods.round()), "{periods} periods do not close the loop");
        assert!(close(s.phase, s.intervals[0] / 2.0), "first dash straddles the start");
    }

    /// Bounds change on layout: the same style at a new size must refit,
    /// not reuse the old gap.
    #[test]
    fn loop_pattern_refits_when_the_box_resizes() {
        let a = loop_stroke(&loop_geometry(BorderStyle::Dashed, 1.0, 100.0, 40.0, [0.0; 4]).unwrap());
        let b = loop_stroke(&loop_geometry(BorderStyle::Dashed, 1.0, 107.0, 40.0, [0.0; 4]).unwrap());
        assert_ne!(a.intervals[1], b.intervals[1]);
    }

    /// A border-width animation re-runs the fit every tick; the dash
    /// length must track the width, not freeze at the start value.
    #[test]
    fn pattern_tracks_the_animated_width() {
        let thin = loop_stroke(&loop_geometry(BorderStyle::Dashed, 1.0, 100.0, 40.0, [0.0; 4]).unwrap());
        let thick = loop_stroke(&loop_geometry(BorderStyle::Dashed, 4.0, 100.0, 40.0, [0.0; 4]).unwrap());
        assert_eq!(thin.intervals[0], 3.0);
        assert_eq!(thick.intervals[0], 12.0);
    }

    #[test]
    fn dots_are_zero_length_round_dashes_on_the_loop() {
        let g = loop_geometry(BorderStyle::Dotted, 2.0, 60.0, 30.0, [0.0; 4]).unwrap();
        let s = loop_stroke(&g);
        assert_eq!(s.intervals[0], 0.0);
        assert!(s.round);
        assert_eq!(s.phase, 0.0);
        let path = LoopPath::new(60.0, 30.0, [0.0; 4], 2.0);
        let n = path.length() / s.intervals[1];
        assert!(close(n, n.round()));
    }

    #[test]
    fn side_line_and_pattern_follow_the_shared_geometry() {
        let g = side_geometry(BorderStyle::Dashed, 1, 2.0, 80.0, 50.0).unwrap();
        assert_eq!([g[0], g[1], g[2], g[3]], [79.0, 0.0, 79.0, 50.0]);
        let s = side_stroke(&g);
        let fitted = border_dash::fit_open(border_dash::base(BorderStyle::Dashed, 2.0).unwrap(), 50.0);
        assert_eq!(s.intervals, fitted.toolkit_array());
        assert_eq!(s.phase, 0.0, "a butt dash at phase 0 already starts the line");
        assert!(!s.round);
    }

    /// Walk a toolkit `[on, off]` pattern at `phase` along an open line
    /// the way Skia's dasher does, returning each mark's centre.
    fn toolkit_mark_centres(s: &Stroke, len: f32) -> Vec<f32> {
        let period = s.intervals[0] + s.intervals[1];
        let mut out = Vec::new();
        // The first mark starts at `-phase`, then every period.
        let mut start = -s.phase;
        while start < len - 1e-3 {
            let end = start + s.intervals[0];
            if end >= -1e-3 {
                out.push((start + end) / 2.0);
            }
            start += period;
        }
        out
    }

    /// The Android open-line phase must reproduce `dash_runs` — the
    /// same marks the GPU / CPU renderers draw — for dots and dashes.
    #[test]
    fn open_side_marks_land_where_dash_runs_puts_them() {
        for style in [BorderStyle::Dashed, BorderStyle::Dotted] {
            for len in [7.0_f32, 23.0, 50.0, 131.0] {
                let g = side_geometry(style, 0, 2.0, len, 30.0).unwrap();
                let s = side_stroke(&g);
                let fitted = border_dash::fit_open(border_dash::base(style, 2.0).unwrap(), len);
                let want: Vec<f32> = dash_runs(fitted, len, false)
                    .iter()
                    .map(|r| r.mid())
                    .collect();
                let got = toolkit_mark_centres(&s, len);
                assert_eq!(got.len(), want.len(), "{style:?} len {len}: {got:?} vs {want:?}");
                for (g, w) in got.iter().zip(&want) {
                    assert!(close(*g, *w), "{style:?} len {len}: {got:?} vs {want:?}");
                }
            }
        }
    }

    #[test]
    fn regression_android_removed_border_keeps_drawing() {
        // Rules with no border / radius / gradient, but a border drawable
        // from the previous apply: the drawable path must run to detach it.
        assert!(needs_drawable_path(false, false, false, true));
        // A plain view never allocates the drawable path.
        assert!(!needs_drawable_path(false, false, false, false));
        assert!(needs_drawable_path(true, false, false, false));
    }

    /// The wire codes are mirrored as Kotlin constants; a renumbering on
    /// one side only would silently draw the wrong pattern.
    #[test]
    fn kotlin_style_codes_match_rust() {
        let kt = include_str!("../runtime/kotlin/io/idealyst/runtime/RustBorderDrawable.kt");
        for (name, code) in [
            ("STYLE_SOLID", STYLE_SOLID),
            ("STYLE_DASHED", STYLE_DASHED),
            ("STYLE_DOTTED", STYLE_DOTTED),
        ] {
            let decl = format!("const val {name}: Int = {code}");
            assert!(kt.contains(&decl), "RustBorderDrawable.kt is missing `{decl}`");
        }
        // The JNI exports in `imp/jni_exports.rs` are bound by name.
        assert!(kt.contains("external fun nativeLoopDash("));
        assert!(kt.contains("external fun nativeSideDash("));
    }
}
