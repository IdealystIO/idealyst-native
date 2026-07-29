//! New-core suite for the private layer: `PrivateLayer(children)`
//! mounted through the scene registry on the shared `host-mock`
//! recording substrate.
//!
//! The new-core lowering is the passthrough container on every target
//! (the capture-EXCLUDED native windows are old-core-only — see
//! src/private_layer_newcore.rs), so what this pins is the contract the
//! whole design hinges on, same as the old-core lowering test in
//! tests/portable.rs: the layer mounts as ONE external node keyed by
//! `PrivateLayerProps` and its children realize INTO it. If the
//! containment broke, a future native port would parent the overlay's
//! children into the recorded tree.
#![cfg(feature = "new-core")]

use host_mock::Harness;
use runtime_scene::Realized;
use runtime_vocabulary::builders;
use runtime_vocabulary::glue::IntoElement;
use screen_recorder::PrivateLayer;

fn harness() -> Harness {
    // The SDK's boot registration seam.
    let h = Harness::with_registry(|r| screen_recorder::register_scene(r));
    h.mute(&["update_text", "on_node_unstyled", "mark_container"]);
    h
}

/// Children mount INSIDE the external node (`insert n0 <- …`) — the
/// old walker's `external.rs` create → children order, and the
/// containment the capture-exclusion design depends on.
#[test]
fn children_mount_inside_the_private_layer_node() {
    let h = harness();
    let el = PrivateLayer(vec![
        builders::text().content("controls").build(),
        builders::text().content("preview").build(),
    ])
    .into_element();
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops().join("\n");
    assert_eq!(
        log,
        "create n0 external screen_recorder::private_layer::PrivateLayerProps\n\
         create n1 text \"controls\"\n\
         insert n0 <- n1\n\
         create n2 text \"preview\"\n\
         insert n0 <- n2",
        "private-layer mount shape drifted from the External-with-children contract"
    );
}

/// Unmount releases the external node (old walker cleanup-guard parity).
#[test]
fn teardown_releases_the_private_layer_node() {
    let h = harness();
    let el = PrivateLayer(vec![builders::text().content("x").build()]).into_element();
    let realized: Realized<u32> = h.mount(el);
    let _ = h.take_log();

    drop(realized);
    let log = h.ops();
    assert!(
        log.iter().any(|l| l == "release_external n0"),
        "unmount must release the external node: {log:?}"
    );
}
