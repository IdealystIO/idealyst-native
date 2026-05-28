//! Reactive component props — `Reactive<T>` routed into a leaf must
//! update the leaf in place when its signals change, WITHOUT rebuilding
//! the parent.
//!
//! This is the seam idea-ui's `Typography` (and any user component with
//! a `Reactive<String>` text prop) relies on: `content = name_signal`
//! or `content = rx!(…)` produces a `Reactive::Dynamic`, which routes
//! through `IntoTextSource` to a `TextSource::Bound(Derived)` so the
//! framework installs an Effect that re-paints just the text node.
//! `content = "x".to_string()` is `Reactive::Static` — a fixed string,
//! no Effect, no reactivity.

#[path = "common/mod.rs"]
mod common;

use runtime_core::{signal, text, Reactive, Signal};

use common::{Event, TestRuntime};

/// A `Reactive<String>` built from a `Signal` (the `content = sig`
/// path) drives an in-place `UpdateText` on signal change — and does
/// NOT emit a fresh `CreateText` (the parent is not rebuilt).
#[test]
fn signal_backed_reactive_content_updates_text_in_place() {
    let rt = TestRuntime::new();
    let name: Signal<String> = signal!("Ada".to_string());

    // `content = name` lowers (via the invocation macro's `.into()`) to
    // `Reactive::from(name)` → `Reactive::Dynamic`. Build it directly
    // here and route it through `text()` exactly as the component does.
    let content: Reactive<String> = name.into();
    let _owner = rt.render(text(content).into());

    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "Ada")),
        "expected initial UpdateText 'Ada' from the reactive content Effect, got: {:#?}",
        rt.events(),
    );

    // Signal change → text re-paints in place. No CreateText (no rebuild).
    rt.backend_mut().clear_events();
    name.set("Lin".to_string());

    let post = rt.events();
    assert!(
        post.iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "Lin")),
        "expected UpdateText 'Lin' after signal.set, got: {:#?}",
        post,
    );
    assert!(
        !post.iter().any(|e| matches!(e, Event::CreateText { .. })),
        "reactive content must update the text node in place, not rebuild it: {:#?}",
        post,
    );
}

/// `rx!(expr)` — a computed `Reactive::Dynamic` over a signal — also
/// updates in place when the read signal changes.
#[test]
fn rx_computed_reactive_content_updates_in_place() {
    let rt = TestRuntime::new();
    let count: Signal<i32> = signal!(0);

    let content: Reactive<String> = runtime_core::rx!(format!("clicked {}×", count.get()));
    let _owner = rt.render(text(content).into());

    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "clicked 0×")),
        "expected initial 'clicked 0×', got: {:#?}",
        rt.events(),
    );

    rt.backend_mut().clear_events();
    count.set(3);

    let post = rt.events();
    assert!(
        post.iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "clicked 3×")),
        "expected UpdateText 'clicked 3×' after count.set(3), got: {:#?}",
        post,
    );
    assert!(
        !post.iter().any(|e| matches!(e, Event::CreateText { .. })),
        "rx! content must update in place, not rebuild: {:#?}",
        post,
    );
}

/// A static `Reactive::Static` content emits a plain `CreateText` and
/// installs NO reactive Effect — an unrelated signal change produces no
/// text events.
#[test]
fn static_reactive_content_is_not_reactive() {
    let rt = TestRuntime::new();
    let unrelated: Signal<i32> = signal!(0);

    // `content = "hi".to_string()` → `Reactive::Static`.
    let content: Reactive<String> = "hi".to_string().into();
    assert!(content.is_static());
    let _owner = rt.render(text(content).into());

    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::CreateText { content } if content == "hi")),
        "static content should create the text verbatim, got: {:#?}",
        rt.events(),
    );

    rt.backend_mut().clear_events();
    unrelated.set(99);

    assert!(
        rt.events().is_empty(),
        "static content must not subscribe to any signal; got events: {:#?}",
        rt.events(),
    );
}
