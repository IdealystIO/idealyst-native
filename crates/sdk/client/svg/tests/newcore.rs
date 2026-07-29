//! New-core suite: the SAME authored surface —
//! `Svg(SvgProps { .. }).with_style(…).bind(r)` — mounted through the
//! scene registry on the shared `host-mock` recording substrate.
//!
//! The host target exercises the NON-web arm of [`svg::register`]: the
//! frozen External-placeholder degradation path (`create_external` +
//! children/style/ref-fill/teardown), which pins the old walker's
//! `external.rs` sequence for a backend with no real handler. The web
//! (`innerHTML`) handler is `WebBackend`-concrete and is covered by the
//! wasm32 check gate + the wave's live web verification.
#![cfg(feature = "new-core")]

use host_mock::Harness;
use runtime_core::{Ref, StyleRules, Tokenized};
use runtime_scene::Realized;
use runtime_vocabulary::glue::IntoElement;
use svg::prelude::*;

fn harness() -> Harness {
    // The SDK's boot registration seam — the same fn an app passes to
    // `backend_web::newcore::start_in` / `backend_ssr::newcore::
    // render_path_with`.
    Harness::with_registry(|r| svg::register(r))
}

#[test]
fn mounts_the_frozen_external_placeholder_on_hosts_without_a_renderer() {
    let h = harness();
    let el = Svg(SvgProps::default()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    // The old walker's posture for an unregistered External: ONE
    // `create_external` keyed by the props type, no other structure.
    let log = h.ops().join("\n");
    assert_eq!(
        log, "create n0 external svg::newcore::SvgProps",
        "placeholder mount shape drifted from the old-core External path"
    );
}

#[test]
fn author_style_lands_on_the_external_node() {
    let h = harness();
    let mut author = StyleRules::default();
    author.width = Some(Tokenized::Literal(runtime_core::Length::Px(120.0)));
    let el = Svg(SvgProps::default()).with_style(author).into_element();
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops();
    assert!(
        log.iter().any(|l| l.starts_with("apply_style n0")),
        "author style must attach to the external node: {log:?}"
    );
}

#[test]
fn bind_fills_the_ref_at_mount_with_the_fallback_ops() {
    let h = harness();
    let r: Ref<SvgHandle> = Ref::new();
    let el = Svg(SvgProps::default()).bind(r.clone()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    // Filled at mount (old-core RefFill::External timing); the host
    // fallback ops report no intrinsic size — the documented
    // degradation (`UnsupportedOps`).
    let size = r.with(|handle| handle.intrinsic_size());
    assert_eq!(
        size,
        Some(None),
        "ref must be filled at mount and fall back to no intrinsic size"
    );
}

#[test]
fn teardown_releases_the_external_node() {
    let h = harness();
    let el = Svg(SvgProps::default()).into_element();
    let realized: Realized<u32> = h.mount(el);
    let _ = h.take_log();

    drop(realized);
    let log = h.ops();
    assert!(
        log.iter().any(|l| l == "release_external n0"),
        "unmount must release the external node (old walker cleanup-guard parity): {log:?}"
    );
}
