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
    use runtime_core::{Color, Gradient, GradientKind, GradientStop, StyleRules};
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

    use runtime_core::Backend;
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
    use runtime_core::{Color, Gradient, GradientKind, GradientStop, StyleRules};
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
                extent: runtime_core::RadialExtent::FarthestCorner,
            },
            stops: vec![
                GradientStop { offset: 0.0, color: Color("#fff".into()) },
                GradientStop { offset: 1.0, color: Color("#000".into()) },
            ],
        }),
        ..Default::default()
    });

    let id = backend.node_id(&node);

    use runtime_core::Backend;
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
            assert_eq!(extent, runtime_core::RadialExtent::FarthestCorner);
        }
        other => panic!("expected Radial in snapshot, got {:?}", other),
    }
}

/// `apply_styled_variants` must emit a node's `breakpoint` overlays as
/// `@media (min-width: …)` rules scoped to its base class — the
/// SSR-critical behavior: the responsive layout lives in the stylesheet
/// (browser-evaluated), so the static first paint is already correct
/// with no JS. Asserts the inserted CSSOM carries the media query and
/// the overlay's resolved properties.
///
/// Runs under `wasm-bindgen-test` in a headless browser (it needs a
/// live CSSOM stylesheet); it is not exercised by `cargo test` on the
/// host.
#[wasm_bindgen_test]
fn apply_styled_variants_emits_media_rule_for_breakpoint_overlay() {
    use runtime_core::{Breakpoint, Length, StyleRules, Tokenized};
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");

    let doc = web_sys::window().unwrap().document().unwrap();
    let element = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&element).unwrap();
    let node: web_sys::Node = element.unchecked_into();

    let base = Rc::new(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(100.0))),
        ..Default::default()
    });
    // Resolved md overlay (base merged with the bp overlay), as the
    // walker hands it over.
    let md_overlay = Rc::new(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(500.0))),
        ..Default::default()
    });
    let bp_overlays = vec![(Breakpoint::Md, md_overlay)];

    use runtime_core::Backend;
    backend.apply_styled_variants(&node, &base, &[], &bp_overlays);

    // Read back every rule the backend inserted into its stylesheet.
    let sheet = backend.sheet();
    let rules = sheet.css_rules().expect("css_rules");
    let mut all = String::new();
    for i in 0..rules.length() {
        if let Some(r) = rules.get(i) {
            all.push_str(&r.css_text());
            all.push('\n');
        }
    }

    assert!(
        all.contains("min-width: 768px"),
        "apply_styled_variants must emit an @media (min-width: 768px) rule for the md \
         breakpoint overlay; stylesheet was:\n{all}",
    );
    assert!(
        all.contains("width: 500px"),
        "the md overlay's resolved properties must live inside the media rule; \
         stylesheet was:\n{all}",
    );
}

// ---------------------------------------------------------------------------
// Font linking — regression for fonts shipping inside the wasm
// ---------------------------------------------------------------------------

/// REGRESSION TEST.
///
/// On web a typeface must be **linked** as a separately-fetched file,
/// not embedded. The bug: `face!` only ever emitted
/// `AssetSource::Embedded { bytes }`, and the web backend turned those
/// bytes into a `blob:` URL — so the whole font (the website ships nine
/// ~400 KB Inter weights) rode inside the wasm download and was
/// re-minted into memory on every page load instead of being fetched
/// and HTTP-cached as a normal static asset.
///
/// After the fix `face!` carries a bundle path (`Bundled` when no
/// byte-consuming backend is in the build, `BundledEmbedded` when one
/// is), and the web backend resolves either to a root-absolute
/// served-file URL. This test pins both shapes to a `/fonts/...` URL
/// (never `blob:`) and checks the emitted `@font-face` rule links it.
#[wasm_bindgen_test]
fn regression_font_is_linked_as_served_file_not_blob() {
    use runtime_core::{
        AssetId, AssetSource, AssetTag, Backend, FontStyle, FontWeight, SystemFallback,
        TypefaceFace, TypefaceId,
    };

    install_mount();
    let mut backend = WebBackend::new("#app");

    // Shape 1: pure-web build (`embed-font-bytes` off) → `Bundled`.
    let bundled_id = AssetId(0xB00D);
    backend.register_asset(
        bundled_id,
        AssetTag::Font,
        &AssetSource::Bundled { path: "fonts/Inter-Regular.ttf" },
    );

    // Shape 2: build that also links a byte-consuming backend (e.g. the
    // website's wgpu Simulator) → `BundledEmbedded`. Web must STILL
    // link the path and ignore the embedded bytes.
    let embedded_id = AssetId(0xB33F);
    backend.register_asset(
        embedded_id,
        AssetTag::Font,
        &AssetSource::BundledEmbedded {
            path: "fonts/Inter-Bold.ttf",
            bytes: b"not-a-real-font-but-must-be-ignored-on-web",
            extension: "ttf",
        },
    );

    let bundled_url = backend
        .asset_urls
        .get(&bundled_id)
        .expect("Bundled font must resolve to a URL");
    let embedded_url = backend
        .asset_urls
        .get(&embedded_id)
        .expect("BundledEmbedded font must resolve to a URL");

    assert_eq!(bundled_url, "/fonts/Inter-Regular.ttf");
    assert_eq!(
        embedded_url, "/fonts/Inter-Bold.ttf",
        "BundledEmbedded must link the served file on web, not mint a blob from the bytes",
    );
    assert!(
        !bundled_url.starts_with("blob:") && !embedded_url.starts_with("blob:"),
        "fonts must be linked, never embedded as a blob: URL",
    );
    assert!(
        !backend.blob_asset_urls.contains(&bundled_id)
            && !backend.blob_asset_urls.contains(&embedded_id),
        "no Blob/object-URL should be minted for a linked font",
    );

    // Registering the typeface must emit an `@font-face` rule whose
    // `src: url(...)` points at the linked file, not a blob.
    let faces = [
        TypefaceFace {
            weight: FontWeight::Normal,
            style: FontStyle::Normal,
            asset: bundled_id,
            source: AssetSource::Bundled { path: "fonts/Inter-Regular.ttf" },
        },
        TypefaceFace {
            weight: FontWeight::Bold,
            style: FontStyle::Normal,
            asset: embedded_id,
            source: AssetSource::BundledEmbedded {
                path: "fonts/Inter-Bold.ttf",
                bytes: b"ignored",
                extension: "ttf",
            },
        },
    ];
    backend.register_typeface(TypefaceId(0xFACE), "Inter", &faces, SystemFallback::SansSerif);

    // Walk the shared stylesheet and collect every @font-face rule text.
    let sheet = backend.sheet();
    let rules = sheet.css_rules().expect("stylesheet css_rules");
    let mut font_face_css = String::new();
    for i in 0..rules.length() {
        if let Some(rule) = rules.get(i) {
            let text = rule.css_text();
            if text.contains("@font-face") {
                font_face_css.push_str(&text);
            }
        }
    }

    assert!(
        font_face_css.contains("url(\"/fonts/Inter-Regular.ttf\")")
            || font_face_css.contains("url(/fonts/Inter-Regular.ttf)"),
        "@font-face must link the Bundled font file; got: {font_face_css}",
    );
    assert!(
        font_face_css.contains("url(\"/fonts/Inter-Bold.ttf\")")
            || font_face_css.contains("url(/fonts/Inter-Bold.ttf)"),
        "@font-face must link the BundledEmbedded font file (not the bytes); got: {font_face_css}",
    );
    assert!(
        !font_face_css.contains("blob:"),
        "@font-face src must never be a blob: URL; got: {font_face_css}",
    );
}

// ---------------------------------------------------------------------------
// First-class-apply timing — regression for the boot/navigation FOUC
// ---------------------------------------------------------------------------

/// REGRESSION TEST.
///
/// The FIRST `apply_style` for a node must set the `class` attribute
/// SYNCHRONOUSLY, not defer it to the batched microtask flush.
///
/// The bug: the FFI-batching work routed every class apply — including
/// the first — through `queue_class_apply`'s microtask-deferred queue.
/// The build walker styles a node before inserting it into its parent,
/// so deferring the first class meant the node was attached and got its
/// FIRST style resolution class-less: `border-color` resolved to
/// `currentColor` (black), `background` to transparent. When the class
/// finally landed, the class's `transition` animated from that unstyled
/// state to the themed value on the first painted frame — a visible
/// border/text/background flicker on every page load and navigation.
///
/// CSS only suppresses transitions on an element's first style
/// computation when that computation already carries the final class.
/// The fix sets the first class synchronously (the node is still
/// detached at apply time, so no reflow); later applies still batch.
///
/// Before the fix this asserts `None` (class queued, not yet flushed —
/// the microtask hasn't run because the test never yields). After, the
/// class is present the instant `apply_style` returns.
#[wasm_bindgen_test]
fn regression_first_class_apply_is_synchronous_no_boot_transition() {
    use runtime_core::{Backend, Color, StyleRules, Tokenized};
    use std::rc::Rc;

    // `install_for_text_bindings` installs the scheduler + the global
    // self-handle (via `install_text_batcher`), so `WEB_BACKEND_HANDLE`
    // is set and `queue_class_apply` takes the BATCHED path — the one
    // the bug lived in. Without the handle it would hit the direct
    // `setAttribute` fallback and the regression couldn't reproduce.
    let backend = install_for_text_bindings();

    let doc = web_sys::window().unwrap().document().unwrap();
    let element = doc.create_element("div").unwrap();
    // DETACHED on purpose: mirrors the walker's order (style applied
    // during `build`, BEFORE the node is `insert`ed into its parent).
    let node: web_sys::Node = element.clone().unchecked_into();

    let rules = Rc::new(StyleRules {
        background: Some(Tokenized::Literal(Color("#ff0000".into()))),
        ..Default::default()
    });

    backend.borrow_mut().apply_style(&node, &rules);

    // No `await`, no microtask turn: the class must already be on the
    // element. A deferred (queued) first apply would leave this `None`.
    let class = element.get_attribute("class");
    assert!(
        class.as_deref().map(|c| !c.is_empty()).unwrap_or(false),
        "first apply_style must set the class synchronously (got {:?}); \
         deferring it to the batch microtask reintroduces the boot/navigation \
         style-transition flicker",
        class,
    );
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

// ---------------------------------------------------------------------------
// text_fmt! reactive text — regression tests for the JS-binding fast path
// ---------------------------------------------------------------------------
//
// The bug: `text_fmt!("Count: {}", bind!(count))` produced a
// `TextSource::JsBinding(JsBindingSpec)`. The walker took the JS-
// binding fast path on web. The web backend's
// `register_reactive_text_binding` registered the binding on the JS
// side but never registered a `signal_js_notifier` for the signal
// ids. When `count.set/update` fired, `Signal::set` called
// `notify_js_subscriber(sid)`, the lookup found no notifier, and the
// text never updated.
//
// Fix: per-signal `stringifiers` now flow from the `text_fmt!` macro
// through `JsBindingSpec` to the web backend's
// `register_reactive_text_binding`, which auto-installs per-signal
// JS notifiers at bind time (only if one isn't already installed —
// preserves notifiers a class binding may have set up first).
//
// These three tests exercise the wasm-side DOM mutation path that
// the host-side `text_fmt_regression.rs` tests can't reach.

/// Bootstrapping shared by every text_fmt regression below. Mounts
/// `#app`, builds a `WebBackend`, wraps it in `Rc<RefCell>`, and
/// installs the text batcher (which sets `WEB_BACKEND_HANDLE` so
/// `supports_js_text_bindings()` returns true). Returns the handle
/// so each test can build its own bindings against it.
fn install_for_text_bindings() -> std::rc::Rc<std::cell::RefCell<WebBackend>> {
    install_mount();
    // Scheduler is needed because `schedule_text_flush` calls
    // `runtime_core::schedule_microtask`. Idempotent — re-running
    // is fine.
    crate::install_scheduler();
    let backend = std::rc::Rc::new(std::cell::RefCell::new(WebBackend::new("#app")));
    crate::install_text_batcher(&backend);
    backend
}

/// REGRESSION — `signal.set(...)` must reach the DOM through the
/// JS-binding fast path. Mount a `text_fmt!("{}", bind!(count))`
/// text node, walk + commit. The initial nodeValue is "0". After
/// `count.set(42)`, the nodeValue must be "42". Before the fix the
/// second assertion fails — no signal_js_notifier is installed at
/// bind time, so `Signal::set` has nothing to call.
#[wasm_bindgen_test]
fn regression_text_fmt_signal_set_updates_dom_via_js_binding() {
    let backend = install_for_text_bindings();
    let count: runtime_core::Signal<u32> = runtime_core::signal!(0u32);

    // Mount through the public `render` entry point so we exercise
    // the walker's JS-binding path (the same path real apps take).
    let _owner = runtime_core::render(
        backend.clone(),
        runtime_core::text(runtime_core::text_fmt!("{}", bind!(count))).into(),
    );

    // Find the only text node we created. `#app` has exactly one
    // child span wrapping a Text node — `WebBackend::create_text_with_id`
    // is what runs for the JS-binding path. Search the subtree.
    fn find_first_text_node(root: &web_sys::Node) -> Option<web_sys::Text> {
        if let Some(t) = root.dyn_ref::<web_sys::Text>() {
            return Some(t.clone());
        }
        // Walk children via `first_child` / `next_sibling` so we
        // don't need the `NodeList` web-sys feature.
        let mut cursor = root.first_child();
        while let Some(c) = cursor {
            if let Some(found) = find_first_text_node(&c) {
                return Some(found);
            }
            cursor = c.next_sibling();
        }
        None
    }

    let doc = web_sys::window().unwrap().document().unwrap();
    let app: web_sys::Node = doc.get_element_by_id("app").unwrap().unchecked_into();
    let text_node = find_first_text_node(&app)
        .expect("text_fmt! must produce a real Text node under #app");

    assert_eq!(
        text_node.node_value().as_deref(),
        Some("0"),
        "initial nodeValue must reflect signal's starting value",
    );

    // Fire a signal change. The Rust subscriber fan-out runs, then
    // `notify_js_subscriber` invokes whichever notifier the framework
    // has registered for this signal. With the fix, the web backend's
    // `register_reactive_text_binding` auto-installs that notifier;
    // without the fix, the notifier slot is empty and this is a
    // silent no-op.
    count.set(42);

    assert_eq!(
        text_node.node_value().as_deref(),
        Some("42"),
        "after signal.set(42), JS dispatcher must have updated nodeValue",
    );
}

/// REGRESSION — pre-existing JS notifier must NOT be clobbered.
/// A class binding (or any other code) may register a custom notifier
/// for a signal before a text binding mounts against it. The web
/// backend's per-signal auto-register loop must call
/// `signal_has_js_notifier` and skip signals that already have one —
/// otherwise the class binding's teardown / dispatch path goes dark.
#[wasm_bindgen_test]
fn regression_text_fmt_existing_js_notifier_not_clobbered() {
    let backend = install_for_text_bindings();
    let s: runtime_core::Signal<u32> = runtime_core::signal!(0u32);

    // Pre-install a counter notifier that ALSO calls into the JS
    // dispatcher (so the text binding still works downstream). The
    // `Cell<u32>` confirms the original closure stays live across
    // the subsequent `register_reactive_text_binding` call.
    let custom_fires = std::rc::Rc::new(std::cell::Cell::new(0u32));
    {
        let custom_fires = custom_fires.clone();
        let sid = s.id();
        // We can't reach `ship_signal_change_to_js` from out here, so
        // re-implement the body of `register_signal_for_js`'s closure
        // inline. The point of the test is "is the closure I
        // registered the one that runs?" — not "does it perfectly
        // mimic what register_signal_for_js does."
        runtime_core::register_signal_js_notifier(sid, move || {
            custom_fires.set(custom_fires.get() + 1);
        });
    }
    assert!(runtime_core::signal_has_js_notifier(s.id()));

    // Mount a text binding on the same signal. The fix's "skip if a
    // notifier already exists" branch must trigger here.
    let _owner = runtime_core::render(
        backend.clone(),
        runtime_core::text(runtime_core::text_fmt!("{}", bind!(s))).into(),
    );

    // Fire the signal. The CUSTOM notifier must run — not get
    // replaced by the auto-installed text-binding stringifier.
    s.set(7);

    assert_eq!(
        custom_fires.get(),
        1,
        "custom notifier must fire once; if it's 0 the text binding clobbered it",
    );

    // Second fire — same expectation. Catches the case where the
    // first set somehow re-installed the auto notifier mid-dispatch.
    s.set(8);
    assert_eq!(custom_fires.get(), 2);
}

/// REGRESSION — two text bindings against the same signal must both
/// update on signal.set. The second binding's auto-register branch
/// hits `signal_has_js_notifier == true` (the first install put one
/// in place), so the loop short-circuits. The previously-installed
/// notifier still ships the change to JS, the JS dispatcher fans out
/// to BOTH subscribers via its own internal subscriber set, and both
/// DOM nodes update.
///
/// Catches the failure mode where the second binding either (a)
/// stomps the first notifier (regression — fix's intent) or (b) the
/// JS-side dispatcher doesn't track multiple subscribers per signal
/// (pre-existing bug we don't want to regress).
#[wasm_bindgen_test]
fn regression_text_fmt_two_bindings_one_signal() {
    let backend = install_for_text_bindings();
    let s: runtime_core::Signal<u32> = runtime_core::signal!(0u32);

    // Render two Text leaves under one View — same signal feeds
    // both via independent `text_fmt!` calls.
    use runtime_core::{text, view};
    let _owner = runtime_core::render(
        backend.clone(),
        view(vec![
            text(runtime_core::text_fmt!("a={}", bind!(s))).into(),
            text(runtime_core::text_fmt!("a={}", bind!(s))).into(),
        ])
        .into(),
    );

    // Collect ALL text nodes under #app.
    fn collect_text_nodes(root: &web_sys::Node, out: &mut Vec<web_sys::Text>) {
        if let Some(t) = root.dyn_ref::<web_sys::Text>() {
            out.push(t.clone());
            return;
        }
        // Walk children via `first_child` / `next_sibling` so we
        // don't need the `NodeList` web-sys feature.
        let mut cursor = root.first_child();
        while let Some(c) = cursor {
            collect_text_nodes(&c, out);
            cursor = c.next_sibling();
        }
    }
    let doc = web_sys::window().unwrap().document().unwrap();
    let app: web_sys::Node = doc.get_element_by_id("app").unwrap().unchecked_into();
    let mut nodes = Vec::new();
    collect_text_nodes(&app, &mut nodes);
    assert_eq!(
        nodes.len(),
        2,
        "expected exactly 2 text nodes under #app, got {}",
        nodes.len(),
    );

    for n in &nodes {
        assert_eq!(n.node_value().as_deref(), Some("a=0"));
    }

    s.set(99);

    for n in &nodes {
        assert_eq!(
            n.node_value().as_deref(),
            Some("a=99"),
            "both text nodes must update after signal.set; one updating but not the other \
             means the second binding's auto-register stomped the first's notifier",
        );
    }
}

// ---------------------------------------------------------------------------
// Cross-backend @font-face dedup — the lazy-chunk double-download fix.
// ---------------------------------------------------------------------------

/// REGRESSION GUARD: registering the same typeface across TWO live
/// `WebBackend` instances on the same wasm thread must inject the
/// `@font-face` rule exactly ONCE.
///
/// The bug this guards against: when a `lazy!` chunk's `mount_chunk`
/// spins up its own `WebBackend` (so the chunk's children get their
/// own walker), it re-runs the theme's typeface registration. Each
/// backend has its own `font_face_rule_indices`, so without a
/// process-wide dedup set BOTH backends emit a `@font-face` rule for
/// the same font URL — the browser then fetches the font file again
/// (the user-reported "double download" on the home page). The fix is
/// the thread-local `FONT_FACES_PRESENT` HashSet in `lib.rs`; this
/// test pins it.
#[wasm_bindgen_test]
fn font_face_dedup_across_backends_inserts_rule_once() {
    use runtime_core::assets::{AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId};
    use runtime_core::{Backend, FontStyle, FontWeight};

    install_mount();

    // Distinct URL/family per test invocation so the thread-local
    // dedup set (which persists across wasm tests on the same thread)
    // doesn't make this test depend on whatever ran before it.
    let family_name = "DedupTestFamily";
    let asset_id = AssetId(0xDEDD_F00D);
    let type_id = TypefaceId(0xFACE_DEDF);
    let url_path = "fonts/__dedup_test_font.ttf";
    let served_url = format!("/{url_path}");
    let face = TypefaceFace {
        weight: FontWeight::Normal,
        style: FontStyle::Normal,
        asset: asset_id,
        source: AssetSource::Bundled { path: url_path },
    };

    // ---- Backend A — first registration injects the rule.
    let mut a = WebBackend::new("#app");
    a.register_asset(asset_id, AssetTag::Font, &AssetSource::Bundled { path: url_path });
    a.register_typeface(type_id, family_name, &[face], SystemFallback::SansSerif);
    let a_indices = a
        .font_face_rule_indices
        .get(&type_id)
        .cloned()
        .expect("backend A: rule indices recorded for typeface");
    assert_eq!(
        a_indices.len(),
        1,
        "backend A must inject exactly ONE @font-face for the single face; got {a_indices:?}"
    );

    // ---- Backend B — same typeface, fresh backend (mirrors a lazy
    // chunk's `mount_chunk` re-running the theme registration). The
    // dedup must catch it before the second @font-face is injected.
    let mut b = WebBackend::new("#app");
    b.register_asset(asset_id, AssetTag::Font, &AssetSource::Bundled { path: url_path });
    b.register_typeface(type_id, family_name, &[face], SystemFallback::SansSerif);
    let b_indices = b
        .font_face_rule_indices
        .get(&type_id)
        .cloned()
        .expect("backend B: rule-index map entry exists (even if empty)");
    assert!(
        b_indices.is_empty(),
        "backend B must NOT inject a duplicate @font-face for the same URL — the \
         cross-backend dedup is the lazy-chunk double-download fix. got indices: {b_indices:?}"
    );

    // Final invariant: scan each backend's CSSOM rules (NOT the
    // `<style>` text content — `insert_rule` updates the live CSSOM
    // but leaves the element's textContent intact) and count
    // `@font-face` rules whose `cssText` references this URL. Exactly
    // one — anywhere more means a second URL fetch.
    let needle = served_url.as_str();
    let mut occurrences = 0usize;
    for sheet in [a.sheet(), b.sheet()] {
        let Ok(rules) = sheet.css_rules() else { continue };
        for j in 0..rules.length() {
            let Some(rule) = rules.item(j) else { continue };
            let text = rule.css_text();
            if text.contains("@font-face") && text.contains(needle) {
                occurrences += 1;
            }
        }
    }
    assert_eq!(
        occurrences, 1,
        "exactly one @font-face for {served_url} across both backends' \
         live stylesheets; got {occurrences}. A second one would re-fetch the font."
    );
}

// ---------------------------------------------------------------------------
// Leaf-primitive hydration adoption.
// ---------------------------------------------------------------------------

/// REGRESSION TEST.
///
/// Leaf primitives whose `create()` body calls `b.doc.create_element(tag)`
/// directly (text_input, text_area, image, …) must FIRST consult
/// `b.hydrate_next(tag)` so the SSR node is adopted instead of a fresh
/// sibling getting appended next to it. Earlier bugs: `<svg>` (icon)
/// and `<input>` (text input on the Demo page) both bypassed the
/// cursor; the SSR-emitted node stayed in the DOM while a fresh one
/// was inserted alongside, and the divergence cascade panicked the
/// navigator's `insertBefore` once the parent's child list desynced.
///
/// This test mounts a tiny SSR-style document into `#app`, hydrates,
/// drives `create_text_input`, and asserts the returned node IS the
/// pre-existing SSR `<input>` (same reference, no fresh duplicate
/// appended).
#[cfg(feature = "hydrate")]
#[wasm_bindgen_test]
fn text_input_create_adopts_ssr_input_during_hydration() {
    use runtime_core::Backend;
    use std::rc::Rc;

    install_mount();
    let doc = web_sys::window().unwrap().document().unwrap();
    let app = doc.get_element_by_id("app").unwrap();

    // SSR-style markup: one root child, one `<input>` inside it. The
    // hydration cursor starts on the root child (the same element a
    // walker-built View would land on).
    app.set_inner_html(
        r#"<div><input value="seed" placeholder="hint"></div>"#,
    );
    let ssr_input = doc
        .query_selector("#app input")
        .unwrap()
        .expect("ssr input must exist");

    let mut backend = WebBackend::hydrate("#app");

    // Drive the walker's order: the View wrapper adopts first, then
    // the input inside it.
    let _wrapper = backend.create_view(&Default::default());
    let input_node = backend.create_text_input(
        "", // initial_value (overridden post-adopt)
        None,
        Rc::new(|_: String| {}),
        None,
        &Default::default(),
    );

    // The returned node must be the SAME element the SSR rendered —
    // adoption succeeded, not a fresh `<input>` next to it.
    let adopted: web_sys::Element = input_node.unchecked_into();
    assert!(
        adopted.is_same_node(Some(ssr_input.as_ref())),
        "text_input::create must adopt the SSR input during hydration; got a fresh element \
         (the divergence would cascade to insertBefore panics)",
    );

    // And no second `<input>` got appended as a sibling.
    let input_count = doc.query_selector_all("#app input").unwrap().length();
    assert_eq!(
        input_count, 1,
        "exactly one input in the DOM after hydration; a fresh duplicate would be the \
         original bug's signature",
    );
}

// ---------------------------------------------------------------------------
// Pointer-keyed dynamic cache rejects stale entries on content mismatch.
// ---------------------------------------------------------------------------

/// REGRESSION TEST.
///
/// `dynamic_by_ptr` keys by raw `*const StyleRules`. When an
/// `Rc<StyleRules>` is dropped and its address is recycled by the
/// allocator for a fresh `Rc` of unrelated content, a naive lookup
/// returns the previous tenant's class. That's the SSG/hydration
/// breakage: the codeblock allocated short-lived `Rc::new(color_rules)`
/// per colored span and dropped them; the address got recycled for a
/// Stack's flex-column rules; the ptr cache routed flex-column nodes
/// to the codeblock's green text color (which has no flex), collapsing
/// every section's vertical stack into inline siblings.
///
/// The fix verifies `cached.content_key == base.content_key()` before
/// using the cached entry and drops the stale row on mismatch. This
/// test forces the stale state deterministically by injecting a stale
/// shared entry under a fresh Rc's pointer, then asserts the apply
/// path emits the correct class.
#[wasm_bindgen_test]
fn dynamic_by_ptr_stale_entry_does_not_misroute_class() {
    use runtime_core::{Backend, Color, FlexDirection, StyleRules, Tokenized};
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");

    let doc = web_sys::window().unwrap().document().unwrap();
    let el1 = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el1).unwrap();
    let node1: web_sys::Node = el1.clone().unchecked_into();

    // Apply COLOR style to node1 — populates `dynamic_by_content` with
    // the color content_key and `dynamic_by_ptr` with ptr(color_rc).
    let color_rules = Rc::new(StyleRules {
        color: Some(Tokenized::Literal(Color("#1f6e5f".into()))),
        ..Default::default()
    });
    backend.apply_styled_states(&node1, &color_rules, &[]);
    let color_class = el1.class_name();
    assert!(!color_class.is_empty(), "color apply must set a class");

    // Grab the shared color entry so we can re-inject it under a
    // different pointer (simulating address recycling).
    let color_key = color_rules.content_key();
    let stale_shared = backend
        .dynamic_by_content
        .get(&color_key)
        .expect("color apply must register in dynamic_by_content")
        .shared
        .clone();

    // Allocate a FLEX style and pin the stale color entry to its
    // pointer. Without the fix this would route flex applies to the
    // color class on the next call.
    let flex_rules = Rc::new(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        ..Default::default()
    });
    let flex_ptr = Rc::as_ptr(&flex_rules);
    backend
        .dynamic_by_ptr
        .insert(flex_ptr, stale_shared.clone());

    // Apply flex to node2. Ptr cache hit returns the stale color
    // entry; the content_key verification must reject it and the
    // resulting class must encode the flex rules, not the color.
    let el2 = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el2).unwrap();
    let node2: web_sys::Node = el2.clone().unchecked_into();
    backend.apply_styled_states(&node2, &flex_rules, &[]);
    let flex_class = el2.class_name();

    assert_ne!(
        flex_class, color_class,
        "stale ptr cache must not route flex-column style to the color class",
    );
    // Stale entry must be evicted so a subsequent apply doesn't trip
    // the same bug.
    assert!(
        !backend.dynamic_by_ptr.contains_key(&flex_ptr)
            || backend
                .dynamic_by_ptr
                .get(&flex_ptr)
                .map(|s| s.content_key == flex_rules.content_key())
                .unwrap_or(false),
        "after content_key mismatch, the ptr entry must be either removed \
         or replaced with the correct content_key",
    );
}

// ---------------------------------------------------------------------------
// Icon fill vs stroke — regression test for the filled-icon support.
// ---------------------------------------------------------------------------

use runtime_core::primitives::icon::{FillRule, IconData};
use runtime_core::Color;

const FILLED_ICON: IconData = IconData {
    view_box: (24, 24),
    paths: &["M12 2l3 7h7l-6 4 3 7-7-4-7 4 3-7-6-4h7z"],
    fill_rule: FillRule::NonZero,
    filled: true,
};

const OUTLINED_ICON: IconData = IconData {
    view_box: (24, 24),
    paths: &["M12 2l3 7h7l-6 4 3 7-7-4-7 4 3-7-6-4h7z"],
    fill_rule: FillRule::NonZero,
    filled: false,
};

/// REGRESSION TEST.
///
/// Before filled-icon support, `icon()` hardcoded `fill="none"` and
/// painted the icon color into `stroke`, so a filled/silhouette
/// `IconData` rendered as a thin outline (or invisible). A filled icon
/// must paint its color into `fill` and disable the stroke; an outlined
/// icon must keep the historic stroke-only rendering. `update_color`
/// must rewrite whichever paint is live.
#[wasm_bindgen_test]
fn regression_filled_icon_paints_fill_not_stroke() {
    install_mount();
    let mut backend = WebBackend::new("#app");

    let red = Color("rgb(255, 0, 0)".to_string());

    // --- Filled icon: color goes to `fill`, stroke disabled. ---
    let filled_node = crate::primitives::icon::create(&mut backend, &FILLED_ICON, Some(&red));
    let filled_el: web_sys::Element = filled_node.clone().dyn_into().unwrap();
    assert_eq!(filled_el.tag_name().to_lowercase(), "svg");
    assert_eq!(
        filled_el.get_attribute("fill").as_deref(),
        Some("rgb(255, 0, 0)"),
        "filled icon must paint the color into fill",
    );
    assert_eq!(
        filled_el.get_attribute("stroke").as_deref(),
        Some("none"),
        "filled icon must disable the stroke",
    );

    // update_color on a filled icon rewrites `fill`, not `stroke`.
    let blue = Color("rgb(0, 0, 255)".to_string());
    crate::primitives::icon::update_color(&filled_node, &blue);
    assert_eq!(
        filled_el.get_attribute("fill").as_deref(),
        Some("rgb(0, 0, 255)"),
        "update_color on a filled icon must rewrite fill",
    );
    assert_eq!(
        filled_el.get_attribute("stroke").as_deref(),
        Some("none"),
        "update_color must not re-enable the stroke on a filled icon",
    );

    // --- Outlined icon (default): historic stroke-only behavior. ---
    let outlined_node = crate::primitives::icon::create(&mut backend, &OUTLINED_ICON, Some(&red));
    let outlined_el: web_sys::Element = outlined_node.clone().dyn_into().unwrap();
    assert_eq!(
        outlined_el.get_attribute("fill").as_deref(),
        Some("none"),
        "outlined icon must keep fill=none",
    );
    assert_eq!(
        outlined_el.get_attribute("stroke").as_deref(),
        Some("rgb(255, 0, 0)"),
        "outlined icon must paint the color into stroke",
    );

    // update_color on an outlined icon rewrites `stroke`, not `fill`.
    crate::primitives::icon::update_color(&outlined_node, &blue);
    assert_eq!(
        outlined_el.get_attribute("stroke").as_deref(),
        Some("rgb(0, 0, 255)"),
        "update_color on an outlined icon must rewrite stroke",
    );
    assert_eq!(
        outlined_el.get_attribute("fill").as_deref(),
        Some("none"),
        "update_color must not paint fill on an outlined icon",
    );
}
