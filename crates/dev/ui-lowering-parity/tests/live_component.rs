//! A component's literal prop, live.
//!
//! A primitive's prop has a setter on the backend seam, so a live patch
//! is one call. A component's does not: its props were consumed and its
//! body already ran. Before this, `Button(label = "Sign in")` — which in
//! an idea-ui app is most of what an author edits — was refused live and
//! waited for the next render.
//!
//! The fix runs the component again from a copy of its props and swaps
//! the subtree. The copy only exists when the props type is `Clone`, and
//! `#[component]` does not require that, so the call site decides with
//! autoref specialization. Both branches are here: the `Clone` one must
//! change the screen, and the other must still refuse and SAY so rather
//! than silently doing nothing.

#![cfg(feature = "ui-overlay")]

use std::borrow::Cow;

use runtime_macros::{component, ui};
use runtime_template::{Edit, LiteralValue};
use runtime_vocabulary::glue::Element;
use runtime_vocabulary::overlay;
use ui_lowering_parity::{mount_scene, scene_shape, Mounted};

/// A props type that IS `Clone`.
///
/// Written with the explicit `#[props]` form, because an inline-props
/// `#[component]` does not derive `Clone` — see the module docs on what
/// that costs. This is the shape the rebuild path is built for.
#[runtime_macros::props]
#[derive(Clone, Default)]
pub struct ChipProps {
    pub label: String,
}

#[component]
fn Chip(props: &ChipProps) -> Element {
    let label = props.label.clone();
    ui! { text { label } }
}

/// A props type that is NOT `Clone` — the ordinary inline-props form,
/// which is every `#[component]` in the tree today.
#[component]
fn Plain(label: String) -> Element {
    ui! { text { label } }
}

fn set_str(node: u32, name: &'static str, value: &'static str) -> Edit {
    Edit::SetProp {
        node,
        name: Cow::Borrowed(name),
        value: LiteralValue::Str(Cow::Borrowed(value)),
    }
}

fn cloneable() -> Element {
    ui! {
        view() {
            Chip(label = "before")
        }
    }
}

fn cloneable_edited() -> Element {
    ui! {
        view() {
            Chip(label = "after")
        }
    }
}

fn not_cloneable() -> Element {
    ui! {
        view() {
            Plain(label = "before")
        }
    }
}

/// The case the phase exists for. The component runs again with the new
/// literal and its subtree is swapped in place, so the screen matches
/// what a recompile would have produced.
#[test]
fn a_clone_able_components_literal_prop_applies_live() {
    overlay::reset();
    let mut mounted = Mounted::new(cloneable);
    assert!(mounted.scene().contains("before"), "{}", mounted.scene());

    let site = mounted.site();
    let outcome = mounted.apply_live(site, &[set_str(1, "label", "after")]);

    assert_eq!(outcome.applied, 1, "{outcome:?}");
    assert_eq!(outcome.refused, 0);
    assert_eq!(
        scene_shape(&mounted.scene()),
        scene_shape(&mount_scene(cloneable_edited)),
        "a rebuilt component must be indistinguishable from a recompile"
    );
    overlay::reset();
}

/// Props that are not `Clone` have nothing to rebuild FROM. The applier
/// refuses and reports it, so a dev server can say "showing on next
/// render" instead of leaving the author wondering — and critically it
/// does NOT fail to compile, which is what a `Clone` bound on
/// `#[component]` would have cost everyone.
#[test]
fn a_non_clone_components_prop_is_refused_live_and_reported() {
    overlay::reset();
    let mut mounted = Mounted::new(not_cloneable);
    let before = mounted.scene();

    let site = mounted.site();
    let outcome = mounted.apply_live(site, &[set_str(1, "label", "after")]);

    assert_eq!(outcome.applied, 0, "nothing to rebuild from");
    assert_eq!(outcome.refused, 1, "and it must be reported, not swallowed");
    assert_eq!(mounted.scene(), before, "the old subtree stays mounted");
    overlay::reset();
}

/// Several props of one instance are one rebuild, not several. Running
/// the body once per changed prop would waste the work AND revert the
/// earlier props, because each rebuild starts from the same remembered
/// copy.
#[runtime_macros::props]
#[derive(Clone, Default)]
pub struct PairProps {
    pub a: String,
    pub b: String,
}

#[component]
fn Pair(props: &PairProps) -> Element {
    let (a, b) = (props.a.clone(), props.b.clone());
    ui! {
        view() {
            text { a }
            text { b }
        }
    }
}

fn pair() -> Element {
    ui! { view() { Pair(a = "a1", b = "b1") } }
}

fn pair_edited() -> Element {
    ui! { view() { Pair(a = "a2", b = "b2") } }
}

#[test]
fn two_props_of_one_instance_are_one_rebuild() {
    overlay::reset();
    let mut mounted = Mounted::new(pair);
    let site = mounted.site();

    let outcome = mounted.apply_live(
        site,
        &[set_str(1, "a", "a2"), set_str(1, "b", "b2")],
    );
    assert_eq!(outcome.applied, 1, "one rebuild, not two: {outcome:?}");
    assert_eq!(
        scene_shape(&mounted.scene()),
        scene_shape(&mount_scene(pair_edited)),
        "both props must survive the single rebuild"
    );
    overlay::reset();
}

/// The swapped-out instance must stop matching. Its node is gone from
/// the backend, and a second patch that still found it would issue calls
/// against something nothing is showing.
#[test]
fn a_second_patch_reaches_the_rebuilt_instance() {
    overlay::reset();
    let mut mounted = Mounted::new(cloneable);
    let site = mounted.site();

    assert_eq!(mounted.apply_live(site, &[set_str(1, "label", "once")]).applied, 1);
    let outcome = mounted.apply_live(site, &[set_str(1, "label", "twice")]);
    assert_eq!(outcome.applied, 1, "the NEW instance takes it: {outcome:?}");
    assert!(mounted.scene().contains("twice"), "{}", mounted.scene());
    assert!(!mounted.scene().contains("once"), "{}", mounted.scene());
    overlay::reset();
}

/// The rebuilder rides along per INSTANCE, not per site: each row of a
/// `for` keeps its own props, so patching the site rebuilds every row
/// from its own values rather than from one row's.
#[runtime_macros::props]
#[derive(Clone, Default)]
pub struct RowProps {
    pub label: String,
}

#[component]
fn Row(props: &RowProps) -> Element {
    let label = props.label.clone();
    ui! { text { label } }
}

fn rows() -> Element {
    let items = vec!["x", "y"];
    ui! {
        view() {
            for item in items {
                Row(label = item.to_string())
            }
        }
    }
}

#[test]
fn every_row_of_a_for_rebuilds_from_its_own_props() {
    overlay::reset();
    let mut mounted = Mounted::new(rows);
    let before = mounted.scene();
    assert!(before.contains('x') && before.contains('y'), "{before}");

    // The `Row` node inside the `for` body. Patching a prop the rows do
    // not share (there is none here) is the point: what matters is that
    // each row rebuilds from ITS label, so an untouched prop keeps its
    // per-row value.
    let site = mounted.site();
    let outcome = mounted.apply_live(site, &[Edit::SetProp {
        node: 2,
        name: Cow::Borrowed("nonesuch"),
        value: LiteralValue::Str(Cow::Borrowed("z")),
    }]);

    // Two instances, each rebuilt from its own remembered props.
    assert_eq!(outcome.applied, 2, "{outcome:?}");
    let after = mounted.scene();
    assert!(after.contains('x'), "row x kept its own label:\n{after}");
    assert!(after.contains('y'), "row y kept its own label:\n{after}");
    overlay::reset();
}
