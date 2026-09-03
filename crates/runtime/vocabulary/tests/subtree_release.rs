//! `Host::release_subtree` — the seam that tells a host a subtree is
//! DISCARDED rather than merely detached.
//!
//! A host cannot infer this from the structural ops: `clear_children`
//! and `remove_child` both mean "detach", and detach is also how a
//! retained subtree gets parked (the navigator's `LazyPersistent`
//! screens come back on the SAME nodes — see `navigator.rs`, which pins
//! that half). So the layers owning the lifetime say so explicitly, and
//! these tests pin that they do.
//!
//! What goes wrong without it is invisible on web, where a DOM node dies
//! with its last reference, and expensive on a host holding a side
//! registry keyed by node: `backend-ios` keeps a strong `Retained<UIView>`
//! plus a Taffy node per view, so a discarded subtree's entries lived
//! forever — walked by every later layout pass, and every unparented
//! node counted as another root to lay out.

use runtime_scene::{dyn_element, realize};
use runtime_vocabulary::builders::{text, view};
use runtime_world::signal;

fn harness(splice: bool) -> host_mock::Harness {
    let h = host_mock::Harness::new();
    h.shared.splice.set(splice);
    h
}

/// The anchored path: the old branch's scope drops, the host is told the
/// subtree is gone, then the anchor clears.
#[test]
fn an_anchored_swap_releases_the_branch_it_discards() {
    let h = harness(false);
    let world = h.world.clone();
    world.enter(|| {
        let flag = signal(true);
        let element = view()
            .child(dyn_element(move || {
                if flag.get() {
                    text().content("first").build()
                } else {
                    text().content("second").build()
                }
            }))
            .build();
        let _realized = realize(&h.backend, &h.registry, element);
        h.take_log();

        flag.set(false);
        world.flush();

        let log = h.take_log();
        assert!(
            log.iter().any(|l| l.starts_with("release_subtree")),
            "the discarded branch is released, not just detached: {log:?}"
        );
        let release = log.iter().position(|l| l.starts_with("release_subtree"));
        let clear = log.iter().position(|l| l.starts_with("clear_children"));
        if let (Some(r), Some(c)) = (release, clear) {
            assert!(
                r < c,
                "released while the subtree is still assembled — a host that walks \
                 its own children to find what to free needs them there: {log:?}"
            );
        }
    });
}

/// The spliced path: same guarantee where the region's nodes sit
/// directly in the real parent, with no anchor of their own.
#[test]
fn a_spliced_swap_releases_the_branch_it_discards() {
    let h = harness(true);
    let world = h.world.clone();
    world.enter(|| {
        let flag = signal(true);
        let element = view()
            .child(dyn_element(move || {
                if flag.get() {
                    text().content("first").build()
                } else {
                    text().content("second").build()
                }
            }))
            .build();
        let _realized = realize(&h.backend, &h.registry, element);
        h.take_log();

        flag.set(false);
        world.flush();

        let log = h.take_log();
        assert!(
            log.iter().any(|l| l.starts_with("release_subtree")),
            "a spliced region discards its old nodes just as an anchored one does: {log:?}"
        );
    });
}

/// The negative that gives the positives meaning: a region that is NOT
/// swapped releases nothing. Without this, a test that fired
/// `release_subtree` on every flush would still pass the two above.
#[test]
fn a_region_that_does_not_swap_releases_nothing() {
    let h = harness(false);
    let world = h.world.clone();
    world.enter(|| {
        let unrelated = signal(0);
        let element = view()
            .child(dyn_element(move || text().content("stable").build()))
            .build();
        let _realized = realize(&h.backend, &h.registry, element);
        h.take_log();

        // Touch a signal the region does not read.
        unrelated.set(1);
        world.flush();

        let log = h.take_log();
        assert!(
            !log.iter().any(|l| l.starts_with("release_subtree")),
            "nothing was discarded, so nothing may be released: {log:?}"
        );
    });
}
