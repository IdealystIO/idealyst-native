//! New-core suite for canvas-native's NON-web arm: on hosts without a
//! ported renderer, `register` installs the frozen External-placeholder
//! degradation path (unregistered payloads panic on the scene registry,
//! so the placeholder handler is what preserves the old walker's
//! "renders a labeled box" posture). The real web (Canvas2D) handler is
//! `WebBackend`-concrete — covered by the wasm32 check gate + the
//! wave's live web verification.
#![cfg(all(feature = "new-core", not(target_arch = "wasm32")))]

use canvas_core::{Canvas, CanvasProps};
use host_mock::Harness;
use runtime_scene::Realized;
use runtime_vocabulary::glue::IntoElement;

fn harness() -> Harness {
    Harness::with_registry(|r| canvas_native::register(r))
}

#[test]
fn hosts_without_a_renderer_mount_the_external_placeholder() {
    let h = harness();
    let el = Canvas(CanvasProps::default()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops();
    assert_eq!(
        log.first().map(String::as_str),
        Some("create n0 external canvas_core::CanvasProps"),
        "placeholder mount shape drifted from the old-core External path: {log:?}"
    );
    // The fill default still lands (a placeholder box is visible).
    assert!(
        log.iter()
            .any(|l| l.starts_with("apply_style n0") && l.contains("Percent(100.0)")),
        "fill default must attach to the placeholder: {log:?}"
    );
}

#[test]
fn teardown_releases_the_placeholder() {
    let h = harness();
    let el = Canvas(CanvasProps::default()).into_element();
    let realized: Realized<u32> = h.mount(el);
    let _ = h.take_log();

    drop(realized);
    let log = h.ops();
    assert!(
        log.iter().any(|l| l == "release_external n0"),
        "unmount must release the external node: {log:?}"
    );
}
