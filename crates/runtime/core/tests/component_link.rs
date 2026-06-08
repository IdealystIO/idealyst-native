//! Walker linkage: a `#[component]` with `methods!` wraps its root in
//! `Element::Component`; the walker must unwrap it and record
//! element↔component so the inspector can resolve a selected element to the
//! component whose methods it can invoke. Gated on `robot` (the variant +
//! linkage only exist there); run with `--features robot`.
#![cfg(feature = "robot")]

#[path = "common/mod.rs"]
mod common;

use crate::common::TestRuntime;
use runtime_core::{robot, view, Element, IntoElement};

/// Mounting `Element::Component { instance, child }` through the walker must
/// link the instance to its root primitive's element id — exercising the
/// full unwrap → arm-pending → child-registers → consume-pending → link
/// sequence the `#[component]` macro relies on.
#[test]
fn walker_links_component_root_to_its_element() {
    let rt = TestRuntime::new();

    let reg = robot::register_component("Counter", Vec::new());
    let instance = reg.id();

    // Pre-mount: registered but not yet linked to any element.
    let before = robot::list_components();
    assert_eq!(
        before.iter().find(|s| s.id == instance).and_then(|s| s.element_id),
        None,
        "no element link before the walk"
    );

    // The macro produces exactly this shape: the component's root primitive
    // wrapped in `Element::Component`.
    let child = view(Vec::new()).into_element();
    let _owner = rt.render(Element::Component { instance, child: Box::new(child) });

    // Post-mount: the walker linked the instance to the root view's id.
    let after = robot::list_components();
    let entry = after
        .iter()
        .find(|s| s.id == instance)
        .expect("component still registered");
    let element_id = entry
        .element_id
        .expect("walker linked the component to its root element");
    // And the reverse lookup the bridge uses resolves back to this instance.
    assert_eq!(robot::component_for_element(element_id), Some(instance));

    drop(reg);
}
