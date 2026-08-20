//! Cartesian additions: interpolation, annotations, sparkline mode, the
//! shape-aware hit index, and heatmaps.

use charts_core::render::Gutters;
use charts_core::scene::PathSeg;
use charts_core::svg::{to_svg, ApproxMetrics};
use charts_core::*;

const BLUE: Color = Color::rgb(0x4c, 0x8d, 0xff);
const PINK: Color = Color::rgb(0xff, 0x6b, 0x9d);
const RED: Color = Color::rgb(0xe5, 0x48, 0x4a);
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

fn steps() -> Vec<Datum> {
    vec![datum(0.0, 2.0), datum(1.0, 5.0), datum(2.0, 3.0)]
}

/// Every `LineTo` in a path, as (x, y) pairs.
fn line_points(p: &Path) -> Vec<(f32, f32)> {
    p.segs
        .iter()
        .filter_map(|s| match s {
            PathSeg::LineTo(a) => Some((a.x, a.y)),
            _ => None,
        })
        .collect()
}

fn first_stroke_path(out: &ChartOutput, layer: Layer) -> Path {
    out.scene
        .marks
        .iter()
        .find_map(|m| match m {
            Mark::Stroke { layer: l, path, .. } if *l == layer => Some(path.clone()),
            _ => None,
        })
        .expect("no stroke on that layer")
}

// ---------------------------------------------------------------------------
// Interpolation
// ---------------------------------------------------------------------------

#[test]
fn step_after_holds_each_value_until_the_next_sample() {
    let spec = ChartSpec::new(vec![Series::new(
        "state",
        SeriesKind::step_line(StepAt::After),
        BLUE,
        steps(),
    )]);
    let out = render(&spec, surface());
    let pts = line_points(&first_stroke_path(&out, Layer::Series));

    // Two segments per interval, and every one of them axis-aligned: that IS
    // the property a stepped line promises, and a diagonal anywhere in the
    // path would mean the chart asserts a transition that never happened.
    assert_eq!(pts.len(), 4, "two segments per interval");
    let start_y = out.y.map(2.0, out.hit.plot().bottom(), out.hit.plot().y);
    // Horizontal first: the value is held to the NEXT x before it jumps.
    assert!((pts[0].1 - start_y).abs() < 0.01, "first move is horizontal");
    assert!((pts[1].0 - pts[0].0).abs() < 0.01, "then vertical at the same x");
}

#[test]
fn step_before_changes_at_the_leading_sample() {
    let spec = ChartSpec::new(vec![Series::new(
        "state",
        SeriesKind::step_line(StepAt::Before),
        BLUE,
        steps(),
    )]);
    let out = render(&spec, surface());
    let pts = line_points(&first_stroke_path(&out, Layer::Series));
    let plot = out.hit.plot();
    let x0 = out.x.map(0.0, plot.x, plot.right());
    // Vertical first: the jump happens at THIS sample's x.
    assert!((pts[0].0 - x0).abs() < 0.01, "first move is vertical at x0");
    assert!((pts[1].1 - pts[0].1).abs() < 0.01, "then horizontal at the new y");
}

#[test]
fn step_mid_changes_halfway_between_samples() {
    let spec = ChartSpec::new(vec![Series::new(
        "state",
        SeriesKind::step_line(StepAt::Mid),
        BLUE,
        steps(),
    )]);
    let out = render(&spec, surface());
    let pts = line_points(&first_stroke_path(&out, Layer::Series));
    let plot = out.hit.plot();
    let (x0, x1) = (
        out.x.map(0.0, plot.x, plot.right()),
        out.x.map(1.0, plot.x, plot.right()),
    );
    assert_eq!(pts.len(), 6, "three segments per interval");
    assert!(
        (pts[0].0 - (x0 + x1) / 2.0).abs() < 0.01,
        "the riser sits at the midpoint"
    );
}

#[test]
fn a_stepped_line_never_curves() {
    let spec = ChartSpec::new(vec![Series::new(
        "state",
        SeriesKind::step_line(StepAt::After),
        BLUE,
        steps(),
    )]);
    let out = render(&spec, surface());
    let path = first_stroke_path(&out, Layer::Series);
    assert!(
        !path
            .segs
            .iter()
            .any(|s| matches!(s, PathSeg::CubicTo(..) | PathSeg::QuadTo(..))),
        "stepping must not emit curves"
    );
}

#[test]
fn golden_stepped_area() {
    let spec = ChartSpec::new(vec![Series::new(
        "sessions",
        SeriesKind::Area(AreaStyle::new(
            LineStyle::new(2.0).stepped(StepAt::After),
            AreaFill::Gradient { top_opacity: 0.4, bottom_opacity: 0.0 },
        )),
        BLUE,
        (0..7)
            .map(|i| datum(i as f64, [4.0, 4.0, 9.0, 9.0, 6.0, 11.0, 11.0][i]))
            .collect(),
    )])
    .y(Axis::linear().include_zero(true));
    render_golden("stepped_area", &spec);
}

// ---------------------------------------------------------------------------
// Annotations
// ---------------------------------------------------------------------------

fn annotated(a: Annotation) -> ChartSpec {
    ChartSpec::new(vec![Series::new(
        "load",
        SeriesKind::line(),
        BLUE,
        vec![datum(0.0, 2.0), datum(1.0, 6.0), datum(2.0, 4.0)],
    )])
    .annotate(a)
}

#[test]
fn a_reference_line_lands_exactly_on_its_value() {
    let spec = annotated(Annotation::y_line(5.0, RED));
    let out = render(&spec, surface());
    let plot = out.hit.plot();
    let want = out.y.map(5.0, plot.bottom(), plot.y);
    let path = first_stroke_path(&out, Layer::Axis);
    let ys: Vec<f32> = path
        .segs
        .iter()
        .filter_map(|s| match s {
            PathSeg::MoveTo(a) | PathSeg::LineTo(a) => Some(a.y),
            _ => None,
        })
        .collect();
    assert!(ys.iter().all(|y| (y - want).abs() < 0.01), "rule is at y=5");
}

/// The bug this guards: a target far outside the data would, if it fed the
/// domain, flatten every real value into the floor of the chart.
#[test]
fn regression_annotation_does_not_widen_the_domain() {
    let bare = render(&annotated(Annotation::y_line(5.0, RED)), surface());
    let far = render(&annotated(Annotation::y_line(5000.0, RED)), surface());
    assert_eq!(
        (bare.y.min, bare.y.max),
        (far.y.min, far.y.max),
        "an annotation must not move the axis"
    );
}

#[test]
fn an_annotation_outside_the_window_draws_nothing() {
    let out = render(&annotated(Annotation::y_line(5000.0, RED)), surface());
    assert!(
        !out.scene.marks.iter().any(|m| m.layer() == Layer::Axis),
        "an off-window rule emits no mark"
    );
}

#[test]
fn a_band_is_clipped_to_the_plot() {
    // Half in, half out: the visible part must be drawn, truncated at the
    // plot edge rather than overflowing it or vanishing.
    let spec = annotated(Annotation::y_band(3.0, 900.0, RED.with_alpha(40)));
    let out = render(&spec, surface());
    let plot = out.hit.plot();
    let rect = out
        .scene
        .marks
        .iter()
        .find_map(|m| match m {
            Mark::Fill { layer: Layer::Background, path, .. } => Some(path.clone()),
            _ => None,
        })
        .expect("band");
    let ys: Vec<f32> = rect
        .segs
        .iter()
        .filter_map(|s| match s {
            PathSeg::MoveTo(a) | PathSeg::LineTo(a) => Some(a.y),
            _ => None,
        })
        .collect();
    let top = ys.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(top >= plot.y - 0.01, "band is clipped at the plot top");
}

#[test]
fn a_rule_paints_above_the_series_and_a_band_below() {
    let spec = ChartSpec::new(vec![Series::new(
        "load",
        SeriesKind::bar(),
        BLUE,
        vec![datum(0.0, 2.0), datum(1.0, 6.0)],
    )])
    .annotate(Annotation::y_line(5.0, RED))
    .annotate(Annotation::y_band(1.0, 3.0, PINK.with_alpha(40)));
    let out = render(&spec, surface());
    let layers: Vec<Layer> = out.scene.marks.iter().map(|m| m.layer()).collect();
    let band = layers.iter().position(|l| *l == Layer::Background).expect("band");
    let series = layers.iter().position(|l| *l == Layer::Series).expect("bars");
    let rule = layers.iter().position(|l| *l == Layer::Axis).expect("rule");
    assert!(band < series && series < rule, "band, then bars, then the rule");
}

#[test]
fn an_annotation_label_carries_its_own_role_and_color() {
    let spec = annotated(Annotation::y_line(5.0, RED).label("SLO"));
    let out = render(&spec, surface());
    let l = out
        .scene
        .labels
        .iter()
        .find(|l| l.role == LabelRole::Annotation)
        .expect("annotation label");
    assert_eq!(l.text, "SLO");
    assert_eq!(l.color, Some(RED));
}

#[test]
fn golden_annotated_line() {
    let spec = ChartSpec::new(vec![Series::new(
        "latency",
        SeriesKind::smooth_line(),
        BLUE,
        (0..8)
            .map(|i| datum(i as f64, [120.0, 180.0, 150.0, 260.0, 210.0, 340.0, 290.0, 200.0][i]))
            .collect(),
    )])
    .y(Axis::linear().include_zero(true).title("ms"))
    .annotate(Annotation::y_band(0.0, 200.0, Color::rgba(0x4c, 0xaf, 0x50, 30)).label("healthy"))
    .annotate(Annotation::y_line(300.0, RED).dashed([6.0, 4.0]).label("SLO"))
    .annotate(Annotation::x_line(5.0, PINK).dashed([3.0, 3.0]).label("deploy"));
    render_golden("annotated_line", &spec);
}

// ---------------------------------------------------------------------------
// Sparkline mode
// ---------------------------------------------------------------------------

#[test]
fn a_sparkline_emits_no_furniture() {
    let spec = ChartSpec::new(vec![Series::new(
        "trend",
        SeriesKind::smooth_line(),
        BLUE,
        (0..12).map(|i| datum(i as f64, (i as f64 * 0.7).sin() * 5.0 + 6.0)).collect(),
    )])
    .legend(true)
    .sparkline();
    let out = render_with(&spec, surface(), &Gutters::Measured(&ApproxMetrics));
    assert!(out.scene.labels.is_empty(), "no labels at all");
    assert!(
        !out.scene.marks.iter().any(|m| m.layer() == Layer::Grid),
        "no gridlines"
    );
    assert!(
        out.scene.marks.iter().any(|m| m.layer() == Layer::Series),
        "but the data is still drawn"
    );
}

/// The point of the mode: the space the axes were holding has to come back,
/// or "no furniture" still costs what the furniture cost.
#[test]
fn suppressed_labels_give_their_gutter_back() {
    let base = ChartSpec::new(vec![Series::new(
        "trend",
        SeriesKind::line(),
        BLUE,
        vec![datum(0.0, 1000.0), datum(1.0, 250_000.0)],
    )]);
    let with_axes = render_with(&base, surface(), &Gutters::Measured(&ApproxMetrics));
    let bare = render_with(
        &base.clone().sparkline(),
        surface(),
        &Gutters::Measured(&ApproxMetrics),
    );
    assert!(
        bare.hit.plot().w > with_axes.hit.plot().w + 20.0,
        "the sparkline reclaims the y gutter ({} vs {})",
        bare.hit.plot().w,
        with_axes.hit.plot().w
    );
    assert!(bare.hit.plot().h > with_axes.hit.plot().h + 10.0, "and the x gutter");
}

#[test]
fn grid_and_labels_toggle_independently() {
    let spec = ChartSpec::new(vec![Series::new(
        "trend",
        SeriesKind::line(),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 4.0)],
    )])
    .y(Axis::linear().labels(false))
    .x(Axis::linear().grid(false));
    let out = render(&spec, surface());
    assert!(
        !out.scene.labels.iter().any(|l| l.role == LabelRole::AxisY),
        "y labels are off"
    );
    assert!(
        out.scene.marks.iter().any(|m| m.layer() == Layer::Grid),
        "y gridlines are still on"
    );
}

// ---------------------------------------------------------------------------
// Shape-aware hit index
// ---------------------------------------------------------------------------

fn bar_chart() -> ChartSpec {
    ChartSpec::new(vec![Series::new(
        "units",
        SeriesKind::bar(),
        BLUE,
        vec![datum(0.0, 10.0), datum(1.0, 4.0)],
    )])
    .x(Axis::category(["a", "b"]))
    .y(Axis::linear().include_zero(true))
}

/// The bug: bars were indexed by a single point at their top-centre, so a
/// pointer anywhere near the base of a tall bar resolved to nothing — the
/// mark was plainly under the cursor and the index disagreed.
#[test]
fn regression_a_bar_is_hit_over_its_whole_body() {
    let out = render(&bar_chart(), surface());
    let plot = out.hit.plot();
    let x = out.x.map(0.0, plot.x, plot.right());
    // Just above the baseline of the tall bar — far from its top.
    let near_base = pt(x, plot.bottom() - 4.0);

    let hit = out.hit.contains(near_base).expect("the bar covers this point");
    assert_eq!(hit.index, 0);
    assert_eq!(hit.distance, 0.0, "inside the mark means zero distance");

    // And this is why the old index failed: the point it stored — the bar's
    // top — is most of the plot away from a pointer sitting on the bar's
    // base, so no proximity radius small enough to be useful could match it.
    let to_anchor = (hit.position.y - near_base.y).abs();
    assert!(
        to_anchor > plot.h * 0.5,
        "the anchor is {to_anchor}px away; a point-only index cannot match here"
    );
}

#[test]
fn a_point_outside_every_bar_is_not_contained() {
    let out = render(&bar_chart(), surface());
    let plot = out.hit.plot();
    // Above the shorter bar's top.
    let x = out.x.map(1.0, plot.x, plot.right());
    assert!(out.hit.contains(pt(x, plot.y + 2.0)).is_none());
}

#[test]
fn a_bar_still_anchors_its_tooltip_at_its_outer_end() {
    let out = render(&bar_chart(), surface());
    let plot = out.hit.plot();
    let x = out.x.map(0.0, plot.x, plot.right());
    let hit = out.hit.contains(pt(x, plot.bottom() - 4.0)).expect("hit");
    let top = out.y.map(10.0, plot.bottom(), plot.y);
    assert!(
        (hit.position.y - top).abs() < 0.01,
        "anchored at the bar's top, not where the pointer is"
    );
}

/// Markers have no area, so containment can never match them — proximity is
/// the only query that makes sense for a 3px dot.
#[test]
fn point_markers_are_matched_by_proximity_not_containment() {
    let spec = ChartSpec::new(vec![Series::new(
        "p",
        SeriesKind::scatter(),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 3.0)],
    )]);
    let out = render(&spec, surface());
    let plot = out.hit.plot();
    let at = pt(
        out.x.map(0.0, plot.x, plot.right()),
        out.y.map(1.0, plot.bottom(), plot.y),
    );
    assert!(out.hit.contains(at).is_none(), "no containment for markers");
    assert!(out.hit.pick(at, 8.0).is_some(), "but `pick` finds it");
}

#[test]
fn pick_prefers_a_covering_mark_over_a_nearer_marker() {
    let spec = ChartSpec::new(vec![
        Series::new("bars", SeriesKind::bar(), BLUE, vec![datum(0.0, 10.0)]),
        Series::new("dots", SeriesKind::scatter(), PINK, vec![datum(0.0, 10.0)]),
    ])
    .y(Axis::linear().include_zero(true));
    let out = render(&spec, surface());
    let plot = out.hit.plot();
    let inside_bar = pt(out.x.map(0.0, plot.x, plot.right()), plot.bottom() - 6.0);
    let hit = out.hit.pick(inside_bar, 40.0).expect("hit");
    assert_eq!(hit.series, 0, "containment wins over a marker within radius");
}

// ---------------------------------------------------------------------------
// Heatmap
// ---------------------------------------------------------------------------

fn heat() -> ChartSpec {
    let ramp = ColorRamp::two(Color::rgb(0x0d, 0x1b, 0x3d), Color::rgb(0xff, 0xd1, 0x66));
    let rows = ["mon", "tue", "wed"];
    ChartSpec::new(
        rows.iter()
            .enumerate()
            .map(|(r, name)| {
                Series::new(
                    *name,
                    SeriesKind::heatmap(ramp.clone()),
                    BLUE,
                    (0..4)
                        .map(|c| cell(c as f64, r as f64, (r * 4 + c) as f64))
                        .collect(),
                )
            })
            .collect(),
    )
    .x(Axis::category(["00", "06", "12", "18"]))
    .y(Axis::category(rows))
}

#[test]
fn a_heatmap_emits_one_cell_per_datum() {
    let out = render(&heat(), surface());
    let cells = out
        .scene
        .marks
        .iter()
        .filter(|m| matches!(m, Mark::Fill { layer: Layer::Series, .. }))
        .count();
    assert_eq!(cells, 12);
}

#[test]
fn heatmap_cells_tile_their_slots() {
    let mut spec = heat();
    for s in &mut spec.series {
        if let SeriesKind::Heatmap(h) = &mut s.kind {
            h.gap = 0.0;
            h.radius = 0.0;
        }
    }
    let out = render(&spec, surface());
    let plot = out.hit.plot();
    let widths: Vec<f32> = out
        .scene
        .marks
        .iter()
        .filter_map(|m| match m {
            Mark::Fill { layer: Layer::Series, path, .. } => {
                let xs: Vec<f32> = path
                    .segs
                    .iter()
                    .filter_map(|s| match s {
                        PathSeg::MoveTo(a) | PathSeg::LineTo(a) => Some(a.x),
                        _ => None,
                    })
                    .collect();
                let lo = xs.iter().copied().fold(f32::INFINITY, f32::min);
                let hi = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                Some(hi - lo)
            }
            _ => None,
        })
        .collect();
    let want = plot.w / 4.0;
    assert!(
        widths.iter().all(|w| (w - want).abs() < 0.01),
        "every cell is exactly one slot wide ({widths:?} vs {want})"
    );
}

#[test]
fn the_ramp_ends_land_on_the_extreme_values() {
    let ramp = ColorRamp::two(Color::rgb(0, 0, 0), Color::rgb(255, 255, 255));
    assert_eq!(ramp.sample(0.0), Color::rgb(0, 0, 0));
    assert_eq!(ramp.sample(1.0), Color::rgb(255, 255, 255));
    assert_eq!(ramp.sample(0.5), Color::rgb(128, 128, 128));
    // Clamped, not extrapolated: a value past the domain is still on-scale.
    assert_eq!(ramp.sample(4.0), Color::rgb(255, 255, 255));
}

#[test]
fn an_explicit_intensity_domain_beats_the_data_extent() {
    let ramp = ColorRamp::two(Color::rgb(0, 0, 0), Color::rgb(255, 255, 255));
    let mk = |style: HeatmapStyle| {
        ChartSpec::new(vec![Series::new(
            "row",
            SeriesKind::Heatmap(style),
            BLUE,
            vec![cell(0.0, 0.0, 0.0), cell(1.0, 0.0, 10.0)],
        )])
        .x(Axis::category(["a", "b"]))
        .y(Axis::category(["row"]))
    };
    let color_of = |spec: &ChartSpec, nth: usize| {
        let out = render(spec, surface());
        out.scene
            .marks
            .iter()
            .filter_map(|m| match m {
                Mark::Fill { layer: Layer::Series, paint: Paint::Solid(c), .. } => Some(*c),
                _ => None,
            })
            .nth(nth)
            .expect("cell")
    };
    // Auto: 10 is the top of the ramp.
    assert_eq!(color_of(&mk(HeatmapStyle::new(ramp.clone())), 1), Color::rgb(255, 255, 255));
    // Pinned to 0..20: 10 is halfway.
    let pinned = HeatmapStyle::new(ramp).domain(0.0, 20.0);
    assert_eq!(color_of(&mk(pinned), 1), Color::rgb(128, 128, 128));
}

/// A heatmap row has a ramp, not a color, so a one-swatch legend entry for
/// it would be a lie about how to read the chart.
#[test]
fn a_heatmap_row_is_not_a_legend_entry() {
    let mut spec = heat();
    spec.legend = true;
    spec.series.push(Series::new(
        "target",
        SeriesKind::line(),
        PINK,
        vec![datum(0.0, 0.0), datum(3.0, 2.0)],
    ));
    let out = render(&spec, surface());
    let names: Vec<&str> = out
        .scene
        .labels
        .iter()
        .filter(|l| l.role == LabelRole::Legend)
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(names, vec!["target"]);
}

#[test]
fn a_heatmap_cell_is_hit_over_its_area() {
    let out = render(&heat(), surface());
    let plot = out.hit.plot();
    let at = pt(
        out.x.map(2.0, plot.x, plot.right()),
        out.y.map(1.0, plot.bottom(), plot.y),
    );
    let hit = out.hit.contains(at).expect("cell under the pointer");
    assert_eq!((hit.series, hit.index), (1, 2), "row 1, column 2");
    assert_eq!(hit.datum.w, 6.0, "and it carries the cell's value");
}

#[test]
fn golden_heatmap() {
    render_golden("heatmap", &heat());
}

/// The y-axis title was offset a fixed distance from the plot edge, so it
/// landed on top of the tick labels for any axis whose numbers were wide
/// enough to fill the gutter — which `measured_padding` had already reserved
/// room beyond.
#[test]
fn regression_the_y_axis_title_clears_the_tick_labels() {
    use charts_core::render::LabelMetrics;
    let spec = ChartSpec::new(vec![Series::new(
        "latency",
        SeriesKind::line(),
        BLUE,
        vec![datum(0.0, 100_000.0), datum(1.0, 400_000.0)],
    )])
    .y(Axis::linear().include_zero(true).title("ms"));
    let out = render_with(&spec, surface(), &Gutters::Measured(&ApproxMetrics));

    let title = out
        .scene
        .labels
        .iter()
        .find(|l| l.role == LabelRole::AxisTitleY)
        .expect("y title");
    let leftmost_tick = out
        .scene
        .labels
        .iter()
        .filter(|l| l.role == LabelRole::AxisY)
        .map(|l| l.anchor.x - ApproxMetrics.measure(&l.text, LabelRole::AxisY).0)
        .fold(f32::INFINITY, f32::min);

    assert!(
        title.anchor.x < leftmost_tick,
        "title at x={} overlaps tick labels starting at x={leftmost_tick}",
        title.anchor.x
    );
    assert!(title.anchor.x >= 0.0, "and it stays on the surface");
}

/// A category axis resolves to `-0.5 ..= n-0.5`, so zero is always strictly
/// inside its window — and the zero rule then drew a line through the middle
/// of the first category. Visible as a seam across the bottom row of a
/// heatmap, and equally wrong for any chart with categorical y.
#[test]
fn regression_no_zero_rule_on_a_category_axis() {
    let out = render(&heat(), surface());
    let plot = out.hit.plot();
    assert!(out.y.min < 0.0 && out.y.max > 0.0, "zero really is inside the window");

    let full_width_rules = out
        .scene
        .marks
        .iter()
        .filter(|m| matches!(m, Mark::Stroke { layer: Layer::Axis, .. }))
        .count();
    assert_eq!(full_width_rules, 0, "no zero rule through the first category");

    // The rule is still drawn where it means something: a linear axis
    // spanning zero.
    let signed = ChartSpec::new(vec![Series::new(
        "delta",
        SeriesKind::bar(),
        BLUE,
        vec![datum(0.0, 4.0), datum(1.0, -3.0)],
    )]);
    let out = render(&signed, surface());
    assert!(
        out.scene
            .marks
            .iter()
            .any(|m| matches!(m, Mark::Stroke { layer: Layer::Axis, .. })),
        "a signed linear axis keeps its zero rule"
    );
    let _ = plot;
}
