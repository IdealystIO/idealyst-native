//! Reactive `alt` plumbing for `Element::Image` — a live `alt` source swaps the
//! native alt / a11y label in place (`Backend::update_image_alt`) WITHOUT
//! rebuilding the image, while a fixed `alt` installs no effect at all.
//!
//! Mirrors the reactive `src` shape (and the `secure` reactive test). The web
//! `<img alt>` / native accessibility-label mechanisms hang off this single
//! `update_image_alt` call, so proving the source threads end-to-end pins the
//! contract.

#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use mock_backend::Event;
use runtime::TestRuntime;
use runtime_core::{image, signal, IntoElement, Signal};

#[test]
fn reactive_alt_updates_in_place_without_rebuild() {
    let rt = TestRuntime::new();
    let described: Signal<bool> = signal!(false);

    let tree = image("https://example.com/x.png")
        .alt_reactive(move || {
            if described.get() {
                Some("A described picture".to_string())
            } else {
                None
            }
        })
        .into_element();
    let _owner = rt.render(tree);

    // Born at the closure's initial value (None).
    assert!(
        rt.events().iter().any(|e| matches!(
            e,
            Event::CreateImage { alt: None, .. }
        )),
        "image is created with the closure's initial alt: {:?}",
        rt.events()
    );

    // Flip → the alt must swap via an in-place update, never a rebuild.
    rt.backend_mut().clear_events();
    described.set(true);
    let evs = rt.events();
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::UpdateImageAlt { alt: Some(a), .. } if a == "A described picture"
        )),
        "flipping the alt source must push update_image_alt in place: {:?}",
        evs
    );
    assert!(
        !evs.iter().any(|e| matches!(e, Event::CreateImage { .. })),
        "the image must NOT be rebuilt — no new CreateImage: {:?}",
        evs
    );
}

#[test]
fn static_alt_installs_no_update_effect() {
    let rt = TestRuntime::new();
    let _owner = rt.render(
        image("https://example.com/x.png")
            .alt("Static label".to_string())
            .into_element(),
    );

    assert!(
        rt.events().iter().any(|e| matches!(
            e,
            Event::CreateImage { alt: Some(a), .. } if a == "Static label"
        )),
        "static alt threads to create_image"
    );
    assert!(
        !rt.events().iter().any(|e| matches!(e, Event::UpdateImageAlt { .. })),
        "a fixed alt installs no effect, so update_image_alt never fires: {:?}",
        rt.events()
    );
}
