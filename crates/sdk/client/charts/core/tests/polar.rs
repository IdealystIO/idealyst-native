//! Pie, donut, gauge, and radial bar charts.
//!
//! Angles throughout follow the crate convention: radians, clockwise, zero
//! at twelve o'clock. Author-facing fields are degrees.

use std::f32::consts::TAU;

use charts_core::polar::point_on;
use charts_core::scene::PathSeg;
use charts_core::svg::scene_to_svg;
use charts_core::*;

const BLUE: Color = Color::rgb(0x4c, 0x8d, 0xff);
const PINK: Color = Color::rgb(0xff, 0x6b, 0x9d);
const MINT: Color = Color::rgb(0x3d, 0xd5, 0x98);
const AMBER: Color = Color::rgb(0xff, 0xb0, 0x3d);
const TEXT: Color = Color::rgb(0x33, 0x33, 0x33);
const SIZE: (f32, f32) = (360.0, 360.0);

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

fn quarters() -> PieSpec {
    PieSpec::new(vec![
        Slice::new("a", 1.0, BLUE),
        Slice::new("b", 1.0, PINK),
        Slice::new("c", 1.0, MINT),
        Slice::new("d", 1.0, AMBER),
    ])
}

/// A point well inside the ring at `degrees` clockwise from twelve
/// o'clock. Deliberately not on a boundary — 0.8 of the radius is inside
/// every donut these tests build, so a hit or miss reflects the angle under
/// test rather than float rounding at an edge.
fn probe(out: &PolarOutput, degrees: f32) -> Point {
    point_on(out.center, out.radius * 0.8, degrees.to_radians())
}

fn dist(a: Point, b: Point) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

#[test]
fn equal_slices_take_equal_quarters() {
    let out = render_pie(&quarters(), surface());
    for (i, deg) in [45.0, 135.0, 225.0, 315.0].iter().enumerate() {
        let hit = out.hit.contains(probe(&out, *deg)).expect("inside the ring");
        assert_eq!(hit.index, i, "the slice at {deg}° is #{i}");
    }
}

#[test]
fn slices_sweep_in_proportion_to_value() {
    // 3:1 — the first slice must own three quarters of the circle.
    let spec = PieSpec::new(vec![Slice::new("a", 3.0, BLUE), Slice::new("b", 1.0, PINK)]);
    let out = render_pie(&spec, surface());
    assert_eq!(out.hit.contains(probe(&out, 269.0)).unwrap().index, 0);
    assert_eq!(out.hit.contains(probe(&out, 271.0)).unwrap().index, 1);
}

#[test]
fn the_start_angle_rotates_the_whole_ring() {
    let out = render_pie(&quarters().start_angle(90.0), surface());
    // Everything shifted a quarter turn clockwise: what was at 45° is now
    // at 135°.
    assert_eq!(out.hit.contains(probe(&out, 135.0)).unwrap().index, 0);
}

#[test]
fn a_donut_has_a_hole() {
    let out = render_pie(&PieSpec::donut(quarters().slices, 0.6), surface());
    let center_ish = point_on(out.center, out.radius * 0.2, 0.5);
    assert!(out.hit.contains(center_ish).is_none(), "the hole is not a slice");
    assert!(out.hit.contains(probe(&out, 45.0)).is_some(), "the ring still is");
}

#[test]
fn a_pie_stays_circular_in_a_wide_rect() {
    // Sized from the SHORTER side, so equal angles keep subtending equal
    // area — an ellipse would misrepresent every share.
    let out = render_pie(&quarters(), Rect::new(0.0, 0.0, 800.0, 200.0));
    assert!((out.radius - 100.0 * 0.9).abs() < 0.01);
}

#[test]
fn non_positive_slices_are_not_drawn_and_do_not_count() {
    let spec = PieSpec::new(vec![
        Slice::new("a", 1.0, BLUE),
        Slice::new("negative", -5.0, PINK),
        Slice::new("zero", 0.0, MINT),
        Slice::new("b", 1.0, AMBER),
    ]);
    let out = render_pie(&spec, surface());
    assert_eq!(out.scene.marks.len(), 2, "only the two real shares");
    // And each got half the circle, not a quarter — the discarded values
    // must not dilute the denominator.
    assert_eq!(out.hit.contains(probe(&out, 90.0)).unwrap().index, 0);
    assert_eq!(out.hit.contains(probe(&out, 270.0)).unwrap().index, 3);
}

#[test]
fn a_hidden_slice_is_excluded_from_the_total() {
    let mut spec = quarters();
    spec.slices[3].visible = false;
    let out = render_pie(&spec, surface());
    // Three visible slices now split the circle into thirds.
    assert_eq!(out.hit.contains(probe(&out, 60.0)).unwrap().index, 0);
    assert_eq!(out.hit.contains(probe(&out, 180.0)).unwrap().index, 1);
    assert_eq!(out.hit.contains(probe(&out, 300.0)).unwrap().index, 2);
}

/// Padding insets each slice; it must not shift where one share ends and the
/// next begins, or the geometry would stop matching the numbers.
#[test]
fn pad_angle_insets_without_moving_the_boundary() {
    let padded = render_pie(&quarters().pad_angle(6.0), surface());
    // Well inside each slice, the answer is unchanged.
    assert_eq!(padded.hit.contains(probe(&padded, 45.0)).unwrap().index, 0);
    // In the gap itself, nothing is hit — the slices genuinely pulled back.
    assert!(padded.hit.contains(probe(&padded, 90.0)).is_none());
}

#[test]
fn a_slice_thinner_than_the_padding_keeps_its_sweep() {
    // A 1% share on a chart with a 20° pad would otherwise be inset out of
    // existence — the opposite of what a chart is for.
    let spec = PieSpec::new(vec![Slice::new("big", 99.0, BLUE), Slice::new("tiny", 1.0, PINK)])
        .pad_angle(20.0);
    let out = render_pie(&spec, surface());
    assert_eq!(out.scene.marks.len(), 2, "the tiny slice survives");
}

#[test]
fn a_semicircle_uses_only_its_own_sweep() {
    let out = render_pie(&quarters().total_angle(180.0).start_angle(-90.0), surface());
    // 180° starting at nine o'clock: the bottom half is empty.
    assert!(out.hit.contains(probe(&out, 180.0)).is_none());
    assert!(out.hit.contains(probe(&out, 0.0)).is_some());
}

// ---------------------------------------------------------------------------
// Hit-testing
// ---------------------------------------------------------------------------

/// The reason wedges are stored as wedges. With one dominant slice, the
/// nearest CENTROID over most of that slice's own area belongs to its small
/// neighbour — so a point-based index answers confidently and wrongly.
#[test]
fn regression_a_centroid_index_picks_the_wrong_slice() {
    let spec = PieSpec::new(vec![Slice::new("bulk", 35.0, BLUE), Slice::new("rest", 1.0, PINK)]);
    let out = render_pie(&spec, surface());
    let p = probe(&out, 340.0); // deep inside `bulk`, near its far edge

    // `bulk` sweeps 0..350° so its centroid is at 175° — diametrically
    // opposite the pointer — while `rest`'s sits at 355°, right beside it.
    let bulk_centroid = point_on(out.center, out.radius / 2.0, 175f32.to_radians());
    let rest_centroid = point_on(out.center, out.radius / 2.0, 355f32.to_radians());
    assert!(
        dist(p, rest_centroid) < dist(p, bulk_centroid),
        "the pointer really is nearer the WRONG slice's centroid"
    );

    // Containment is unmoved by that, which is the whole point.
    assert_eq!(out.hit.contains(p).unwrap().index, 0);
    // And `nearest` agrees, because it tests containment before falling back
    // to distance — so the fix reaches every query, not just the new one.
    assert_eq!(out.hit.nearest(p).unwrap().index, 0);
}

#[test]
fn outside_the_ring_is_not_a_hit() {
    let out = render_pie(&quarters(), surface());
    let far = point_on(out.center, out.radius * 1.5, 0.5);
    assert!(out.hit.contains(far).is_none());
}

#[test]
fn a_hit_carries_the_slice_index_and_value() {
    let spec = PieSpec::new(vec![Slice::new("a", 2.0, BLUE), Slice::new("b", 6.0, PINK)]);
    let out = render_pie(&spec, surface());
    let hit = out.hit.contains(probe(&out, 180.0)).expect("hit");
    assert_eq!(hit.index, 1);
    assert_eq!(hit.datum.y, 6.0, "the slice's value");
}

#[test]
fn a_full_ring_is_hit_all_the_way_round() {
    let spec = RadialSpec::new(vec![RadialBar::new("done", 40.0, BLUE)]);
    let out = render_radial(&spec, surface());
    let rc = out.radius - spec.thickness / 2.0;
    for deg in [0.0, 90.0, 180.0, 270.0, 359.0] {
        let p = point_on(out.center, rc, (deg as f32).to_radians());
        assert!(out.hit.contains(p).is_some(), "the ring is hit at {deg}°");
    }
}

// ---------------------------------------------------------------------------
// Emphasis
// ---------------------------------------------------------------------------

#[test]
fn an_emphasised_slice_grows_outward() {
    let plain = render_pie(&quarters(), surface());
    let hovered = render_pie(
        &quarters().highlight(SliceHighlight::hovered(0)).hover_grow(10.0),
        surface(),
    );
    let far = |out: &PolarOutput, nth: usize| -> f32 {
        let path = match &out.scene.marks[nth] {
            Mark::Fill { path, .. } => path.clone(),
            _ => panic!("expected a wedge"),
        };
        path.segs
            .iter()
            .filter_map(|s| match s {
                PathSeg::MoveTo(a) | PathSeg::LineTo(a) | PathSeg::CubicTo(_, _, a) => {
                    Some(((a.x - out.center.x).powi(2) + (a.y - out.center.y).powi(2)).sqrt())
                }
                _ => None,
            })
            .fold(0.0f32, f32::max)
    };
    assert!(far(&hovered, 0) > far(&plain, 0) + 9.0, "the hovered slice grew");
    assert!((far(&hovered, 1) - far(&plain, 1)).abs() < 0.01, "its neighbour did not");
}

#[test]
fn dim_others_fades_only_the_unemphasised() {
    let spec = quarters().highlight(SliceHighlight::hovered(0).dim_others(true));
    let out = render_pie(&spec, surface());
    let alpha = |nth: usize| match &out.scene.marks[nth] {
        Mark::Fill { paint: Paint::Solid(c), .. } => c.a,
        _ => panic!("expected a solid wedge"),
    };
    assert_eq!(alpha(0), 255, "the hovered slice keeps full opacity");
    assert!(alpha(1) < 255, "the others are faded");
}

#[test]
fn a_style_callback_can_recolor_one_slice() {
    let f: SliceStyleFn = std::rc::Rc::new(|ctx: &SliceContext<'_>| {
        // The share is handed in precomputed, which is the number a
        // conditional format actually asks about.
        if ctx.fraction < 0.3 {
            SliceOverride::color(Color::rgb(0x99, 0x99, 0x99))
        } else {
            SliceOverride::default()
        }
    });
    let spec = PieSpec::new(vec![Slice::new("a", 9.0, BLUE), Slice::new("b", 1.0, PINK)])
        .styled(f);
    let out = render_pie(&spec, surface());
    let color = |nth: usize| match &out.scene.marks[nth] {
        Mark::Fill { paint: Paint::Solid(c), .. } => *c,
        _ => panic!("expected a solid wedge"),
    };
    assert_eq!(color(0), BLUE);
    assert_eq!(color(1), Color::rgb(0x99, 0x99, 0x99));
}

/// Two specs sharing one `Rc` compare equal; rebuilding the closure does
/// not. Hosts memoise on this, so getting it wrong re-renders every tick.
#[test]
fn style_callbacks_compare_by_identity() {
    let f: SliceStyleFn = std::rc::Rc::new(|_: &SliceContext<'_>| SliceOverride::default());
    let g: SliceStyleFn = std::rc::Rc::new(|_: &SliceContext<'_>| SliceOverride::default());
    let base = || PieSpec::new(vec![Slice::new("a", 1.0, BLUE)]);
    assert_eq!(base().styled(f.clone()), base().styled(f.clone()));
    assert_ne!(base().styled(f), base().styled(g));
}

// ---------------------------------------------------------------------------
// Radial bars and gauges
// ---------------------------------------------------------------------------

#[test]
fn a_gauge_sweeps_its_share_of_the_range() {
    let spec = RadialSpec::gauge("cpu", 25.0, 100.0, BLUE);
    assert_eq!(spec.fraction(25.0), 0.25);
    let out = render_radial(&spec, surface());
    // Track plus bar.
    assert_eq!(out.scene.marks.len(), 2);
}

/// An arc past a full turn wraps and overwrites itself, so an over-max value
/// would read as a SMALLER one. Clamping at least reads as "at the top".
#[test]
fn a_value_beyond_the_range_clamps_rather_than_wrapping() {
    let spec = RadialSpec::gauge("cpu", 400.0, 100.0, BLUE);
    assert_eq!(spec.fraction(400.0), 1.0);
    assert_eq!(spec.fraction(-50.0), 0.0);
}

#[test]
fn a_zero_valued_bar_draws_only_its_track() {
    let spec = RadialSpec::new(vec![RadialBar::new("none", 0.0, BLUE)]);
    let out = render_radial(&spec, surface());
    assert_eq!(out.scene.marks.len(), 1, "the track, and nothing else");
}

#[test]
fn rings_are_laid_out_from_the_outside_in() {
    let spec = RadialSpec::new(vec![
        RadialBar::new("outer", 60.0, BLUE),
        RadialBar::new("inner", 40.0, PINK),
    ])
    .track(None);
    let out = render_radial(&spec, surface());
    let radius_of = |nth: usize| match &out.scene.marks[nth] {
        Mark::Stroke { path, .. } => match path.segs[0] {
            PathSeg::MoveTo(a) => {
                ((a.x - out.center.x).powi(2) + (a.y - out.center.y).powi(2)).sqrt()
            }
            _ => panic!("expected a move"),
        },
        _ => panic!("expected a stroke"),
    };
    assert!(radius_of(0) > radius_of(1), "the first bar is the outer ring");
}

/// A hover that reflows every other ring reads as a glitch, so an
/// emphasised ring thickens in place.
#[test]
fn hovering_a_ring_does_not_move_the_others() {
    let bars = vec![
        RadialBar::new("a", 60.0, BLUE),
        RadialBar::new("b", 40.0, PINK),
        RadialBar::new("c", 20.0, MINT),
    ];
    let plain = render_radial(&RadialSpec::new(bars.clone()).track(None), surface());
    let hovered = render_radial(
        &RadialSpec::new(bars).track(None).highlight(SliceHighlight::hovered(0)),
        surface(),
    );
    let radius_of = |out: &PolarOutput, nth: usize| match &out.scene.marks[nth] {
        Mark::Stroke { path, .. } => match path.segs[0] {
            PathSeg::MoveTo(a) => {
                ((a.x - out.center.x).powi(2) + (a.y - out.center.y).powi(2)).sqrt()
            }
            _ => panic!("expected a move"),
        },
        _ => panic!("expected a stroke"),
    };
    for nth in 1..3 {
        assert!(
            (radius_of(&plain, nth) - radius_of(&hovered, nth)).abs() < 0.01,
            "ring {nth} stayed put"
        );
    }
    let width_of = |out: &PolarOutput, nth: usize| match &out.scene.marks[nth] {
        Mark::Stroke { stroke, .. } => stroke.width,
        _ => panic!("expected a stroke"),
    };
    assert!(width_of(&hovered, 0) > width_of(&plain, 0), "but it did thicken");
}

/// The pointer over a ring is asking what that ring's value is, and it asks
/// just as much from the empty part — the same reasoning as the cartesian
/// highlight band covering a whole slot.
#[test]
fn the_empty_part_of_a_ring_still_reports_its_value() {
    let spec = RadialSpec::gauge("cpu", 10.0, 100.0, BLUE);
    let out = render_radial(&spec, surface());
    let rc = out.radius - spec.thickness / 2.0;
    // Near the far end of the 270° sweep, well past where the 10% arc stops.
    let p = point_on(out.center, rc, (-135.0f32 + 260.0).to_radians());
    let hit = out.hit.contains(p).expect("the track is hit");
    assert_eq!(hit.datum.y, 10.0);
}

#[test]
fn rings_stop_when_they_run_out_of_room() {
    // Twenty rings cannot fit in 360px at 14+6 each; the renderer must stop
    // rather than draw degenerate inner rings on top of each other.
    let bars: Vec<RadialBar> =
        (0..20).map(|i| RadialBar::new(format!("r{i}"), 50.0, BLUE)).collect();
    let out = render_radial(&RadialSpec::new(bars).track(None), surface());
    assert!(out.scene.marks.len() < 20);
    assert!(!out.scene.marks.is_empty());
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

#[test]
fn a_pie_tween_starts_at_the_source_and_lands_on_the_destination() {
    let from = PieSpec::new(vec![Slice::new("a", 1.0, BLUE), Slice::new("b", 3.0, PINK)]);
    let to = PieSpec::new(vec![Slice::new("a", 3.0, BLUE), Slice::new("b", 1.0, PINK)]);
    assert_eq!(lerp_pie(&from, &to, TweenAt::uniform(0.0)).unwrap().slices[0].value, 1.0);
    assert_eq!(lerp_pie(&from, &to, TweenAt::uniform(1.0)).unwrap().slices[0].value, 3.0);
    let mid = lerp_pie(&from, &to, TweenAt::uniform(0.5)).unwrap().slices[0].value;
    assert!(mid > 1.0 && mid < 3.0, "and passes between them");
}

#[test]
fn a_pie_tween_with_a_different_slice_count_snaps() {
    let from = PieSpec::new(vec![Slice::new("a", 1.0, BLUE)]);
    let to = PieSpec::new(vec![Slice::new("a", 1.0, BLUE), Slice::new("b", 1.0, PINK)]);
    assert!(lerp_pie(&from, &to, TweenAt::uniform(0.5)).is_none());
    // The renderer still produces the destination rather than nothing.
    let out = render_pie_tween(&from, &to, TweenAt::uniform(0.5), surface());
    assert_eq!(out.scene.marks.len(), 2);
}

/// A gauge whose max changes mid-transition would otherwise snap its whole
/// scale on frame one while the arc glided — the same glitch the cartesian
/// tween interpolates the domain to avoid.
#[test]
fn a_radial_tween_interpolates_the_range_too() {
    let from = RadialSpec::gauge("x", 50.0, 100.0, BLUE);
    let to = RadialSpec::gauge("x", 50.0, 200.0, BLUE);
    let mid = lerp_radial(&from, &to, TweenAt::uniform(0.5)).unwrap();
    assert!(mid.max > 100.0 && mid.max < 200.0);
}

/// Slice colors ride the COLOR clock, independently of the value clock.
///
/// This used to assert the opposite — that a recolor snapped to the
/// destination on frame one. It was changed deliberately: a slice sliding to
/// a new share while its color jumped read as a glitch, and the two channels
/// now have their own `Transition` exactly so a host can pick the balance.
#[test]
fn slice_colors_ride_the_color_clock() {
    let from = PieSpec::new(vec![Slice::new("a", 1.0, BLUE)]);
    let to = PieSpec::new(vec![Slice::new("a", 2.0, PINK)]);

    // Settled ends are exact — an animation must not leave a rounding
    // residue on the color it lands on.
    assert_eq!(lerp_pie(&from, &to, TweenAt::uniform(0.0)).unwrap().slices[0].color, BLUE);
    assert_eq!(lerp_pie(&from, &to, TweenAt::uniform(1.0)).unwrap().slices[0].color, PINK);

    let mid = lerp_pie(&from, &to, TweenAt::uniform(0.5)).unwrap().slices[0].color;
    assert_ne!(mid, BLUE);
    assert_ne!(mid, PINK);
    assert!(
        mid.r > BLUE.r.min(PINK.r) && mid.r < BLUE.r.max(PINK.r),
        "the midpoint lies between the endpoints: {mid:?}"
    );

    // The two clocks are genuinely independent: holding color at zero while
    // the value runs must leave the color untouched.
    let split = lerp_pie(&from, &to, TweenAt { value: 1.0, color: 0.0 }).unwrap();
    assert_eq!(split.slices[0].color, BLUE, "color pinned at the `from` end");
    assert_eq!(split.slices[0].value, 2.0, "…while the value has fully arrived");
}

// ---------------------------------------------------------------------------
// Degenerate input
// ---------------------------------------------------------------------------

#[test]
fn an_empty_pie_renders_an_empty_but_valid_scene() {
    let out = render_pie(&PieSpec::new(Vec::new()), surface());
    assert!(out.scene.marks.is_empty());
    assert!(out.hit.is_empty());
}

#[test]
fn a_zero_area_rect_renders_nothing() {
    let out = render_pie(&quarters(), Rect::new(0.0, 0.0, 0.0, 0.0));
    assert!(out.scene.marks.is_empty());
    let out = render_radial(&RadialSpec::gauge("x", 1.0, 2.0, BLUE), Rect::new(0.0, 0.0, 0.0, 0.0));
    assert!(out.scene.marks.is_empty());
}

#[test]
fn a_degenerate_range_does_not_divide_by_zero() {
    let spec = RadialSpec::new(vec![RadialBar::new("x", 5.0, BLUE)]).range(3.0, 3.0);
    assert_eq!(spec.fraction(5.0), 0.0);
    let out = render_radial(&spec, surface());
    assert!(out.scene.marks.iter().all(|m| matches!(m, Mark::Stroke { .. })));
}

#[test]
fn a_full_turn_is_a_closed_ring() {
    let spec = PieSpec::donut(vec![Slice::new("only", 1.0, BLUE)], 0.5);
    let out = render_pie(&spec, surface());
    // One slice covering the whole circle: hit anywhere in the band.
    for deg in [0.0f32, 90.0, 180.0, 270.0] {
        assert!(out.hit.contains(probe(&out, deg)).is_some(), "hit at {deg}°");
    }
    // And the hole is still a hole.
    assert!(out.hit.contains(point_on(out.center, out.radius * 0.1, 0.0)).is_none());
}

#[test]
fn the_angle_convention_is_clockwise_from_twelve() {
    let c = pt(0.0, 0.0);
    let up = point_on(c, 10.0, 0.0);
    assert!(up.x.abs() < 0.001 && (up.y + 10.0).abs() < 0.001, "0 is straight up");
    let right = point_on(c, 10.0, TAU / 4.0);
    assert!((right.x - 10.0).abs() < 0.001 && right.y.abs() < 0.001, "a quarter turn is right");
}

// ---------------------------------------------------------------------------
// Goldens
// ---------------------------------------------------------------------------

#[test]
fn golden_donut() {
    let spec = PieSpec::donut(
        vec![
            Slice::new("direct", 38.0, BLUE),
            Slice::new("search", 27.0, PINK),
            Slice::new("social", 21.0, MINT),
            Slice::new("email", 14.0, AMBER),
        ],
        0.62,
    )
    .pad_angle(2.0)
    .center("1,284")
    .center_sub("sessions");
    let out = render_pie(&spec, surface());
    check_golden("donut", &scene_to_svg(&out.scene, SIZE, TEXT));
}

#[test]
fn golden_pie_with_leader_labels() {
    let spec = PieSpec::new(vec![
        Slice::new("alpha", 44.0, BLUE),
        Slice::new("beta", 30.0, PINK),
        Slice::new("gamma", 18.0, MINT).explode(0.08),
        Slice::new("delta", 8.0, AMBER),
    ])
    .labels(PieLabels::Leader);
    let out = render_pie(&spec, surface());
    check_golden("pie_leader", &scene_to_svg(&out.scene, SIZE, TEXT));
}

#[test]
fn golden_gauge() {
    let spec = RadialSpec::gauge("throughput", 72.0, 100.0, MINT)
        .center("72%")
        .center_sub("of capacity");
    let out = render_radial(&spec, surface());
    check_golden("gauge", &scene_to_svg(&out.scene, SIZE, TEXT));
}

#[test]
fn golden_radial_bars() {
    let spec = RadialSpec::new(vec![
        RadialBar::new("api", 86.0, BLUE),
        RadialBar::new("web", 64.0, PINK),
        RadialBar::new("jobs", 41.0, MINT),
        RadialBar::new("cache", 23.0, AMBER),
    ])
    .thickness(20.0)
    .gap(8.0)
    .labels(true);
    let out = render_radial(&spec, surface());
    check_golden("radial_bars", &scene_to_svg(&out.scene, SIZE, TEXT));
}

/// A round cap extends the arc backwards past its nominal start angle by
/// half the ring thickness. Measuring the label gap from the start angle
/// alone therefore put the text underneath its own cap.
#[test]
fn regression_a_ring_label_clears_its_own_round_cap() {
    let spec = RadialSpec::new(vec![RadialBar::new("api", 86.0, BLUE)])
        .thickness(20.0)
        .labels(true);
    let out = render_radial(&spec, surface());
    let label = out
        .scene
        .labels
        .iter()
        .find(|l| l.text == "api")
        .expect("ring label");

    // The cap is a half-disc of radius thickness/2 centred on the arc's
    // start point; anything within that distance is covered by it.
    let rc = out.radius - spec.thickness / 2.0;
    let arc_start = point_on(out.center, rc, spec.start_angle.to_radians());
    assert!(
        dist(label.anchor, arc_start) > spec.thickness / 2.0,
        "label is {}px from the arc start, inside the {}px cap",
        dist(label.anchor, arc_start),
        spec.thickness / 2.0
    );
}
