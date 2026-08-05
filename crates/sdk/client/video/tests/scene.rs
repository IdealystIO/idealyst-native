//! The authored surface — `Video(VideoProps { .. }).with_style(…)
//! .bind(v)` — mounted through the scene registry on the shared
//! `host-mock` recording substrate.
//!
//! `host-mock` is not one of the concrete native registries, so
//! [`video::register`]'s registration-time type dispatch falls through
//! to the PLACEHOLDER arm here: the frozen External degradation path
//! (`create_external` + style/ref-fill/teardown) a host with no player
//! gets. The web (`<video>`) handler is `WebBackend`-concrete and the
//! macOS/iOS/Android players are `*Backend`-concrete; those are covered
//! by the per-target check gates plus live verification (no host-side
//! harness can construct an AVPlayer).

use host_mock::Harness;
use runtime_shared::{Ref, StyleRules, Tokenized};
use runtime_scene::Realized;
use runtime_vocabulary::glue::IntoElement;
use video::prelude::*;

fn harness() -> Harness {
    // The SDK's boot registration seam — the same fn an app passes to
    // `backend_web::newcore::start_in` / `backend_ssr::newcore::
    // render_path_with`.
    Harness::with_registry(|r| video::register(r))
}

#[test]
fn mounts_the_frozen_external_placeholder_on_hosts_without_a_player() {
    let h = harness();
    let el = Video(VideoProps::default()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    // The placeholder posture: ONE `create_external` keyed by the props
    // type, no other structure. The type name is the SDK's external
    // "kind" identity — it must stay `video::VideoProps`, the same
    // string the web handler stamps as `data-external-kind`.
    let log = h.ops().join("\n");
    assert_eq!(
        log, "create n0 external video::VideoProps",
        "placeholder mount shape / external kind name drifted"
    );
}

#[test]
fn author_style_lands_on_the_external_node() {
    let h = harness();
    let mut author = StyleRules::default();
    author.width = Some(Tokenized::Literal(runtime_shared::Length::Px(320.0)));
    let el = Video(VideoProps::default()).with_style(author).into_element();
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops();
    assert!(
        log.iter().any(|l| l.starts_with("apply_style n0")),
        "author style must attach to the external node: {log:?}"
    );
}

#[test]
fn bind_fills_the_ref_at_mount() {
    let h = harness();
    let r: Ref<VideoHandle> = Ref::new();
    let el = Video(VideoProps::default()).bind(r.clone()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    // Filled at mount. The host node is not a node any real player ops
    // recognize, so every op degrades to its documented default —
    // position 0.0.
    let position = r.with(|handle| handle.position());
    assert_eq!(
        position,
        Some(0.0),
        "ref must be filled at mount and degrade to the default ops"
    );
}

#[test]
fn teardown_releases_the_external_node() {
    let h = harness();
    let el = Video(VideoProps::default()).into_element();
    let realized: Realized<u32> = h.mount(el);
    let _ = h.take_log();

    drop(realized);
    let log = h.ops();
    assert!(
        log.iter().any(|l| l == "release_external n0"),
        "unmount must release the external node: {log:?}"
    );
}

/// `VideoHandle: PartialEq` compares the mounted node, so a handle equals its own
/// clones and never equals a handle onto a different mounted `Video`.
/// Required for the handle to sit in a `Signal` at all (`Signal<T>` bounds
/// `T: PartialEq` at creation and `get`), and only this crate can supply
/// the impl — the orphan rule blocks an app crate.
///
/// The unequal half is the load-bearing one: every handle on a target
/// shares the same `&'static` ops vtable, so an impl that compared `ops`
/// would collapse two distinct nodes into one and swallow a re-target.
#[test]
fn video_handles_compare_by_mounted_node_identity() {
    let h = harness();

    let ra: Ref<VideoHandle> = Ref::new();
    let rb: Ref<VideoHandle> = Ref::new();
    let _a: Realized<u32> = h.mount(Video(VideoProps::default()).bind(ra.clone()).into_element());
    let _b: Realized<u32> = h.mount(Video(VideoProps::default()).bind(rb.clone()).into_element());

    let ha = ra.with(|handle| handle.clone()).expect("a mounted");
    let hb = rb.with(|handle| handle.clone()).expect("b mounted");

    assert!(ha == ha.clone(), "clones of one handle must compare equal");
    assert!(ha != hb, "handles onto two different mounted nodes must compare unequal");
}
