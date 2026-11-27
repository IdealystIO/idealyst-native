//! `Refreshable` mounted through the scene registry on the shared
//! `host-mock` recording substrate — the passthrough handler every backend
//! without a native refresh control (web, desktop, SSR, GPU, mocks) gets.
//!
//! What it pins: the wrapper is ONE layout-transparent anchor and the child
//! realizes INTO it, unchanged. A plain view in its place would be a flex
//! item that changes the child's layout.

use std::rc::Rc;

use host_mock::Harness;
use pull_refresh::{Refreshable, RefreshableProps};
use runtime_core::{ui, BuildElement};
use runtime_scene::Realized;
use runtime_vocabulary::builders;
use runtime_world::signal;

fn harness() -> Harness {
    let h = Harness::with_registry(|r| pull_refresh::register(r));
    h.mute(&["update_text", "on_node_unstyled", "mark_container"]);
    h
}

#[test]
fn the_child_mounts_unchanged_inside_a_transparent_anchor() {
    let h = harness();
    // Through `ui!`, as an app writes it: the block is the `children` prop.
    let el = ui! {
        Refreshable() {
            text { "rows" }
        }
    };
    let _realized: Realized<u32> = h.mount(el);
    let log = h.ops().join("\n");
    assert_eq!(
        log,
        "create n0 anchor\n\
         create n1 text \"rows\"\n\
         insert n0 <- n1",
        "Refreshable passthrough shape drifted"
    );
}

/// A child that is not a scroll container (and reactive props on a host
/// with no control) must render, not panic.
#[test]
fn a_child_that_is_not_a_scroller_renders_with_reactive_props() {
    let h = harness();
    let realized: Realized<u32> = h.world.enter(|| {
        let loading = signal(true);
        let el = RefreshableProps {
            refreshing: loading.into(),
            on_refresh: Rc::new(move || loading.set(true)),
            tint: Some(runtime_shared::Color("#2563eb".into())).into(),
            children: vec![builders::view().build()],
        }
        .build();
        runtime_scene::realize(&h.backend, &h.registry, el)
    });
    assert!(h.ops().iter().any(|l| l.starts_with("insert n0 <-")), "{:?}", h.ops());
    drop(realized);
}
