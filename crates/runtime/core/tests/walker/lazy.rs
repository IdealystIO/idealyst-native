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

    let elem = lazy_split(|| Box::pin(async { Ok(text("CHUNK").into_element()) }))
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

    let elem = lazy_split(|| Box::pin(async { Ok(text("CHUNK").into_element()) }))
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

/// Collect the text content of every `CreateText` event so far.
fn created_texts(rt: &TestRuntime) -> Vec<String> {
    rt.events()
        .iter()
        .filter_map(|e| match e {
            Event::CreateText { content } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

/// When the loader fails, the walker fires `Error` and mounts the author's
/// `.on_error(..)` UI (with the failure message) in place of the loading UI —
/// the chunk body never renders. Before this the loader couldn't fail
/// (`Output = Element`); a chunk fetch error was swallowed to an empty
/// view with no way for the author to react.
#[test]
fn error_ui_renders_on_load_failure() {
    let rt = TestRuntime::new();

    let elem = lazy_split(|| {
        Box::pin(async {
            let r: Result<runtime_core::Element, String> = Err("boom".to_string());
            r
        })
    })
    .placeholder(|| text("LOADING").into_element())
    .on_error(|e| text(format!("ERR:{}", e.message())).into_element())
    .into_element();
    let _owner = rt.render(elem);

    let texts = created_texts(&rt);
    assert!(
        texts.iter().any(|t| t == "ERR:boom"),
        "error UI (with message) must mount on load failure; texts: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t == "CHUNK"),
        "chunk body must not render when the load failed; texts: {texts:?}"
    );
}

/// `LazyError::retry()` re-drives the loader. A loader that fails once then
/// succeeds ends up showing the chunk body after the error UI's retry handle
/// fires — proving the retry path re-enters the load (under the same chunk
/// scope) rather than being cosmetic.
#[test]
fn retry_reloads_after_error() {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    let rt = TestRuntime::new();

    let attempts = Rc::new(Cell::new(0u32));
    let retry_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));

    let elem = {
        let attempts = attempts.clone();
        lazy_split(move || {
            let n = attempts.get();
            attempts.set(n + 1);
            Box::pin(async move {
                if n == 0 {
                    Err("first attempt fails".to_string())
                } else {
                    Ok(text("CHUNK").into_element())
                }
            })
        })
        .placeholder(|| text("LOADING").into_element())
        .on_error({
            let retry_slot = retry_slot.clone();
            move |e| {
                // Capture the retry handle so the test can fire it (a real app
                // wires it to a button's on_press).
                *retry_slot.borrow_mut() = Some(e.retry());
                text("ERR").into_element()
            }
        })
        .into_element()
    };
    let _owner = rt.render(elem);

    assert!(
        created_texts(&rt).iter().any(|t| t == "ERR"),
        "first attempt fails → error UI mounts"
    );
    assert_eq!(attempts.get(), 1, "loader invoked once so far");

    // Fire retry — re-drives the loader, which succeeds this time.
    let retry = retry_slot.borrow().clone().expect("error UI captured a retry handle");
    retry();

    assert_eq!(attempts.get(), 2, "retry re-invokes the loader");
    assert!(
        created_texts(&rt).iter().any(|t| t == "CHUNK"),
        "retry's successful load swaps the chunk body in"
    );
}

/// `#[component(lazy)]` compiles the body into a chunk and yields `Element::Lazy`
/// whose props (the fn parameters) cross the split. On native the chunk resolves
/// synchronously, so rendering the tag mounts the body. Proves the macro wires
/// props → loader → body end to end.
#[test]
fn lazy_component_attribute_mounts_body_with_props() {
    use runtime_core::{component, BuildElement};

    /// A "heavy" component, always chunked.
    #[component(lazy)]
    fn Heavy(#[prop(static)] label: String) -> runtime_core::Element {
        text(label.as_str()).into_element()
    }

    let rt = TestRuntime::new();
    // The `ui!` tag form lowers to exactly this `BuildElement::build`.
    let elem = Heavy { label: "HEAVY-BODY".to_string(), ..Default::default() }.build();
    let _owner = rt.render(elem);

    assert!(
        created_texts(&rt).iter().any(|t| t == "HEAVY-BODY"),
        "lazy component's chunk body must mount (props threaded across the split)"
    );
}

/// Regression: a ZERO-parameter `#[component(lazy)]` must compile and mount.
/// The inline-props detector used to route empty parameter lists to the legacy
/// marker-struct path, and lazy mode then hard-errored with "requires inline
/// props" — forcing authors to add a dummy parameter. Zero-arg lazy components
/// are the common shape for route screens and heavy-SDK corners, so the empty
/// list now counts as the (trivially) inline shape: the generated props struct
/// carries only the `loading` / `error` config fields.
#[test]
fn regression_zero_arg_lazy_component_compiles_and_mounts() {
    use runtime_core::{component, BuildElement};

    /// A heavy component that takes no input.
    #[component(lazy)]
    fn HeavyZero() -> runtime_core::Element {
        text("ZERO-ARG-BODY").into_element()
    }

    let rt = TestRuntime::new();
    // The generated props struct still has the config fields (proof the
    // inline glue ran): `loading` is settable even with no data props.
    let elem = HeavyZero {
        loading: (|| text("ZERO-LOADING").into_element()).into(),
        ..Default::default()
    }
    .build();
    let _owner = rt.render(elem);

    assert!(
        created_texts(&rt).iter().any(|t| t == "ZERO-ARG-BODY"),
        "zero-arg lazy component's chunk body must mount"
    );
}

/// `#[component(lazy, retryable)]` derives `Clone` on the props (so the loader
/// can be re-invoked) and still mounts the body normally. Compile-plus-behavior
/// coverage of the retryable codegen path (the non-retryable one is the test
/// above).
#[test]
fn lazy_component_retryable_variant_mounts_body() {
    use runtime_core::{component, BuildElement};

    /// A heavy component whose load is retryable.
    #[component(lazy, retryable)]
    fn HeavyRetry(#[prop(static)] label: String) -> runtime_core::Element {
        text(label.as_str()).into_element()
    }

    let rt = TestRuntime::new();
    let elem = HeavyRetry { label: "RETRY-BODY".to_string(), ..Default::default() }.build();
    let _owner = rt.render(elem);

    assert!(
        created_texts(&rt).iter().any(|t| t == "RETRY-BODY"),
        "retryable lazy component must mount its chunk body"
    );
}

/// The `loading` config prop on a lazy component drives the loading UI. On the
/// SSR path (`renders_lazy_chunks = false`) the loader never runs, so the
/// loading UI is the final output — proving the generated `loading` prop is
/// wired into the builder.
#[test]
fn lazy_component_loading_prop_shows_on_ssr() {
    use runtime_core::{component, BuildElement};

    /// Heavy, with an author-provided loading UI.
    #[component(lazy)]
    fn HeavySsr(#[prop(static)] label: String) -> runtime_core::Element {
        text(label.as_str()).into_element()
    }

    let rt = TestRuntime::with_config(MockBackendConfig {
        renders_lazy_chunks: Some(false),
        ..MockBackendConfig::default()
    });
    let elem = HeavySsr {
        label: "BODY".to_string(),
        loading: (|| text("LOADING-UI").into_element()).into(),
        ..Default::default()
    }
    .build();
    let _owner = rt.render(elem);

    let texts = created_texts(&rt);
    assert!(texts.iter().any(|t| t == "LOADING-UI"), "loading prop must render; {texts:?}");
    assert!(!texts.iter().any(|t| t == "BODY"), "chunk body must not render on SSR; {texts:?}");
}
