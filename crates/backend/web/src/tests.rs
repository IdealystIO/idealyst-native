//! Browser-side unit tests for `WebBackend`.
//!
//! Test functions use `#[wasm_bindgen_test]` instead of plain
//! `#[test]`; the `wasm_bindgen_test_configure!` line below
//! switches the runner into browser mode so `web_sys::Node` and
//! friends work.
//!
//! Inline rather than a `tests/` directory because the tests need
//! `pub(crate)` access to `WebBackend::node_id`.
//!
//! ## Running locally
//!
//! From the repo root:
//!
//! ```sh
//! # Safari (built into macOS — one-time setup):
//! sudo safaridriver --enable           # once per machine
//! cd crates/backend/web
//! wasm-pack test --headless --safari --release
//!
//! # Chrome (cross-platform, needs chromedriver on PATH):
//! brew install --cask chromedriver     # macOS, once
//! cd crates/backend/web
//! wasm-pack test --headless --chrome --release
//! ```
//!
//! `wasm-pack test` takes ~10s on a clean build and a few seconds
//! on incremental. Tests don't run as part of plain `cargo test`
//! because `backend-web` only compiles for `wasm32-unknown-unknown`.

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

use crate::WebBackend;
use wasm_bindgen::JsCast;

/// Set up a `#app` element in the document so `WebBackend::new`
/// can find a mount point. Idempotent — drops any prior `#app` and
/// re-creates it so tests don't bleed state.
fn install_mount() {
    let doc = web_sys::window().expect("window").document().expect("document");
    if let Some(existing) = doc.get_element_by_id("app") {
        existing.remove();
    }
    let div = doc.create_element("div").expect("create div");
    div.set_id("app");
    doc.body()
        .expect("body")
        .append_child(&div)
        .expect("append #app to body");
}

// ---------------------------------------------------------------------------
// node_id invariant — the regression test for the gradient-animation bug.
// ---------------------------------------------------------------------------

/// REGRESSION TEST.
///
/// Two `web_sys::Node` wrappers around the same JS DOM object must
/// resolve to the same `node_id`. Previously, `node_id` keyed off
/// the Rust wrapper's address (`*const Node`), so the same DOM
/// element could end up with multiple ids if the framework
/// constructed multiple wrappers (e.g. one via `apply_style` and
/// another via `Ref<ViewHandle>`'s `Rc<dyn Any>` round-trip). That
/// silently broke per-node state because writes via wrapper A
/// stamped state under id 1, but reads via wrapper B looked under
/// id 2.
///
/// Today's `node_id` resolves via a JS-side `WeakMap<Node, u32>`
/// (see `runtime/js/node_ids.js`), so the JS object's identity is
/// what determines the id. This test pins the invariant down.
#[wasm_bindgen_test]
fn node_id_is_stable_across_distinct_rust_wrappers_for_same_dom_node() {
    install_mount();
    let mut backend = WebBackend::new("#app");

    // Build an element directly so we can construct multiple
    // wrappers around the same JS object below.
    let doc = web_sys::window().unwrap().document().unwrap();
    let element = doc.create_element("div").expect("create element");

    // Two SEPARATE Rust wrappers around the SAME JS object. Each
    // `.clone().into()` produces a fresh `web_sys::Node` wrapper —
    // different Rust stack addresses, same underlying JS Element.
    let wrapper_a: web_sys::Node = element.clone().unchecked_into();
    let wrapper_b: web_sys::Node = element.clone().unchecked_into();
    let wrapper_c: web_sys::Node = element.unchecked_into();

    // Sanity: the wrapper addresses really are different in Rust.
    // If they ever happened to coincide, the test wouldn't be
    // exercising the WeakMap fallback path it's designed to test.
    let pa = &wrapper_a as *const web_sys::Node;
    let pb = &wrapper_b as *const web_sys::Node;
    let pc = &wrapper_c as *const web_sys::Node;
    assert_ne!(pa, pb, "wrappers should occupy different Rust addresses");
    assert_ne!(pa, pc);
    assert_ne!(pb, pc);

    let id_a = backend.node_id(&wrapper_a);
    let id_b = backend.node_id(&wrapper_b);
    let id_c = backend.node_id(&wrapper_c);

    assert_eq!(
        id_a, id_b,
        "wrapper_a and wrapper_b reference the same JS object — node_id must match",
    );
    assert_eq!(
        id_b, id_c,
        "wrapper_c also references the same JS object — node_id must match",
    );
}

/// Repeat lookup: calling `node_id` twice with the SAME wrapper
/// must return the same id. (Trivially true with the WeakMap
/// design, but worth pinning down — a future "fast cache" path
/// that returned a stale id on cache collision would fail here
/// before it could ship.)
#[wasm_bindgen_test]
fn node_id_cache_returns_same_id_for_same_wrapper() {
    install_mount();
    let mut backend = WebBackend::new("#app");

    let doc = web_sys::window().unwrap().document().unwrap();
    let wrapper: web_sys::Node = doc.create_element("div").unwrap().unchecked_into();

    let id_first = backend.node_id(&wrapper);
    let id_second = backend.node_id(&wrapper);
    assert_eq!(
        id_first, id_second,
        "second node_id call with the same wrapper must return the cached id",
    );
}

/// Distinct DOM elements must get distinct ids. The WeakMap is
/// keyed by JS object identity, so different elements always get
/// different `next++` allocations.
#[wasm_bindgen_test]
fn node_id_returns_distinct_ids_for_distinct_dom_elements() {
    install_mount();
    let mut backend = WebBackend::new("#app");

    let doc = web_sys::window().unwrap().document().unwrap();
    let n1: web_sys::Node = doc.create_element("div").unwrap().unchecked_into();
    let n2: web_sys::Node = doc.create_element("div").unwrap().unchecked_into();
    let n3: web_sys::Node = doc.create_element("span").unwrap().unchecked_into();

    let id1 = backend.node_id(&n1);
    let id2 = backend.node_id(&n2);
    let id3 = backend.node_id(&n3);
    assert_ne!(id1, id2, "distinct elements must get distinct ids");
    assert_ne!(id1, id3);
    assert_ne!(id2, id3);
}

/// Text nodes don't carry attributes, but they DO go through
/// `node_id` for some style-path internals. Verify the WeakMap
/// path handles them — `WeakMap` accepts any object key, including
/// Text nodes, so a Text node should get a stable id like an
/// Element does. This is the case the previous "stamp a
/// data-attribute" implementation explicitly couldn't handle (Text
/// nodes don't have `setAttribute`).
#[wasm_bindgen_test]
fn node_id_handles_text_nodes_via_weakmap() {
    install_mount();
    let mut backend = WebBackend::new("#app");

    let doc = web_sys::window().unwrap().document().unwrap();
    let text = doc.create_text_node("hello");
    let wrapper_a: web_sys::Node = text.clone().unchecked_into();
    let wrapper_b: web_sys::Node = text.unchecked_into();

    let id_a = backend.node_id(&wrapper_a);
    let id_b = backend.node_id(&wrapper_b);
    assert_eq!(
        id_a, id_b,
        "Text nodes must resolve to a stable id via the WeakMap, even though they can't carry a data-* attribute",
    );
}

/// REGRESSION — ADDRESS-REUSE RESISTANCE.
///
/// The bug the prior pointer-keyed cache shipped wasn't just
/// "two wrappers, same element, different ids" — it was also
/// "Rust allocator recycles a freed wrapper's address for a
/// fresh wrapper of a DIFFERENT element; cache hit returns the
/// old element's id for the new element." That second mode is
/// nastier because per-node state stamped under id N suddenly
/// looks correct from the cache's perspective but belongs to a
/// stale DOM element.
///
/// This test exercises a tight create-wrapper / call-node_id /
/// drop-wrapper loop. The Rust allocator very likely reuses
/// freed addresses across iterations. With the old cache, that
/// would surface as duplicate ids in the output (different DOM
/// elements but same cached pointer address). With the WeakMap-
/// only design, each DOM element gets its own id regardless of
/// wrapper allocation order.
///
/// The assertion is "all returned ids are unique" — fails if
/// any two distinct DOM elements got the same id.
#[wasm_bindgen_test]
fn node_id_unique_across_many_create_drop_cycles() {
    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let body = doc.body().expect("body");

    const N: usize = 100;
    let mut ids: Vec<u32> = Vec::with_capacity(N);
    // Hold strong references to elements so the JS objects stay
    // alive across iterations — otherwise GC could collect them
    // mid-loop and the WeakMap entries would clear, making the
    // address-collision test meaningless.
    let mut keepalive: Vec<web_sys::Element> = Vec::with_capacity(N);
    for _ in 0..N {
        let element = doc.create_element("div").unwrap();
        let wrapper: web_sys::Node = element.clone().unchecked_into();
        ids.push(backend.node_id(&wrapper));
        keepalive.push(element);
        // wrapper drops here; its address is available for reuse.
    }

    // All distinct elements → all distinct ids.
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        N,
        "node_id returned duplicates across {} distinct DOM elements (means a stale cache returned a prior element's id); raw ids: {:?}",
        N,
        ids,
    );

    // Hold the keepalive Vec to the end so elements survive
    // any intermediate GC sweep.
    drop(keepalive);
    let _ = body;
}

// ---------------------------------------------------------------------------
// Gradient snapshot fires from BOTH apply paths
// ---------------------------------------------------------------------------

/// `apply_style` (path used when `Backend::handles_states_natively`
/// is `false`, or as the no-overlays branch of
/// `apply_styled_states`) snapshots the gradient shape onto the
/// node's animation state so per-frame
/// `set_animated_color(GradientStopColor)` writes can rebuild
/// `background-image` without re-walking the stylesheet.
#[wasm_bindgen_test]
fn apply_style_snapshots_gradient_shape_for_animation() {
    use framework_core::{Color, Gradient, GradientKind, GradientStop, StyleRules};
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");

    let doc = web_sys::window().unwrap().document().unwrap();
    let element = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&element).unwrap();
    let node: web_sys::Node = element.unchecked_into();

    let rules = Rc::new(StyleRules {
        background_gradient: Some(Gradient {
            kind: GradientKind::Linear { angle_deg: 45.0 },
            stops: vec![
                GradientStop { offset: 0.0, color: Color("#000".into()) },
                GradientStop { offset: 1.0, color: Color("#fff".into()) },
            ],
        }),
        ..Default::default()
    });

    // Look up the id BEFORE apply so we know what to check.
    let id = backend.node_id(&node);

    use framework_core::Backend;
    backend.apply_style(&node, &rules);

    let snapshot = backend
        .animated_states
        .get(&id)
        .expect("apply_style must populate animated_states for the node");
    assert!(
        snapshot.gradient_shape.is_some(),
        "apply_style must snapshot gradient_shape so per-frame GradientStopColor writes work",
    );
    assert_eq!(snapshot.gradient_stops.len(), 2, "both stops must be snapshotted");
}

/// `apply_styled_states` MUST also snapshot the gradient — this is
/// the path web uses by default (`handles_states_natively = true`).
/// The earlier bug had `impl_apply_style` priming the gradient
/// snapshot but `impl_apply_styled_states` not, so on web the
/// snapshot was always `None` and every `GradientStopColor` write
/// hit the early-return — the entire welcome-vignette pulse went
/// dark. This test pins the snapshot down on both paths so future
/// drift between the two functions fails CI here, not visually in
/// the welcome example.
#[wasm_bindgen_test]
fn apply_styled_states_snapshots_gradient_shape_for_animation() {
    use framework_core::{Color, Gradient, GradientKind, GradientStop, StyleRules};
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");

    let doc = web_sys::window().unwrap().document().unwrap();
    let element = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&element).unwrap();
    let node: web_sys::Node = element.unchecked_into();

    let base = Rc::new(StyleRules {
        background_gradient: Some(Gradient {
            kind: GradientKind::Radial {
                center: (0.5, 0.5),
                radius: 1.0,
                extent: framework_core::RadialExtent::FarthestCorner,
            },
            stops: vec![
                GradientStop { offset: 0.0, color: Color("#fff".into()) },
                GradientStop { offset: 1.0, color: Color("#000".into()) },
            ],
        }),
        ..Default::default()
    });

    let id = backend.node_id(&node);

    use framework_core::Backend;
    // `apply_styled_states` with an empty overlay list — same
    // shape the framework uses when `handles_states_natively`
    // returns true but the node has no per-state styling.
    backend.apply_styled_states(&node, &base, &[]);

    let snapshot = backend
        .animated_states
        .get(&id)
        .expect("apply_styled_states must populate animated_states for the node");
    assert!(
        snapshot.gradient_shape.is_some(),
        "apply_styled_states must snapshot gradient_shape — drift between the two apply \
         paths is what broke the welcome vignette before. Don't let it drift again.",
    );
    // Verify the extent round-tripped through the snapshot.
    match snapshot.gradient_shape.as_ref().unwrap().kind {
        crate::animated::GradientShapeKind::Radial { extent, .. } => {
            assert_eq!(extent, framework_core::RadialExtent::FarthestCorner);
        }
        other => panic!("expected Radial in snapshot, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Microbenchmark: node_id FFI cost
// ---------------------------------------------------------------------------

/// Microbench / measurement for `node_id`. Not a regression
/// gate — the assertion at the bottom is loose (10k calls under
/// 5 seconds) just to catch a runaway-broken implementation. The
/// per-call cost gets logged to the browser console so a human
/// can read it after a run.
///
/// Why this exists: post-cache-removal, every `node_id` call FFIs
/// into the JS WeakMap. `node_id` fires from `apply_style`,
/// `apply_styled_states`, `set_animated_*`, `register_styled_node`,
/// and `impl_on_node_unstyled` — i.e. once per styled node per
/// apply, plus once per teardown. Worth knowing the absolute
/// per-call cost so we can judge whether a future fast-path cache
/// (e.g. `Rc<Node>` keyed) is worth the complexity.
#[wasm_bindgen_test]
fn benchmark_node_id_ffi_cost() {
    use web_sys::console;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let body = doc.body().unwrap();
    let performance = web_sys::window().unwrap().performance().unwrap();

    // Bench A: REPEAT calls on the same Rust wrapper. Hits the
    // WeakMap with the same JS object N times. Pure FFI cost —
    // no DOM allocation in the hot loop. Headless Safari is
    // touchy about how much it does inside a single test fn,
    // so the count is intentionally modest; per-call timing is
    // still stable.
    const N_SAME: usize = 500;
    let element = doc.create_element("div").unwrap();
    let wrapper: web_sys::Node = element.unchecked_into();
    // (Intentionally NOT appending to body — empirically the
    //  combo of `append_child` + a tight follow-up loop wedges
    //  headless safaridriver, even though the same pattern works
    //  outside the test runner.)
    let _ = &body;

    // Warm up — first call lazily injects the shim + caches the
    // js_sys::Function handle. Don't include that in the timing.
    let _ = backend.node_id(&wrapper);

    let t0 = performance.now();
    for _ in 0..N_SAME {
        let _ = backend.node_id(&wrapper);
    }
    let t_same = performance.now() - t0;
    let per_call_same_us = (t_same * 1000.0) / N_SAME as f64;

    // Bench B: each call is on a DIFFERENT DOM element (worst
    // case at scale — `apply_style` over many styled rows). Each
    // call mints a fresh WeakMap entry.
    const N_DISTINCT: usize = 200;
    let mut nodes: Vec<web_sys::Node> = Vec::with_capacity(N_DISTINCT);
    for _ in 0..N_DISTINCT {
        let el = doc.create_element("div").unwrap();
        nodes.push(el.unchecked_into());
    }

    let t1 = performance.now();
    for n in &nodes {
        let _ = backend.node_id(n);
    }
    let t_distinct = performance.now() - t1;
    let per_call_distinct_us = (t_distinct * 1000.0) / N_DISTINCT as f64;

    console::log_1(
        &format!(
            "[bench] node_id repeat-same-wrapper: {:.0}ms / {} calls = {:.2}µs/call",
            t_same, N_SAME, per_call_same_us
        )
        .into(),
    );
    console::log_1(
        &format!(
            "[bench] node_id distinct-wrappers:   {:.0}ms / {} calls = {:.2}µs/call",
            t_distinct, N_DISTINCT, per_call_distinct_us
        )
        .into(),
    );

    // Loose sanity gates — catch a catastrophically broken
    // implementation, not a perf regression. Real cost should be
    // a few µs per call even on headless Safari; if a run exceeds
    // 1 ms per call (1 second for 1000 calls) something is
    // profoundly wrong.
    assert!(
        per_call_same_us < 1_000.0,
        "node_id repeat-same-wrapper degraded to {:.2}µs/call (>1ms each is broken)",
        per_call_same_us,
    );
    assert!(
        per_call_distinct_us < 1_000.0,
        "node_id distinct-wrappers degraded to {:.2}µs/call (>1ms each is broken)",
        per_call_distinct_us,
    );
}
