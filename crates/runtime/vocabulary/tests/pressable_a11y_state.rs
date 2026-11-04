//! A pressable's live accessibility state: a checkbox built on `pressable`
//! tells assistive tech whether it is checked, and keeps telling it as the
//! value changes, without rebuilding the node.
#![cfg(feature = "robot")]

use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::accessibility::{AccessibilityProps, AccessibilityTraits, Role};
use runtime_vocabulary::builders::pressable;
use runtime_vocabulary::robot::{Query, Robot};
use runtime_world::signal;

fn checkbox_bag(label: &str) -> AccessibilityProps {
    AccessibilityProps {
        label: Some(label.into()),
        role: Some(Role::Checkbox),
        hint: Some("kept across updates".into()),
        traits: AccessibilityTraits::REQUIRED,
        ..Default::default()
    }
}

fn checked(robot: &Robot, test_id: &str) -> Option<bool> {
    let element = robot.find(Query::test_id(test_id)).expect("registered");
    robot.state(&element).expect("live").expect("a pressable reports state").checked
}

#[test]
fn regression_a_checkbox_pressable_reports_its_checked_state_live() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let value = h.world.enter(|| signal(true));
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            pressable(|| {})
                .a11y(checkbox_bag("Include archived"))
                .a11y_state(move || AccessibilityTraits::checked_if(value.get()))
                .test_id("box")
                .build(),
        )
    });
    let held = || h.a11y_by_label("Include archived").expect("the pressable holds its bag");
    let robot_checked = || checked(&robot, "box");

    // The first value is in the bag `create_pressable` gets, so a backend
    // that only reads it there (SSR) prints it, with no extra write.
    assert!(held().traits.contains(AccessibilityTraits::CHECKED));
    assert_eq!(h.a11y_update_count(), 0, "no update for the value already created with");
    assert_eq!(robot_checked(), Some(true));

    h.world.enter(|| value.set(false));
    h.flush();
    let bag = held();
    assert!(!bag.traits.contains(AccessibilityTraits::CHECKED), "the write reached the host");
    assert!(bag.traits.contains(AccessibilityTraits::REQUIRED), "static traits stay");
    assert_eq!(bag.role, Some(Role::Checkbox), "the whole bag is re-sent");
    assert_eq!(bag.hint.as_deref(), Some("kept across updates"));
    assert_eq!(h.a11y_update_count(), 1);
    assert_eq!(robot_checked(), Some(false));

    h.world.enter(|| value.set(true));
    h.flush();
    assert!(held().traits.contains(AccessibilityTraits::CHECKED));
    assert_eq!(h.a11y_update_count(), 2);
    assert_eq!(robot_checked(), Some(true));
    robot.reset();
}

/// Rebuilding the element to change a trait would drop keyboard focus: the
/// binding must update the node it already has.
#[test]
fn regression_a_state_change_keeps_the_same_node() {
    let h = Harness::new();
    let value = h.world.enter(|| signal(false));
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            pressable(|| {})
                .a11y(checkbox_bag("x"))
                .a11y_state(move || AccessibilityTraits::checked_if(value.get()))
                .build(),
        )
    });
    h.clear_ops();
    h.world.enter(|| value.set(true));
    h.flush();
    let ops = h.ops();
    assert!(
        !ops.iter().any(|op| op.contains("create") || op.contains("remove")),
        "no node is created or removed: {ops:?}"
    );
    assert!(h.a11y_by_label("x").unwrap().traits.contains(AccessibilityTraits::CHECKED));
}

/// An equal recompute is not a change: a screen reader re-announces on a
/// write, so a binding reading a noisy signal must stay quiet.
#[test]
fn an_unchanged_state_writes_nothing() {
    let h = Harness::new();
    let n = h.world.enter(|| signal(1));
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            pressable(|| {})
                .a11y(checkbox_bag("x"))
                .a11y_state(move || AccessibilityTraits::checked_if(n.get() > 0))
                .build(),
        )
    });
    h.world.enter(|| n.set(2));
    h.flush();
    assert_eq!(h.a11y_update_count(), 0);
}

#[test]
fn mixed_reads_as_mixed_on_the_host_and_not_checked_to_the_robot() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            pressable(|| {})
                .a11y(checkbox_bag("all"))
                .a11y_state(|| AccessibilityTraits::MIXED)
                .test_id("all")
                .build(),
        )
    });
    assert!(h.a11y_by_label("all").unwrap().traits.contains(AccessibilityTraits::MIXED));
    assert_eq!(checked(&robot, "all"), Some(false));
    robot.reset();
}

/// A plain button has no checked state to report, and no binding means no
/// update path: a bare pressable must stay as cheap as it was.
#[test]
fn a_plain_pressable_has_no_checked_state_and_no_updates() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let press: Rc<dyn Fn()> = Rc::new(|| {});
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            pressable(move || press()).test_id("plain").build(),
        )
    });
    assert_eq!(checked(&robot, "plain"), None);
    assert_eq!(h.a11y_update_count(), 0);
    robot.reset();
}
