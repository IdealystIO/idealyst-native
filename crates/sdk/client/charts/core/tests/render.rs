//! Golden + invariant tests for the render pipeline.
//!
//! The goldens are SVG rather than pixels: they diff readably, they are
//! stable across machines (the approximate metrics are deterministic and
//! font-independent by construction), and a reviewer can open one. Refresh
//! with `UPDATE_GOLDENS=1 cargo test -p charts-core`.

use charts_core::render::Gutters;
use charts_core::svg::{to_svg, ApproxMetrics};
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
    spec.series[0].kind = SeriesKind::Line { width: 2.5, smooth: true, dash: vec![], points: true };
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
