//! The authored surface — `Svg(SvgProps { .. }).with_style(…).bind(r)`
//! — mounted through the scene registry on the shared `host-mock`
//! recording substrate.
//!
//! The host target exercises the PLACEHOLDER arm of [`svg::register`]:
//! the frozen External degradation path (`create_external` +
//! children/style/ref-fill/teardown) a host with no real renderer gets.
//! The web (`innerHTML`) handler is `WebBackend`-concrete and the
//! iOS/Android vector walks are `IosBackend`/`AndroidBackend`-concrete;
//! those are covered by the per-target check gates plus live
//! verification (no host-side harness can construct a UIKit view).

use host_mock::Harness;
use runtime_shared::{Ref, StyleRules, Tokenized};
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

    // The placeholder posture: ONE `create_external` keyed by the props
    // type, no other structure. The type name is the SDK's external
    // "kind" identity — it must stay `svg::SvgProps`, the same string
    // the web handler stamps as `data-external-kind`.
    let log = h.ops().join("\n");
    assert_eq!(
        log, "create n0 external svg::SvgProps",
        "placeholder mount shape / external kind name drifted"
    );
}

#[test]
fn author_style_lands_on_the_external_node() {
    let h = harness();
    let mut author = StyleRules::default();
    author.width = Some(Tokenized::Literal(runtime_shared::Length::Px(120.0)));
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

    // Filled at mount; the host fallback ops report no intrinsic size —
    // the documented degradation (`UnsupportedOps`).
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
        "unmount must release the external node: {log:?}"
    );
}
