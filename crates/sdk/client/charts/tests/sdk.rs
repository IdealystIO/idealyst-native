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
        SeriesKind::Line { width: 2.0, smooth: false, dash: vec![6.0, 4.0], points: false },
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

/// The tree is [root [ plot_row [ y_gutter, plot [ canvas, tooltip ] ] ],
/// x_gutter ]. Pinning it matters because the gutters' *existence* is what
/// lets the framework lay out labels — a refactor that collapsed them would
/// silently move label positioning back into the canvas.
#[test]
fn builds_the_gutter_plus_plot_tree() {
    with_chart(ChartProps { spec: line_spec().into(), ..Default::default() }, |el| {
        let root = children_of(el);
        assert_eq!(root.len(), 2, "plot row + x-axis gutter");
        assert!(is_reactive_hole(&root[1]), "x-axis labels rebuild reactively");

        let row = children_of(&root[0]);
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
        let row = children_of(&children_of(el)[0]);
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
            assert_eq!(children_of(el).len(), 2);
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
        assert_eq!(children_of(&outer[0]).len(), 2, "plot row + x gutter");
    });
}
