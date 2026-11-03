//! The robot names an element the way a screen reader does: a pressable
//! or view with no text of its own is found by its a11y label.
//!
//! An icon-only button (`IconButton(label = "Open menu")`) carries its
//! name only as an a11y label; the pressable registered `label: None`, so
//! VoiceOver read "Open menu" while `find_element(label = "Open menu")`
//! found nothing on any backend.
#![cfg(feature = "robot")]

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_vocabulary::builders::{pressable, view};

fn named(label: &str) -> AccessibilityProps {
    AccessibilityProps { label: Some(label.into()), ..Default::default() }
}
use runtime_vocabulary::robot::{Query, Robot};

#[test]
fn regression_an_icon_only_pressable_is_found_by_its_a11y_label() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .child(pressable(|| {}).a11y(named("Open menu")).test_id("menu").build())
                .child(view().a11y(named("Notifications panel")).test_id("panel").build())
                .child(pressable(|| {}).test_id("unnamed").build())
                .build(),
        )
    });
    let by_id = |id: &str| robot.find(Query::test_id(id)).unwrap_or_else(|| panic!("{id} registered"));

    let menu = robot.find(Query::label("Open menu")).expect("found by its a11y label");
    assert_eq!(menu.id, by_id("menu").id);
    assert_eq!(by_id("menu").label.as_deref(), Some("Open menu"), "element JSON carries the name");
    let panel = robot.find(Query::label("Notifications panel")).expect("a labelled view too");
    assert_eq!(panel.id, by_id("panel").id);
    assert!(robot.find(Query::label_contains("menu")).is_some(), "label_contains reads it as well");
    assert_eq!(by_id("unnamed").label, None, "no a11y label, no name");
    robot.reset();
}
