//! The overlay applier, against real mounted scenes.
//!
//! Each case stages a [`Patch`] and rebuilds the same `ui!`, then
//! compares the rendered tree. Building twice is not incidental: a patch
//! is applied inside `__overlay::tag`, at the moment each node is built,
//! which is what makes it survive the rebuilds a reactive site does on
//! its own. A test that patched a tree it already had would prove
//! something weaker.
//!
//! What is asserted is the SCENE — what the backend was told to put on
//! screen — not the `Element`. A patch's entire purpose is the former.
//!
//! Half of these cases are refusals, and that is deliberate. An applier
//! that silently half-applies an edit is worse than one that refuses it:
//! the author sees a tree that is neither the old version nor the new,
//! with nothing saying so. Every refusal here is a case where the answer
//! is "rebuild", and the applier says so through [`Outcome::refused`]
//! rather than by guessing.

#![cfg(feature = "ui-overlay")]

use std::borrow::Cow;

use runtime_macros::{component, ui};
use runtime_template::{Edit, LiteralValue, NewNode, PropEntry, PropValue};
use runtime_vocabulary::glue::{signal, Element, Signal};
use runtime_vocabulary::overlay;
use ui_lowering_parity::mount_scene;

/// A `#[component]` with a literal-typed prop and a children slot — the
/// two things an overlay can reach on a component.
#[component]
fn Badge(label: String, children: Vec<Element>) -> Element {
    ui! {
        view() {
            text { label }
            children
        }
    }
}

/// A component with NO children field, to pin that inserting one with
/// children is refused rather than silently dropping them.
#[component]
fn Pill(label: String) -> Element {
    ui! { text { label } }
}

/// The site key of a built tree's outermost tag.
fn site_of(build: impl FnOnce() -> Element) -> u64 {
    let harness = host_mock::Harness::new();
    harness.world.enter(|| {
        let element = build();
        overlay::tags(&element).first().expect("a tagged node").site
    })
}

fn set_str(node: u32, name: &'static str, value: &'static str) -> Edit {
    Edit::SetProp {
        node,
        name: Cow::Borrowed(name),
        value: LiteralValue::Str(Cow::Borrowed(value)),
    }
}

fn text_node(content: &'static str) -> NewNode {
    NewNode {
        kind: Cow::Borrowed("text"),
        props: Cow::Owned(vec![PropEntry {
            name: Cow::Borrowed("content"),
            value: PropValue::Lit(LiteralValue::Str(Cow::Borrowed(content))),
        }]),
        children: Cow::Borrowed(&[]),
    }
}

/// Build once to learn the site, stage, then build again and return the
/// scene. The two builds are the point — see the module docs.
fn scene_with(build: fn() -> Element, edits: Vec<Edit>) -> String {
    overlay::reset();
    let site = site_of(build);
    overlay::stage_key(site, edits);
    let scene = mount_scene(build);
    overlay::reset();
    scene
}

fn scene_without(build: fn() -> Element) -> String {
    overlay::reset();
    mount_scene(build)
}

// ===========================================================================
// Literal props
// ===========================================================================

fn static_text() -> Element {
    ui! {
        view() {
            text { "before" }
        }
    }
}

#[test]
fn a_literal_on_a_primitive_changes_what_is_rendered() {
    let before = scene_without(static_text);
    assert!(before.contains("before"), "{before}");

    // Node 0 is the `view`, node 1 the `text` — a preorder walk of the
    // body, which is what the descriptor says too.
    let after = scene_with(static_text, vec![set_str(1, "content", "after")]);
    assert!(after.contains("after"), "{after}");
    assert!(!after.contains("before"), "{after}");
}

fn component_prop() -> Element {
    ui! {
        view() {
            Badge(label = "before")
        }
    }
}

/// A component's props are consumed before its `Element` exists, so this
/// is the one edit applied at the call site rather than to the built
/// tree. It goes through the generated inherent `__apply_literal`.
#[test]
fn a_literal_on_a_component_prop_changes_what_is_rendered() {
    assert!(scene_without(component_prop).contains("before"));
    let after = scene_with(component_prop, vec![set_str(1, "label", "after")]);
    assert!(after.contains("after"), "{after}");
    assert!(!after.contains("before"), "{after}");
}

/// A prop name the props type does not carry is a no-op, not an error —
/// the same contract `__apply_literal` has everywhere.
#[test]
fn an_unknown_prop_name_changes_nothing() {
    let before = scene_without(static_text);
    let after = scene_with(static_text, vec![set_str(1, "nonesuch", "x")]);
    assert_eq!(before, after);
}

// ===========================================================================
// Refusals
// ===========================================================================

fn reactive_text() -> Element {
    // A signal per BUILD, not a shared one: each case mounts against a
    // fresh `World`, and a signal outliving its world is a panic by
    // design (`dead-world-read`).
    let count: Signal<i32> = signal(0);
    ui! {
        view() {
            text { move || format!("n={}", count.get()) }
        }
    }
}

/// Reactive content is CODE: the compiled closure owns the value.
/// Overwriting it with a constant would disconnect the binding, and the
/// author would see a number that had silently stopped updating. Refused.
#[test]
fn a_reactive_prop_is_refused_not_overwritten() {
    let before = scene_without(reactive_text);
    let after = scene_with(reactive_text, vec![set_str(1, "content", "frozen")]);
    assert_eq!(before, after, "the binding must survive the refusal");
    assert!(!after.contains("frozen"), "{after}");
}

fn styled_text() -> Element {
    ui! {
        view() {
            text(a11y_label = "old") { "x" }
        }
    }
}

/// A literal of the wrong SHAPE for the prop is refused rather than
/// coerced. A differ that sends an int for a string prop has a bug; the
/// running app must not paper over it.
#[test]
fn a_literal_of_the_wrong_shape_is_refused() {
    let before = scene_without(styled_text);
    let after = scene_with(
        styled_text,
        vec![Edit::SetProp {
            node: 1,
            name: Cow::Borrowed("a11y_label"),
            value: LiteralValue::Int(7),
        }],
    );
    assert_eq!(before, after);
}

// ===========================================================================
// Children
// ===========================================================================

fn two_children() -> Element {
    ui! {
        view() {
            text { "one" }
            text { "two" }
        }
    }
}

/// Insert, remove and reorder are one edit — `SetChildren` — because all
/// three are "the child list is now that".
#[test]
fn set_children_inserts_removes_and_reorders_in_one_edit() {
    let before = scene_without(two_children);
    assert!(before.contains("one") && before.contains("two"));

    let after = scene_with(
        two_children,
        vec![Edit::SetChildren {
            node: 0,
            children: Cow::Owned(vec![text_node("two"), text_node("three"), text_node("one")]),
        }],
    );
    let two = after.find("two").expect("two");
    let three = after.find("three").expect("three");
    let one = after.find("one").expect("one");
    assert!(two < three && three < one, "reordered and inserted:\n{after}");
}

fn conditional_children() -> Element {
    let count: Signal<i32> = signal(0);
    ui! {
        view() {
            text { "static" }
            if count.get() > 0 {
                text { "conditional" }
            }
        }
    }
}

/// A child list containing a reactive region is one whose contents are
/// decided at runtime. Replacing it would delete live code, so the
/// applier refuses — checking the LIVE children rather than trusting
/// whoever built the patch.
#[test]
fn set_children_refuses_a_list_holding_a_reactive_region() {
    let before = scene_without(conditional_children);
    let after = scene_with(
        conditional_children,
        vec![Edit::SetChildren { node: 0, children: Cow::Owned(vec![text_node("replaced")]) }],
    );
    assert_eq!(before, after, "the reactive region must survive");
    assert!(!after.contains("replaced"), "{after}");
}

/// Nothing knows how to build an `image` from literals — its source is
/// not data. Refused as a whole, so the child list is left alone rather
/// than half-replaced.
#[test]
fn set_children_refuses_a_subtree_it_cannot_build() {
    let before = scene_without(two_children);
    let after = scene_with(
        two_children,
        vec![Edit::SetChildren {
            node: 0,
            children: Cow::Owned(vec![
                text_node("kept"),
                NewNode {
                    kind: Cow::Borrowed("image"),
                    props: Cow::Borrowed(&[]),
                    children: Cow::Borrowed(&[]),
                },
            ]),
        }],
    );
    assert_eq!(before, after, "an unbuildable node must not half-apply");
    assert!(!after.contains("kept"), "{after}");
}

// ===========================================================================
// Constructing components
// ===========================================================================

/// A component registers its constructor at its own call site, so the
/// overlay can insert one the program already renders somewhere.
#[test]
fn a_rendered_component_can_be_inserted_elsewhere() {
    let after = scene_with(
        component_prop,
        vec![Edit::SetChildren {
            node: 0,
            children: Cow::Owned(vec![NewNode {
                kind: Cow::Borrowed("Badge"),
                props: Cow::Owned(vec![PropEntry {
                    name: Cow::Borrowed("label"),
                    value: PropValue::Lit(LiteralValue::Str(Cow::Borrowed("inserted"))),
                }]),
                children: Cow::Borrowed(&[]),
            }]),
        }],
    );
    assert!(after.contains("inserted"), "{after}");
}

fn pill() -> Element {
    ui! {
        view() {
            Pill(label = "p")
        }
    }
}

/// A props type with no `children` field cannot take children, and
/// `__apply_children` says so. Refused, rather than dropping them.
#[test]
fn inserting_a_component_that_cannot_hold_children_is_refused() {
    let before = scene_without(pill);
    let after = scene_with(
        pill,
        vec![Edit::SetChildren {
            node: 0,
            children: Cow::Owned(vec![NewNode {
                kind: Cow::Borrowed("Pill"),
                props: Cow::Borrowed(&[]),
                children: Cow::Owned(vec![text_node("dropped")]),
            }]),
        }],
    );
    assert_eq!(before, after);
    assert!(!after.contains("dropped"), "{after}");
}

/// A component the program has never rendered has no constructor. That
/// is a refusal, never a panic — see `overlay::construct`'s docs for why
/// the registration is lazy.
#[test]
fn inserting_an_unknown_component_is_refused() {
    overlay::reset();
    assert_eq!(overlay::ctor_count(), 0);
    let before = scene_without(two_children);
    let after = scene_with(
        two_children,
        vec![Edit::SetChildren {
            node: 0,
            children: Cow::Owned(vec![NewNode {
                kind: Cow::Borrowed("NeverRendered"),
                props: Cow::Borrowed(&[]),
                children: Cow::Borrowed(&[]),
            }]),
        }],
    );
    assert_eq!(before, after);
}

// ===========================================================================
// Staging
// ===========================================================================

/// A patch survives rebuilds of its site, and unstaging reverts it. This
/// is why the applier lives in `tag` rather than running once over a
/// finished tree.
#[test]
fn a_patch_applies_on_every_rebuild_and_unstaging_reverts_it() {
    overlay::reset();
    let site = site_of(static_text);
    overlay::stage_key(site, vec![set_str(1, "content", "patched")]);
    assert_eq!(overlay::staged_count(), 1);

    assert!(mount_scene(static_text).contains("patched"));
    assert!(mount_scene(static_text).contains("patched"), "and again");

    overlay::unstage_key(site);
    assert_eq!(overlay::staged_count(), 0);
    assert!(mount_scene(static_text).contains("before"));
    overlay::reset();
}
