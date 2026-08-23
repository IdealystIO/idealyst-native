//! Golden + invariant tests for the render pipeline.
//!
//! The goldens are SVG rather than pixels: they diff readably, they are
//! stable across machines (the approximate metrics are deterministic and
//! font-independent by construction), and a reviewer can open one. Refresh
//! with `UPDATE_GOLDENS=1 cargo test -p charts-core`.

use charts_core::render::Gutters;
use charts_core::svg::{to_svg, ApproxMetrics};
use charts_core::scene::{GradientStop, PointInstance};
use charts_core::*;

const BLUE: Color = Color::rgb(0x4c, 0x8d, 0xff);
const PINK: Color = Color::rgb(0xff, 0x6b, 0x9d);
const TEXT: Color = Color::rgb(0x33, 0x33, 0x33);
const SIZE: (f32, f32) = (480.0, 300.0);

fn surface() -> Rect {
    Rect::new(0.0, 0.0, SIZE.0, SIZE.1)
}

fn check_golden(name: &str, svg: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(format!("{name}.svg"));
    if std::env::var("UPDATE_GOLDENS").is_ok() {
        std::fs::write(&path, svg).expect("write golden");
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}.svg — run with UPDATE_GOLDENS=1"));
    assert_eq!(want.trim(), svg.trim(), "golden mismatch for {name}");
}

fn render_golden(name: &str, spec: &ChartSpec) -> ChartOutput {
    let out = render_with(spec, surface(), &Gutters::Measured(&ApproxMetrics));
    check_golden(name, &to_svg(&out, SIZE, TEXT));
    out
}

fn line_spec() -> ChartSpec {
    ChartSpec::new(vec![Series::new(
        "revenue",
        SeriesKind::line(),
        BLUE,
        (0..8).map(|i| datum(i as f64, [3.0, 7.0, 5.0, 9.0, 8.0, 12.0, 11.0, 15.0][i])).collect(),
    )])
    .y(Axis::linear().include_zero(true).title("USD"))
}

#[test]
fn golden_line() {
    render_golden("line", &line_spec());
}

#[test]
fn golden_smooth_line_and_points() {
    let mut spec = line_spec();
    spec.series[0].kind = SeriesKind::Line(
        LineStyle::new(2.5).smooth().with_points(PointStyle::new(3.5)),
    );
    render_golden("smooth_line", &spec);
}

#[test]
fn golden_area() {
    let mut spec = line_spec();
    spec.series[0].kind = SeriesKind::area();
    render_golden("area", &spec);
}

#[test]
fn golden_grouped_bars() {
    let spec = ChartSpec::new(vec![
        Series::new("q1", SeriesKind::bar(), BLUE, vec![datum(0.0, 4.0), datum(1.0, 7.0), datum(2.0, 5.0)]),
        Series::new("q2", SeriesKind::bar(), PINK, vec![datum(0.0, 6.0), datum(1.0, 3.0), datum(2.0, 8.0)]),
    ])
    .x(Axis::category(["north", "south", "east"]))
    .y(Axis::linear().include_zero(true))
    .legend(true);
    render_golden("grouped_bars", &spec);
}

#[test]
fn golden_stacked_bars() {
    let spec = ChartSpec::new(vec![
        Series::new("q1", SeriesKind::bar(), BLUE, vec![datum(0.0, 4.0), datum(1.0, 7.0), datum(2.0, 5.0)]),
        Series::new("q2", SeriesKind::bar(), PINK, vec![datum(0.0, 6.0), datum(1.0, 3.0), datum(2.0, 8.0)]),
    ])
    .x(Axis::category(["north", "south", "east"]))
    .y(Axis::linear().include_zero(true))
    .bars(BarLayout::Stacked);
    render_golden("stacked_bars", &spec);
}

#[test]
fn golden_scatter() {
    let spec = ChartSpec::new(vec![Series::new(
        "samples",
        SeriesKind::scatter(),
        PINK,
        (0..20).map(|i| datum(i as f64 * 0.5, ((i * 7) % 13) as f64)).collect(),
    )]);
    render_golden("scatter", &spec);
}

// ---------------------------------------------------------------------------
// Invariants
// ---------------------------------------------------------------------------

/// Smoothing must not invent values outside the data's range.
///
/// Regression guard for the reason `monotone_cubic` is Fritsch-Carlson and
/// not Catmull-Rom: a plain spline through a plateau followed by a rise
/// dips BELOW the plateau first, which on a chart of non-negative values
/// renders a visible excursion under zero that the data does not contain.
#[test]
fn regression_smoothing_never_overshoots_data_range() {
    let data = vec![
        datum(0.0, 0.0),
        datum(1.0, 0.0),
        datum(2.0, 0.0),
        datum(3.0, 10.0),
        datum(4.0, 10.0),
        datum(5.0, 0.0),
    ];
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::smooth_line(),
        BLUE,
        data,
    )]);
    let plot = Rect::new(0.0, 0.0, 300.0, 200.0);
    let out = render(&spec, plot);

    // Every control point and endpoint of the curve must stay within the
    // pixel band spanned by the data. Control points are what a spline
    // overshoots with, so checking only the on-curve points would pass even
    // for an overshooting implementation.
    let ys: Vec<f32> = out
        .scene
        .marks
        .iter()
        .filter_map(|m| match m {
            Mark::Stroke { path, .. } => Some(path),
            _ => None,
        })
        .flat_map(|p| p.segs.iter())
        .flat_map(|s| match s {
            PathSeg::MoveTo(a) | PathSeg::LineTo(a) => vec![a.y],
            PathSeg::QuadTo(c, a) => vec![c.y, a.y],
            PathSeg::CubicTo(c1, c2, a) => vec![c1.y, c2.y, a.y],
            PathSeg::Close => vec![],
        })
        .collect();
    assert!(!ys.is_empty(), "expected a stroked curve");

    let top = out.y.map(10.0, plot.bottom(), plot.y);
    let bottom = out.y.map(0.0, plot.bottom(), plot.y);
    for y in ys {
        assert!(
            y >= top - 0.01 && y <= bottom + 0.01,
            "curve escaped the data range: y={y} not within [{top}, {bottom}]"
        );
    }
}

/// A `Fixed` domain is a viewport and must survive verbatim — this is the
/// contract pan/zoom is built on. If auto-fitting could widen it back to
/// the data extent, a pan would snap back on the next render.
#[test]
fn fixed_domain_is_not_widened_by_data() {
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::line(),
        BLUE,
        vec![datum(0.0, 0.0), datum(100.0, 1000.0)],
    )])
    .x(Axis::linear().domain(Domain::fixed(10.0, 20.0)))
    .y(Axis::linear().domain(Domain::fixed(-5.0, 5.0)));

    let out = render(&spec, surface());
    assert_eq!((out.x.min, out.x.max), (10.0, 20.0));
    assert_eq!((out.y.min, out.y.max), (-5.0, 5.0));
}

/// Zooming about a focus point leaves that point where it was on screen.
/// Without this, pinch-zoom drifts away from the fingers.
#[test]
fn zoom_preserves_the_focus_point() {
    let d = Domain::fixed(0.0, 100.0);
    let focus_frac = 0.25; // data value 25
    let zoomed = d.zoom(0.5, focus_frac);
    let Domain::Fixed { min, max } = zoomed else {
        panic!("zoom of a fixed domain must stay fixed");
    };
    // 25 must still sit a quarter of the way across the new window.
    let where_now = (25.0 - min) / (max - min);
    assert!(
        (where_now - focus_frac).abs() < 1e-9,
        "focus drifted to {where_now}"
    );
    assert!((max - min - 50.0).abs() < 1e-9, "expected half the width");
}

#[test]
fn translate_shifts_without_rescaling() {
    let Domain::Fixed { min, max } = Domain::fixed(0.0, 10.0).translate(0.5) else {
        panic!("expected fixed");
    };
    assert_eq!((min, max), (5.0, 15.0));
}

/// A stacked chart's y domain must cover the stack total, not the tallest
/// single segment — otherwise the top of every stack is clipped.
#[test]
fn regression_stacked_domain_covers_the_total() {
    let spec = ChartSpec::new(vec![
        Series::new("a", SeriesKind::bar(), BLUE, vec![datum(0.0, 60.0)]),
        Series::new("b", SeriesKind::bar(), PINK, vec![datum(0.0, 60.0)]),
    ])
    .x(Axis::category(["only"]))
    .bars(BarLayout::Stacked);

    let out = render(&spec, surface());
    assert!(
        out.y.max >= 120.0,
        "stacked domain must reach the 120 total, got {}",
        out.y.max
    );
}

/// Paint order is a correctness property, not a cosmetic one: gridlines
/// drawn over the series is a real and frequently-shipped bug.
#[test]
fn marks_are_sorted_into_paint_order() {
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::area(),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 2.0), datum(2.0, 1.5)],
    )]);
    let out = render(&spec, surface());
    let layers: Vec<Layer> = out.scene.marks.iter().map(|m| m.layer()).collect();
    let mut sorted = layers.clone();
    sorted.sort();
    assert_eq!(layers, sorted, "marks must be emitted in layer order");
    assert!(layers.contains(&Layer::Grid) && layers.contains(&Layer::Series));
}

/// A hidden series draws nothing but keeps its identity — the legend still
/// lists it, and the other series keep their colors and their group slots.
#[test]
fn hidden_series_draws_nothing_but_keeps_its_slot() {
    let mut spec = ChartSpec::new(vec![
        Series::new("a", SeriesKind::bar(), BLUE, vec![datum(0.0, 1.0)]),
        Series::new("b", SeriesKind::bar(), PINK, vec![datum(0.0, 2.0)]),
    ])
    .x(Axis::category(["one"]));

    let both = render(&spec, surface());
    spec.series[0].visible = false;
    let one = render(&spec, surface());

    assert_eq!(both.hit.column_at(pt(100.0, 100.0)).len(), 2);
    let remaining = one.hit.column_at(pt(100.0, 100.0));
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].series, 1,
        "surviving hit must still index the ORIGINAL series list"
    );
}

/// A tooltip hovering a column shows every series at that x, ordered by
/// series — not just whichever mark is nearest the cursor vertically.
#[test]
fn column_hit_returns_all_series_at_that_x() {
    let spec = ChartSpec::new(vec![
        Series::new("a", SeriesKind::line(), BLUE, vec![datum(0.0, 1.0), datum(1.0, 5.0)]),
        Series::new("b", SeriesKind::line(), PINK, vec![datum(0.0, 9.0), datum(1.0, 2.0)]),
    ]);
    let out = render(&spec, surface());

    // Hover near the right edge, vertically close to series "a".
    let near_right = pt(out.scene.plot.right() - 1.0, out.scene.plot.bottom());
    let col = out.hit.column_at(near_right);
    assert_eq!(col.len(), 2, "both series should report at this x");
    assert_eq!(col[0].series, 0);
    assert_eq!(col[1].series, 1);
    assert_eq!(col[0].datum.y, 5.0);
    assert_eq!(col[1].datum.y, 2.0);
}

#[test]
fn nearest_within_rejects_far_pointers() {
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::scatter(),
        BLUE,
        vec![datum(0.0, 0.0)],
    )]);
    let out = render(&spec, surface());
    let p = out.hit.nearest(pt(0.0, 0.0)).expect("some datum");
    assert!(out.hit.nearest_within(p.position, 2.0).is_some());
    assert!(out
        .hit
        .nearest_within(pt(p.position.x + 500.0, p.position.y), 10.0)
        .is_none());
}

/// Every y identical would make the domain zero-width and every mapped
/// pixel a NaN. The scale must widen it instead.
#[test]
fn regression_constant_series_does_not_produce_nan() {
    let spec = ChartSpec::new(vec![Series::new(
        "flat",
        SeriesKind::line(),
        BLUE,
        vec![datum(0.0, 5.0), datum(1.0, 5.0), datum(2.0, 5.0)],
    )]);
    let out = render(&spec, surface());
    assert!(out.y.max > out.y.min, "domain must have width");
    for m in &out.scene.marks {
        if let Mark::Stroke { path, .. } = m {
            for seg in &path.segs {
                if let PathSeg::MoveTo(a) | PathSeg::LineTo(a) = seg {
                    assert!(a.x.is_finite() && a.y.is_finite(), "NaN in path: {a:?}");
                }
            }
        }
    }
}

/// A single data point is a degenerate extent in BOTH axes at once.
#[test]
fn single_point_renders_without_panicking() {
    let spec = ChartSpec::new(vec![Series::new(
        "one",
        SeriesKind::scatter(),
        BLUE,
        vec![datum(42.0, 42.0)],
    )]);
    let out = render(&spec, surface());
    assert_eq!(out.hit.nearest(pt(0.0, 0.0)).map(|r| r.datum.x), Some(42.0));
}

#[test]
fn empty_spec_renders_an_empty_but_valid_scene() {
    let out = render(&ChartSpec::default(), surface());
    assert!(out.hit.is_empty());
    assert!(out.y.max > out.y.min);
}

/// Log axes cannot place non-positive values; those points are dropped
/// rather than drawn at a bogus coordinate.
#[test]
fn log_axis_drops_non_positive_points() {
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::scatter(),
        BLUE,
        vec![datum(1.0, -5.0), datum(2.0, 0.0), datum(3.0, 100.0)],
    )])
    .y(Axis::log());
    let out = render(&spec, surface());

    let hits: Vec<_> = (0..3)
        .filter_map(|_| out.hit.nearest(pt(0.0, 0.0)))
        .collect();
    assert!(!hits.is_empty());
    assert!(
        !out.hit.is_empty(),
        "the positive point should still be plotted"
    );
    assert_eq!(
        out.hit.column_at(pt(out.scene.plot.right(), 0.0)).len(),
        1,
        "only the positive datum is plottable"
    );
}

/// A radius larger than the bar is clamped to a pill rather than producing
/// self-intersecting curves.
#[test]
fn oversized_corner_radius_clamps() {
    let r = Rect::new(0.0, 0.0, 10.0, 4.0);
    let p = Path::rounded_rect(r, [999.0; 4]);
    for seg in &p.segs {
        let pts = match seg {
            PathSeg::MoveTo(a) | PathSeg::LineTo(a) => vec![*a],
            PathSeg::QuadTo(c, a) => vec![*c, *a],
            PathSeg::CubicTo(c1, c2, a) => vec![*c1, *c2, *a],
            PathSeg::Close => vec![],
        };
        for a in pts {
            assert!(
                a.x >= -0.01 && a.x <= 10.01 && a.y >= -0.01 && a.y <= 4.01,
                "control point {a:?} escaped the rect"
            );
        }
    }
}

/// A log axis must not hang, whatever the data or the hand-set domain.
///
/// Regression guard for a real infinite loop in plotters 0.3.7. Clamping a
/// log axis's lower bound to `f64::MIN_POSITIVE` produces a ~308-decade
/// span; `LogCoord::key_points` then computes `end / start`, which overflows
/// f64 to `+inf`, and `inf as usize` SATURATES to `usize::MAX`. The loop
/// `while max_points < bold_count / cnt { cnt += 1 }` consequently needs
/// ~3.7e18 iterations to terminate. No panic, no diagnostic — the process
/// just stops responding, which is why this test asserts by completing.
#[test]
fn regression_log_axis_with_extreme_span_terminates() {
    // All-non-positive data: nothing legitimately positive to anchor on.
    let a = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::line(),
        BLUE,
        vec![datum(1.0, -5.0), datum(2.0, 0.0)],
    )])
    .y(Axis::log());
    let out = render(&a, surface());
    assert!(out.y.min > 0.0, "log domain must stay positive");
    assert!(out.y.max / out.y.min <= 1e12 + 1.0, "span must be clamped");

    // A caller setting a deliberately absurd viewport by hand.
    let b = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::line(),
        BLUE,
        vec![datum(1.0, 1.0), datum(2.0, 10.0)],
    )])
    .y(Axis::log().domain(Domain::fixed(f64::MIN_POSITIVE, 1e30)));
    let out = render(&b, surface());
    assert!(out.y.min > 0.0);
    assert!(!out.y.ticks.is_empty(), "clamped axis must still produce ticks");
}

/// A log axis over a single positive value still has width.
#[test]
fn log_axis_with_one_positive_point_has_width() {
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::scatter(),
        BLUE,
        vec![datum(1.0, 50.0)],
    )])
    .y(Axis::log());
    let out = render(&spec, surface());
    assert!(out.y.max > out.y.min && out.y.min > 0.0);
}

/// The reference SVG must be valid SVG 1.1, not browser-only CSS.
///
/// Regression guard: emitting `rgba(...)` for a translucent fill renders
/// correctly in WebKit and silently fails in strict SVG 1.1 renderers —
/// gridlines disappear and gradient fills fall back to black. Alpha belongs
/// in `fill-opacity` / `stroke-opacity`.
#[test]
fn regression_svg_uses_opacity_attributes_not_rgba() {
    let mut spec = line_spec();
    spec.series[0].kind = SeriesKind::area(); // translucent fill + faint grid
    let out = render_with(&spec, surface(), &Gutters::Measured(&ApproxMetrics));
    let svg = to_svg(&out, SIZE, TEXT);

    assert!(!svg.contains("rgba("), "SVG 1.1 has no alpha in <color>");
    assert!(
        svg.contains("stroke-opacity=") || svg.contains("fill-opacity="),
        "translucent marks must carry an explicit opacity attribute"
    );
}

/// A zero-area plot renders nothing.
///
/// Every host passes through this state: the plot rect is measured after
/// first mount, so frame one always has size (0, 0). Emitting the full
/// mark set there wastes a render per chart per mount and hands the
/// renderer geometry collapsed onto a degenerate line. The axes still
/// resolve, so a caller can read the domain before layout lands.
#[test]
fn zero_area_plot_renders_no_marks() {
    let out = render(&line_spec(), Rect::new(0.0, 0.0, 0.0, 0.0));
    assert!(out.scene.marks.is_empty(), "no marks for a zero-area plot");
    assert!(out.scene.labels.is_empty(), "no labels either");
    assert!(out.hit.is_empty());
    assert!(out.y.max > out.y.min, "but the domain is still resolved");
    assert!(!out.y.ticks.is_empty(), "and ticks are still available");

    // Degenerate in one axis only is just as unrenderable.
    let flat = render(&line_spec(), Rect::new(0.0, 0.0, 300.0, 0.0));
    assert!(flat.scene.marks.is_empty());
}

/// An auto-fitted domain must extend PAST the data, not stop on it.
///
/// Regression guard: tick selection only returns values inside the range,
/// so rounding the domain to `max(hi, last_tick)` leaves it pinned to the
/// raw data max. The topmost point then sits exactly on the plot's top
/// edge, and because the plot clips its overflow the outer half of the
/// line's stroke is shaved off — the peak renders visibly flat.
#[test]
fn regression_auto_domain_extends_past_the_data_extreme() {
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::line(),
        BLUE,
        vec![datum(0.0, 3.0), datum(1.0, 11.6), datum(2.0, 5.0)],
    )])
    .y(Axis::linear().include_zero(true));

    let out = render(&spec, surface());
    assert!(
        out.y.max > 11.6,
        "domain top {} must clear the data max 11.6",
        out.y.max
    );

    // And the peak must be strictly inside the plot, by more than a stroke
    // half-width, so nothing is clipped.
    let peak_y = out.y.map(11.6, out.scene.plot.bottom(), out.scene.plot.y);
    assert!(
        peak_y > out.scene.plot.y + 2.0,
        "peak at y={peak_y} is on the plot's top edge {}",
        out.scene.plot.y
    );
}

/// Rounding outward must not add a whole empty step when the data already
/// ends exactly on a tick.
#[test]
fn auto_domain_does_not_pad_data_already_on_a_tick() {
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::bar(),
        BLUE,
        vec![datum(0.0, 0.0), datum(1.0, 100.0)],
    )])
    .x(Axis::category(["a", "b"]))
    .y(Axis::linear().include_zero(true));

    let out = render(&spec, surface());
    assert!(
        out.y.max <= 125.0,
        "expected a tight domain around 100, got {}",
        out.y.max
    );
}

/// In a stacked column only the OUTERMOST segment rounds its corners.
///
/// Regression guard: applying the grouped-bar rule (every bar rounds its
/// top) to a stack puts a rounded seam between segments, so one column
/// reads as a pile of separate pills. Reported from the demo.
#[test]
fn regression_stacked_segments_round_only_at_the_stack_ends() {
    let spec = ChartSpec::new(vec![
        Series::new("bottom", SeriesKind::bar(), BLUE, vec![datum(0.0, 5.0)]),
        Series::new("middle", SeriesKind::bar(), PINK, vec![datum(0.0, 5.0)]),
        Series::new("top", SeriesKind::bar(), BLUE, vec![datum(0.0, 5.0)]),
    ])
    .x(Axis::category(["only"]))
    .bars(BarLayout::Stacked);

    let out = render(&spec, surface());
    // Bars are filled paths; a rounded one carries cubic segments, a square
    // one is only move/line/close.
    let curvy: Vec<bool> = out
        .scene
        .marks
        .iter()
        .filter_map(|m| match m {
            Mark::Fill { layer: Layer::Series, path, .. } => Some(
                path.segs.iter().any(|s| matches!(s, PathSeg::CubicTo(..))),
            ),
            _ => None,
        })
        .collect();

    assert_eq!(curvy.len(), 3, "three stacked segments");
    assert_eq!(
        curvy.iter().filter(|c| **c).count(),
        1,
        "exactly one segment (the top of the stack) may be rounded"
    );
    // Emission order follows series order, so the LAST one is the stack top.
    assert!(curvy[2], "the topmost segment is the one that rounds");
}

/// Grouped bars are each their own column, so every one still rounds.
#[test]
fn grouped_bars_all_round_their_own_top() {
    let spec = ChartSpec::new(vec![
        Series::new("a", SeriesKind::bar(), BLUE, vec![datum(0.0, 5.0)]),
        Series::new("b", SeriesKind::bar(), PINK, vec![datum(0.0, 7.0)]),
    ])
    .x(Axis::category(["only"]))
    // Without a zero baseline the domain fits 5..7 and the y=5 bar has
    // ZERO height — which correctly degrades to no rounding, and would make
    // this test assert the wrong thing.
    .y(Axis::linear().include_zero(true))
    .bars(BarLayout::Grouped);

    let out = render(&spec, surface());
    let curvy = out
        .scene
        .marks
        .iter()
        .filter(|m| matches!(m, Mark::Fill { layer: Layer::Series, .. }))
        .filter(|m| match m {
            Mark::Fill { path, .. } => path.segs.iter().any(|s| matches!(s, PathSeg::CubicTo(..))),
            _ => false,
        })
        .count();
    assert_eq!(curvy, 2, "both grouped bars round independently");
}

// ---------------------------------------------------------------------------
// Per-kind styling + emphasis
// ---------------------------------------------------------------------------

fn point_batches(out: &ChartOutput) -> Vec<Vec<PointInstance>> {
    out.scene
        .marks
        .iter()
        .filter_map(|m| match m {
            Mark::Points { instances, .. } => Some(instances.clone()),
            _ => None,
        })
        .collect()
}

fn scatter_spec(style: PointStyle) -> ChartSpec {
    ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::Scatter(style),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 2.0), datum(2.0, 3.0)],
    )])
}

/// Hovering a column enlarges exactly the points in it.
#[test]
fn hovered_column_enlarges_only_its_points() {
    let style = PointStyle::new(3.0).hover(9.0);
    let mut spec = scatter_spec(style);
    spec.highlight = Highlight::column(1.0);

    let out = render(&spec, surface());
    let batch = point_batches(&out).pop().expect("a scatter batch");
    let radii: Vec<f32> = batch.iter().map(|p| p.half.x).collect();
    assert_eq!(radii, vec![3.0, 9.0, 3.0], "only the hovered column grows");
}

/// A selected point uses the selected radius, and selection outranks hover.
#[test]
fn selection_outranks_hover() {
    let style = PointStyle::new(3.0).hover(9.0).selected(12.0);
    let mut spec = scatter_spec(style);
    spec.highlight = Highlight::column(1.0).with_points(vec![DatumRef { series: 0, index: 1 }]);

    let out = render(&spec, surface());
    let batch = point_batches(&out).pop().expect("a scatter batch");
    assert_eq!(batch[1].half.x, 12.0, "selected wins over hovered");
}

/// `dim_others` fades series that contain nothing emphasised, and leaves the
/// emphasised one at full opacity.
#[test]
fn dim_others_fades_untouched_series() {
    let mut spec = ChartSpec::new(vec![
        Series::new("a", SeriesKind::line(), BLUE, vec![datum(0.0, 1.0), datum(1.0, 2.0)]),
        Series::new("b", SeriesKind::line(), PINK, vec![datum(5.0, 1.0), datum(6.0, 2.0)]),
    ]);
    // Column 0.0 exists only in series "a".
    spec.highlight = Highlight::column(0.0).dim_others(true);

    let out = render(&spec, surface());
    let alphas: Vec<u8> = out
        .scene
        .marks
        .iter()
        .filter_map(|m| match m {
            Mark::Stroke { layer: Layer::Series, paint: Paint::Solid(c), .. } => Some(c.a),
            _ => None,
        })
        .collect();
    assert_eq!(alphas.len(), 2);
    assert_eq!(alphas[0], 255, "the emphasised series stays opaque");
    assert!(alphas[1] < 255, "the other series is dimmed, got {}", alphas[1]);
}

/// An emphasised series thickens its stroke.
#[test]
fn hover_thickens_the_line() {
    let mut spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::Line(LineStyle::new(2.0).hover_width(6.0)),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 2.0)],
    )]);
    let plain = render(&spec, surface());
    spec.highlight = Highlight::column(0.0);
    let hovered = render(&spec, surface());

    let width_of = |o: &ChartOutput| -> f32 {
        o.scene
            .marks
            .iter()
            .find_map(|m| match m {
                Mark::Stroke { layer: Layer::Series, stroke, .. } => Some(stroke.width),
                _ => None,
            })
            .expect("a series stroke")
    };
    assert_eq!(width_of(&plain), 2.0);
    assert_eq!(width_of(&hovered), 6.0);
}

/// Each `AreaFill` maps to the paint it names — and `None` emits no fill at
/// all rather than a transparent one.
#[test]
fn area_fill_modes_map_to_their_paints() {
    let fill_paint = |fill: AreaFill| -> Option<Paint> {
        let spec = ChartSpec::new(vec![Series::new(
            "s",
            SeriesKind::Area(AreaStyle::default().fill(fill)),
            BLUE,
            vec![datum(0.0, 1.0), datum(1.0, 2.0)],
        )]);
        render(&spec, surface()).scene.marks.iter().find_map(|m| match m {
            Mark::Fill { layer: Layer::AreaFill, paint, .. } => Some(paint.clone()),
            _ => None,
        })
    };

    assert!(fill_paint(AreaFill::None).is_none(), "None must emit no fill mark");
    match fill_paint(AreaFill::Flat { opacity: 0.5 }) {
        Some(Paint::Solid(c)) => assert_eq!(c.a, 128, "flat fill takes the named opacity"),
        other => panic!("expected a solid fill, got {other:?}"),
    }
    match fill_paint(AreaFill::Gradient { top_opacity: 1.0, bottom_opacity: 0.0 }) {
        Some(Paint::Linear { stops, .. }) => {
            assert_eq!(stops.len(), 2);
            assert_eq!(stops[0].color.a, 255);
            assert_eq!(stops[1].color.a, 0);
        }
        other => panic!("expected a gradient, got {other:?}"),
    }
    let custom = vec![
        GradientStop { offset: 0.0, color: PINK },
        GradientStop { offset: 0.5, color: BLUE },
        GradientStop { offset: 1.0, color: PINK.with_alpha(0) },
    ];
    match fill_paint(AreaFill::Stops(custom.clone())) {
        Some(Paint::Linear { stops, .. }) => assert_eq!(stops, custom),
        other => panic!("expected explicit stops, got {other:?}"),
    }
}

/// A ring is a second instanced batch UNDER the fills, not a per-point
/// stroke — so a ringed scatter stays two draw ops regardless of point count.
#[test]
fn ring_emits_one_extra_batch_beneath_the_fill() {
    let out = render(&scatter_spec(PointStyle::new(4.0).ring(PINK, 2.0)), surface());
    let batches = point_batches(&out);
    assert_eq!(batches.len(), 2, "ring batch + fill batch");
    // The ring is emitted first (drawn under) and is larger by its width.
    assert_eq!(batches[0][0].color, PINK);
    assert_eq!(batches[0][0].half.x, 6.0);
    assert_eq!(batches[1][0].half.x, 4.0);
}

/// Marker shape is carried by the instance's corner radius.
#[test]
fn point_shapes_map_to_corner_radii() {
    let radius_for = |shape: PointShape| -> f32 {
        let out = render(&scatter_spec(PointStyle::new(4.0).shape(shape)), surface());
        point_batches(&out)[0][0].radius
    };
    assert_eq!(radius_for(PointShape::Circle), 4.0, "radius == half-extent is a circle");
    assert_eq!(radius_for(PointShape::Square), 0.0);
    assert!(radius_for(PointShape::RoundedSquare) > 0.0);
    assert!(radius_for(PointShape::RoundedSquare) < 4.0);
}

/// A bar recolors only the emphasised datum.
#[test]
fn bar_hover_color_applies_to_the_hovered_bar_only() {
    let mut spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::Bar(BarStyle::new(4.0).hover_color(PINK)),
        BLUE,
        vec![datum(0.0, 3.0), datum(1.0, 5.0), datum(2.0, 4.0)],
    )])
    .x(Axis::category(["a", "b", "c"]))
    .y(Axis::linear().include_zero(true));
    spec.highlight = Highlight::column(1.0);

    let out = render(&spec, surface());
    let colors: Vec<Color> = out
        .scene
        .marks
        .iter()
        .filter_map(|m| match m {
            Mark::Fill { layer: Layer::Series, paint: Paint::Solid(c), .. } => Some(*c),
            _ => None,
        })
        .collect();
    assert_eq!(colors, vec![BLUE, PINK, BLUE]);
}

/// A per-series `width_fraction` overrides the chart-wide bar padding.
#[test]
fn bar_width_fraction_overrides_group_padding() {
    let bar_width = |style: BarStyle| -> f32 {
        let spec = ChartSpec::new(vec![Series::new(
            "s",
            SeriesKind::Bar(style),
            BLUE,
            vec![datum(0.0, 3.0)],
        )])
        .x(Axis::category(["only"]))
        .y(Axis::linear().include_zero(true));
        let out = render(&spec, surface());
        out.scene
            .marks
            .iter()
            .find_map(|m| match m {
                Mark::Fill { layer: Layer::Series, path, .. } => {
                    let xs: Vec<f32> = path
                        .segs
                        .iter()
                        .filter_map(|s| match s {
                            PathSeg::MoveTo(a) | PathSeg::LineTo(a) => Some(a.x),
                            PathSeg::CubicTo(_, _, a) => Some(a.x),
                            _ => None,
                        })
                        .collect();
                    let lo = xs.iter().cloned().fold(f32::INFINITY, f32::min);
                    let hi = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                    Some(hi - lo)
                }
                _ => None,
            })
            .expect("a bar")
    };

    let default_w = bar_width(BarStyle::default());
    let narrow = bar_width(BarStyle::default().width_fraction(0.25));
    assert!(narrow < default_w, "0.25 must be narrower than the 0.8 default");
    assert!((narrow / default_w - 0.25 / 0.8).abs() < 0.05);
}

/// With nothing emphasised, `dim_others` changes nothing — an idle chart
/// must not render faded.
#[test]
fn empty_highlight_dims_nothing() {
    let mut spec = scatter_spec(PointStyle::new(3.0));
    spec.highlight = Highlight::default().dim_others(true);
    let out = render(&spec, surface());
    for p in &point_batches(&out)[0] {
        assert_eq!(p.color.a, 255, "an idle chart must not be dimmed");
        assert_eq!(p.half.x, 3.0, "and must not be enlarged");
    }
}

// ---------------------------------------------------------------------------
// Highlight band + per-mark style callback
// ---------------------------------------------------------------------------

fn band_rect(out: &ChartOutput) -> Option<(f32, f32)> {
    out.scene.marks.iter().find_map(|m| match m {
        Mark::Fill { layer: Layer::Background, path, .. } => {
            let xs: Vec<f32> = path
                .segs
                .iter()
                .filter_map(|s| match s {
                    PathSeg::MoveTo(a) | PathSeg::LineTo(a) => Some(a.x),
                    _ => None,
                })
                .collect();
            let lo = xs.iter().cloned().fold(f32::INFINITY, f32::min);
            let hi = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            Some((lo, hi - lo))
        }
        _ => None,
    })
}

fn bar_band_spec() -> ChartSpec {
    ChartSpec::new(vec![
        Series::new("a", SeriesKind::bar(), BLUE, vec![datum(0.0, 3.0), datum(1.0, 5.0), datum(2.0, 4.0)]),
        Series::new("b", SeriesKind::bar(), PINK, vec![datum(0.0, 2.0), datum(1.0, 6.0), datum(2.0, 1.0)]),
    ])
    .x(Axis::category(["a", "b", "c"]))
    .y(Axis::linear().include_zero(true))
    .highlight_band(Color::rgba(0, 0, 0, 20))
}

/// The band covers the whole CATEGORY SLOT, not just the bar.
///
/// That is the entire point of it: with grouped bars the pointer is often in
/// the gap between two bars, and emphasising bars alone leaves the column
/// looking inactive there.
#[test]
fn highlight_band_spans_the_whole_category_slot() {
    let mut spec = bar_band_spec();
    spec.highlight = Highlight::column(1.0);

    let out = render(&spec, surface());
    let (x0, w) = band_rect(&out).expect("a background band");
    let slot = out.scene.plot.w / 3.0;
    assert!((w - slot).abs() < 0.5, "band width {w} should equal the slot {slot}");

    // And it is centred on the hovered category.
    let center = out.x.map(1.0, out.scene.plot.x, out.scene.plot.right());
    assert!((x0 + w / 2.0 - center).abs() < 0.5);
}

/// The band is painted BEHIND the data — a translucent rect over the bars
/// would wash them out.
#[test]
fn highlight_band_paints_behind_the_series() {
    let mut spec = bar_band_spec();
    spec.highlight = Highlight::column(1.0);
    let out = render(&spec, surface());

    let band_at = out.scene.marks.iter().position(|m| m.layer() == Layer::Background);
    let first_series = out.scene.marks.iter().position(|m| m.layer() == Layer::Series);
    assert!(band_at.is_some() && first_series.is_some());
    assert!(band_at < first_series, "the band must sort before the series");
}

#[test]
fn no_band_without_a_highlighted_column() {
    let out = render(&bar_band_spec(), surface());
    assert!(band_rect(&out).is_none(), "an idle chart draws no band");
}

/// A style callback recolors individual bars — the conditional-formatting
/// case (negative values in red, thresholds, per-category branding).
#[test]
fn style_callback_recolors_individual_bars() {
    let red = Color::rgb(0xd0, 0x30, 0x30);
    let f: StyleFn = std::rc::Rc::new(move |ctx: &MarkContext| {
        if ctx.datum.y < 0.0 {
            MarkOverride::color(red)
        } else {
            MarkOverride::default()
        }
    });
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::bar(),
        BLUE,
        vec![datum(0.0, 3.0), datum(1.0, -2.0), datum(2.0, 4.0)],
    )
    .styled(f)])
    .x(Axis::category(["a", "b", "c"]));

    let out = render(&spec, surface());
    let colors: Vec<Color> = out
        .scene
        .marks
        .iter()
        .filter_map(|m| match m {
            Mark::Fill { layer: Layer::Series, paint: Paint::Solid(c), .. } => Some(*c),
            _ => None,
        })
        .collect();
    assert_eq!(colors, vec![BLUE, red, BLUE], "only the negative bar recolors");
}

/// The callback sees the resolved emphasis, so it can build on hover state
/// instead of reimplementing it.
#[test]
fn style_callback_receives_the_resolved_emphasis() {
    let gold = Color::rgb(0xff, 0xc1, 0x07);
    let f: StyleFn = std::rc::Rc::new(move |ctx: &MarkContext| match ctx.emphasis {
        Emphasis::None => MarkOverride::default(),
        _ => MarkOverride::color(gold).with_radius(14.0),
    });
    let mut spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::Scatter(PointStyle::new(3.0)),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 2.0)],
    )
    .styled(f)]);
    spec.highlight = Highlight::column(1.0);

    let out = render(&spec, surface());
    let batch = point_batches(&out).pop().expect("a scatter batch");
    assert_eq!(batch[0].color, BLUE);
    assert_eq!(batch[1].color, gold, "the hovered point takes the override");
    assert_eq!(batch[1].half.x, 14.0, "and its radius");
}

/// Two specs sharing one `Rc` callback compare equal; a fresh closure does
/// not. This is the memoisation contract — a host that rebuilds its callback
/// every tick would re-render every tick.
#[test]
fn style_callback_compares_by_identity() {
    let make = |f: Option<StyleFn>| {
        let mut s = Series::new("s", SeriesKind::bar(), BLUE, vec![datum(0.0, 1.0)]);
        s.style_fn = f;
        ChartSpec::new(vec![s])
    };
    let shared: StyleFn = std::rc::Rc::new(|_: &MarkContext| MarkOverride::default());

    assert_eq!(make(Some(shared.clone())), make(Some(shared.clone())), "same Rc is equal");
    assert_eq!(make(None), make(None));

    let other: StyleFn = std::rc::Rc::new(|_: &MarkContext| MarkOverride::default());
    assert_ne!(make(Some(shared)), make(Some(other)), "a different Rc is not equal");
}

/// `opacity` on an override multiplies the resolved alpha.
#[test]
fn style_callback_opacity_multiplies_alpha() {
    let f: StyleFn = std::rc::Rc::new(|ctx: &MarkContext| {
        if ctx.index == 1 {
            MarkOverride::opacity(0.5)
        } else {
            MarkOverride::default()
        }
    });
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::Scatter(PointStyle::new(3.0)),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 2.0)],
    )
    .styled(f)]);

    let out = render(&spec, surface());
    let batch = point_batches(&out).pop().expect("a batch");
    assert_eq!(batch[0].color.a, 255);
    assert_eq!(batch[1].color.a, 128);
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

fn tween_spec(values: [f64; 3]) -> ChartSpec {
    ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::bar(),
        BLUE,
        values.iter().enumerate().map(|(i, v)| datum(i as f64, *v)).collect(),
    )])
    .x(Axis::category(["a", "b", "c"]))
    .y(Axis::linear().include_zero(true))
}

/// A transition must LAND exactly on the plain destination render.
///
/// Otherwise the chart visible at rest depends on whether an animation
/// happened to run — the worst kind of inconsistency, because it only shows
/// up some of the time.
#[test]
fn transition_lands_exactly_on_the_destination_render() {
    let (a, b) = (tween_spec([1.0, 2.0, 3.0]), tween_spec([9.0, 4.0, 6.0]));
    let at1 = render_tween(&a, &b, TweenAt::uniform(1.0), surface(), &Gutters::None);
    assert_eq!(at1.scene.marks, render(&b, surface()).scene.marks);
    assert_eq!(at1.scene.labels, render(&b, surface()).scene.labels);
}

/// At t=0 the DATA is still the source's, even though the labels have
/// already switched to the destination's.
///
/// The split is deliberate: values glide, labels do not churn. Pinning it
/// here so a future change to tick handling cannot silently start animating
/// the marks from the wrong place.
#[test]
fn transition_starts_from_the_source_data() {
    let (a, b) = (tween_spec([1.0, 2.0, 3.0]), tween_spec([9.0, 4.0, 6.0]));
    let series_marks = |o: &ChartOutput| -> Vec<Mark> {
        o.scene.marks.iter().filter(|m| m.layer() == Layer::Series).cloned().collect()
    };
    // Same domain in both specs would make this vacuous; assert it is not.
    let at0 = render_tween(&a, &b, TweenAt::uniform(0.0), surface(), &Gutters::None);
    assert_eq!(
        series_marks(&at0),
        series_marks(&render(&a, surface())),
        "the marks at t=0 are the source's"
    );
}

/// Mid-transition, values sit between the endpoints.
#[test]
fn transition_midpoint_lies_between_the_endpoints() {
    let (a, b) = (tween_spec([0.0, 0.0, 0.0]), tween_spec([10.0, 10.0, 10.0]));
    let mid = charts_core::lerp_data(&a, &b, TweenAt::uniform(0.5)).expect("same shape");
    for d in &mid.series[0].data {
        assert!(d.y > 0.0 && d.y < 10.0, "expected an intermediate value, got {}", d.y);
    }
}

/// The DOMAIN tweens too. Without it the axis jumps to its final range on
/// the first frame while the marks glide into it, which reads as a glitch.
#[test]
fn transition_interpolates_the_axis_domain() {
    let (a, b) = (tween_spec([1.0, 1.0, 1.0]), tween_spec([100.0, 100.0, 100.0]));
    let at0 = render_tween(&a, &b, TweenAt::uniform(0.0), surface(), &Gutters::None);
    let mid = render_tween(&a, &b, TweenAt::uniform(0.5), surface(), &Gutters::None);
    let at1 = render_tween(&a, &b, TweenAt::uniform(1.0), surface(), &Gutters::None);

    assert!(mid.y.max > at0.y.max, "domain must grow from the source");
    assert!(mid.y.max < at1.y.max, "and not reach the destination early");
}

/// Tick LABELS come from the destination for the whole transition, so they
/// do not churn through arbitrary intermediate values while the gridlines
/// move.
#[test]
fn transition_keeps_the_destination_tick_labels() {
    let (a, b) = (tween_spec([1.0, 1.0, 1.0]), tween_spec([100.0, 100.0, 100.0]));
    let target = render(&b, surface());
    let want: Vec<String> = target.y.ticks.iter().map(|t| t.label.clone()).collect();

    for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let f = render_tween(&a, &b, TweenAt::uniform(t), surface(), &Gutters::None);
        let got: Vec<String> = f.y.ticks.iter().map(|t| t.label.clone()).collect();
        assert_eq!(got, want, "labels must stay put at t={t}");
    }
}

/// Specs that cannot be paired point-for-point snap to the destination
/// rather than pairing unrelated points.
#[test]
fn transition_snaps_when_the_shape_changes() {
    let a = tween_spec([1.0, 2.0, 3.0]);
    let b = ChartSpec::new(vec![
        Series::new("s", SeriesKind::bar(), BLUE, vec![datum(0.0, 1.0)]),
        Series::new("t", SeriesKind::bar(), PINK, vec![datum(0.0, 2.0)]),
    ])
    .x(Axis::category(["a"]));

    assert!(!charts_core::same_shape(&a, &b));
    assert!(charts_core::lerp_data(&a, &b, TweenAt::uniform(0.5)).is_none());

    let mid = render_tween(&a, &b, TweenAt::uniform(0.5), surface(), &Gutters::None);
    let plain = render(&b, surface());
    assert_eq!(mid.scene.marks, plain.scene.marks, "a shape change snaps to the destination");
}

/// Values and colors interpolate on their OWN clocks; everything else takes
/// effect at once.
///
/// The color half used to assert the opposite — that a recolor switched
/// immediately. That was changed deliberately when colors got their own
/// `Transition`: a bar gliding to a new height while its fill jumped read as
/// a glitch. What must still be instant is HIGHLIGHT: a point becoming
/// selected is a state change the user just caused, and easing into it makes
/// the UI feel unresponsive rather than smooth.
#[test]
fn values_and_colors_interpolate_on_separate_clocks() {
    let a = tween_spec([1.0, 2.0, 3.0]);
    let mut b = tween_spec([1.0, 2.0, 3.0]);
    b.series[0].color = PINK;

    // Colour held at the `from` end while the value clock is fully run.
    let held = charts_core::lerp_data(&a, &b, TweenAt { value: 1.0, color: 0.0 })
        .expect("same shape");
    assert_eq!(held.series[0].color, a.series[0].color, "the color clock governs color");

    let mid = charts_core::lerp_data(&a, &b, TweenAt::uniform(0.5)).expect("same shape");
    assert_ne!(mid.series[0].color, PINK, "mid-transition is not the destination");
    assert_ne!(mid.series[0].color, a.series[0].color, "…nor the origin");

    // Settled is exact — no rounding residue on the color it lands on.
    let end = charts_core::lerp_data(&a, &b, TweenAt::SETTLED).expect("same shape");
    assert_eq!(end.series[0].color, PINK);
}

/// Highlight is NOT interpolated: it comes from `to` whole.
///
/// Guards the boundary the change above could erode — once colors animate it
/// is tempting to animate emphasis too, and a selection that fades in feels
/// laggy rather than smooth.
#[test]
fn a_transition_does_not_interpolate_highlight() {
    let a = tween_spec([1.0, 2.0, 3.0]);
    let mut b = tween_spec([1.0, 2.0, 3.0]);
    b.highlight.points = vec![charts_core::DatumRef { series: 0, index: 1 }];

    let mid = charts_core::lerp_data(&a, &b, TweenAt::uniform(0.5)).expect("same shape");
    assert_eq!(mid.highlight, b.highlight, "selection lands at once, mid-transition or not");
}

/// Easing is smooth and pinned at both ends, so a transition starts and
/// finishes at rest.
#[test]
fn easing_is_pinned_at_both_ends() {
    assert_eq!(charts_core::ease_in_out(0.0), 0.0);
    assert_eq!(charts_core::ease_in_out(1.0), 1.0);
    assert!((charts_core::ease_in_out(0.5) - 0.5).abs() < 1e-6, "symmetric at the midpoint");
    // Monotonic.
    let mut prev = -1.0;
    for i in 0..=20 {
        let v = charts_core::ease_in_out(i as f32 / 20.0);
        assert!(v >= prev, "easing must not go backwards");
        prev = v;
    }
    // And clamped outside 0..1 rather than overshooting.
    assert_eq!(charts_core::ease_in_out(-1.0), 0.0);
    assert_eq!(charts_core::ease_in_out(2.0), 1.0);
}
