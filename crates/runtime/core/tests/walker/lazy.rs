//! Walker coverage for `Element::Lazy` — the placeholder vs.
//! chunk-body gate driven by `Backend::renders_lazy_chunks()`.
//!
//! The gate exists for SSR: native chunk loaders resolve synchronously
//! on first poll (the chunk's `async fn` is compiled in), so without
//! the gate the server would emit the chunk's body and diverge from
//! the live client's `.placeholder(…)`. SSR overrides
//! `renders_lazy_chunks` to `false`; this suite pins both branches of
//! the gate against the walker so a regression in one is caught here
//! and a regression in the other is caught by `SsrBackend`'s
//! `renders_lazy_chunks_returns_false` test.
//!
//! Gated on the `async-driver` feature because the spawn_async block
//! in `walker/lazy.rs` is only compiled in then — without
//! `async-driver` the chunk path is a no-op regardless of the gate, so
//! the test couldn't distinguish.
//!
//! Run with: `cargo test -p runtime-core --features async-driver --test walker lazy`

#![cfg(feature = "async-driver")]

use runtime_core::primitives::lazy::lazy_split;
use runtime_core::{text, IntoElement};

use crate::common::{Event, MockBackendConfig, TestRuntime};

/// REGRESSION GUARD: when `Backend::renders_lazy_chunks()` is `false`
/// (the SSR contract), the walker mounts the placeholder and does NOT
/// resolve the chunk loader. Otherwise the chunk's body ends up in the
/// server HTML, diverges from the client's placeholder, and triggers a
/// hydration remount of the subtree (cratering GPU-canvas chunks).
#[test]
fn placeholder_only_when_renders_lazy_chunks_is_false() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        renders_lazy_chunks: Some(false),
        ..MockBackendConfig::default()
    });

    let elem = lazy_split(|| Box::pin(async { text("CHUNK").into_element() }))
        .placeholder(|| text("LOADING").into_element())
        .into_element();
    let _owner = rt.render(elem);

    let events = rt.events();
    let texts: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::CreateText { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"LOADING"),
        "placeholder text must be mounted; events: {events:#?}"
    );
    assert!(
        !texts.contains(&"CHUNK"),
        "chunk body must NOT render when renders_lazy_chunks=false (the SSR \
         contract); events: {events:#?}"
    );
    // No clear_children — the placeholder is the final state, never
    // wiped by a chunk swap-in.
    assert!(
        !events.iter().any(|e| matches!(e, Event::ClearChildren { .. })),
        "clear_children must NOT fire when the chunk doesn't load; events: {events:#?}"
    );
}

/// COMPLEMENT: with the trait default (`renders_lazy_chunks() = true`,
/// what every live backend reports) the walker resolves the loader and
/// swaps the chunk body in over the placeholder. Pins the live-path
/// half of the gate so a regression that turned the chunk into a
/// permanent placeholder loop would surface here.
///
/// The loader's future resolves synchronously on first poll on native;
/// `spawn_async`'s pollster fallback blocks the thread until the swap
/// completes, so by the time `render()` returns the chunk is mounted.
#[test]
fn chunk_body_renders_when_renders_lazy_chunks_is_true() {
    // Default config — `renders_lazy_chunks` is `None`, which maps to
    // the trait default of `true` in the MockBackend override.
    let rt = TestRuntime::new();

    let elem = lazy_split(|| Box::pin(async { text("CHUNK").into_element() }))
        .placeholder(|| text("LOADING").into_element())
        .into_element();
    let _owner = rt.render(elem);

    let events = rt.events();
    let texts: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::CreateText { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"LOADING"),
        "placeholder mounts first; events: {events:#?}"
    );
    assert!(
        texts.contains(&"CHUNK"),
        "chunk body must render once the loader resolves; events: {events:#?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::ClearChildren { .. })),
        "clear_children must fire to evict the placeholder before the chunk \
         is inserted; events: {events:#?}"
    );
}
