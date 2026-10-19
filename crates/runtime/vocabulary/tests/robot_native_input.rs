//! `tap_native` / `drag_native`: the robot taps through the backend's real
//! input path (`IntrospectionOps::native_pointer`), checks the point the
//! way a finger would fail — invisible, outside the window, covered — and
//! taps through the hit test on a backend that can't synthesize input.
//!
//! host-mock stands in for the toolkit: `frames` lays elements out,
//! `covered` makes its hit test answer "something else is on top",
//! `native_input` receives the injected steps.
#![cfg(feature = "robot")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::__serde_json::{json, Value};
use runtime_shared::primitives::portal::ViewportRect;
use runtime_vocabulary::builders::{pressable, view};
use runtime_vocabulary::caps::{PointerStep, PointerStepPhase};
use runtime_vocabulary::robot::bridge::invoke_command;
use runtime_vocabulary::robot::{Query, Robot, RobotError, TapMechanism};

fn rect(x: f32, y: f32, width: f32, height: f32) -> ViewportRect {
    ViewportRect { x, y, width, height }
}

/// A 400×300 window: root view (node 0, no frame), a pressable `go`
/// (node 1) at (20, 20, 100×40), and a `scrim` view (node 2) over it.
fn fixture(h: &Harness, presses: Rc<Cell<u32>>) -> runtime_scene::Realized<host_mock::Node> {
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .child(pressable(move || presses.set(presses.get() + 1)).test_id("go").build())
                .child(view().test_id("scrim").build())
                .build(),
        )
    });
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width: 400.0, height: 300.0 });
    let mut frames = h.shared.frames.borrow_mut();
    frames.insert(1, rect(20.0, 20.0, 100.0, 40.0));
    frames.insert(2, rect(0.0, 0.0, 400.0, 300.0));
    drop(frames);
    realized
}

fn find(robot: &Robot, id: &str) -> runtime_vocabulary::robot::Element {
    robot.find(Query::test_id(id)).unwrap_or_else(|| panic!("{id} registered"))
}

/// A backend with input injection gets the tap as down + up at the
/// element's center, and runs it with the backend NOT borrowed — the
/// delivery borrows it mutably, as a toolkit's event handler running
/// author code can.
#[test]
fn a_native_tap_injects_down_and_up_at_the_center_with_the_backend_free() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let presses = Rc::new(Cell::new(0));
    let _r = fixture(&h, presses.clone());
    let got: Rc<RefCell<Vec<PointerStep>>> = Rc::default();
    {
        let (got, backend) = (got.clone(), h.backend.clone());
        *h.shared.native_input.borrow_mut() = Some(Rc::new(move |steps: &[PointerStep]| {
            let _free = backend.try_borrow_mut().map_err(|_| "the backend is still borrowed".to_string())?;
            got.borrow_mut().extend_from_slice(steps);
            Ok(())
        }));
    }

    assert_eq!(robot.tap_native(&find(&robot, "go")), Ok(TapMechanism::Native));
    assert_eq!(
        got.borrow().as_slice(),
        &[
            PointerStep { phase: PointerStepPhase::Down, x: 70.0, y: 40.0 },
            PointerStep { phase: PointerStepPhase::Up, x: 70.0, y: 40.0 },
        ]
    );
    assert_eq!(presses.get(), 0, "the press is the toolkit's to deliver, not the robot's");

    let out: Value = runtime_shared::__serde_json::from_str(
        &invoke_command("tap_native", &json!({ "element_id": find(&robot, "go").id.0 })).unwrap(),
    )
    .unwrap();
    assert_eq!(out, json!({ "mechanism": "native" }));
    robot.reset();
}

/// Something on top takes the tap: the robot refuses, naming what is
/// there — as Playwright reports an element that intercepts the click.
#[test]
fn a_covered_element_refuses_the_tap_and_names_what_covers_it() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let presses = Rc::new(Cell::new(0));
    let _r = fixture(&h, presses.clone());
    h.shared.covered.borrow_mut().insert(1);

    let err = robot.tap_native(&find(&robot, "go")).unwrap_err();
    assert_eq!(
        err,
        RobotError::Covered { action: "tap_native", x: 70.0, y: 40.0, by: Some("View test_id=\"scrim\"".into()) }
    );
    assert!(err.to_string().contains("View test_id=\"scrim\" would receive the tap"), "{err}");
    assert_eq!(presses.get(), 0);

    // Uncovered (the scrim went away): through the hit test, the press runs.
    h.shared.covered.borrow_mut().clear();
    assert_eq!(robot.tap_native(&find(&robot, "go")), Ok(TapMechanism::HitTest));
    assert_eq!(presses.get(), 1);
    robot.reset();
}

/// What a finger can't reach is refused: an invisible element, a center
/// outside the window; with no frame at all there is nowhere to tap.
#[test]
fn unreachable_elements_are_refused() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let presses = Rc::new(Cell::new(0));
    let _r = fixture(&h, presses.clone());

    h.shared.hidden.borrow_mut().insert(1);
    assert_eq!(robot.tap_native(&find(&robot, "go")), Err(RobotError::NotVisible("tap_native")));
    h.shared.hidden.borrow_mut().clear();

    h.shared.frames.borrow_mut().insert(1, rect(20.0, 290.0, 100.0, 40.0));
    let err = robot.tap_native(&find(&robot, "go")).unwrap_err();
    assert_eq!(err, RobotError::OutsideWindow { action: "tap_native", x: 70.0, y: 310.0 });
    assert!(err.to_string().contains("scroll the element into view"), "{err}");

    h.shared.frames.borrow_mut().remove(&1);
    assert_eq!(robot.tap_native(&find(&robot, "go")), Err(RobotError::Unsupported("tap_native")));
    assert_eq!(presses.get(), 0);
    robot.reset();
}

/// `drag_native` takes element-relative points, like `drag`, and injects
/// down / moves / up in window coordinates; without injection it is
/// unsupported (no hit-test tier for a gesture).
#[test]
fn a_native_drag_injects_the_path_in_window_coordinates() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = fixture(&h, Rc::new(Cell::new(0)));
    let go = find(&robot, "go");
    assert_eq!(robot.drag_native(&go, &[(5.0, 5.0), (50.0, 5.0)]), Err(RobotError::Unsupported("drag_native")));

    let got: Rc<RefCell<Vec<PointerStep>>> = Rc::default();
    {
        let got = got.clone();
        *h.shared.native_input.borrow_mut() = Some(Rc::new(move |steps: &[PointerStep]| {
            got.borrow_mut().extend_from_slice(steps);
            Ok(())
        }));
    }
    robot.drag_native(&go, &[(5.0, 5.0), (30.0, 5.0), (50.0, 10.0)]).unwrap();
    let step = |phase, x, y| PointerStep { phase, x, y };
    assert_eq!(
        got.borrow().as_slice(),
        &[
            step(PointerStepPhase::Down, 25.0, 25.0),
            step(PointerStepPhase::Move, 50.0, 25.0),
            step(PointerStepPhase::Move, 70.0, 30.0),
            step(PointerStepPhase::Up, 70.0, 30.0),
        ]
    );

    // A refusal from the platform is the action's error.
    *h.shared.native_input.borrow_mut() = Some(Rc::new(|_: &[PointerStep]| Err("no window".to_string())));
    assert_eq!(robot.tap_native(&go), Err(RobotError::Native("no window".into())));
    robot.reset();
}

/// `hover_native` moves the pointer over the element's center with no
/// button held; `press_native` / `release_native` split a tap so a test can
/// check the pressed state between them. Same point checks as a tap, and no
/// fallback where the backend can't inject (a touch platform can't hover).
#[test]
fn hover_and_split_press_inject_their_own_steps() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = fixture(&h, Rc::new(Cell::new(0)));
    let got: Rc<RefCell<Vec<PointerStep>>> = Rc::default();
    {
        let got = got.clone();
        *h.shared.native_input.borrow_mut() = Some(Rc::new(move |steps: &[PointerStep]| {
            got.borrow_mut().extend_from_slice(steps);
            Ok(())
        }));
    }
    let go = find(&robot, "go");
    assert_eq!(robot.hover_native(&go), Ok(()));
    assert_eq!(robot.press_native(&go), Ok(()));
    assert_eq!(robot.release_native(&go), Ok(()));
    let at = |phase| PointerStep { phase, x: 70.0, y: 40.0 };
    assert_eq!(
        got.borrow().as_slice(),
        &[at(PointerStepPhase::Hover), at(PointerStepPhase::Down), at(PointerStepPhase::Up)]
    );
    assert!(invoke_command("hover_native", &json!({ "element_id": go.id.0 })).is_ok(), "bridge verb");

    h.shared.covered.borrow_mut().insert(1);
    assert!(matches!(robot.hover_native(&go), Err(RobotError::Covered { action: "hover_native", .. })));
    h.shared.covered.borrow_mut().clear();

    *h.shared.native_input.borrow_mut() = None;
    assert_eq!(robot.hover_native(&go), Err(RobotError::Unsupported("hover_native")));
    assert_eq!(robot.press_native(&go), Err(RobotError::Unsupported("press_native")));
}
