//! `Canvas3d` through the scene registry on the host-mock substrate: the size
//! seam, both painters, the pick handle, the orbit handlers driving repaints,
//! and the SSR host.
//!
//! The test renderer does what every real renderer does: paint through
//! `Canvas3dPrim::paint()` inside a subtree-owned effect, and report the size
//! it learns through `size_reporter()`. (Without an installed scheduler the
//! deferred size write is staged at `report` time and committed by `flush`.)

use std::cell::RefCell;

use canvas3d_core::prelude::*;
use canvas3d_core::{Canvas3dPrim, Frame3d};
use canvas_core::SizeReporter;
use host_mock::Harness;
use runtime_scene::{MountCx, Realized};
use runtime_shared::{TouchEvent, TouchId, TouchPhase, TouchPoint, WheelEvent, WheelKind};
use runtime_vocabulary::caps::DocumentOps;
use runtime_vocabulary::glue::IntoElement;

thread_local! {
    static REPORTER: RefCell<Option<SizeReporter>> = const { RefCell::new(None) };
    static FRAMES: RefCell<Vec<Frame3d>> = const { RefCell::new(Vec::new()) };
}

fn harness() -> Harness {
    FRAMES.with(|f| f.borrow_mut().clear());
    Harness::with_registry(|r| {
        r.register::<Canvas3dPrim, _>(|cx: &mut MountCx<'_, _>, prim, _children| {
            let node = cx.backend().borrow_mut().create_element("canvas");
            REPORTER.with(|s| *s.borrow_mut() = Some(prim.size_reporter()));
            let prim = prim.clone();
            runtime_world::effect(move || {
                let frame = prim.paint();
                FRAMES.with(|f| f.borrow_mut().push(frame));
            });
            node
        });
    })
}

fn report(w: f32, h: f32) {
    REPORTER.with(|s| s.borrow().clone()).expect("renderer stored a reporter").report(w, h);
}

fn frames() -> usize {
    FRAMES.with(|f| f.borrow().len())
}

fn with_last<R>(f: impl FnOnce(&Frame3d) -> R) -> R {
    FRAMES.with(|fr| f(fr.borrow().last().expect("at least one frame")))
}

fn cube() -> Model {
    Model::from_mesh(MeshData::cube(1.0), Material::default())
}

#[test]
fn reported_size_reaches_scene_and_overlay_and_repaints() {
    let h = harness();
    let el = h.world.enter(|| {
        Canvas3d(Canvas3dProps {
            draw: canvas3d_core::draw(|_| {}),
            overlay: Some(canvas_core::draw(|_| {})),
            ..Default::default()
        })
        .into_element()
    });
    let _r: Realized<u32> = h.mount(el);
    h.flush();
    assert_eq!(with_last(|f| f.scene.size()), (0.0, 0.0), "first paint runs before layout");

    report(320.0, 240.0);
    h.flush();
    with_last(|f| {
        assert_eq!(f.scene.size(), (320.0, 240.0));
        assert_eq!(f.overlay.as_ref().map(|o| o.size()), Some((320.0, 240.0)), "overlay painted at the same size");
    });
    let n = frames();
    report(320.0, 240.0);
    h.flush();
    assert_eq!(frames(), n, "an unchanged size does not repaint");
}

#[test]
fn no_overlay_painter_means_no_overlay_scene() {
    let h = harness();
    let el = h.world.enter(|| Canvas3d(Canvas3dProps::default()).into_element());
    let _r: Realized<u32> = h.mount(el);
    h.flush();
    assert!(with_last(|f| f.overlay.is_none()));
}

#[test]
fn handle_picks_against_the_last_painted_scene() {
    let h = harness();
    let handle = Canvas3dHandle::new();
    let model = cube();
    let el = h.world.enter(|| {
        let model = model.clone();
        Canvas3d(Canvas3dProps {
            draw: canvas3d_core::draw(move |s| {
                s.camera(Camera::look_at(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO));
                s.model(&model, Mat4::IDENTITY).pick_id(42);
            }),
            handle: Some(handle.clone()),
            ..Default::default()
        })
        .into_element()
    });
    let _r: Realized<u32> = h.mount(el);
    h.flush();
    assert_eq!(handle.pick(0.0, 0.0), None, "a 0×0 view has nothing under any point");
    report(400.0, 300.0);
    h.flush();
    assert_eq!(handle.size(), (400.0, 300.0));
    assert_eq!(handle.pick(200.0, 150.0).map(|hit| hit.pick_id), Some(42));
    assert_eq!(handle.pick(5.0, 5.0), None);
}

fn touch(id: u64, phase: TouchPhase, x: f32, y: f32) -> TouchEvent {
    TouchEvent {
        id: TouchId(id),
        phase,
        position: TouchPoint::new(x, y),
        window_position: TouchPoint::new(x, y),
        timestamp_ns: 0,
        force: None,
    }
}

/// The orbit controller end to end: a drag through its `on_touch` handler
/// writes the camera signal, the painter (which reads it) re-runs, and the
/// new frame's camera has moved.
#[test]
fn orbit_drag_moves_the_painted_camera() {
    let h = harness();
    let handle = Canvas3dHandle::new();
    let (el, orbit) = h.world.enter(|| {
        let orbit = OrbitCamera::new(OrbitConfig { pitch: 0.0, ..Default::default() });
        let o = orbit.clone();
        let el = Canvas3d(Canvas3dProps {
            draw: canvas3d_core::draw(move |s| {
                s.camera(o.camera());
            }),
            handle: Some(handle.clone()),
            ..Default::default()
        })
        .into_element();
        (el, orbit)
    });
    let _r: Realized<u32> = h.mount(el);
    report(400.0, 300.0);
    h.flush();
    let eye0 = with_last(|f| f.scene.get_camera().eye);

    let on_touch = h.world.enter(|| orbit.touch_handler(&handle));
    let r = on_touch(&touch(1, TouchPhase::Began, 100.0, 100.0));
    assert!(r.consumed && !r.claim, "consume at Began, claim only once it moves");
    let r = on_touch(&touch(1, TouchPhase::Moved, 160.0, 100.0));
    assert!(r.consumed && r.claim);
    h.flush();
    let eye1 = with_last(|f| f.scene.get_camera().eye);
    assert!(eye1.x < eye0.x - 0.1, "dragging right swung the camera left: {eye0} → {eye1}");
    assert!(((eye1 - Vec3::ZERO).length() - 5.0).abs() < 1e-3, "orbit keeps the distance");

    on_touch(&touch(1, TouchPhase::Ended, 160.0, 100.0));
    assert_eq!(on_touch(&touch(9, TouchPhase::Moved, 0.0, 0.0)).consumed, false, "untracked pointers bubble");
}

#[test]
fn two_finger_pinch_dollies() {
    let h = harness();
    let handle = Canvas3dHandle::new();
    let orbit = h.world.enter(|| OrbitCamera::new(OrbitConfig::default()));
    let on_touch = h.world.enter(|| orbit.touch_handler(&handle));
    let d0 = orbit.state().distance;
    on_touch(&touch(1, TouchPhase::Began, 100.0, 100.0));
    on_touch(&touch(2, TouchPhase::Began, 200.0, 100.0));
    // Spread doubles → zoom in → half the distance.
    on_touch(&touch(2, TouchPhase::Moved, 300.0, 100.0));
    h.flush();
    let d1 = orbit.state().distance;
    assert!((d1 - d0 * 0.5).abs() < 1e-3, "{d0} → {d1}");
}

#[test]
fn wheel_scroll_and_pinch_dolly_the_right_way() {
    let h = harness();
    let orbit = h.world.enter(|| OrbitCamera::new(OrbitConfig::default()));
    let on_wheel = h.world.enter(|| orbit.wheel_handler());
    let wheel = |kind, delta_y, scale| WheelEvent {
        kind,
        delta_x: 0.0,
        delta_y,
        scale,
        rotation: 0.0,
        position: TouchPoint::ZERO,
        window_position: TouchPoint::ZERO,
        timestamp_ns: 0,
    };
    let d0 = orbit.state().distance;
    assert!(on_wheel(&wheel(WheelKind::Scroll, 100.0, 1.0)).consumed);
    h.flush();
    let d1 = orbit.state().distance;
    assert!(d1 > d0, "scrolling down moves away");
    on_wheel(&wheel(WheelKind::Zoom, 0.0, 2.0));
    h.flush();
    assert!((orbit.state().distance - d1 * 0.5).abs() < 1e-3, "pinch-out ×2 halves the distance");
    assert!(!on_wheel(&wheel(WheelKind::Rotate, 0.0, 1.0)).consumed, "rotation is left for others");
}

#[test]
fn ssr_host_mounts_a_real_canvas_element() {
    let h = Harness::with_registry(|r| canvas3d_core::register_ssr(r));
    let el = Canvas3d(Canvas3dProps::default()).into_element();
    let _r: Realized<u32> = h.mount(el);
    let log = h.ops();
    assert_eq!(log.first().map(String::as_str), Some("create n0 element \"canvas\""), "{log:?}");
}

/// Draw order and payload of what the painter emitted survive into the frame
/// (the renderer reads exactly this).
#[test]
fn painted_scene_carries_models_lines_and_lights() {
    let h = harness();
    let grid = Lines::grid(2.0, 1.0);
    let model = cube();
    let el = h.world.enter(|| {
        let (grid, model) = (grid.clone(), model.clone());
        Canvas3d(Canvas3dProps {
            draw: canvas3d_core::draw(move |s| {
                s.clear(Color::BLACK)
                    .ambient(Color::WHITE, 0.2)
                    .light(Light::directional(Vec3::NEG_Y, Color::WHITE, 2.0))
                    .lines(&grid, Color::GRAY, LineDepth::Tested);
                s.model(&model, Mat4::IDENTITY);
            }),
            ..Default::default()
        })
        .into_element()
    });
    let _r: Realized<u32> = h.mount(el);
    h.flush();
    with_last(|f| {
        assert_eq!(f.scene.clear_color(), Color::BLACK);
        assert_eq!(f.scene.get_lights().len(), 1);
        assert_eq!(f.scene.line_batches().len(), 1);
        assert_eq!(f.scene.models()[0].model.id(), model.id());
    });
}

#[test]
fn renderer_info_is_reported_reactively() {
    let h = harness();
    let handle = h.world.enter(Canvas3dHandle::new);
    let seen = std::rc::Rc::new(RefCell::new(Vec::new()));
    let el = h.world.enter(|| {
        let (handle, seen) = (handle.clone(), seen.clone());
        Canvas3d(Canvas3dProps {
            draw: canvas3d_core::draw(|_| {}),
            overlay: Some(canvas_core::draw(move |_| seen.borrow_mut().push(handle.renderer_info()))),
            ..Default::default()
        })
        .into_element()
    });
    let _r: Realized<u32> = h.mount(el);
    h.flush();
    handle.report_renderer("WebGL2");
    h.flush();
    assert_eq!(seen.borrow().first(), Some(&None));
    assert_eq!(seen.borrow().last(), Some(&Some("WebGL2".to_string())), "the overlay re-ran with the info");
}

/// Android delivers several motion events in one dispatch turn, so orbit
/// writes are staged, not yet committed, when the next event reads the state.
/// Reading the COMMITTED value there (`peek`) dropped every delta but the last
/// in a burst — a fast drag under-rotated (seen live on the Android emulator
/// as an `idealyst[staged-read]` warning). Two moves of 30 px in one turn must
/// orbit exactly as far as one move of 60 px.
#[test]
fn regression_android_batched_moves_compose_in_one_turn() {
    let h = harness();
    let handle = Canvas3dHandle::new();
    let cfg = OrbitConfig::default();
    let (batched, single) = h.world.enter(|| (OrbitCamera::new(cfg), OrbitCamera::new(cfg)));

    let on_touch = h.world.enter(|| batched.touch_handler(&handle));
    on_touch(&touch(1, TouchPhase::Began, 100.0, 100.0));
    h.flush();
    on_touch(&touch(1, TouchPhase::Moved, 130.0, 100.0));
    on_touch(&touch(1, TouchPhase::Moved, 160.0, 100.0)); // same turn: no flush between
    h.flush();

    let on_touch = h.world.enter(|| single.touch_handler(&handle));
    on_touch(&touch(1, TouchPhase::Began, 100.0, 100.0));
    h.flush();
    on_touch(&touch(1, TouchPhase::Moved, 160.0, 100.0));
    h.flush();

    assert!((batched.state().yaw - single.state().yaw).abs() < 1e-5, "{} vs {}", batched.state().yaw, single.state().yaw);
}

/// Same for the wheel: two notches in one turn dolly twice.
#[test]
fn regression_batched_wheel_events_compose_in_one_turn() {
    let h = harness();
    let orbit = h.world.enter(|| OrbitCamera::new(OrbitConfig::default()));
    let on_wheel = h.world.enter(|| orbit.wheel_handler());
    let notch = WheelEvent {
        kind: WheelKind::Zoom,
        delta_x: 0.0,
        delta_y: 0.0,
        scale: 2.0,
        rotation: 0.0,
        position: TouchPoint::ZERO,
        window_position: TouchPoint::ZERO,
        timestamp_ns: 0,
    };
    let d0 = orbit.state().distance;
    on_wheel(&notch);
    on_wheel(&notch);
    h.flush();
    assert!((orbit.state().distance - d0 * 0.25).abs() < 1e-4, "{d0} → {}", orbit.state().distance);
}
