//! Reactive `url` plumbing for `Element::Link` — a live `url` source swaps the
//! native `<a href>` in place (`Backend::update_link_url`) WITHOUT rebuilding
//! the link, while a fixed `url` installs no effect at all.
//!
//! The native href mechanisms (web `anchor.set_href`) aren't reachable from a
//! host test, but they all hang off exactly this `update_link_url` call, so
//! proving the source threads end-to-end pins the contract. Mirrors the
//! `secure` reactive test.

// Include only the harness pieces we need — see `text_input_secure_reactive`
// for why we don't pull the whole `common` module.
#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use mock_backend::Event;
use runtime::TestRuntime;
use runtime_core::{external_link, signal, IntoElement, Signal};

#[test]
fn reactive_url_updates_in_place_without_rebuild() {
    let rt = TestRuntime::new();
    let toggled: Signal<bool> = signal!(false);

    // `url` follows the signal: example.com while false, docs while true.
    let tree = external_link("", Vec::new())
        .url(move || {
            if toggled.get() {
                "https://example.com/docs".to_string()
            } else {
                "https://example.com".to_string()
            }
        })
        .into_element();
    let _owner = rt.render(tree);

    // Born at the closure's initial value.
    assert!(
        rt.events().iter().any(|e| matches!(
            e,
            Event::CreateLink { url, .. } if url == "https://example.com"
        )),
        "link is created at the closure's initial url: {:?}",
        rt.events()
    );

    // Flip → the href must swap via an in-place update, never a rebuild.
    rt.backend_mut().clear_events();
    toggled.set(true);
    let evs = rt.events();
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::UpdateLinkUrl { url, .. } if url == "https://example.com/docs"
        )),
        "flipping the url source must push update_link_url in place: {:?}",
        evs
    );
    assert!(
        !evs.iter().any(|e| matches!(e, Event::CreateLink { .. })),
        "the link must NOT be rebuilt — no new CreateLink: {:?}",
        evs
    );
}

#[test]
fn static_url_installs_no_update_effect() {
    let rt = TestRuntime::new();
    // A plain `external_link` with no `.url(...)` is a fixed href.
    let _owner = rt.render(external_link("https://example.com", Vec::new()).into_element());

    assert!(
        rt.events().iter().any(|e| matches!(
            e,
            Event::CreateLink { url, .. } if url == "https://example.com"
        )),
        "static url threads to create_link"
    );
    assert!(
        !rt.events().iter().any(|e| matches!(e, Event::UpdateLinkUrl { .. })),
        "a fixed url installs no effect, so update_link_url never fires: {:?}",
        rt.events()
    );
}
