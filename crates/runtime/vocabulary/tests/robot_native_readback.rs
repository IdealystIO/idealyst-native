//! `Robot::introspect_native` separates a backend with no native read from an
//! element a backend with one can't read yet.
#![cfg(feature = "robot")]

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::__serde_json::{json, Value};
use runtime_shared::introspect::{NativeNode, NativeRect};
use runtime_vocabulary::builders::view;
use runtime_vocabulary::robot::bridge::invoke_command;
use runtime_vocabulary::robot::{Query, Robot, RobotError};

fn leaf() -> NativeNode {
    NativeNode::leaf("MockView", NativeRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 })
}

fn call(verb: &str, element_id: u32) -> Value {
    let out = invoke_command(verb, &json!({ "element_id": element_id })).expect(verb);
    runtime_shared::__serde_json::from_str(&out).expect("json")
}

/// A backend with the read answers `None` only until the element is laid out; a
/// check of the widget retries that rather than skipping.
#[test]
fn a_backend_that_reads_answers_none_until_laid_out() {
    let h = Harness::new();
    h.shared.native_readback.set(true);
    let _realized = h.world.enter(|| realize(&h.backend, &h.registry, view().test_id("v").build()));
    let robot = Robot::new();
    let el = robot.find(Query::test_id("v")).expect("registered");

    assert_eq!(robot.introspect_native(&el).expect("the backend reads"), None, "not laid out yet");
    assert_eq!(call("introspect_native", el.id.0), Value::Null);

    h.shared.native_trees.borrow_mut().insert(0, leaf());
    assert_eq!(robot.introspect_native(&el).expect("the backend reads"), Some(leaf()));
    assert_eq!(call("introspect_native", el.id.0)["class"], "MockView");
}

/// A check of the native widget skipped whenever the read answered `None`, so a
/// backend that claimed the read and stopped answering looked like one that never
/// had it: a skip, never a failure. No read at all now reports as that, and the
/// bridge's parity verbs keep treating it as no data rather than an error.
#[test]
fn regression_native_check_skip_hid_a_backend_that_stopped_reading() {
    let h = Harness::new();
    let _realized = h.world.enter(|| realize(&h.backend, &h.registry, view().test_id("v").build()));
    let robot = Robot::new();
    let el = robot.find(Query::test_id("v")).expect("registered");

    assert!(matches!(robot.introspect_native(&el), Err(RobotError::ActionNotAvailable(_))));
    assert_eq!(call("introspect_native", el.id.0), Value::Null);
    assert_eq!(call("introspect_subtree", el.id.0), json!({}));
}
