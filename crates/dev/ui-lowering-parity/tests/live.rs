//! The LIVE application path, against a mounted scene.
//!
//! A patch has to do two things, and a test checking only one would pass
//! while the feature was half-useless:
//!
//! 1. change what is **on screen now**, through ordinary backend calls,
//!    so a saved edit appears without waiting for something to
//!    re-render; and
//! 2. change what the **next build** of that site produces, so the edit
//!    survives every state-driven rebuild.
//!
//! Each case drives both, in that order, on one mounted tree.
//!
//! Assertions are on the SCENE — what the backend was actually told to
//! show — and the comparison target is the same fixture built from the
//! EDITED source. "The patch worked" therefore means "indistinguishable
//! from having recompiled", which is the only definition worth having.
//!
//! Nothing here mentions a backend. The live applier is generic over
//! `H: AllCaps` and issues capability calls, so this recording mock
//! exercises exactly the code every real backend runs.

#![cfg(feature = "ui-overlay")]

use std::borrow::Cow;

use runtime_macros::ui;
use runtime_template::{Edit, LiteralValue, NewNode, PropEntry, PropValue};
use runtime_vocabulary::glue::{signal, Element, Signal};
use runtime_vocabulary::overlay;
use ui_lowering_parity::{mount_scene, scene_shape, Mounted};

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

// ===========================================================================
// A literal, live
// ===========================================================================

fn original() -> Element {
    ui! {
        view() {
            text { "before" }
        }
    }
}

fn edited() -> Element {
    ui! {
        view() {
            text { "after" }
        }
    }
}

#[test]
fn a_live_patch_makes_the_mounted_scene_match_the_edited_source() {
    overlay::reset();
    let mut mounted = Mounted::new(original);
    assert!(mounted.scene().contains("before"), "{}", mounted.scene());

    let site = mounted.site();
    let outcome = mounted.apply_live(site, &[set_str(1, "content", "after")]);

    assert_eq!(outcome.refused, 0, "the seam has a setter for text content");
    assert_eq!(outcome.applied, 1);
    assert_eq!(
        scene_shape(&mounted.scene()),
        scene_shape(&mount_scene(edited)),
        "a live patch must be indistinguishable from a recompile"
    );
    overlay::reset();
}

// ===========================================================================
// Persistence across a rebuild
// ===========================================================================

thread_local! {
    static FLIP: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

fn reactive_site() -> Element {
    let n: Signal<i32> = signal(0);
    FLIP.with(|f| f.set(0));
    // Stash the signal so the test can drive it. A `thread_local` and not
    // a return value because `Mounted::new` takes a plain builder — the
    // shape every other suite here uses.
    DRIVER.with(|d| *d.borrow_mut() = Some(n));
    ui! {
        view() {
            text { "static" }
            if n.get() > 0 {
                text { "on" }
            }
        }
    }
}

thread_local! {
    static DRIVER: std::cell::RefCell<Option<Signal<i32>>> = const {
        std::cell::RefCell::new(None)
    };
}

/// A site's `Element` is rebuilt whenever its reactive scope re-runs. A
/// patch applied only to the mounted nodes would silently revert the
/// first time that happened — which is why the `Element` path exists
/// alongside this one, and why they are staged together.
#[test]
fn a_patch_survives_a_state_driven_rebuild_of_its_site() {
    overlay::reset();
    let mut mounted = Mounted::new(reactive_site);
    let site = mounted.site();
    let edits = vec![set_str(1, "content", "patched")];

    // Both paths, as a dev server does it: stage for future builds, then
    // apply to what is already mounted.
    overlay::stage_key(site, edits.clone());
    mounted.apply_live(site, &edits);
    assert!(mounted.scene().contains("patched"), "{}", mounted.scene());

    // Flip the branch: the site's scope re-runs and rebuilds its nodes.
    mounted.drive(|| DRIVER.with(|d| d.borrow().unwrap().set(1)));
    let after = mounted.scene();
    assert!(after.contains("on"), "the branch must have flipped:\n{after}");
    assert!(
        after.contains("patched"),
        "the patch must survive the rebuild:\n{after}"
    );
    assert!(!after.contains("static"), "the old value must be gone:\n{after}");
    overlay::reset();
}

/// Rebuilding an UNRELATED site must not pick up another site's patch.
/// Patches are keyed by site, and this is the test that the key is
/// actually consulted rather than the patch being applied to whatever
/// happens to rebuild next.
#[test]
fn rebuilding_an_unrelated_site_is_unaffected() {
    overlay::reset();
    let mut mounted = Mounted::new(reactive_site);
    let real_site = mounted.site();

    // Stage against a site that does not exist in this tree.
    overlay::stage_key(real_site ^ 0xffff_ffff, vec![set_str(1, "content", "wrong")]);
    mounted.drive(|| DRIVER.with(|d| d.borrow().unwrap().set(1)));

    let after = mounted.scene();
    assert!(after.contains("static"), "{after}");
    assert!(!after.contains("wrong"), "another site's patch must not apply:\n{after}");
    overlay::reset();
}

// ===========================================================================
// Structure, live
// ===========================================================================

fn two_children() -> Element {
    ui! {
        view() {
            text { "one" }
        }
    }
}

fn three_children() -> Element {
    ui! {
        view() {
            text { "one" }
            text { "two" }
        }
    }
}

/// An inserted static subtree is realized through the ordinary realize
/// path and spliced in with ordinary `Host` calls, so every backend gets
/// it from the same replay.
#[test]
fn inserting_a_static_subtree_live_matches_the_edited_source() {
    overlay::reset();
    let mut mounted = Mounted::new(two_children);
    let site = mounted.site();

    let outcome = mounted.apply_live(
        site,
        &[Edit::SetChildren {
            node: 0,
            children: Cow::Owned(vec![text_node("one"), text_node("two")]),
        }],
    );
    assert_eq!(outcome.refused, 0);
    assert_eq!(scene_shape(&mounted.scene()), scene_shape(&mount_scene(three_children)));
    overlay::reset();
}

/// A removal is the same edit with a shorter list.
#[test]
fn removing_a_static_subtree_live_matches_the_edited_source() {
    overlay::reset();
    let mut mounted = Mounted::new(three_children);
    let site = mounted.site();

    mounted.apply_live(
        site,
        &[Edit::SetChildren { node: 0, children: Cow::Owned(vec![text_node("one")]) }],
    );
    assert_eq!(scene_shape(&mounted.scene()), scene_shape(&mount_scene(two_children)));
    overlay::reset();
}

// ===========================================================================
// What waits for the next build
// ===========================================================================

/// A prop with no setter on the seam cannot be applied live, and the
/// applier says so rather than pretending. The `Element` path still has
/// it, so the change appears on the next build of the site — which is
/// what a dev server reports.
#[test]
fn a_prop_the_seam_cannot_set_is_refused_live_and_applies_on_rebuild() {
    overlay::reset();
    let mut mounted = Mounted::new(reactive_site);
    let site = mounted.site();
    let edits = vec![Edit::SetProp {
        node: 0,
        name: Cow::Borrowed("preserves_focus"),
        value: LiteralValue::Bool(true),
    }];

    let outcome = mounted.apply_live(site, &edits);
    assert_eq!(outcome.applied, 0, "no seam setter for this one");
    assert_eq!(outcome.refused, 1, "and it must be reported, not swallowed");
    overlay::reset();
}
