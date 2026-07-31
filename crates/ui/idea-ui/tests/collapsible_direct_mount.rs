//! Regression: mounting a measured `Collapsible` DIRECTLY must not abort.
//!
//! `measured_body` acquires two resources during the component body — a
//! deferred `after_animation_frame` setup task and the `LayoutSubscription`
//! it installs — and anchors their teardown to the component's scope. It
//! reached for `on_cleanup` to do that, which reads the effect stack and
//! panics when it is empty.
//!
//! A component body is exactly that empty-stack position. The bug hid
//! because the common path into a page is a reactive route swap, and a swap
//! runs inside an effect, which happens to satisfy `on_cleanup`. Mount the
//! same subtree directly — a deep link straight to a page containing a
//! measured Collapsible, or SSG/SSR rendering that path — and the app
//! aborted with "on_cleanup called outside an effect".
//!
//! Found by the premint dump's route crawl, which mounts every route
//! directly rather than navigating to it. The fix is
//! `runtime_world::on_scope_drop`, the legal hook for the build position;
//! its kernel-level semantics are pinned in `runtime-world`'s suite.
//!
//! This test mounts through the real `realize` path against `host-mock`,
//! which is the closest reachable reproduction — the panic was in the
//! reactive kernel's ownership layer, not in anything backend-specific, so
//! a mock host exercises the identical code.

use std::rc::Rc;

use idea_ui::components::collapsible::{Collapsible, CollapsibleTransition};
use runtime_core::{signal, text, ui, Reactive};

/// The default transition IS `Measured`, so the plain call site is the
/// affected one — worth stating, because a test that only exercised an
/// explicit `transition =` would leave the common shape uncovered.
#[test]
fn regression_measured_collapsible_mounts_outside_an_effect() {
    assert_eq!(
        CollapsibleTransition::default(),
        CollapsibleTransition::Measured,
        "if the default stops being Measured this test stops covering the \
         common call site"
    );

    let harness = host_mock::Harness::new();
    let tree = harness.world.enter(|| {
        let open = signal(false);
        ui! {
            Collapsible(
                title = "Advanced settings".to_string(),
                value = open,
                on_change = Rc::new(move |v: bool| open.set(v)) as Rc<dyn Fn(bool)>,
            ) {
                text("body")
            }
        }
    });

    // Directly — no enclosing effect, no route swap. This is the call that
    // aborted.
    let realized = harness.mount(tree);
    harness.flush();

    // And the scope must still tear down cleanly: the whole point of the
    // anchor is that the deferred setup task and the layout subscription
    // die with the component rather than outliving its signals.
    drop(realized);
}

/// `Snap` takes the other body branch (no measurement, no deferred setup),
/// so it never had the bug — pinned so a future refactor that unifies the
/// two branches cannot reintroduce it on this side unnoticed.
#[test]
fn snap_collapsible_also_mounts_directly() {
    let harness = host_mock::Harness::new();
    let tree = harness.world.enter(|| {
        let open = signal(true);
        ui! {
            Collapsible(
                title = Reactive::Static("Shipping".to_string()),
                value = open,
                on_change = Rc::new(move |v: bool| open.set(v)) as Rc<dyn Fn(bool)>,
                transition = CollapsibleTransition::Snap,
            ) {
                text("body")
            }
        }
    });
    let realized = harness.mount(tree);
    harness.flush();
    drop(realized);
}
