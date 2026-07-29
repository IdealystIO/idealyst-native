//! New-core suite: the SAME authored surface —
//! `MapView(MapViewProps { .. }).with_style(…)` — mounted through the
//! scene registry on the shared `host-mock` recording substrate.
//!
//! The host target exercises the NON-web arm of [`maps::register`]: the
//! frozen External-placeholder degradation path (`create_external` +
//! style/teardown), which pins the old walker's `external.rs` sequence
//! for a backend with no real handler. The web (iframe) handler is
//! `WebBackend`-concrete and is covered by the wasm32 check gate.
#![cfg(feature = "new-core")]

use host_mock::Harness;
use maps::{MapView, MapViewProps};
use runtime_core::{StyleRules, Tokenized};
use runtime_scene::Realized;
use runtime_vocabulary::glue::IntoElement;

fn props() -> MapViewProps {
    MapViewProps {
        lat: 37.7749,
        lon: -122.4194,
        zoom: 12.0,
    }
}

fn harness() -> Harness {
    // The SDK's boot registration seam — the same fn an app passes to
    // `backend_web::newcore::start_in` / `backend_ssr::newcore::
    // render_path_with`.
    Harness::with_registry(|r| maps::register(r))
}

#[test]
fn mounts_the_frozen_external_placeholder_on_hosts_without_a_map() {
    let h = harness();
    let el = MapView(props()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    // The old walker's posture for an unregistered External: ONE
    // `create_external` keyed by the props type, no other structure.
    let log = h.ops().join("\n");
    assert_eq!(
        log, "create n0 external maps_core::MapViewProps",
        "placeholder mount shape drifted from the old-core External path"
    );
}

#[test]
fn author_style_lands_on_the_external_node() {
    let h = harness();
    let mut author = StyleRules::default();
    author.height = Some(Tokenized::Literal(runtime_core::Length::Px(300.0)));
    let el = MapView(props()).with_style(author).into_element();
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops();
    assert!(
        log.iter().any(|l| l.starts_with("apply_style n0")),
        "author style must attach to the external node: {log:?}"
    );
}

#[test]
fn teardown_releases_the_external_node() {
    let h = harness();
    let el = MapView(props()).into_element();
    let realized: Realized<u32> = h.mount(el);
    let _ = h.take_log();

    drop(realized);
    let log = h.ops();
    assert!(
        log.iter().any(|l| l == "release_external n0"),
        "unmount must release the external node (old walker cleanup-guard parity): {log:?}"
    );
}
