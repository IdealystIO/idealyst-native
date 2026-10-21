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

/// Regression: the measured body's padding and border must sit INSIDE the
/// measured node, so the animated max-height covers them whatever their
/// value. The body read its chrome back by resolving its own style in
/// Rust, which a `--premint-only` bundle cannot do (it panicked on the
/// first Collapsible); a fixed constant instead clips a restyled body.
/// The structure that needs neither: clip > unpadded measured column >
/// padded body. Pinned on the applied styles: the padded node's parent
/// carries no padding, and its grandparent is the clip.
#[test]
fn regression_measured_body_padding_sits_inside_the_measured_node() {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use runtime_core::{Overflow, StyleRules};

    let harness = host_mock::Harness::new();
    let styles: Rc<RefCell<HashMap<u32, StyleRules>>> = Rc::default();
    {
        let styles = styles.clone();
        harness.set_style_line(move |n, rules| {
            styles.borrow_mut().insert(n, rules.clone());
            format!("apply_style n{n}")
        });
    }
    let tree = harness.world.enter(|| {
        idea_ui::install_idea_theme(idea_ui::light_theme());
        let open = signal(true);
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
    let _realized = harness.mount(tree);
    harness.flush();

    let styles = styles.borrow();
    let padded_body = styles
        .iter()
        .find(|(_, r)| r.padding_top.is_some() && r.border_top_width.is_some() && r.opacity.is_some())
        .map(|(n, _)| *n)
        .expect("the measured body applies its padded, bordered, fading style");
    let parent_of = |node: u32| -> u32 {
        fn walk(h: &host_mock::Harness, at: u32, target: u32) -> Option<u32> {
            let kids = h.children_of(at);
            if kids.contains(&target) {
                return Some(at);
            }
            kids.into_iter().find_map(|k| walk(h, k, target))
        }
        harness.live_roots().into_iter().find_map(|r| walk(&harness, r, node)).expect("a parent")
    };
    let measured = parent_of(padded_body);
    let measured_rules = styles.get(&measured).cloned().unwrap_or_default();
    assert!(
        measured_rules.padding_top.is_none() && measured_rules.border_top_width.is_none(),
        "the measured node carries no chrome of its own, so its height is the body's whole box"
    );
    let clip = parent_of(measured);
    assert_eq!(
        styles.get(&clip).and_then(|r| r.overflow),
        Some(Overflow::Hidden),
        "the clip that the max-height animates wraps the measured node"
    );
}
