//! The robot reads a primitive's own state, and acts on a control as a
//! user would: a disabled control refuses, and an action moves the native
//! widget before its change callback reports it.
//!
//! Before this, element JSON carried only `{id, kind, test_id, label}` — a
//! test could not read a field's text, a toggle's checked state or whether
//! a control was disabled without the app exposing a signal — and every
//! robot action called the author's RAW callback, ahead of the disabled
//! gate and the slider's step snap. So a robot could type into a disabled
//! field, flip a disabled toggle and press a disabled button, and a test
//! could never check that "disabled" blocks input.
#![cfg(feature = "robot")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::__serde_json::{self as serde_json, json};
use runtime_vocabulary::builders::{button, pressable, slider, text_area, text_input, toggle, view};
use runtime_vocabulary::robot::bridge::invoke_command;
use runtime_vocabulary::robot::{ElementState, Query, Robot, RobotError, StateValue};
use runtime_world::signal;

fn harness() -> Harness {
    let h = Harness::new();
    h.record_all();
    h
}

fn sink<T: 'static>() -> (Rc<RefCell<Vec<T>>>, impl Fn(T) + 'static) {
    let got = Rc::new(RefCell::new(Vec::new()));
    let g = got.clone();
    (got, move |v: T| g.borrow_mut().push(v))
}

fn state(robot: &Robot, test_id: &str) -> ElementState {
    let el = robot.find(Query::test_id(test_id)).unwrap_or_else(|| panic!("{test_id} registered"));
    robot.state(&el).expect("element live").unwrap_or_else(|| panic!("{test_id} reports state"))
}

fn ops(h: &Harness, prefix: &str) -> Vec<String> {
    h.ops().into_iter().filter(|l| l.starts_with(prefix)).collect()
}

/// Every robot action on a disabled control fails with `Disabled` and never
/// reaches the author — the robot used to call the raw callbacks, so all of
/// these went through.
#[test]
fn regression_robot_actions_refuse_a_disabled_control() {
    let robot = Robot::new();
    robot.reset();
    let h = harness();
    let (toggled, on_toggle) = sink::<bool>();
    let (slid, on_slide) = sink::<f32>();
    let (typed, on_type) = sink::<String>();
    let (area_typed, on_area) = sink::<String>();
    let presses = Rc::new(Cell::new(0));
    let clicks = Rc::new(Cell::new(0));
    let (p, c) = (presses.clone(), clicks.clone());
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .child(toggle().test_id("t").value(false).on_change(on_toggle).disabled(true).build())
                .child(slider().test_id("s").value(0.0).on_change(on_slide).disabled(true).build())
                .child(text_input().test_id("i").value("").on_change(on_type).disabled(true).build())
                .child(text_area().test_id("a").value("").on_change(on_area).disabled(true).build())
                .child(pressable(move || p.set(p.get() + 1)).test_id("p").disabled(true).build())
                .child(button().test_id("b").label("Go").on_press(move || c.set(c.get() + 1)).disabled(true).build())
                .build(),
        )
    });
    let find = |id: &str| robot.find(Query::test_id(id)).unwrap();

    assert_eq!(robot.set_toggle(&find("t"), true), Err(RobotError::Disabled("set_toggle")));
    assert_eq!(robot.set_slider(&find("s"), 1.0), Err(RobotError::Disabled("set_slider")));
    assert_eq!(robot.type_text(&find("i"), "hi"), Err(RobotError::Disabled("type_text")));
    assert_eq!(robot.type_text(&find("a"), "hi"), Err(RobotError::Disabled("type_text")));
    assert_eq!(robot.focus(&find("i")), Err(RobotError::Disabled("focus")));
    assert_eq!(robot.click(&find("p")), Err(RobotError::Disabled("click")));
    assert_eq!(robot.click(&find("b")), Err(RobotError::Disabled("click")));

    assert!(toggled.borrow().is_empty() && slid.borrow().is_empty());
    assert!(typed.borrow().is_empty() && area_typed.borrow().is_empty());
    assert_eq!((presses.get(), clicks.get()), (0, 0), "no press reached the author");
    for id in ["t", "s", "i", "a", "p", "b"] {
        assert_eq!(state(&robot, id).disabled, Some(true), "{id} reports disabled");
    }
    robot.reset();
}

/// The disabled state follows a live source, and the robot follows it: a
/// control that becomes enabled takes the action.
#[test]
fn a_control_enabled_later_takes_the_robot_action() {
    let robot = Robot::new();
    robot.reset();
    let h = harness();
    let (toggled, on_toggle) = sink::<bool>();
    let (disabled, _realized) = h.world.enter(|| {
        let disabled = signal(true);
        let realized = realize(
            &h.backend,
            &h.registry,
            toggle().test_id("t").value(false).on_change(on_toggle).disabled(move || disabled.get()).build(),
        );
        (disabled, realized)
    });
    let t = robot.find(Query::test_id("t")).unwrap();
    assert_eq!(robot.set_toggle(&t, true), Err(RobotError::Disabled("set_toggle")));

    h.world.enter(|| disabled.set(false));
    h.flush();
    assert_eq!(state(&robot, "t").disabled, Some(false));
    robot.set_toggle(&t, true).expect("an enabled toggle flips");
    assert_eq!(*toggled.borrow(), vec![true]);
    robot.reset();
}

/// State is what the control shows: an uncontrolled toggle shows what it
/// was flipped to (the native switch moves with the robot's flip), a
/// controlled one shows the author's value after each write-back.
#[test]
fn toggle_state_is_what_the_switch_shows() {
    let robot = Robot::new();
    robot.reset();
    let h = harness();
    let (checked, _realized) = h.world.enter(|| {
        let checked = signal(false);
        let realized = realize(
            &h.backend,
            &h.registry,
            view()
                .child(toggle().test_id("free").value(false).on_change(|_| {}).build())
                .child(toggle().test_id("bound").value(move || checked.get()).on_change(|_| {}).build())
                .build(),
        );
        (checked, realized)
    });
    assert_eq!(state(&robot, "free").checked, Some(false));

    h.clear_ops();
    robot.set_toggle(&robot.find(Query::test_id("free")).unwrap(), true).unwrap();
    assert_eq!(state(&robot, "free").checked, Some(true), "uncontrolled: the flip shows");
    assert_eq!(ops(&h, "update_toggle_value"), vec!["update_toggle_value n1 true"], "the switch moved");

    h.world.enter(|| checked.set(true));
    h.flush();
    assert_eq!(state(&robot, "bound").checked, Some(true), "controlled: the write-back shows");
    robot.reset();
}

/// `set_slider` goes through the slider's own change path: clamped to the
/// track, snapped to `step`, the thumb moved — the author sees what a drag
/// would give them, not the raw number the robot was handed.
#[test]
fn regression_set_slider_snaps_and_clamps_like_a_drag() {
    let robot = Robot::new();
    robot.reset();
    let h = harness();
    let (slid, on_slide) = sink::<f32>();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            slider().test_id("s").value(0.0).range(0.0, 10.0).step(0.5).on_change(on_slide).build(),
        )
    });
    let s = robot.find(Query::test_id("s")).unwrap();
    h.clear_ops();
    robot.set_slider(&s, 3.3).unwrap();
    robot.set_slider(&s, 42.0).unwrap();
    assert_eq!(*slid.borrow(), vec![3.5, 10.0], "snapped to the step, clamped to max");
    assert_eq!(state(&robot, "s").value, Some(StateValue::Number(10.0)));
    assert_eq!(ops(&h, "update_slider_value"), vec!["update_slider_value n0 3.5", "update_slider_value n0 10"]);
    robot.reset();
}

/// A text field reports its text and focus: typed text, a controlled
/// write-back, and the focus the field's notifier reports — passed on to
/// the author's `on_focus` too.
#[test]
fn text_input_state_reports_text_and_focus() {
    let robot = Robot::new();
    robot.reset();
    let h = harness();
    let (focus_seen, on_focus) = sink::<bool>();
    let (text, _realized) = h.world.enter(|| {
        let text = signal(String::from("start"));
        let realized = realize(
            &h.backend,
            &h.registry,
            view()
                .child(text_input().test_id("free").value("").on_change(|_| {}).on_focus(on_focus).build())
                .child(text_input().test_id("bound").value(move || text.get()).on_change(|_| {}).build())
                .child(text_area().test_id("area").value("").on_change(|_| {}).build())
                .build(),
        );
        (text, realized)
    });
    let free = robot.find(Query::test_id("free")).unwrap();
    h.clear_ops();
    robot.type_text(&free, "hello").unwrap();
    assert_eq!(state(&robot, "free").value, Some(StateValue::Text("hello".into())));
    assert_eq!(ops(&h, "update_text_input_value"), vec!["update_text_input_value n1 \"hello\""], "the field shows it");
    assert_eq!(state(&robot, "bound").value, Some(StateValue::Text("start".into())));
    h.world.enter(|| text.set("changed".into()));
    h.flush();
    assert_eq!(state(&robot, "bound").value, Some(StateValue::Text("changed".into())));

    // Focus: the field's own notifier (the mock fires it by hand, as a
    // native field would when it takes focus).
    assert_eq!(state(&robot, "free").focused, Some(false));
    let notifier = h.shared.focus_handlers.borrow()[0].1.clone();
    notifier(true);
    assert_eq!(state(&robot, "free").focused, Some(true));
    assert_eq!(*focus_seen.borrow(), vec![true], "the author's on_focus still hears it");

    // A text area now takes focus / blur too.
    let area = robot.find(Query::test_id("area")).unwrap();
    robot.focus(&area).expect("text_area focus");
    robot.blur(&area).expect("text_area blur");
    robot.type_text(&area, "lines").unwrap();
    assert_eq!(state(&robot, "area").value, Some(StateValue::Text("lines".into())));
    robot.reset();
}

/// Over the bridge: `get_state`, and the same state on `find_element` and
/// in the snapshot. A stateless element's JSON is unchanged.
#[test]
fn bridge_reports_state_on_elements_and_through_get_state() {
    let robot = Robot::new();
    robot.reset();
    let h = harness();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .test_id("root")
                .child(toggle().test_id("t").value(true).on_change(|_| {}).build())
                .build(),
        )
    });
    let found: serde_json::Value =
        serde_json::from_str(&invoke_command("find_element", &json!({"test_id": "t"})).unwrap()).unwrap();
    assert_eq!(found["state"], json!({"checked": true, "disabled": false}));
    let got: serde_json::Value = serde_json::from_str(
        &invoke_command("get_state", &json!({"element_id": found["id"]})).unwrap(),
    )
    .unwrap();
    assert_eq!(got, json!({"checked": true, "disabled": false}));

    let root: serde_json::Value =
        serde_json::from_str(&invoke_command("find_element", &json!({"test_id": "root"})).unwrap()).unwrap();
    assert!(root.get("state").is_none(), "a view has no state field: {root}");
    assert_eq!(
        invoke_command("get_state", &json!({"element_id": root["id"]})).unwrap(),
        "{}"
    );

    let snapshot: serde_json::Value =
        serde_json::from_str(&invoke_command("get_snapshot", &json!({})).unwrap()).unwrap();
    assert_eq!(snapshot[0]["children"][0]["state"]["checked"], json!(true));

    let disabled_err = {
        let _r2 = h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                toggle().test_id("off").value(false).on_change(|_| {}).disabled(true).build(),
            )
        });
        let off: serde_json::Value =
            serde_json::from_str(&invoke_command("find_element", &json!({"test_id": "off"})).unwrap()).unwrap();
        invoke_command("set_toggle", &json!({"element_id": off["id"], "value": true})).unwrap_err()
    };
    assert!(disabled_err.contains("disabled"), "{disabled_err}");
    robot.reset();
}
