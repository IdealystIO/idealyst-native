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
/// hover fallback where the backend can't inject (a touch platform can't
/// hover); a split press there goes through the hit test (see
/// `a_held_press_through_the_hit_test_*`).
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
    assert_eq!(robot.press_native(&go), Ok(TapMechanism::Native));
    assert_eq!(robot.release_native(&go), Ok(TapMechanism::Native));
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
    // No injection and no frame (so no hit test either): nothing can press.
    h.shared.frames.borrow_mut().remove(&1);
    assert_eq!(robot.press_native(&go), Err(RobotError::Unsupported("press_native")));
}

/// CrewForge E2E report #5: a test id on a transparent wrapper view whose
/// child is the pressable (idea-ui `Button` with a reactive `loading`).
/// Web's real input lands on the pressable under the wrapper's center; the
/// hit-test tier (iOS) pressed the ADDRESSED wrapper instead and answered
/// "action 'click' not available". It must press what the hit test finds
/// at the point — and only that: a second pressable in the wrapper, away
/// from the center, stays untouched.
///
/// The iOS tier itself (UIKit `hitTest:` in `hit_contains_impl`) needs a
/// simulator; this exercises the shared decision against host-mock's hit
/// test, which is the same `IntrospectionOps::hit_contains` contract.
#[test]
fn regression_tap_native_on_wrapper_presses_hit_tested_child() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let (side, center) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let _r = {
        let (side, center) = (side.clone(), center.clone());
        h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                view()
                    .child(
                        view()
                            .test_id("login-submit")
                            .child(pressable(move || side.set(side.get() + 1)).test_id("side").build())
                            .child(pressable(move || center.set(center.get() + 1)).test_id("center").build())
                            .build(),
                    )
                    .build(),
            )
        })
    };
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width: 400.0, height: 300.0 });
    {
        // root 0, wrapper 1 (20,20 200×40; center 120,40), `side` 2 on its
        // left end, `center` 3 under the wrapper's center.
        let mut frames = h.shared.frames.borrow_mut();
        frames.insert(1, rect(20.0, 20.0, 200.0, 40.0));
        frames.insert(2, rect(20.0, 20.0, 40.0, 40.0));
        frames.insert(3, rect(80.0, 20.0, 80.0, 40.0));
    }
    let wrapper = find(&robot, "login-submit");
    assert_eq!(robot.click(&wrapper), Err(RobotError::ActionNotAvailable("click")), "the wrapper has no press of its own");

    assert_eq!(robot.tap_native(&wrapper), Ok(TapMechanism::HitTest));
    assert_eq!((center.get(), side.get()), (1, 0), "the pressable under the center takes the tap");

    // Over the bridge, as the MCP tool / robot-test `Locator::click` drive it.
    let out: Value = runtime_shared::__serde_json::from_str(
        &invoke_command("tap_native", &json!({ "element_id": wrapper.id.0 })).unwrap(),
    )
    .unwrap();
    assert_eq!(out, json!({ "mechanism": "hit_test" }));
    assert_eq!(center.get(), 2);

    // The refusal stays about the point: covered there, nothing is pressed.
    h.shared.covered.borrow_mut().insert(1);
    assert!(matches!(robot.tap_native(&wrapper), Err(RobotError::Covered { .. })));
    assert_eq!((center.get(), side.get()), (2, 0));
    robot.reset();
}

/// Regression (CrewForge, iOS): a `tap_native` through the hit-test tier on
/// an element with nothing pressable at or above it — a list row that is
/// plain text — failed with "action 'click' not available", where the same
/// tap on web (real events) lands and simply does nothing. A finger on
/// non-interactive content is not an error: the tap succeeds, nothing runs.
#[test]
fn regression_hit_test_tap_on_non_interactive_content_lands_without_error() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = h.world.enter(|| {
        realize(&h.backend, &h.registry, view().child(view().test_id("plain-row").build()).build())
    });
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width: 400.0, height: 300.0 });
    h.shared.frames.borrow_mut().insert(1, rect(20.0, 20.0, 200.0, 40.0));
    let row = find(&robot, "plain-row");
    assert_eq!(robot.click(&row), Err(RobotError::ActionNotAvailable("click")), "click still says there is no press");
    assert_eq!(robot.tap_native(&row), Ok(TapMechanism::HitTest));
    // The refusal stays about the point.
    h.shared.covered.borrow_mut().insert(1);
    assert!(matches!(robot.tap_native(&row), Err(RobotError::Covered { .. })));
    robot.reset();
}

/// `context_click` injects ONE secondary-button click at the element's
/// center — the backend turns it into its platform's right-click sequence —
/// and reports the native mechanism over the bridge.
#[test]
fn a_context_click_injects_a_secondary_click_at_the_center() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let presses = Rc::new(Cell::new(0));
    let _r = fixture(&h, presses.clone());
    let got: Rc<RefCell<Vec<PointerStep>>> = Rc::default();
    {
        let got = got.clone();
        *h.shared.native_input.borrow_mut() = Some(Rc::new(move |steps: &[PointerStep]| {
            got.borrow_mut().extend_from_slice(steps);
            Ok(())
        }));
    }
    let go = find(&robot, "go");
    assert_eq!(robot.context_click_native(&go), Ok(TapMechanism::Native));
    assert_eq!(
        got.borrow().as_slice(),
        &[PointerStep { phase: PointerStepPhase::SecondaryClick, x: 70.0, y: 40.0 }]
    );
    let out: Value = runtime_shared::__serde_json::from_str(
        &invoke_command("context_click", &json!({ "element_id": go.id.0 })).unwrap(),
    )
    .unwrap();
    assert_eq!(out, json!({ "mechanism": "native" }));
    assert_eq!(presses.get(), 0, "a right click is not a press");

    // The same point checks as a tap.
    h.shared.covered.borrow_mut().insert(1);
    assert!(matches!(robot.context_click_native(&go), Err(RobotError::Covered { action: "context_click", .. })));
    robot.reset();
}

/// CrewForge E2E report B5: a right-click menu (the Schedule grid's "Mark
/// as absent") is an `on_touch` that opens on a `PointerButton::Secondary`
/// `Began`, and no verb could send one. Through the hit-test tier (a
/// backend that can't inject a secondary click) the robot delivers that
/// `Began` to the handler under the point, reporting `Secondary` while it
/// runs, bubbling past a handler that ignores it — and never runs a
/// pressable's press.
#[test]
fn regression_context_click_reaches_a_secondary_press_handler() {
    use runtime_shared::{PointerButton, TouchEvent, TouchPhase, TouchResponse};
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let seen: Rc<RefCell<Vec<(TouchPhase, PointerButton)>>> = Rc::default();
    let presses = Rc::new(Cell::new(0));
    let _r = {
        let (seen, presses) = (seen.clone(), presses.clone());
        h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                view()
                    .child(
                        view()
                            .test_id("cell")
                            .on_touch(Rc::new(move |e: &TouchEvent| {
                                seen.borrow_mut().push((e.phase, runtime_shared::pointer_button()));
                                if runtime_shared::pointer_button().opens_context_menu() {
                                    TouchResponse::CONSUMED
                                } else {
                                    TouchResponse::IGNORED
                                }
                            }))
                            .child(
                                view()
                                    .test_id("label")
                                    .on_touch(Rc::new(|_: &TouchEvent| TouchResponse::IGNORED))
                                    .build(),
                            )
                            .build(),
                    )
                    .child(pressable(move || presses.set(presses.get() + 1)).test_id("btn").build())
                    .build(),
            )
        })
    };
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width: 400.0, height: 300.0 });
    {
        // root 0, cell 1, label 2 inside it, btn 3 beside.
        let mut frames = h.shared.frames.borrow_mut();
        frames.insert(1, rect(0.0, 0.0, 200.0, 50.0));
        frames.insert(2, rect(10.0, 10.0, 100.0, 30.0));
        frames.insert(3, rect(0.0, 100.0, 100.0, 40.0));
    }
    let label = find(&robot, "label");
    assert_eq!(robot.context_click_native(&label), Ok(TapMechanism::HitTest));
    assert_eq!(seen.borrow().as_slice(), &[(TouchPhase::Began, PointerButton::Secondary)]);

    let out: Value = runtime_shared::__serde_json::from_str(
        &invoke_command("context_click", &json!({ "element_id": find(&robot, "cell").id.0 })).unwrap(),
    )
    .unwrap();
    assert_eq!(out, json!({ "mechanism": "hit_test" }));
    assert_eq!(seen.borrow().len(), 2, "Began-only, one per click");

    // A pressable swallows a secondary press: nothing runs.
    assert_eq!(robot.context_click_native(&find(&robot, "btn")), Ok(TapMechanism::HitTest));
    assert_eq!(presses.get(), 0);
    robot.reset();
}

/// On a backend that can't inject (iOS), `press_native` held the press and
/// never told the style's state machine, so `to_be_pressed` timed out on a
/// control a real finger lights. The press lights the element under the point
/// and its ancestors, as a real touch reaches every recognizer up the chain,
/// and the release clears them.
#[test]
fn regression_ios_press_through_the_hit_test_never_lit_pressed() {
    use runtime_shared::{StyleRules, StyleSheet};
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let width = |px: f32| StyleRules { width: Some(runtime_shared::Length::Px(px).into()), ..Default::default() };
    let sheet = move || {
        Rc::new(
            StyleSheet::new(move |_| width(10.0))
                .variant("__state_hovered", "on", move |_| width(20.0))
                .variant("__state_pressed", "on", move |_| width(30.0)),
        )
    };
    let presses = Rc::new(Cell::new(0));
    let _r = {
        let presses = presses.clone();
        h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                view()
                    .test_id("card")
                    .style(sheet())
                    .child(pressable(move || presses.set(presses.get() + 1)).test_id("go").style(sheet()).build())
                    .child(view().test_id("aside").style(sheet()).build())
                    .build(),
            )
        })
    };
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width: 400.0, height: 300.0 });
    {
        let mut frames = h.shared.frames.borrow_mut();
        frames.insert(0, rect(0.0, 0.0, 400.0, 300.0));
        frames.insert(1, rect(20.0, 20.0, 100.0, 40.0));
        frames.insert(2, rect(20.0, 100.0, 100.0, 40.0));
    }
    let pressed = |id: &str| robot.state(&find(&robot, id)).unwrap().unwrap().pressed;
    assert_eq!(pressed("go"), Some(false));

    let go = find(&robot, "go");
    assert_eq!(robot.press_native(&go), Ok(TapMechanism::HitTest));
    assert_eq!(pressed("go"), Some(true), "the control under the finger");
    assert_eq!(pressed("card"), Some(true), "its ancestor's recognizer sees the touch too");
    assert_eq!(pressed("aside"), Some(false), "a sibling does not");
    assert_eq!(presses.get(), 0, "a press runs on release");

    assert_eq!(robot.release_native(&go), Ok(TapMechanism::HitTest));
    assert_eq!(pressed("go"), Some(false));
    assert_eq!(pressed("card"), Some(false));
    assert_eq!(presses.get(), 1);

    // A press abandoned by the next one does not stay lit.
    let aside = find(&robot, "aside");
    assert_eq!(robot.press_native(&go), Ok(TapMechanism::HitTest));
    assert_eq!(robot.press_native(&aside), Ok(TapMechanism::HitTest));
    assert_eq!(pressed("go"), Some(false));
    assert_eq!(pressed("aside"), Some(true));
    robot.reset();
}

/// A long press on a backend that can't inject (iOS): `press_native`
/// delivers `Began` through the hit test and HOLDS the stream; real time
/// passes between the two verbs (the test sleeps, as a robot-test
/// `long_press` does); `release_native` delivers `Ended` stamped with that
/// real time. A pressable under the point is pressed on the release.
#[test]
fn a_held_press_through_the_hit_test_spans_real_time() {
    use runtime_shared::{TouchEvent, TouchPhase, TouchResponse};
    // A real monotonic clock, as every native host installs at mount.
    runtime_shared::time::install_time_source(Box::new(runtime_shared::time::InstantTimeSource::new()));
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let events: Rc<RefCell<Vec<(TouchPhase, u64)>>> = Rc::default();
    let presses = Rc::new(Cell::new(0));
    let _r = {
        let (events, presses) = (events.clone(), presses.clone());
        h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                view()
                    .child(
                        view()
                            .test_id("row")
                            .on_touch(Rc::new(move |e: &TouchEvent| {
                                events.borrow_mut().push((e.phase, e.timestamp_ns));
                                TouchResponse::CONSUMED
                            }))
                            .build(),
                    )
                    .child(pressable(move || presses.set(presses.get() + 1)).test_id("btn").build())
                    .build(),
            )
        })
    };
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width: 400.0, height: 300.0 });
    {
        let mut frames = h.shared.frames.borrow_mut();
        frames.insert(1, rect(0.0, 0.0, 200.0, 50.0));
        frames.insert(2, rect(0.0, 100.0, 100.0, 40.0));
    }
    let row = find(&robot, "row");
    assert_eq!(robot.press_native(&row), Ok(TapMechanism::HitTest));
    assert_eq!(events.borrow().iter().map(|e| e.0).collect::<Vec<_>>(), [TouchPhase::Began]);
    std::thread::sleep(std::time::Duration::from_millis(60));
    let out: Value = runtime_shared::__serde_json::from_str(
        &invoke_command("release_native", &json!({ "element_id": row.id.0 })).unwrap(),
    )
    .unwrap();
    assert_eq!(out, json!({ "mechanism": "hit_test" }));
    let ev = events.borrow().clone();
    assert_eq!(ev.iter().map(|e| e.0).collect::<Vec<_>>(), [TouchPhase::Began, TouchPhase::Ended]);
    assert!(ev[1].1 - ev[0].1 >= 60_000_000, "the hold is as long as the real time that passed: {ev:?}");

    let btn = find(&robot, "btn");
    assert_eq!(robot.press_native(&btn), Ok(TapMechanism::HitTest));
    assert_eq!(presses.get(), 0, "a press runs on release, as a finger lifting does");
    assert_eq!(robot.release_native(&btn), Ok(TapMechanism::HitTest));
    assert_eq!(presses.get(), 1);
    robot.reset();
}
