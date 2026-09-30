//! Border-routing decision shared by the iOS and macOS backends.
//!
//! Both UIKit and AppKit expose the same two ways to stroke a border,
//! with the same sharp split:
//!
//!   * `CALayer.borderWidth`/`borderColor` strokes ONE uniform border
//!     that follows the layer's `cornerRadius` exactly — the stroke
//!     curves around rounded corners with no seams.
//!   * Per-side `UIView`/`NSView` bars can express asymmetric
//!     widths/colors, but each bar is a straight rectangle. With a corner
//!     radius the parent's clip mask slices the ends off every bar,
//!     leaving notches/gaps at each corner — a straight bar can't trace a
//!     curve.
//!
//! So each backend routes uniform borders (the common card) through
//! CALayer and reserves per-side bars for the genuinely asymmetric case
//! CALayer can't represent. This module owns that decision so it's
//! unit-tested once on the host and the two backends converge byte for
//! byte (Rule #7) — see [`uniform_border`].
//!
//! NOT OS-gated — pure `runtime_shared` logic, so it builds and tests on the
//! host while iOS + macOS share one source of truth.

use runtime_shared::border_dash::{self, LoopPath};
use runtime_shared::{BorderStyle, Color};

/// Decide whether a per-side border collapses to a single uniform
/// CALayer stroke. `widths` and `colors` are the four resolved sides
/// in `[top, right, bottom, left]` order; a `None` color falls back to
/// the first author-supplied color (matching the per-side bar path) so
/// `border_width` set without an explicit color still counts as
/// uniform.
///
/// Returns `Some((width, color))` when all four sides share the same
/// width and effective color — the caller strokes the layer, which
/// traces `cornerRadius` cleanly. Returns `None` for the asymmetric
/// case (e.g. a `border-bottom`-only spec), where the caller draws
/// straight per-side bars.
pub fn uniform_border(widths: [f32; 4], colors: &[Option<Color>; 4]) -> Option<(f32, Color)> {
    if !widths.iter().all(|w| (*w - widths[0]).abs() < f32::EPSILON) {
        return None;
    }
    let fallback = colors.iter().find_map(|c| c.clone());
    let eff: Vec<Option<Color>> =
        colors.iter().map(|c| c.clone().or_else(|| fallback.clone())).collect();
    let first = eff[0].clone()?;
    if eff.iter().all(|c| c.as_ref() == Some(&first)) {
        Some((widths[0], first))
    } else {
        None
    }
}

/// How a view's border gets painted — the full routing decision, including
/// the line pattern. Both backends `match` on this so they cannot disagree
/// about which mechanism a style lands on (Rule #7).
///
/// `CALayer.borderWidth` has no dash pattern, so a dashed/dotted border can't
/// ride the uniform CALayer stroke; it becomes a stroked `CAShapeLayer`
/// instead (see [`crate::border_dash_layer`]). The uniform-vs-per-side split is
/// the same one [`uniform_border`] makes for solid borders, so a style that
/// only flips `border_style` keeps its shape: a rounded card stays one loop
/// that follows the corner curve, a bottom underline stays one straight line.
#[derive(Clone, Debug, PartialEq)]
pub enum BorderRoute {
    /// No side has a width: clear every border mechanism.
    None,
    /// Uniform solid border → `CALayer.borderWidth`/`borderColor`.
    Layer { width: f32, color: Color },
    /// Asymmetric solid border → straight per-side bar views
    /// (`[top, right, bottom, left]`, `None` = side not drawn).
    Bars([Option<(f32, Color)>; 4]),
    /// Uniform dashed/dotted border → one closed `CAShapeLayer` loop along
    /// the border's centreline, following the corner radius.
    DashedLoop { style: BorderStyle, width: f32, color: Color },
    /// Asymmetric dashed/dotted border → one open dashed line per side.
    DashedSides { style: BorderStyle, sides: [Option<(f32, Color)>; 4] },
}

/// Route a style's border. `style` is `StyleRules::border_style` (`None` =
/// solid); `widths`/`colors` are the four resolved sides, `[top, right,
/// bottom, left]`, with the same fallback-colour rule as [`uniform_border`].
pub fn route_border(
    style: Option<BorderStyle>,
    widths: [f32; 4],
    colors: &[Option<Color>; 4],
) -> BorderRoute {
    if !widths.iter().any(|w| *w > 0.0) {
        return BorderRoute::None;
    }
    let style = style.unwrap_or_default();
    let patterned = style != BorderStyle::Solid;
    if let Some((width, color)) = uniform_border(widths, colors) {
        return if patterned {
            BorderRoute::DashedLoop { style, width, color }
        } else {
            BorderRoute::Layer { width, color }
        };
    }
    let fallback = colors.iter().find_map(|c| c.clone());
    let sides: [Option<(f32, Color)>; 4] = std::array::from_fn(|i| {
        if widths[i] <= 0.0 {
            return None;
        }
        colors[i].clone().or_else(|| fallback.clone()).map(|c| (widths[i], c))
    });
    if patterned {
        BorderRoute::DashedSides { style, sides }
    } else {
        BorderRoute::Bars(sides)
    }
}

/// One step of a stroke path, in the target LAYER's coordinates. Mirrors the
/// CoreGraphics calls the CALayer side replays one for one
/// (`CGPathMoveToPoint` / `CGPathAddLineToPoint` / `CGPathAddArcToPoint` /
/// `CGPathCloseSubpath`).
///
/// Corners use the tangent form (`ArcTo`), not `CGPathAddArc`'s
/// centre + angles + `clockwise` flag: the tangent form is pure geometry with
/// no orientation flag, so the same op list is correct in a y-down layer
/// (UIKit, a flipped `NSView`) and — after flipping every point — in a y-up
/// one (a non-flipped `NSView`), with no chance of an arc bulging the wrong way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathOp {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    /// Arc of `r` tangent to (current → `(x1, y1)`) and (`(x1, y1)` → `(x2, y2)`).
    ArcTo { x1: f32, y1: f32, x2: f32, y2: f32, r: f32 },
    Close,
}

/// Everything a `CAShapeLayer` needs to stroke one patterned border piece.
#[derive(Clone, Debug, PartialEq)]
pub struct DashStroke {
    pub ops: Vec<PathOp>,
    /// `lineWidth` — the border width.
    pub line_width: f32,
    /// `lineDashPattern` — `[on, off]` in toolkit terms (dots are
    /// zero-length round-capped dashes, see
    /// [`border_dash::Dash::toolkit_array`]).
    pub dash: [f32; 2],
    /// `lineDashPhase`.
    pub phase: f32,
    /// `lineCap`: round for dots, butt for dashes.
    pub round_cap: bool,
}

/// Map a y-down path coordinate into the layer: identity for a y-down layer,
/// mirrored about the box's horizontal midline for a y-up one.
fn fy(y: f32, box_h: f32, y_down: bool) -> f32 {
    if y_down { y } else { box_h - y }
}

/// The closed patterned loop for a UNIFORM dashed/dotted border.
///
/// `radius` is the layer's live (already clamped) `cornerRadius` — the same
/// single radius the CALayer background and a solid CALayer border follow, so
/// a dashed border traces exactly the curve its own fill is rounded to. (Both
/// Apple backends collapse the four authored corner radii to one
/// `cornerRadius`; tracing different per-corner radii here would put the dashes
/// off the fill's edge.) `LoopPath` still carries four radii, so the path
/// builder is per-corner already if the backends ever grow per-corner fills.
///
/// Returns `None` for a solid style, a zero width, or an empty box.
pub fn loop_stroke(
    style: BorderStyle,
    box_w: f32,
    box_h: f32,
    radius: f32,
    width: f32,
    y_down: bool,
) -> Option<DashStroke> {
    if !(box_w > 0.0 && box_h > 0.0) {
        return None;
    }
    let base = border_dash::base(style, width)?;
    let path = LoopPath::new(box_w, box_h, [radius.max(0.0); 4], width);
    let fitted = border_dash::fit_closed(base, path.length());
    let [tl, tr, br, bl] = path.radii;
    let (x0, y0, x1, y1) = (path.x, path.y, path.x + path.w, path.y + path.h);
    let p = |x: f32, y: f32| (x, fy(y, box_h, y_down));
    let arc = |(ax, ay): (f32, f32), (bx, by): (f32, f32), r: f32| PathOp::ArcTo {
        x1: ax,
        y1: ay,
        x2: bx,
        y2: by,
        r,
    };
    // Same walk as `LoopPath::point_at`: clockwise (in y-down terms) from the
    // left end of the top edge. CoreAnimation starts the dash pattern at the
    // first `MoveTo`, so this start point is where the fitted pattern's seam
    // lands — matching `dash_runs` and every other backend.
    let (sx, sy) = p(x0 + tl, y0);
    let (ex, ey) = p(x1 - tr, y0);
    let (rx, ry) = p(x1, y1 - br);
    let (bx, by) = p(x0 + bl, y1);
    let (lx, ly) = p(x0, y0 + tl);
    let ops = vec![
        PathOp::MoveTo(sx, sy),
        PathOp::LineTo(ex, ey),
        arc(p(x1, y0), p(x1, y0 + tr), tr),
        PathOp::LineTo(rx, ry),
        arc(p(x1, y1), p(x1 - br, y1), br),
        PathOp::LineTo(bx, by),
        arc(p(x0, y1), p(x0, y1 - bl), bl),
        PathOp::LineTo(lx, ly),
        arc(p(x0, y0), p(x0 + tl, y0), tl),
        PathOp::Close,
    ];
    Some(DashStroke {
        ops,
        line_width: width,
        dash: fitted.toolkit_array(),
        phase: fitted.toolkit_phase(),
        round_cap: fitted.round,
    })
}

/// One side's open patterned line for an ASYMMETRIC dashed/dotted border.
/// `side` is `0..4` = top, right, bottom, left. The line spans the full side
/// (like the solid per-side bars) at half its width in from the edge, fitted
/// with [`border_dash::fit_open`] so it starts and ends on a mark.
pub fn side_stroke(
    style: BorderStyle,
    box_w: f32,
    box_h: f32,
    side: usize,
    width: f32,
    y_down: bool,
) -> Option<DashStroke> {
    if !(box_w > 0.0 && box_h > 0.0) {
        return None;
    }
    let base = border_dash::base(style, width)?;
    let (ax, ay, bx, by) = border_dash::side_line(box_w, box_h, side, width);
    let len = if side % 2 == 0 { box_w } else { box_h };
    let fitted = border_dash::fit_open(base, len);
    Some(DashStroke {
        ops: vec![
            PathOp::MoveTo(ax, fy(ay, box_h, y_down)),
            PathOp::LineTo(bx, fy(by, box_h, y_down)),
        ],
        line_width: width,
        dash: fitted.toolkit_array(),
        // Open line: the first mark STARTS at the corner (the shared
        // helper; `toolkit_phase` is the closed-loop one).
        phase: fitted.open_toolkit_phase(),
        round_cap: fitted.round,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(s: &str) -> Option<Color> {
        Some(Color(s.to_string()))
    }

    // The bug: a uniform border on a rounded card was drawn as four
    // straight bars, whose corners get sliced by the parent's
    // rounded-corner clip mask, leaving notches. The fix routes the
    // uniform case to a CALayer stroke (which follows cornerRadius).
    // This test pins the routing decision that makes that happen.
    #[test]
    fn regression_uniform_rounded_border_uses_calayer() {
        // All four sides identical width + color → collapse to CALayer.
        let widths = [1.0; 4];
        let colors = [col("#e5e5e5"), col("#e5e5e5"), col("#e5e5e5"), col("#e5e5e5")];
        assert_eq!(uniform_border(widths, &colors), Some((1.0, Color("#e5e5e5".into()))));
    }

    #[test]
    fn width_without_per_side_color_falls_back_and_collapses() {
        // Author set a single border color (top) + equal widths; the
        // fallback fills the other sides, so it's still uniform.
        let widths = [2.0; 4];
        let colors = [col("#000"), None, None, None];
        assert_eq!(uniform_border(widths, &colors), Some((2.0, Color("#000".into()))));
    }

    #[test]
    fn differing_widths_stay_per_side() {
        // A bottom-only border (the per-side feature this must not
        // regress) must NOT collapse — it needs a single bar.
        let widths = [0.0, 0.0, 1.0, 0.0];
        let colors = [None, None, col("#000"), None];
        assert_eq!(uniform_border(widths, &colors), None);
    }

    #[test]
    fn differing_colors_stay_per_side() {
        // Equal widths but two distinct colors → CALayer can't express
        // it, so keep the per-side bars.
        let widths = [1.0; 4];
        let colors = [col("#f00"), col("#0f0"), col("#f00"), col("#0f0")];
        assert_eq!(uniform_border(widths, &colors), None);
    }
    // ---- patterned borders -------------------------------------------------

    fn black4() -> [Option<Color>; 4] {
        [col("#000"), col("#000"), col("#000"), col("#000")]
    }

    // The bug: `CALayer.borderWidth` cannot dash, so before this routing a
    // `border_style: Dashed` card was stroked by the uniform CALayer path and
    // rendered as a SOLID border on iOS/macOS while web showed dashes. A
    // patterned uniform border must NOT land on the CALayer stroke.
    #[test]
    fn regression_dashed_border_does_not_fall_back_to_solid_calayer() {
        let route = route_border(Some(BorderStyle::Dashed), [1.0; 4], &black4());
        assert!(
            matches!(route, BorderRoute::DashedLoop { style: BorderStyle::Dashed, width, .. } if width == 1.0),
            "got {route:?}"
        );
        let route = route_border(Some(BorderStyle::Dotted), [2.0; 4], &black4());
        assert!(matches!(route, BorderRoute::DashedLoop { style: BorderStyle::Dotted, .. }));
    }

    #[test]
    fn solid_and_unset_style_keep_the_calayer_stroke() {
        for style in [None, Some(BorderStyle::Solid)] {
            assert_eq!(
                route_border(style, [1.0; 4], &black4()),
                BorderRoute::Layer { width: 1.0, color: Color("#000".into()) }
            );
        }
    }

    #[test]
    fn asymmetric_patterned_border_routes_to_dashed_sides_with_fallback_colour() {
        let widths = [0.0, 0.0, 2.0, 1.0];
        let colors = [None, None, col("#f00"), None];
        match route_border(Some(BorderStyle::Dashed), widths, &colors) {
            BorderRoute::DashedSides { style, sides } => {
                assert_eq!(style, BorderStyle::Dashed);
                assert_eq!(sides[0], None);
                assert_eq!(sides[1], None);
                assert_eq!(sides[2], Some((2.0, Color("#f00".into()))));
                // The left side had no colour of its own → first authored one.
                assert_eq!(sides[3], Some((1.0, Color("#f00".into()))));
            }
            other => panic!("expected DashedSides, got {other:?}"),
        }
        // Same shape as the solid bars — only the mechanism differs.
        assert!(matches!(route_border(None, widths, &colors), BorderRoute::Bars(_)));
    }

    #[test]
    fn no_width_routes_to_none_whatever_the_style() {
        assert_eq!(route_border(Some(BorderStyle::Dotted), [0.0; 4], &black4()), BorderRoute::None);
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn loop_stroke_uses_the_shared_fitted_pattern() {
        // 100×40 box, radius 8, 2px dashed.
        let s = loop_stroke(BorderStyle::Dashed, 100.0, 40.0, 8.0, 2.0, true).unwrap();
        let path = LoopPath::new(100.0, 40.0, [8.0; 4], 2.0);
        let fitted = border_dash::fit_closed(border_dash::base(BorderStyle::Dashed, 2.0).unwrap(), path.length());
        assert_eq!(s.dash, fitted.toolkit_array());
        assert_eq!(s.phase, fitted.toolkit_phase());
        assert_eq!(s.line_width, 2.0);
        assert!(!s.round_cap);
        // Whole periods around the loop — the seam closes on a gap.
        let n = path.length() / (s.dash[0] + s.dash[1]);
        assert!(close(n, n.round()), "{n}");
    }

    #[test]
    fn dotted_loop_is_zero_length_round_dashes() {
        let s = loop_stroke(BorderStyle::Dotted, 50.0, 50.0, 0.0, 3.0, true).unwrap();
        assert_eq!(s.dash[0], 0.0);
        assert!(s.round_cap);
        assert_eq!(s.phase, 0.0);
    }

    #[test]
    fn loop_path_starts_at_the_top_edge_and_insets_half_the_width() {
        // y-down: start at (inset + r', inset) where r' = r - w/2.
        let s = loop_stroke(BorderStyle::Dashed, 100.0, 40.0, 8.0, 2.0, true).unwrap();
        assert_eq!(s.ops[0], PathOp::MoveTo(8.0, 1.0));
        assert_eq!(s.ops[1], PathOp::LineTo(92.0, 1.0));
        assert_eq!(s.ops[2], PathOp::ArcTo { x1: 99.0, y1: 1.0, x2: 99.0, y2: 8.0, r: 7.0 });
        assert_eq!(*s.ops.last().unwrap(), PathOp::Close);
    }

    #[test]
    fn y_up_layer_mirrors_the_path_so_the_top_edge_stays_on_top() {
        // A non-flipped NSView's layer has y = 0 at the BOTTOM. The visual top
        // edge must land at y = box_h - w/2 there, or a flipped / non-flipped
        // pair would start their dash pattern on opposite edges.
        let s = loop_stroke(BorderStyle::Dashed, 100.0, 40.0, 0.0, 2.0, false).unwrap();
        assert_eq!(s.ops[0], PathOp::MoveTo(1.0, 39.0));
        let side = side_stroke(BorderStyle::Dashed, 100.0, 40.0, 0, 2.0, false).unwrap();
        assert_eq!(side.ops, vec![PathOp::MoveTo(0.0, 39.0), PathOp::LineTo(100.0, 39.0)]);
        let side = side_stroke(BorderStyle::Dashed, 100.0, 40.0, 0, 2.0, true).unwrap();
        assert_eq!(side.ops, vec![PathOp::MoveTo(0.0, 1.0), PathOp::LineTo(100.0, 1.0)]);
    }

    #[test]
    fn side_stroke_fits_per_side_and_starts_on_a_mark() {
        let s = side_stroke(BorderStyle::Dashed, 40.0, 100.0, 1, 1.0, true).unwrap();
        // The right side is box_h long.
        let fitted = border_dash::fit_open(border_dash::base(BorderStyle::Dashed, 1.0).unwrap(), 100.0);
        assert_eq!(s.dash, fitted.toolkit_array());
        assert_eq!(s.phase, 0.0, "a butt dash starts AT the line start");
        assert_eq!(s.ops, vec![PathOp::MoveTo(39.5, 0.0), PathOp::LineTo(39.5, 100.0)]);
    }

    #[test]
    fn side_stroke_puts_dots_where_dash_runs_does() {
        // A dot (zero-length round dash) at pattern position 0 lands at
        // distance `k·period − phase`; the first must be centred on `on/2`
        // — the first run's midpoint in the shared reference.
        let fitted = border_dash::fit_open(border_dash::base(BorderStyle::Dotted, 2.0).unwrap(), 30.0);
        let runs = border_dash::dash_runs(fitted, 30.0, false);
        let phase = side_stroke(BorderStyle::Dotted, 30.0, 10.0, 0, 2.0, true).unwrap().phase;
        assert_eq!(phase, fitted.open_toolkit_phase());
        let first = fitted.period() - phase;
        assert!(close(first, runs[0].mid()), "{first} vs {}", runs[0].mid());
        let last = first + (runs.len() - 1) as f32 * fitted.period();
        assert!(close(last, runs.last().unwrap().mid()), "{last}");
    }

    #[test]
    fn empty_box_or_solid_style_plans_nothing() {
        assert!(loop_stroke(BorderStyle::Dashed, 0.0, 10.0, 0.0, 1.0, true).is_none());
        assert!(loop_stroke(BorderStyle::Solid, 10.0, 10.0, 0.0, 1.0, true).is_none());
        assert!(side_stroke(BorderStyle::Dotted, 10.0, 0.0, 0, 1.0, true).is_none());
    }
}
