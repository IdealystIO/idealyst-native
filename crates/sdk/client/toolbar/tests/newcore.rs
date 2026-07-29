//! New-core suite: the SAME authored surface —
//! `Toolbar(ToolbarProps { .. }).with_style(…).bind(r)` — mounted
//! through the scene registry on the shared `host-mock` recording
//! substrate.
//!
//! The harness exercises the GENERIC placeholder arm of
//! [`toolbar::register`]: `HostMock` is not `MacosBackend`, so the
//! registration-time type dispatch falls through to the frozen
//! External-placeholder degradation path (`create_external` +
//! style/ref-fill/teardown), which pins the old walker's `external.rs`
//! sequence for a backend with no real handler. The
//! `MacosBackend`-concrete NSToolbar leg is covered by the unit tests
//! in `macos_shared`/`macos_newcore` (item translation + click-flush
//! ordering) and by running the `toolbar-demo` example under a
//! new-core boot — NSToolbar/NSWindow construction needs a live main
//! thread, so no integration test can reach it (documented in
//! macos_newcore.rs).
#![cfg(feature = "new-core")]

use std::cell::Cell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_core::{Ref, StyleRules, Tokenized};
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
fn mounts_the_frozen_external_placeholder_on_hosts_without_a_toolbar() {
    let h = harness();
    let el = Toolbar(ToolbarProps::default()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    // The old walker's posture for an unregistered External: ONE
    // `create_external` keyed by the props type, no other structure.
    let log = h.ops().join("\n");
    assert_eq!(
        log, "create n0 external toolbar::newcore::ToolbarProps",
        "placeholder mount shape drifted from the old-core External path"
    );
}

/// Old-core parity: on targets with no toolbar impl the old core never
/// ran a handler, so the reactive `items` closure was never evaluated.
/// The placeholder arm must preserve that (no phantom subscriptions on
/// hosts that render nothing).
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
        "unsupported hosts must not evaluate items (old-core no-handler parity)"
    );
}

#[test]
fn author_style_lands_on_the_external_node() {
    let h = harness();
    let mut author = StyleRules::default();
    author.width = Some(Tokenized::Literal(runtime_core::Length::Px(120.0)));
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

    // Filled at mount (old-core RefFill::External timing). On this
    // host the node is host-mock's, so `set_visible` silently no-ops
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
        "unmount must release the external node (old walker cleanup-guard parity): {log:?}"
    );
}
