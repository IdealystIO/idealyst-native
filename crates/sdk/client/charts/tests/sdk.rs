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

/// A pointer frame with only the plot-local position set.
///
/// The window position and plot rect are pass-through data the hit test never
/// consults, so the resolution tests leave them zero; the tests that care that
/// they survive the round trip set them explicitly.
fn at(x: f32, y: f32) -> PointerFrame {
    PointerFrame { local: charts_core::pt(x, y), ..Default::default() }
}

#[test]
fn hover_resolves_every_series_in_the_column() {
    let spec = ChartSpec::new(vec![
        Series::new("a", SeriesKind::line(), BLUE, vec![datum(0.0, 1.0), datum(1.0, 5.0)]),
        Series::new("b", SeriesKind::line(), PINK, vec![datum(0.0, 9.0), datum(1.0, 2.0)]),
    ]);
    let out = render(&spec, IrRect::new(0.0, 0.0, 300.0, 200.0));

    let h = hover_at(&out, at(299.0, 100.0)).expect("a column near the right edge");
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

    let h = hover_at(&out, at(150.0, 100.0)).expect("the single category column");
    assert_eq!(h.entries.len(), 2, "both bars in the group");
}

#[test]
fn hover_on_an_empty_chart_is_none() {
    let out = render(&ChartSpec::default(), IrRect::new(0.0, 0.0, 300.0, 200.0));
    assert!(hover_at(&out, at(10.0, 10.0)).is_none());
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

/// The tree is [root [ legend, plot_row [ y_gutter, plot [ canvas, labels ] ],
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
        // Two, not three: the SDK renders NO hover surface. A third child
        // here means someone put a tooltip back inside the plot — where the
        // plot's own `overflow: hidden` would clip it.
        assert_eq!(plot.len(), 2, "canvas + in-plot labels, and nothing else");
        assert!(is_reactive_hole(&plot[1]), "annotation labels rebuild reactively");
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
                    Chart(spec = line_spec())
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
    let got = visual_state(Some(&tspec([1.0, 1.0])), Some(&t), TweenAt::uniform(1.0)).expect("a state");
    assert_eq!(got.series[0].data, t.series[0].data);
}

/// Mid-flight it is the interpolated state, so a change landing during an
/// animation continues from what is on screen.
#[test]
fn visual_state_mid_flight_is_interpolated() {
    let got = visual_state(Some(&tspec([0.0, 0.0])), Some(&tspec([10.0, 10.0])), TweenAt::uniform(0.5))
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
    let after_first = visual_state(Some(&a), Some(&b), TweenAt::uniform(1.0)).expect("settled");
    assert_eq!(after_first.series[0].data[0].y, 10.0);

    // Second change must therefore start from `b`, not from `a`.
    let start_of_second = visual_state(Some(&after_first), Some(&b), TweenAt::uniform(1.0)).expect("settled");
    assert_eq!(
        start_of_second.series[0].data[0].y, 10.0,
        "the second transition starts where the first ended"
    );

    // And a frame early in that second transition sits between 10 and 4 —
    // never back near the original 1.
    let early = charts_core::lerp_data(&start_of_second, &c, TweenAt::uniform(0.25)).expect("same shape");
    let y = early.series[0].data[0].y;
    assert!(y > 4.0 && y <= 10.0, "expected a value between 10 and 4, got {y}");
    assert!(y > 2.0, "must not fall back toward the original value");
}

/// With no history there is nothing to animate from.
#[test]
fn visual_state_is_none_before_the_first_spec() {
    assert!(visual_state(None, None, TweenAt::uniform(1.0)).is_none());
}

/// A shape change cannot be interpolated, so the visual state is the target
/// — which is what makes the transition snap rather than pair unrelated
/// points.
#[test]
fn visual_state_falls_back_to_the_target_on_a_shape_change() {
    let a = tspec([1.0, 1.0]);
    let b = ChartSpec::new(vec![Series::new("s", SeriesKind::bar(), BLUE, vec![datum(0.0, 9.0)])]);
    let got = visual_state(Some(&a), Some(&b), TweenAt::uniform(0.5)).expect("a state");
    assert_eq!(got.series[0].data.len(), 1);
    assert_eq!(got.series[0].data[0].y, 9.0);
}

// ===========================================================================
// Polar components
// ===========================================================================

fn pie_spec() -> PieSpec {
    PieSpec::donut(
        vec![
            Slice::new("direct", 40.0, BLUE),
            Slice::new("search", 35.0, PINK),
            Slice::new("social", 25.0, Color::rgb(0x3d, 0xd5, 0x98)),
        ],
        0.6,
    )
}

fn with_pie<R>(props: PieChartProps, f: impl FnOnce(&Element) -> R) -> R {
    let world = World::new();
    let el = world.enter(|| runtime_scene::component_scope(|| PieChart(&props)));
    world.enter(|| f(&el))
}

fn with_radial<R>(props: RadialChartProps, f: impl FnOnce(&Element) -> R) -> R {
    let world = World::new();
    let el = world.enter(|| runtime_scene::component_scope(|| RadialChart(&props)));
    world.enter(|| f(&el))
}

/// A polar chart needs no gutters — its labels sit at their anchors inside
/// the plot — so the tree is one level shallower than the cartesian one.
/// Pinning it guards against a refactor quietly reintroducing gutter boxes
/// that would then take space no polar label uses.
#[test]
fn a_pie_builds_a_legend_plus_plot_tree() {
    with_pie(PieChartProps { spec: pie_spec().into(), ..Default::default() }, |el| {
        let root = children_of(el);
        assert_eq!(root.len(), 2, "legend + plot");
        assert!(is_reactive_hole(&root[0]), "the legend rebuilds reactively");

        let plot = children_of(&root[1]);
        // Same as the cartesian tree: canvas + labels, no hover surface.
        assert_eq!(plot.len(), 2, "canvas + label layer, and nothing else");
        assert!(is_reactive_hole(&plot[1]), "labels rebuild reactively");
    });
}

#[test]
fn a_radial_chart_builds_the_same_shape() {
    let spec = RadialSpec::gauge("cpu", 62.0, 100.0, BLUE);
    with_radial(RadialChartProps { spec: spec.into(), ..Default::default() }, |el| {
        assert_eq!(children_of(el).len(), 2);
    });
}

#[test]
fn a_polar_chart_with_no_data_still_builds() {
    with_pie(PieChartProps::default(), |el| {
        assert_eq!(children_of(el).len(), 2);
    });
}

#[test]
fn wedge_marks_reach_the_canvas_as_even_odd_fills() {
    let out = render_pie(&pie_spec(), IrRect::new(0.0, 0.0, 300.0, 300.0));
    let mut s = Scene::new();
    charts::marks_into_scene(&out.scene.marks, &mut s, 0.0, 0.0);
    let fills: Vec<&DrawOp> =
        s.ops().iter().filter(|o| matches!(o, DrawOp::Fill { .. })).collect();
    assert_eq!(fills.len(), 3, "one fill per slice");
    // Even-odd is what punches the donut hole; NonZero would fill it in.
    for f in fills {
        match f {
            DrawOp::Fill { fill_rule, .. } => {
                assert_eq!(*fill_rule, canvas_core::FillRule::EvenOdd)
            }
            _ => unreachable!(),
        }
    }
}

/// The offset exists so a host can place several charts on one canvas; a
/// wedge's cubics have to move with everything else.
#[test]
fn a_translated_wedge_moves_every_control_point() {
    let out = render_pie(&pie_spec(), IrRect::new(0.0, 0.0, 300.0, 300.0));
    let mut a = Scene::new();
    let mut b = Scene::new();
    charts::marks_into_scene(&out.scene.marks, &mut a, 0.0, 0.0);
    charts::marks_into_scene(&out.scene.marks, &mut b, 40.0, 0.0);
    let xs = |s: &Scene| -> Vec<f32> {
        s.ops()
            .iter()
            .filter_map(|o| match o {
                DrawOp::Fill { path, .. } => Some(path.clone()),
                _ => None,
            })
            .flat_map(|p| {
                p.segs
                    .iter()
                    .flat_map(|seg| match seg {
                        canvas_core::PathSeg::MoveTo { x, .. }
                        | canvas_core::PathSeg::LineTo { x, .. } => vec![*x],
                        canvas_core::PathSeg::QuadTo { cx, x, .. } => vec![*cx, *x],
                        canvas_core::PathSeg::CubicTo { c1x, c2x, x, .. } => {
                            vec![*c1x, *c2x, *x]
                        }
                        canvas_core::PathSeg::Close => vec![],
                    })
                    .collect::<Vec<f32>>()
            })
            .collect()
    };
    let (left, right) = (xs(&a), xs(&b));
    assert_eq!(left.len(), right.len());
    assert!(
        left.iter().zip(&right).all(|(l, r)| (r - l - 40.0).abs() < 0.01),
        "every control point shifted by exactly the offset"
    );
}

#[test]
fn polar_hover_resolves_the_slice_under_the_pointer() {
    use charts::__test_support::polar_hover_at;
    let out = render_pie(&pie_spec(), IrRect::new(0.0, 0.0, 300.0, 300.0));
    let labels: Vec<String> = pie_spec().slices.iter().map(|s| s.label.clone()).collect();

    // 45° clockwise from twelve, inside the ring: the first slice spans
    // 0..144°, so this is `direct`.
    let p = charts_core::polar::point_on(out.center, out.radius * 0.8, 45f32.to_radians());
    let hover = polar_hover_at(&out, &labels, at(p.x, p.y)).expect("a slice is under the pointer");
    assert_eq!(hover.index, 0);
    assert_eq!(hover.label, "direct");
    assert_eq!(hover.value, 40.0);
}

/// The hole is genuinely nothing, not a near miss — so the tooltip has to
/// disappear there rather than latch onto whichever slice is closest.
#[test]
fn hovering_the_donut_hole_resolves_to_nothing() {
    use charts::__test_support::polar_hover_at;
    let out = render_pie(&pie_spec(), IrRect::new(0.0, 0.0, 300.0, 300.0));
    let labels: Vec<String> = pie_spec().slices.iter().map(|s| s.label.clone()).collect();
    assert!(polar_hover_at(&out, &labels, at(out.center.x, out.center.y)).is_none());
}

// ===========================================================================
// Gutter collapse
// ===========================================================================

/// A 40px sparkline handed the default 22px x-gutter and a 6px legend row has
/// 12px of plot left, and it renders as a broken-looking fragment rather than
/// a small chart. `sparkline()` promises the space back; this is the code
/// that actually returns it.
#[test]
fn regression_a_sparkline_reserves_no_gutter_space() {
    use charts::__test_support::gutters_for;
    let spec = line_spec();
    assert_eq!(
        gutters_for(&spec, 44.0, 22.0),
        (44.0, 22.0),
        "an ordinary chart keeps its gutters"
    );
    assert_eq!(
        gutters_for(&spec.clone().sparkline(), 44.0, 22.0),
        (0.0, 0.0),
        "a sparkline gives both back"
    );
}

#[test]
fn an_axis_title_keeps_its_gutter_even_with_labels_off() {
    use charts::__test_support::gutters_for;
    // The title still has to go somewhere, so the gutter cannot collapse just
    // because the tick labels are gone.
    let spec = line_spec().y(Axis::linear().labels(false).title("USD"));
    assert_eq!(gutters_for(&spec, 44.0, 22.0).0, 44.0);
}

#[test]
fn the_two_gutters_collapse_independently() {
    use charts::__test_support::gutters_for;
    let spec = line_spec().y(Axis::linear().labels(false));
    assert_eq!(gutters_for(&spec, 44.0, 22.0), (0.0, 22.0));
}

/// The core emits annotation text as `LabelRole::Annotation` placements, and
/// the component routed labels only into the two axis gutters — so a
/// threshold line drew and its label silently did not.
#[test]
fn regression_annotation_labels_are_materialized() {
    let spec = line_spec().annotate(Annotation::y_line(5.0, PINK).label("SLO"));
    let out = render(&spec, IrRect::new(0.0, 0.0, 300.0, 200.0));
    use charts_core::LabelRole;
    let roles: Vec<LabelRole> = out.scene.labels.iter().map(|l| l.role).collect();
    assert!(roles.contains(&LabelRole::Annotation), "the core emits it");

    // The plot box carries a second child — the in-plot label layer — which
    // is where a plot-local anchor belongs. Routing it into a gutter would
    // put it in the wrong coordinate space entirely.
    with_chart(ChartProps { spec: spec.into(), ..Default::default() }, |el| {
        let row = children_of(&children_of(el)[1]);
        assert_eq!(children_of(&row[1]).len(), 2);
    });
}

// ===========================================================================
// The pointer frame
// ===========================================================================
//
// The SDK renders no hover surface, so a caller's tooltip lives outside the
// chart's tree and has to place itself. Everything it needs to do that
// travels in `PointerFrame`. These pin the two properties that make
// out-of-tree placement possible at all; without them the only implementable
// behaviour is cursor-following in window space.

/// The window position and plot rect survive the hit test unchanged.
///
/// They are pass-through data — `hover_at` consults only `local` — which is
/// exactly why this needs pinning: a refactor that rebuilt the frame from the
/// hit instead of carrying the caller's would zero both fields and every
/// out-of-tree surface would collapse onto the window origin.
#[test]
fn the_pointer_frame_reaches_the_callback_intact() {
    let spec = ChartSpec::new(vec![Series::new(
        "a",
        SeriesKind::line(),
        BLUE,
        vec![datum(0.0, 1.0), datum(1.0, 5.0)],
    )]);
    let out = render(&spec, IrRect::new(0.0, 0.0, 300.0, 200.0));

    let frame = PointerFrame {
        local: charts_core::pt(150.0, 100.0),
        window: charts_core::pt(462.0, 388.0),
        plot: ViewportRect { x: 312.0, y: 288.0, width: 300.0, height: 200.0 },
    };
    let h = hover_at(&out, frame).expect("a column under the pointer");
    assert_eq!(h.at, frame, "the frame is carried, not reconstructed");
}

/// `to_viewport` maps plot-local geometry into the space the window position
/// is already in — the conversion that makes "place beside the hovered mark"
/// expressible from outside the chart.
///
/// Pinned against the frame's own numbers rather than a literal, so the test
/// states the invariant (local + plot origin == viewport) instead of
/// restating the arithmetic.
#[test]
fn to_viewport_offsets_plot_local_geometry_by_the_plot_origin() {
    let frame = PointerFrame {
        local: charts_core::pt(20.0, 30.0),
        window: charts_core::pt(332.0, 318.0),
        plot: ViewportRect { x: 312.0, y: 288.0, width: 300.0, height: 200.0 },
    };
    // The cursor is the one point whose answer we already know: converting
    // `local` must land on `window`.
    let round_tripped = frame.to_viewport(frame.local);
    assert_eq!(round_tripped, frame.window, "the cursor converts to itself");

    // And any other plot-local point — a mark anchor, a bounds corner —
    // shifts by the same origin.
    let mark = charts_core::pt(0.0, 0.0);
    assert_eq!(frame.to_viewport(mark), charts_core::pt(312.0, 288.0));
}

/// A hovered bar reports its whole body, not just the anchor `position`.
///
/// This is the payload half of the same story `charts-core`'s bounds tests
/// cover: a caller placing a surface at the bar's vertical middle needs the
/// rect to reach it through `ChartHover`.
#[test]
fn a_hovered_bar_reports_its_bounds_through_the_callback() {
    let spec = ChartSpec::new(vec![Series::new(
        "a",
        SeriesKind::bar(),
        BLUE,
        vec![datum(0.0, 4.0), datum(1.0, 9.0)],
    )])
    .x(Axis::category(["one", "two"]));
    let out = render(&spec, IrRect::new(0.0, 0.0, 300.0, 200.0));

    // The TALLER bar: an unspecified domain runs from the data minimum, so
    // the smallest bar is zero-height and would make this test pass for the
    // wrong reason.
    let h = hover_at(&out, at(225.0, 100.0)).expect("a bar column");
    let entry = &h.entries[0];
    let MarkBounds::Rect(r) = entry.bounds else {
        panic!("a bar reports rect bounds, got {:?}", entry.bounds);
    };
    assert!(r.h > 0.0, "the bar has a body to place against");
    // The anchor sits on the rect's edge; the middle does not. If these were
    // ever equal, `bounds` would be adding nothing over `position`.
    let middle_y = r.y + r.h / 2.0;
    assert!(
        (entry.position.y - middle_y).abs() > 1.0,
        "the bar's anchor ({}) and its middle ({middle_y}) are different points",
        entry.position.y
    );
}

/// The polar components report the same frame, so one placement helper serves
/// a pie and a line chart. Divergence here would force callers to write the
/// conversion twice.
#[test]
fn a_polar_hover_carries_the_same_pointer_frame() {
    let spec = PieSpec::new(vec![
        Slice::new("a", 3.0, BLUE),
        Slice::new("b", 1.0, PINK),
    ]);
    let out = charts_core::render_pie(&spec, IrRect::new(0.0, 0.0, 300.0, 300.0));
    let labels = vec!["a".to_string(), "b".to_string()];

    let frame = PointerFrame {
        local: charts_core::pt(out.center.x, out.center.y - 40.0),
        window: charts_core::pt(500.0, 410.0),
        plot: ViewportRect { x: 350.0, y: 300.0, width: 300.0, height: 300.0 },
    };
    let h = polar_hover_at(&out, &labels, frame).expect("a slice under the pointer");
    assert_eq!(h.at, frame);
    assert!(
        matches!(h.hit.bounds, MarkBounds::Wedge { .. }),
        "a slice reports its wedge, so a caller can place along the bisector"
    );
}

// ===========================================================================
// Transition channels
// ===========================================================================
//
// The chart declares transitions with the framework's own
// `Transition { duration_ms, easing }` and derives both channels' eased
// fractions from ONE wall-clock elapsed. These pin that arithmetic, which is
// otherwise only observable by eye on a 420 ms animation.

/// No transition means SNAP — settled at every elapsed, including zero.
///
/// The alternative reading, "animate instantly", divides by a zero duration.
/// A declared `0 ms` is treated the same way for the same reason.
#[test]
fn an_undeclared_channel_is_settled_immediately() {
    assert_eq!(channel_at(None, 0.0), 1.0);
    assert_eq!(channel_at(None, 5_000.0), 1.0);
    assert_eq!(channel_at(Some(Transition::new(0, Easing::Linear)), 0.0), 1.0);
}

/// A channel runs from 0 to 1 across its own duration and clamps past it.
#[test]
fn a_channel_runs_across_its_own_duration() {
    let tr = Some(Transition::new(400, Easing::Linear));
    assert_eq!(channel_at(tr, 0.0), 0.0);
    assert!((channel_at(tr, 200.0) - 0.5).abs() < 1e-6);
    assert_eq!(channel_at(tr, 400.0), 1.0);
    assert_eq!(channel_at(tr, 10_000.0), 1.0, "clamped, never overshooting");
}

/// The two channels are independent: at one instant a short colour fade can
/// be finished while a longer value glide is still running.
///
/// This is the whole point of splitting them — a 420 ms glide is right for a
/// bar changing height and sluggish for the same bar changing hue.
#[test]
fn the_two_channels_advance_independently() {
    let value = Some(Transition::new(400, Easing::Linear));
    let color = Some(Transition::new(100, Easing::Linear));

    let at = tween_at(value, color, 100.0);
    assert_eq!(at.color, 1.0, "the short colour fade has landed");
    assert!(at.value < 1.0, "…while the value glide is still running");

    let at = tween_at(value, color, 400.0);
    assert_eq!((at.value, at.color), (1.0, 1.0), "both settled at the span");
}

/// The frame loop has to outlast the LONGER channel, or the other one freezes
/// part-resolved on screen.
#[test]
fn the_span_is_the_longer_channel() {
    let short = Some(Transition::new(100, Easing::Linear));
    let long = Some(Transition::new(400, Easing::Linear));
    assert_eq!(transition_span_ms(short, long), 400);
    assert_eq!(transition_span_ms(long, short), 400);
    assert_eq!(transition_span_ms(None, short), 100);
    assert_eq!(transition_span_ms(None, None), 0, "nothing declared, no loop");
}

/// Easing comes from the framework's own evaluator, so a curve means the same
/// thing here as on a stylesheet's `background_transition`.
///
/// Guards against this crate quietly growing a private easing dialect — the
/// duplicate-vocabulary failure the whole design is meant to avoid.
#[test]
fn easing_matches_the_frameworks_own_curve() {
    let tr = Some(Transition::new(1000, Easing::EaseInOut));
    for ms in [0.0, 250.0, 500.0, 750.0, 1000.0] {
        let expect = runtime_core::animation::apply_easing(ms / 1000.0, Easing::EaseInOut);
        assert!(
            (channel_at(tr, ms) - expect).abs() < 1e-6,
            "at {ms}ms the chart must use the framework curve"
        );
    }
    // A non-linear curve is actually being applied, so the test above is not
    // comparing linear against linear.
    assert!((channel_at(tr, 250.0) - 0.25).abs() > 0.01);
}

/// `visual_state` keeps animating while EITHER channel is unsettled.
///
/// The bug this guards: gating on the value channel alone would freeze a
/// colour fade the moment the values landed, so a long fade after a short
/// glide would simply stop part-way.
#[test]
fn visual_state_animates_while_either_channel_runs() {
    let a = tspec([0.0, 0.0]);
    let mut b = tspec([0.0, 0.0]);
    b.series[0].color = PINK;

    let mid = visual_state(Some(&a), Some(&b), TweenAt { value: 1.0, color: 0.5 })
        .expect("a state");
    assert_ne!(mid.series[0].color, PINK, "the colour is still mid-fade");
    assert_ne!(mid.series[0].color, a.series[0].color);

    let done = visual_state(Some(&a), Some(&b), TweenAt::SETTLED).expect("a state");
    assert_eq!(done.series[0].color, PINK);
}

/// A `StyleFn` threshold FADES across the transition instead of flipping at
/// the frame the tweened value crosses it.
///
/// This is the bug that motivated the colour channel. The callback is a
/// function of the datum, so resolving it once per frame against the
/// interpolated value makes a bar snap from blue to red mid-glide. Resolving
/// BOTH ends and interpolating the two answers is what the axis domain
/// already did, and now colour does too.
#[test]
fn a_threshold_recolor_fades_instead_of_flipping() {
    let red = charts_core::Color::rgb(0xff, 0x00, 0x00);
    let threshold: StyleFn = Rc::new(move |ctx: &MarkContext| {
        if ctx.datum.y >= 10.0 {
            MarkOverride::color(red)
        } else {
            MarkOverride::default()
        }
    });
    let mk = |y: f64| {
        let mut s = ChartSpec::new(vec![Series::new(
            "a",
            SeriesKind::bar(),
            BLUE,
            vec![datum(0.0, y)],
        )]);
        s.series[0].style_fn = Some(threshold.clone());
        s
    };
    // 5 -> 15 crosses the threshold, so the two ends resolve to different
    // colours: BLUE at the start, red at the end.
    let (from, to) = (mk(5.0), mk(15.0));
    let rect = IrRect::new(0.0, 0.0, 300.0, 200.0);

    let fill_of = |out: &charts_core::ChartOutput| {
        out.scene
            .marks
            .iter()
            .find_map(|m| match m {
                charts_core::Mark::Fill { paint: charts_core::Paint::Solid(c), layer, .. }
                    if *layer == charts_core::Layer::Series =>
                {
                    Some(*c)
                }
                _ => None,
            })
            .expect("a bar fill")
    };

    let start = fill_of(&charts_core::render_tween(
        &from, &to, TweenAt::uniform(0.0), rect, &charts_core::Gutters::None));
    let mid = fill_of(&charts_core::render_tween(
        &from, &to, TweenAt::uniform(0.5), rect, &charts_core::Gutters::None));
    let end = fill_of(&charts_core::render_tween(
        &from, &to, TweenAt::uniform(1.0), rect, &charts_core::Gutters::None));

    assert_eq!(start, BLUE, "below the threshold at the start");
    assert_eq!(end, red, "above it at the end");
    assert_ne!(mid, BLUE, "…and genuinely between the two mid-transition");
    assert_ne!(mid, red);
    assert!(mid.r > BLUE.r && mid.r < red.r, "fading toward red: {mid:?}");

    // Holding the colour clock at zero must pin the START colour even though
    // the value has fully crossed the threshold — proof the callback is
    // resolved at both ends rather than against the tweened datum.
    let pinned = fill_of(&charts_core::render_tween(
        &from, &to, TweenAt { value: 1.0, color: 0.0 }, rect, &charts_core::Gutters::None));
    assert_eq!(pinned, BLUE, "the colour clock governs the callback's answer");
}

/// The colour channel must be governed by its OWN clock, not by where the
/// value channel happens to be.
///
/// Regression test for a real bug. `resolve_mark_color` resolved the
/// destination end of the fade against the INTERPOLATED datum instead of the
/// target one, so with different durations per channel — the whole reason the
/// channels are split — a threshold recolor still flipped. Concretely: values
/// over 420 ms, colour over 180 ms, data animating 5 -> 15 across a `y >= 10`
/// threshold. At 180 ms the colour clock is done but the tweened value is
/// only ~40% of the way, still below 10, so the callback answered "blue" and
/// the bar stayed blue — then snapped to red later, when the interpolated
/// value crossed 10. Exactly the hard switch the colour channel exists to
/// remove, merely postponed.
///
/// It hid behind `TweenAt::uniform`, where `color == 1.0` implies
/// `value == 1.0` and the destination resolves correctly by coincidence. The
/// assertions below use DIFFERENT per-channel fractions on purpose.
#[test]
fn regression_color_channel_is_independent_of_the_value_clock() {
    let red = charts_core::Color::rgb(0xff, 0x00, 0x00);
    let threshold: StyleFn = Rc::new(move |ctx: &MarkContext| {
        if ctx.datum.y >= 10.0 {
            MarkOverride::color(red)
        } else {
            MarkOverride::default()
        }
    });
    let mk = |y: f64| {
        let mut s = ChartSpec::new(vec![Series::new(
            "a",
            SeriesKind::bar(),
            BLUE,
            vec![datum(0.0, y)],
        )]);
        s.series[0].style_fn = Some(threshold.clone());
        // A fixed domain, so the bar is a real rect at every value.
        s.y = Axis::linear().domain(charts_core::Domain::fixed(0.0, 20.0));
        s
    };
    let (from, to) = (mk(5.0), mk(15.0));
    let rect = IrRect::new(0.0, 0.0, 300.0, 200.0);

    let fill_at = |at: TweenAt| {
        let out = charts_core::render_tween(&from, &to, at, rect, &charts_core::Gutters::None);
        out.scene
            .marks
            .iter()
            .find_map(|m| match m {
                charts_core::Mark::Fill { paint: charts_core::Paint::Solid(c), layer, .. }
                    if *layer == charts_core::Layer::Series =>
                {
                    Some(*c)
                }
                _ => None,
            })
            .expect("a bar fill")
    };

    // Colour finished, value only a fifth of the way: the tweened y is 7,
    // BELOW the threshold. The colour must nonetheless be the destination's,
    // because the colour transition is over.
    assert_eq!(
        fill_at(TweenAt { value: 0.2, color: 1.0 }),
        red,
        "a completed colour clock must hold the DESTINATION colour, whatever \
         the half-animated value would resolve to"
    );

    // The mirror: value finished, colour untouched. The tweened y is 15,
    // above the threshold — but the colour clock has not started.
    assert_eq!(
        fill_at(TweenAt { value: 1.0, color: 0.0 }),
        BLUE,
        "an unstarted colour clock must hold the ORIGIN colour"
    );

    // And the fade itself is driven by the colour fraction alone, with the
    // value pinned at the origin the whole time.
    let mid = fill_at(TweenAt { value: 0.0, color: 0.5 });
    assert!(
        mid.r > BLUE.r && mid.r < red.r && mid != BLUE && mid != red,
        "half-faded on the colour clock with the value unmoved: {mid:?}"
    );
}

/// The polar path has the same two-clock contract as the cartesian one.
///
/// Sibling of `regression_color_channel_is_independent_of_the_value_clock`.
/// `PieChart` resolves its `SliceStyleFn` through a separate code path, so
/// the cartesian test cannot catch a regression here — and the identical bug
/// was present in both.
#[test]
fn regression_polar_color_channel_is_independent_of_the_value_clock() {
    let red = charts_core::Color::rgb(0xff, 0x00, 0x00);
    let mk = |v: f64| {
        PieSpec::new(vec![Slice::new("a", v, BLUE), Slice::new("b", 10.0, PINK)]).styled(
            Rc::new(move |ctx: &SliceContext<'_>| {
                if ctx.index == 0 && ctx.value >= 10.0 {
                    SliceOverride::color(red)
                } else {
                    SliceOverride::default()
                }
            }),
        )
    };
    let (from, to) = (mk(5.0), mk(15.0));
    let rect = IrRect::new(0.0, 0.0, 300.0, 300.0);

    // Slices are emitted in order, so the first fill is slice 0 — the one the
    // callback targets. The `BLUE` assertion below is what guards that
    // assumption: slice 1 is PINK, so a reordering fails loudly rather than
    // silently testing the wrong wedge. (Hit-probing straight up from the
    // centre instead would sit exactly on a wedge boundary, where the
    // later-painted slice wins.)
    let fill_at = |at: TweenAt| {
        let out = charts_core::render_pie_tween(&from, &to, at, rect);
        out.scene
            .marks
            .iter()
            .find_map(|m| match m {
                charts_core::Mark::Fill { paint: charts_core::Paint::Solid(c), .. } => Some(*c),
                _ => None,
            })
            .expect("a slice fill")
    };

    assert_eq!(
        fill_at(TweenAt { value: 0.2, color: 1.0 }),
        red,
        "a completed colour clock holds the destination colour"
    );
    assert_eq!(
        fill_at(TweenAt { value: 1.0, color: 0.0 }),
        BLUE,
        "an unstarted colour clock holds the origin colour"
    );
}
