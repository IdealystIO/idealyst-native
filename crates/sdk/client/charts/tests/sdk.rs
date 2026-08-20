//! Integration suite for the idealyst binding.
//!
//! Three layers, each tested where it can actually be observed:
//!
//! - the IR -> `canvas_core::Scene` adapter, which is pure;
//! - the pointer -> hovered-column mapping, which is pure once extracted;
//! - the element tree the component builds, inspected through the scene
//!   `Element` type rather than a real backend — the same substrate the
//!   `table` SDK's suite uses.

use std::rc::Rc;

use canvas_core::{DrawOp, PaintKind, Scene};
use charts::__test_support::hover_at;
use charts::*;
use charts_core::Rect as IrRect;
use runtime_scene::Element;
use runtime_world::World;

const BLUE: Color = Color::rgb(0x4c, 0x8d, 0xff);
const PINK: Color = Color::rgb(0xff, 0x6b, 0x9d);

fn line_spec() -> ChartSpec {
    ChartSpec::new(vec![Series::new(
        "revenue",
        SeriesKind::line(),
        BLUE,
        vec![datum(0.0, 3.0), datum(1.0, 7.0), datum(2.0, 5.0)],
    )])
}

// ===========================================================================
// Adapter
// ===========================================================================

fn scene_of(spec: &ChartSpec) -> Scene {
    let out = render(spec, IrRect::new(0.0, 0.0, 300.0, 200.0));
    let mut s = Scene::new();
    charts::marks_into_scene(&out.scene.marks, &mut s, 0.0, 0.0);
    s
}

#[test]
fn every_mark_kind_survives_the_adapter() {
    let spec = ChartSpec::new(vec![
        Series::new("line", SeriesKind::line(), BLUE, vec![datum(0.0, 1.0), datum(1.0, 2.0)]),
        Series::new("area", SeriesKind::area(), PINK, vec![datum(0.0, 2.0), datum(1.0, 1.0)]),
        Series::new(
            "dots",
            SeriesKind::scatter(),
            BLUE,
            vec![datum(0.0, 1.5), datum(1.0, 1.8)],
        ),
    ]);
    let s = scene_of(&spec);
    let ops = s.ops();
    assert!(ops.iter().any(|o| matches!(o, DrawOp::Stroke { .. })), "line/grid strokes");
    assert!(ops.iter().any(|o| matches!(o, DrawOp::Fill { .. })), "area fill");
    assert!(ops.iter().any(|o| matches!(o, DrawOp::Shapes { .. })), "scatter batch");
}

/// A scatter must reach the instanced batch, not become N separate fills.
/// That batch is the whole reason a large scatter is affordable — the GPU
/// renderer draws it in one pass — so silently expanding it here would be a
/// throughput regression invisible to a pixel comparison.
#[test]
fn regression_scatter_uses_one_instanced_batch_not_per_point_fills() {
    let spec = ChartSpec::new(vec![Series::new(
        "dots",
        SeriesKind::scatter(),
        BLUE,
        (0..500).map(|i| datum(i as f64, (i % 17) as f64)).collect(),
    )]);
    let s = scene_of(&spec);
    let batches: Vec<&DrawOp> = s
        .ops()
        .iter()
        .filter(|o| matches!(o, DrawOp::Shapes { .. }))
        .collect();
    assert_eq!(batches.len(), 1, "one batch for the series");
    match batches[0] {
        DrawOp::Shapes { shapes, .. } => assert_eq!(shapes.len(), 500),
        _ => unreachable!(),
    }
    // And no per-point fills leaked alongside it.
    let fills = s.ops().iter().filter(|o| matches!(o, DrawOp::Fill { .. })).count();
    assert!(fills < 10, "expected no per-point fills, saw {fills}");
}

/// A gradient's endpoints live in the same coordinate space as the geometry
/// it paints, so translating the marks must translate the gradient too.
/// Getting this wrong leaves an area fill's fade anchored to the wrong
/// height — visible only as a subtly wrong gradient, never as a crash.
#[test]
fn regression_gradient_endpoints_translate_with_the_geometry() {
    let spec = ChartSpec::new(vec![Series::new(
        "area",
        SeriesKind::area(),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 2.0)],
    )]);
    let out = render(&spec, IrRect::new(0.0, 0.0, 300.0, 200.0));

    let grad_y = |dy: f32| -> (f32, f32) {
        let mut s = Scene::new();
        charts::marks_into_scene(&out.scene.marks, &mut s, 0.0, dy);
        s.ops()
            .iter()
            .find_map(|o| match o {
                DrawOp::Fill { paint, .. } => match &paint.kind {
                    PaintKind::Linear(g) => Some((g.y0, g.y1)),
                    _ => None,
                },
                _ => None,
            })
            .expect("area fill should carry a linear gradient")
    };

    let (a0, a1) = grad_y(0.0);
    let (b0, b1) = grad_y(50.0);
    assert!((b0 - a0 - 50.0).abs() < 0.01, "gradient start must shift with the marks");
    assert!((b1 - a1 - 50.0).abs() < 0.01, "gradient end must shift with the marks");
}

#[test]
fn cubic_segments_reach_the_canvas_unflattened() {
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::smooth_line(),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 5.0), datum(2.0, 2.0), datum(3.0, 6.0)],
    )]);
    let s = scene_of(&spec);
    let has_cubic = s.ops().iter().any(|o| match o {
        DrawOp::Stroke { path, .. } => path
            .segs
            .iter()
            .any(|sg| matches!(sg, canvas_core::PathSeg::CubicTo { .. })),
        _ => false,
    });
    assert!(has_cubic, "a smoothed line must arrive as cubics, not line segments");
}

#[test]
fn dash_pattern_survives_the_adapter() {
    let spec = ChartSpec::new(vec![Series::new(
        "target",
        SeriesKind::Line(LineStyle::default().dashed([6.0, 4.0])),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 2.0)],
    )]);
    let s = scene_of(&spec);
    let dashed = s.ops().iter().any(|o| match o {
        DrawOp::Stroke { stroke, .. } => stroke.dash == vec![6.0, 4.0],
        _ => false,
    });
    assert!(dashed, "dash pattern must reach the canvas stroke");
}

// ===========================================================================
// Pointer -> hovered column
// ===========================================================================

#[test]
fn hover_resolves_every_series_in_the_column() {
    let spec = ChartSpec::new(vec![
        Series::new("a", SeriesKind::line(), BLUE, vec![datum(0.0, 1.0), datum(1.0, 5.0)]),
        Series::new("b", SeriesKind::line(), PINK, vec![datum(0.0, 9.0), datum(1.0, 2.0)]),
    ]);
    let out = render(&spec, IrRect::new(0.0, 0.0, 300.0, 200.0));

    let h = hover_at(&out, 299.0, 100.0).expect("a column near the right edge");
    assert_eq!(h.entries.len(), 2, "both series report at the hovered x");
    assert_eq!(h.entries[0].series, 0);
    assert_eq!(h.entries[1].series, 1);
    assert_eq!(h.entries[0].datum.y, 5.0);
}

/// Grouped bars sit side by side by design, so their pixel x differ within
/// one category. The tooltip must still list the whole group.
#[test]
fn regression_hover_over_grouped_bars_returns_the_whole_group() {
    let spec = ChartSpec::new(vec![
        Series::new("a", SeriesKind::bar(), BLUE, vec![datum(0.0, 4.0)]),
        Series::new("b", SeriesKind::bar(), PINK, vec![datum(0.0, 6.0)]),
    ])
    .x(Axis::category(["only"]));
    let out = render(&spec, IrRect::new(0.0, 0.0, 300.0, 200.0));

    let h = hover_at(&out, 150.0, 100.0).expect("the single category column");
    assert_eq!(h.entries.len(), 2, "both bars in the group");
}

#[test]
fn hover_on_an_empty_chart_is_none() {
    let out = render(&ChartSpec::default(), IrRect::new(0.0, 0.0, 300.0, 200.0));
    assert!(hover_at(&out, 10.0, 10.0).is_none());
}

// ===========================================================================
// Component tree
// ===========================================================================

/// Step through a component boundary. `component_scope` wraps its result in
/// `Element::Owned`, so every structural assertion has to unwrap first.
fn unwrap_owned(el: &Element) -> &Element {
    match el {
        Element::Owned { element, .. } => unwrap_owned(element),
        other => other,
    }
}

fn children_of(el: &Element) -> &[Element] {
    match unwrap_owned(el) {
        Element::Item { children, .. } => children,
        Element::Fragment(children) => children,
        _ => panic!("expected an Element::Item or Fragment"),
    }
}

/// True for a reactive hole (`when` / `switch`), which is what the label
/// layers and the tooltip lower to. Their contents only exist once realized
/// against a host, so the tree test asserts on their PRESENCE, not inside.
fn is_reactive_hole(el: &Element) -> bool {
    matches!(unwrap_owned(el), Element::Dyn(_))
}

/// Build the component inside a world + scope, as a real mount would, and
/// run `f` against the tree while that world is still alive.
///
/// The world has to outlive the assertion, not just the build: the canvas
/// painter reads a memo, and reading a signal whose World has been dropped
/// aborts by design. A real app keeps the world for the app's lifetime, so
/// this mirrors the actual arrangement rather than working around it.
fn with_chart<R>(props: ChartProps, f: impl FnOnce(&Element) -> R) -> R {
    let world = World::new();
    let el = world.enter(|| runtime_scene::component_scope(|| Chart(&props)));
    world.enter(|| f(&el))
}

/// The tree is [root [ legend, plot_row [ y_gutter, plot [ canvas, tooltip ] ],
/// x_gutter ]]. Pinning it matters because the gutters' *existence* is what
/// lets the framework lay out labels — a refactor that collapsed them would
/// silently move label positioning back into the canvas — and because the
/// legend is a real flex row rather than absolutely-placed text.
#[test]
fn builds_the_gutter_plus_plot_tree() {
    with_chart(ChartProps { spec: line_spec().into(), ..Default::default() }, |el| {
        let root = children_of(el);
        assert_eq!(root.len(), 3, "legend + plot row + x-axis gutter");
        assert!(is_reactive_hole(&root[0]), "the legend rebuilds reactively");
        assert!(is_reactive_hole(&root[2]), "x-axis labels rebuild reactively");

        let row = children_of(&root[1]);
        assert_eq!(row.len(), 2, "y-axis gutter + plot area");
        assert!(is_reactive_hole(&row[0]), "y-axis labels rebuild reactively");

        let plot = children_of(&row[1]);
        assert_eq!(plot.len(), 2, "canvas + tooltip layer");
        assert!(is_reactive_hole(&plot[1]), "the tooltip is a reactive hole");
    });
}

/// The chart must not draw before its plot has been measured.
///
/// Regression guard for the alternative — guessing a size and painting
/// against it — which produces a visibly wrong chart on the first frame
/// that then jumps once the real size arrives. An empty first scene is the
/// intended behavior, so it is pinned rather than left to chance.
#[test]
fn canvas_paints_nothing_before_the_first_layout() {
    with_chart(ChartProps { spec: line_spec().into(), ..Default::default() }, |el| {
        let row = children_of(&children_of(el)[1]);
        let canvas_el = &children_of(&row[1])[0];

        let prim = match unwrap_owned(canvas_el) {
            Element::Item { data, .. } => data
                .downcast_ref::<canvas_core::CanvasPrim>()
                .expect("the plot's first child is the Canvas"),
            _ => panic!("expected the canvas item"),
        };
        // Plot size is still (0, 0): the layout callback has not fired.
        let scene = canvas_core::paint_scene(&prim.props);
        assert!(scene.is_empty(), "an unmeasured chart must paint an empty scene");
    });
}

#[test]
fn optional_callbacks_are_only_wired_when_present() {
    // Absent by default — the props struct must not manufacture a no-op.
    let bare = ChartProps::default();
    assert!(bare.on_hover.is_none());
    assert!(bare.tooltip_content.is_none());

    let with_cb = ChartProps {
        on_hover: Some(Rc::new(|_| {})),
        ..Default::default()
    };
    assert!(with_cb.on_hover.is_some());
    // Building with a callback attached must not panic.
    with_chart(with_cb, |_| {});
}

#[test]
fn author_style_replaces_the_default_root() {
    let custom = Rc::new(runtime_core::StyleSheet::r#static(runtime_core::StyleRules::default()));
    with_chart(
        ChartProps { spec: line_spec().into(), style: Some(custom), ..Default::default() },
        |el| {
            // Still the same shape — the style channel must not change structure.
            assert_eq!(children_of(el).len(), 3);
        },
    );
}

/// The component must be reachable through `ui!` struct-literal syntax,
/// which is what `#[component]` exists to provide. A compile-time check:
/// if the macro's `pub type Chart = ChartProps` alias were not re-exported
/// alongside the fn, this would fail to build even though `Chart(&props)`
/// still worked — the two forms live in different namespaces and it is
/// entirely possible to ship one without the other.
#[test]
fn usable_from_the_ui_macro() {
    use runtime_core::ui;

    let world = World::new();
    let el = world.enter(|| {
        runtime_scene::component_scope(|| {
            ui! {
                view() {
                    Chart(spec = line_spec(), tooltip = false)
                }
            }
        })
    });
    world.enter(|| {
        // view -> Chart(Owned) -> root
        let outer = children_of(&el);
        assert_eq!(outer.len(), 1, "the view wraps exactly one chart");
        assert_eq!(children_of(&outer[0]).len(), 3, "legend + plot row + x gutter");
    });
}

// ===========================================================================
// Emphasis wiring
// ===========================================================================

/// Emphasis is keyed on the DATA column, not the pointer's pixel position.
///
/// This is what keeps the render memo from re-running on every pointer move:
/// `hover_at` reports a full `ChartHover` (which carries pixels and so
/// changes constantly), but the value fed to the highlight is the datum's x,
/// which only changes when the pointer crosses into a new column.
#[test]
fn hover_resolves_to_a_stable_column_across_pixel_moves() {
    let spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::scatter(),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 5.0), datum(2.0, 3.0)],
    )]);
    let out = render(&spec, IrRect::new(0.0, 0.0, 300.0, 200.0));

    let col = |x: f32| out.hit.column_at(charts_core::pt(x, 100.0)).first().map(|e| e.datum.x);
    // Several distinct pixel positions inside the same column agree.
    let a = col(148.0);
    let b = col(150.0);
    let c = col(152.0);
    assert_eq!(a, b);
    assert_eq!(b, c);
    assert_eq!(a, Some(1.0));
    // A position over a different column reports a different one.
    assert_ne!(col(2.0), a);
}

/// The emphasis knobs reach the marks: a hovered column enlarges its points
/// and thickens its line, end to end through the spec the SDK builds.
#[test]
fn highlight_reaches_the_rendered_marks() {
    let style = PointStyle::new(3.0).hover(9.0);
    let mut spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::Scatter(style),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 2.0)],
    )]);
    let plain = render(&spec, IrRect::new(0.0, 0.0, 300.0, 200.0));
    spec.highlight = Highlight::column(1.0);
    let hovered = render(&spec, IrRect::new(0.0, 0.0, 300.0, 200.0));

    let radii = |o: &ChartOutput| -> Vec<f32> {
        o.scene
            .marks
            .iter()
            .filter_map(|m| match m {
                charts_core::Mark::Points { instances, .. } => {
                    Some(instances.iter().map(|p| p.half.x).collect::<Vec<_>>())
                }
                _ => None,
            })
            .flatten()
            .collect()
    };
    assert_eq!(radii(&plain), vec![3.0, 3.0]);
    assert_eq!(radii(&hovered), vec![3.0, 9.0]);
}

/// A styled series survives the adapter with its emphasis applied — the
/// enlarged marker must reach the canvas batch, not just the IR.
#[test]
fn emphasised_marker_size_reaches_the_canvas() {
    let mut spec = ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::Scatter(PointStyle::new(3.0).hover(10.0)),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 2.0)],
    )]);
    spec.highlight = Highlight::column(1.0);
    let s = scene_of(&spec);
    let halves: Vec<f32> = s
        .ops()
        .iter()
        .filter_map(|o| match o {
            DrawOp::Shapes { shapes, .. } => Some(shapes.iter().map(|sh| sh.hw).collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(halves.contains(&10.0), "the hovered marker's size must reach the canvas: {halves:?}");
}

// ===========================================================================
// Transition state
// ===========================================================================

use charts::__test_support::visual_state;

fn tspec(v: [f64; 2]) -> ChartSpec {
    ChartSpec::new(vec![Series::new(
        "s",
        SeriesKind::bar(),
        BLUE,
        v.iter().enumerate().map(|(i, y)| datum(i as f64, *y)).collect(),
    )])
}

/// A settled chart displays its target.
#[test]
fn visual_state_at_rest_is_the_target() {
    let t = tspec([5.0, 6.0]);
    let got = visual_state(Some(&tspec([1.0, 1.0])), Some(&t), 1.0).expect("a state");
    assert_eq!(got.series[0].data, t.series[0].data);
}

/// Mid-flight it is the interpolated state, so a change landing during an
/// animation continues from what is on screen.
#[test]
fn visual_state_mid_flight_is_interpolated() {
    let got = visual_state(Some(&tspec([0.0, 0.0])), Some(&tspec([10.0, 10.0])), 0.5)
        .expect("a state");
    for d in &got.series[0].data {
        assert!(d.y > 0.0 && d.y < 10.0, "expected intermediate, got {}", d.y);
    }
}

/// The successive-change case that broke in the demo.
///
/// Regression guard: the second data change animated from the chart's
/// ORIGINAL values instead of from where the first change settled — the
/// visible symptom was the bars snapping back before animating again. It
/// happened because the animation's start was read from a signal only ever
/// written when an animation BEGINS, so it never advanced past the first
/// spec. Chaining `visual_state` reproduces the sequence exactly.
#[test]
fn regression_successive_changes_start_from_the_previous_target() {
    let (a, b, c) = (tspec([1.0, 1.0]), tspec([10.0, 10.0]), tspec([4.0, 4.0]));

    // First change settles on `b`.
    let after_first = visual_state(Some(&a), Some(&b), 1.0).expect("settled");
    assert_eq!(after_first.series[0].data[0].y, 10.0);

    // Second change must therefore start from `b`, not from `a`.
    let start_of_second = visual_state(Some(&after_first), Some(&b), 1.0).expect("settled");
    assert_eq!(
        start_of_second.series[0].data[0].y, 10.0,
        "the second transition starts where the first ended"
    );

    // And a frame early in that second transition sits between 10 and 4 —
    // never back near the original 1.
    let early = charts_core::lerp_data(&start_of_second, &c, 0.25).expect("same shape");
    let y = early.series[0].data[0].y;
    assert!(y > 4.0 && y <= 10.0, "expected a value between 10 and 4, got {y}");
    assert!(y > 2.0, "must not fall back toward the original value");
}

/// With no history there is nothing to animate from.
#[test]
fn visual_state_is_none_before_the_first_spec() {
    assert!(visual_state(None, None, 1.0).is_none());
}

/// A shape change cannot be interpolated, so the visual state is the target
/// — which is what makes the transition snap rather than pair unrelated
/// points.
#[test]
fn visual_state_falls_back_to_the_target_on_a_shape_change() {
    let a = tspec([1.0, 1.0]);
    let b = ChartSpec::new(vec![Series::new("s", SeriesKind::bar(), BLUE, vec![datum(0.0, 9.0)])]);
    let got = visual_state(Some(&a), Some(&b), 0.5).expect("a state");
    assert_eq!(got.series[0].data.len(), 1);
    assert_eq!(got.series[0].data[0].y, 9.0);
}
