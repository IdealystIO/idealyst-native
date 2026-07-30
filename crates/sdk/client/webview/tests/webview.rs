//! `WebView(WebViewProps { .. }).with_style(…).bind(r)` mounted through
//! the scene registry on the shared `host-mock` recording substrate.
//!
//! The host target exercises the NON-web arm of [`webview::register`]:
//! the External-placeholder degradation path (`create_external` +
//! style/ref-fill/teardown). The web (`<iframe>`) handler is
//! `WebBackend`-concrete and is covered by the wasm32 check gate.

use host_mock::Harness;
use runtime_shared::{Ref, StyleRules, Tokenized};
use runtime_scene::Realized;
use runtime_vocabulary::glue::{BuildElement, IntoElement};
use webview::prelude::*;

fn harness() -> Harness {
    // The SDK's boot registration seam — the same fn an app passes to
    // `backend_web::newcore::start_in` / `backend_ssr::newcore::
    // render_path_with`.
    Harness::with_registry(|r| webview::register(r))
}

#[test]
fn mounts_the_frozen_external_placeholder_on_hosts_without_a_renderer() {
    let h = harness();
    let el = WebView(WebViewProps::default()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    // The placeholder posture: ONE `create_external` keyed by the
    // props type, no other structure.
    let log = h.ops().join("\n");
    assert_eq!(
        log, "create n0 external webview::WebViewProps",
        "placeholder mount shape drifted from the External degradation path"
    );
}

#[test]
fn author_style_lands_on_the_external_node() {
    let h = harness();
    let mut author = StyleRules::default();
    author.width = Some(Tokenized::Literal(runtime_shared::Length::Px(320.0)));
    let el = WebView(WebViewProps::default())
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
fn bind_fills_the_ref_at_mount_and_ops_fall_back() {
    let h = harness();
    let r: Ref<WebViewHandle> = Ref::new();
    let el = WebView(WebViewProps::default())
        .bind(r.clone())
        .into_element();
    let _realized: Realized<u32> = h.mount(el);

    // Filled at mount; the host fallback ops degrade via
    // `UnsupportedOps` — post_message/reload no-op, execute_js errors.
    let result = r.with(|handle| {
        handle.post_message("ping");
        handle.reload();
        handle.execute_js("1 + 1")
    });
    let result = result.expect("ref must be filled at mount");
    assert!(
        result.is_err(),
        "host fallback ops must report execute_js as unsupported"
    );
}

#[test]
fn teardown_releases_the_external_node() {
    let h = harness();
    let el = WebView(WebViewProps::default()).into_element();
    let realized: Realized<u32> = h.mount(el);
    let _ = h.take_log();

    drop(realized);
    let log = h.ops();
    assert!(
        log.iter().any(|l| l == "release_external n0"),
        "unmount must release the external node (teardown-guard contract): {log:?}"
    );
}

/// The `WebView` `ui!` tag contract: the PascalCase struct-literal
/// dispatch goes through the glue `BuildElement`, and the built element
/// mounts the same placeholder.
#[test]
fn build_element_tag_contract_mounts_the_same_external() {
    let h = harness();
    let el = BuildElement::build(WebView {
        url: url("https://example.com"),
        ..BuildElement::defaults()
    });
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops().join("\n");
    assert_eq!(
        log, "create n0 external webview::WebViewProps",
        "tag dispatch must build the same external item"
    );
}
