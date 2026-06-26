//! `ui!` / `jsx!` accessibility-attribute lowering.
//!
//! The author-facing a11y surface is the attribute set (`a11y_label`,
//! `a11y_role`, `a11y_hint`, `a11y_hidden`, `a11y_traits`, `live_region`,
//! and the whole-struct `accessibility`) that both macros lower to the
//! identically-named `Bound` setters.
//!
//! Interception applies to the **named-prop tag form** (`button(label =
//! …, a11y_label = …)`). The positional/expression form (`button("x",
//! cb)`) parses as a raw Rust expression, where authors chain the
//! `Bound::a11y_*` setters directly (covered by `builder.rs`'s
//! `a11y_builder_tests`). These tests build a single primitive through
//! each macro and inspect the resulting `Element`'s `accessibility`
//! field directly — no mount needed.

use runtime_core::accessibility::{AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role};
use runtime_core::{jsx, ui, Element};

/// Pull the `accessibility` field out of a node-bearing element.
fn a11y_of(el: &Element) -> &AccessibilityProps {
    match el {
        Element::View { accessibility, .. }
        | Element::Text { accessibility, .. }
        | Element::Button { accessibility, .. } => accessibility,
        _ => panic!("unexpected element variant"),
    }
}

#[test]
fn ui_granular_attrs_lower_to_setters() {
    let el: Element = ui! {
        button(
            label = "Save",
            on_click = || {},
            a11y_label = "Save document",
            a11y_hint = "Writes changes to disk",
            a11y_role = Role::Button,
        )
    };
    let a = a11y_of(&el);
    assert_eq!(a.label.as_deref(), Some("Save document"));
    assert_eq!(a.hint.as_deref(), Some("Writes changes to disk"));
    assert_eq!(a.role, Some(Role::Button));
}

#[test]
fn ui_bag_attr_replaces_wholesale() {
    let el: Element = ui! {
        view(accessibility = AccessibilityProps {
            label: Some("Toolbar".into()),
            role: Some(Role::Toolbar),
            hidden: false,
            ..Default::default()
        })
    };
    let a = a11y_of(&el);
    assert_eq!(a.label.as_deref(), Some("Toolbar"));
    assert_eq!(a.role, Some(Role::Toolbar));
}

#[test]
fn ui_hidden_and_traits_and_live_region() {
    let el: Element = ui! {
        view(
            a11y_hidden = true,
            a11y_traits = AccessibilityTraits::SELECTED | AccessibilityTraits::DISABLED,
            live_region = LiveRegionPriority::Assertive,
        )
    };
    let a = a11y_of(&el);
    assert!(a.hidden);
    assert!(a.traits.contains(AccessibilityTraits::SELECTED));
    assert!(a.traits.contains(AccessibilityTraits::DISABLED));
    assert_eq!(a.live_region, Some(LiveRegionPriority::Assertive));
}

#[test]
fn ui_coexists_with_other_props() {
    // a11y attrs partition out alongside `style`; the rest still flow to
    // the primitive's own emitter (here `text`'s `content`).
    let el: Element = ui! {
        text(content = "hello", a11y_label = "greeting")
    };
    assert!(matches!(el, Element::Text { .. }));
    assert_eq!(a11y_of(&el).label.as_deref(), Some("greeting"));
}

#[test]
fn ui_plain_primitive_has_default_a11y() {
    let el: Element = ui! { view() };
    assert!(a11y_of(&el).is_default());
}

#[test]
fn jsx_granular_attrs_lower_to_setters() {
    let el: Element = jsx! {
        <button label="Open" a11y_label="Open menu" a11y_role={Role::Button} />
    };
    let a = a11y_of(&el);
    assert_eq!(a.label.as_deref(), Some("Open menu"));
    assert_eq!(a.role, Some(Role::Button));
}

#[test]
fn jsx_bag_attr_replaces_wholesale() {
    let el: Element = jsx! {
        <view accessibility={AccessibilityProps {
            label: Some("Region".into()),
            ..Default::default()
        }} />
    };
    assert_eq!(a11y_of(&el).label.as_deref(), Some("Region"));
}
