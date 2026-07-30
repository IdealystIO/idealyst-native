//! `Toolbar(ToolbarProps { .. }).with_style(…).bind(r)` mounted through
//! the scene registry on the shared `host-mock` recording substrate.
//!
//! The harness exercises the GENERIC placeholder arm of
//! [`toolbar::register`]: `HostMock` is not a desktop backend, so the
//! registration-time type dispatch falls through to the
//! External-placeholder degradation path (`create_external` +
//! style/ref-fill/teardown). The `MacosBackend`-concrete NSToolbar leg is
//! covered by the unit tests in `macos_shared`/`macos.rs` (item
//! translation + click-flush ordering) and by running the
//! `toolbar-demo` example — NSToolbar/NSWindow construction needs a live
//! main thread, so no integration test can reach it (documented in
//! src/macos.rs).

use std::cell::Cell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_shared::{Ref, StyleRules, Tokenized};
use runtime_scene::Realized;
use runtime_vocabulary::glue::IntoElement;
use toolbar::prelude::*;

fn harness() -> Harness {
    // The SDK's boot registration seam — the same fn an app passes to
    // `host_appkit::newcore::run_with` (macOS) or any other new-core
    // boot entry's register argument.
    Harness::with_registry(|r| toolbar::register(r))
}

#[test]
fn mounts_the_external_placeholder_on_hosts_without_a_toolbar() {
    let h = harness();
    let el = Toolbar(ToolbarProps::default()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    // The placeholder posture: ONE `create_external` keyed by the
    // props type, no other structure.
    let log = h.ops().join("\n");
    assert_eq!(
        log, "create n0 external toolbar::ToolbarProps",
        "placeholder mount shape drifted from the External degradation path"
    );
}

/// On hosts with no toolbar leg there is no native toolbar to populate,
/// so the reactive `items` closure must never be evaluated (no phantom
/// subscriptions on hosts that render nothing).
#[test]
fn placeholder_mount_never_evaluates_the_items_closure() {
    let h = harness();
    let calls = Rc::new(Cell::new(0u32));
    let calls_in = calls.clone();
    let el = Toolbar(ToolbarProps {
        items: Box::new(move || {
            calls_in.set(calls_in.get() + 1);
            Vec::new()
        }),
        visible: true,
    })
    .into_element();
    let _realized: Realized<u32> = h.mount(el);
    assert_eq!(
        calls.get(),
        0,
        "unsupported hosts must not evaluate items"
    );
}

#[test]
fn author_style_lands_on_the_external_node() {
    let h = harness();
    let mut author = StyleRules::default();
    author.width = Some(Tokenized::Literal(runtime_shared::Length::Px(120.0)));
    let el = Toolbar(ToolbarProps::default())
        .with_style(author)
        .into_element();
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops();
    assert!(
        log.iter().any(|l| l.starts_with("apply_style n0")),
        "author style must attach to the external node: {log:?}"
    );
}

#[test]
fn bind_fills_the_ref_at_mount_and_set_visible_is_a_no_op() {
    let h = harness();
    let r: Ref<ToolbarHandle> = Ref::new();
    let el = Toolbar(ToolbarProps::default()).bind(r.clone()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    // Filled at mount. On this host the node is host-mock's, so
    // `set_visible` silently no-ops
    // regardless of which ops static is compiled in (on macOS the
    // shared ops' MacosNode downcast fails; elsewhere UnsupportedOps)
    // — the documented degradation.
    let filled = r.with(|handle| {
        handle.set_visible(false);
        handle.set_visible(true);
    });
    assert_eq!(
        filled,
        Some(()),
        "ref must be filled at mount; set_visible must not panic on a non-macOS node"
    );
}

#[test]
fn teardown_releases_the_external_node() {
    let h = harness();
    let el = Toolbar(ToolbarProps::default()).into_element();
    let realized: Realized<u32> = h.mount(el);
    let _ = h.take_log();

    drop(realized);
    let log = h.ops();
    assert!(
        log.iter().any(|l| l == "release_external n0"),
        "unmount must release the external node (teardown-guard contract): {log:?}"
    );
}

/// `ToolbarHandle: PartialEq` compares the mounted node, so a handle equals its own
/// clones and never equals a handle onto a different mounted `Toolbar`.
/// Required for the handle to sit in a `Signal` at all (`Signal<T>` bounds
/// `T: PartialEq` at creation and `get`), and only this crate can supply
/// the impl — the orphan rule blocks an app crate.
///
/// The unequal half is the load-bearing one: every handle on a target
/// shares the same `&'static` ops vtable, so an impl that compared `ops`
/// would collapse two distinct nodes into one and swallow a re-target.
#[test]
fn toolbar_handles_compare_by_mounted_node_identity() {
    let h = harness();

    let ra: Ref<ToolbarHandle> = Ref::new();
    let rb: Ref<ToolbarHandle> = Ref::new();
    let _a: Realized<u32> = h.mount(Toolbar(ToolbarProps::default()).bind(ra.clone()).into_element());
    let _b: Realized<u32> = h.mount(Toolbar(ToolbarProps::default()).bind(rb.clone()).into_element());

    let ha = ra.with(|handle| handle.clone()).expect("a mounted");
    let hb = rb.with(|handle| handle.clone()).expect("b mounted");

    assert!(ha == ha.clone(), "clones of one handle must compare equal");
    assert!(ha != hb, "handles onto two different mounted nodes must compare unequal");
}
