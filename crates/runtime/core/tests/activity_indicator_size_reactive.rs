//! Reactive `size` plumbing for `Element::ActivityIndicator` — a live `size`
//! source resizes the spinner in place (`Backend::update_activity_indicator_size`)
//! WITHOUT rebuilding the node, while a fixed `size` installs no effect.
//!
//! Web re-applies the CSS diameter in place; native spinners
//! (`UIActivityIndicatorView` / `ProgressBar`) fix their style at construction
//! and inherit the backend no-op. The contract they share is this single
//! `update_activity_indicator_size` call, so proving the source threads
//! end-to-end pins it. Mirrors the `secure` reactive test.

#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use mock_backend::Event;
use runtime::TestRuntime;
use runtime_core::primitives::activity_indicator::{activity_indicator, ActivityIndicatorSize};
use runtime_core::{signal, IntoElement, Signal};

#[test]
fn reactive_size_updates_in_place_without_rebuild() {
    let rt = TestRuntime::new();
    let large: Signal<bool> = signal!(false);

    let tree = activity_indicator()
        .size_reactive(move || {
            if large.get() {
                ActivityIndicatorSize::Large
            } else {
                ActivityIndicatorSize::Small
            }
        })
        .into_element();
    let _owner = rt.render(tree);

    // Born at the closure's initial value (one CreateActivityIndicator).
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::CreateActivityIndicator)),
        "spinner is created once: {:?}",
        rt.events()
    );

    // Flip → the size must update via an in-place call, never a rebuild.
    rt.backend_mut().clear_events();
    large.set(true);
    let evs = rt.events();
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::UpdateActivityIndicatorSize { size: ActivityIndicatorSize::Large, .. }
        )),
        "flipping the size source must push update_activity_indicator_size in place: {:?}",
        evs
    );
    assert!(
        !evs.iter().any(|e| matches!(e, Event::CreateActivityIndicator)),
        "the spinner must NOT be rebuilt — no new CreateActivityIndicator: {:?}",
        evs
    );
}

#[test]
fn static_size_installs_no_update_effect() {
    let rt = TestRuntime::new();
    let _owner = rt.render(
        activity_indicator()
            .size(ActivityIndicatorSize::Large)
            .into_element(),
    );

    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::CreateActivityIndicator)),
        "static size threads to create_activity_indicator"
    );
    assert!(
        !rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateActivityIndicatorSize { .. })),
        "a fixed size installs no effect, so update_activity_indicator_size never fires: {:?}",
        rt.events()
    );
}
