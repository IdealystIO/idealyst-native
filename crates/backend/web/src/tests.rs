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
pub(crate) fn install_mount() {
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
    use runtime_shared::{Color, Gradient, GradientKind, GradientStop, StyleRules};
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

    backend.apply_style_impl(&node, &rules);

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
    use runtime_shared::{Color, Gradient, GradientKind, GradientStop, StyleRules};
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
                extent: runtime_shared::RadialExtent::FarthestCorner,
            },
            stops: vec![
                GradientStop { offset: 0.0, color: Color("#fff".into()) },
                GradientStop { offset: 1.0, color: Color("#000".into()) },
            ],
        }),
        ..Default::default()
    });

    let id = backend.node_id(&node);

    // `apply_styled_states` with an empty overlay list — same
    // shape the framework uses when `handles_states_natively`
    // returns true but the node has no per-state styling.
    backend.apply_styled_states_impl(&node, &base, &[]);

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
            assert_eq!(extent, runtime_shared::RadialExtent::FarthestCorner);
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
    use runtime_shared::{Breakpoint, Length, StyleRules, Tokenized};
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

    backend.apply_styled_variants_impl(&node, &base, &[], &bp_overlays, &[]);

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

/// `apply_styled_variants` emits an `@container (min-width: …)` rule per
/// container overlay, and `mark_container` sets `container-type: inline-size`
/// on the containment node — the web realization of `container (min_width: N)`.
///
/// Runs under `wasm-bindgen-test` in a headless browser (needs a live
/// CSSOM stylesheet + a real element style object).
#[wasm_bindgen_test]
fn apply_styled_variants_emits_container_query_rule() {
    use runtime_shared::{Length, StyleRules, Tokenized};
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");

    let doc = web_sys::window().unwrap().document().unwrap();
    let container = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&container).unwrap();
    let container_node: web_sys::Node = container.clone().unchecked_into();
    let element = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&element).unwrap();
    let node: web_sys::Node = element.unchecked_into();

    backend.mark_container_impl(&container_node);

    let base = Rc::new(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(100.0))),
        ..Default::default()
    });
    let overlay = Rc::new(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(500.0))),
        ..Default::default()
    });
    backend.apply_styled_variants_impl(&node, &base, &[], &[], &[(400.0, overlay)]);

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
        all.contains("min-width: 400px"),
        "must emit an @container (min-width: 400px) rule; stylesheet was:\n{all}",
    );
    assert!(
        all.contains("width: 500px"),
        "the container overlay's resolved properties must live inside the @container rule; \
         stylesheet was:\n{all}",
    );
    // The containment context carries `container-type: inline-size` as an
    // inline style (survives the className replace `queue_class_apply` does).
    let style_attr = container.get_attribute("style").unwrap_or_default();
    assert!(
        style_attr.contains("container-type") && style_attr.contains("inline-size"),
        "mark_container must set container-type: inline-size inline; got style=\"{style_attr}\"",
    );
}

/// REGRESSION TEST.
///
/// Re-applying the scrollbar theme must not corrupt the absolute rule
/// indices that `pregen` / `dynamic` / `free_rule_indices` track.
///
/// `impl_set_scrollbar_theme` used to drop its prior rules with a raw
/// CSSOM `deleteRule`, which physically removes the rule and renumbers
/// every *later* rule's index down by one. The backend addresses every
/// rule by absolute index and never renumbered those trackers, so once
/// any class rule had been appended AFTER the scrollbar rules, a
/// scrollbar re-apply (which fires on every theme re-target — i.e. every
/// light/dark toggle) left every later index stale. The next
/// unregister/recycle then operated on the WRONG physical rule: it freed
/// a slot that no longer held the class it thought, leaving a still-live
/// class's rule behind while a recycle overwrote an unrelated rule. On a
/// real page that surfaced as shared CSS classes vanishing from the sheet
/// under repeated theme toggles, collapsing flex containers (segmented
/// controls, the toolbar) to default block flow.
///
/// The fix routes scrollbar rule removal through the index-stable
/// free-slot recycler (`self.delete_rule` + `self.insert_rule_raw`), the
/// same path every other rule uses, so no absolute index ever shifts.
#[wasm_bindgen_test]
fn scrollbar_reapply_preserves_other_rule_indices() {
    use runtime_shared::{Color, Length, StyleRules, Tokenized};
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");

    let thumb = Tokenized::Literal(Color::from("#888888"));
    let track = Tokenized::Literal(Color::from("#222222"));

    // Scrollbar rules go in FIRST, so the class rules below land at
    // higher indices — exactly the arrangement a real deleteRule would
    // renumber out from under the trackers.
    backend.set_scrollbar_theme_impl(&thumb, &track);

    // Three classes with distinctive bodies, appended after the
    // scrollbar rules.
    let mk = |w: f32| {
        Rc::new(StyleRules {
            width: Some(Tokenized::Literal(Length::Px(w))),
            ..Default::default()
        })
    };
    let a = mk(111.0);
    let b = mk(222.0);
    let c = mk(333.0);
    backend.register_stylesheet_impl(std::slice::from_ref(&a));
    backend.register_stylesheet_impl(std::slice::from_ref(&b));
    backend.register_stylesheet_impl(std::slice::from_ref(&c));

    // Re-apply the scrollbar theme — the per-toggle path that used to
    // shift every later index.
    backend.set_scrollbar_theme_impl(&thumb, &track);
    backend.set_scrollbar_theme_impl(&thumb, &track);

    // Drop B: its refcount hits zero, so its tracked rule index is freed.
    backend.unregister_stylesheet_impl(std::slice::from_ref(&b));
    // Mint a new class, which recycles the slot B just freed.
    let d = mk(444.0);
    backend.register_stylesheet_impl(std::slice::from_ref(&d));

    let sheet = backend.sheet();
    let rules = sheet.css_rules().expect("css_rules");
    let mut all = String::new();
    for i in 0..rules.length() {
        if let Some(r) = rules.get(i) {
            all.push_str(&r.css_text());
            all.push('\n');
        }
    }

    // The still-registered classes survive intact with their own bodies.
    assert!(
        all.contains("width: 111px"),
        "class A was clobbered by scrollbar re-apply index drift:\n{all}",
    );
    assert!(
        all.contains("width: 333px"),
        "class C was clobbered by scrollbar re-apply index drift:\n{all}",
    );
    assert!(
        all.contains("width: 444px"),
        "newly minted class D (recycled B's slot) is wrong:\n{all}",
    );
    // B was the ONLY class unregistered, so its rule must be gone. Pre-fix
    // the stale index made `delete_rule` free the wrong slot, orphaning
    // B's rule in the sheet.
    assert!(
        !all.contains("width: 222px"),
        "class B should have been removed, but a stale rule index orphaned it:\n{all}",
    );
}

/// REGRESSION TEST.
///
/// `insert_rule_raw` must never panic when the browser rejects a rule.
/// `CSSStyleSheet.insertRule()` is specified to THROW `SyntaxError` on a
/// selector the engine can't parse — most visibly a `::-webkit-scrollbar*`
/// pseudo on Gecko (Firefox). The old code `.expect(...)`ed that fallible
/// call, so under `panic = abort` a single browser-disliked rule aborted
/// the entire WASM module before first paint: the Firefox white-screen
/// this guards against. The fix backfills an inert placeholder at the same
/// index instead, so the app survives AND every absolute CSSOM index the
/// backend tracks stays valid.
///
/// The headless test runner is Chrome/Safari, and both accept
/// `::-webkit-scrollbar`, so a webkit pseudo wouldn't reproduce the throw
/// here. We instead feed `insert_rule_raw` a selector that EVERY engine
/// rejects (`!` is an illegal selector start in Chrome, Safari, and
/// Firefox alike) and assert: (a) the call returns without panicking,
/// (b) the rejected rule's slot holds the inert placeholder, and (c) a
/// class registered AFTER it still round-trips to its own body — proving
/// the placeholder kept the slot occupied so no tracked index drifted.
#[wasm_bindgen_test]
fn rejected_css_rule_does_not_abort_or_drift_indices() {
    use runtime_shared::{Length, StyleRules, Tokenized};
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");

    // A class with a distinctive body, registered first.
    let a = Rc::new(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(111.0))),
        ..Default::default()
    });
    backend.register_stylesheet_impl(std::slice::from_ref(&a));

    // A rule no engine can parse. Pre-fix this `.expect()`-panics and
    // aborts the module; post-fix it backfills the placeholder.
    let rejected_idx = backend.insert_rule_raw("!!!bogus { color: red; }");

    // A second class registered AFTER the rejected insert. Its body must
    // round-trip intact — proving the placeholder kept the slot occupied
    // and no index drifted.
    let b = Rc::new(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(222.0))),
        ..Default::default()
    });
    backend.register_stylesheet_impl(std::slice::from_ref(&b));

    let sheet = backend.sheet();
    let rules = sheet.css_rules().expect("css_rules");

    // The rejected rule's slot holds the inert placeholder, NOT the garbage.
    let at_rejected = rules
        .get(rejected_idx)
        .map(|r| r.css_text())
        .unwrap_or_default();
    assert!(
        at_rejected.contains("__idl-rejected-rule"),
        "rejected rule slot should hold the inert placeholder, got: {at_rejected}",
    );

    // Both real classes survive with their own bodies.
    let mut all = String::new();
    for i in 0..rules.length() {
        if let Some(r) = rules.get(i) {
            all.push_str(&r.css_text());
            all.push('\n');
        }
    }
    assert!(all.contains("width: 111px"), "class A lost:\n{all}");
    assert!(all.contains("width: 222px"), "class B lost:\n{all}");
}

/// REGRESSION TEST.
///
/// A pressable (idea-ui `Button`/`Chip`/`IconButton`/...) renders as a
/// `<div>` on web, and `set_disabled` marks the disabled node with the
/// HTML `disabled` *attribute*. The disabled state overlay used to be
/// emitted under the `:disabled` *pseudo-class* (`.cls:disabled`), which
/// only matches real form controls (button/input/select/...). So
/// `.cls:disabled` never matched a `<div disabled>` and the disabled
/// styling (`state disabled { opacity: 0.45 }`, etc.) was silently
/// dropped on web — a disabled button looked identical to an enabled one
/// (click was still blocked elsewhere; it was purely the visual that
/// vanished). The fix emits the overlay under the `[disabled]` attribute
/// selector, which matches any element carrying the attribute — div
/// pressables AND form controls alike.
///
/// This test pins the selector down: it asserts the emitted rule uses
/// `[disabled]` (never `:disabled`) and then proves the rule actually
/// matches a `<div disabled>` via `Element.matches` — the DOM-level
/// behavior the bug got wrong. It fails against the old `:disabled`
/// mapping and passes after the fix.
#[wasm_bindgen_test]
fn regression_web_disabled_state_styles_div_pressable() {
    use runtime_shared::{Length, StateBits, StyleRules, Tokenized};
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");

    let doc = web_sys::window().unwrap().document().unwrap();
    // A pressable is a `<div>`, NOT a form control — this is the whole
    // point of the bug.
    let element = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&element).unwrap();
    let node: web_sys::Node = element.clone().unchecked_into();

    let base = Rc::new(StyleRules {
        opacity: Some(Tokenized::Literal(1.0)),
        ..Default::default()
    });
    // The disabled overlay the walker resolves for `state disabled { ... }`.
    let disabled_overlay = Rc::new(StyleRules {
        opacity: Some(Tokenized::Literal(0.45)),
        // A second property so we can tell the overlay rule apart from
        // the base rule by content if needed.
        width: Some(Tokenized::Literal(Length::Px(42.0))),
        ..Default::default()
    });
    let overlays = vec![(StateBits::DISABLED, disabled_overlay)];

    backend.apply_styled_states_impl(&node, &base, &overlays);

    // Pull every CssStyleRule the backend inserted and locate the one
    // carrying the disabled overlay's selector.
    let sheet = backend.sheet();
    let rules = sheet.css_rules().expect("css_rules");
    let mut disabled_selector: Option<String> = None;
    let mut all = String::new();
    for i in 0..rules.length() {
        let Some(rule) = rules.get(i) else { continue };
        let text = rule.css_text();
        all.push_str(&text);
        all.push('\n');
        if let Ok(style_rule) = rule.dyn_into::<web_sys::CssStyleRule>() {
            let selector = style_rule.selector_text();
            if selector.contains("[disabled]") || selector.contains(":disabled") {
                disabled_selector = Some(selector);
            }
        }
    }

    let selector = disabled_selector.unwrap_or_else(|| {
        panic!("no disabled-state rule was emitted; stylesheet was:\n{all}")
    });

    assert!(
        selector.contains("[disabled]"),
        "disabled state must be emitted as the `[disabled]` attribute selector so it \
         matches a `<div disabled>` pressable; got selector `{selector}`",
    );
    assert!(
        !selector.contains(":disabled"),
        "disabled state must NOT use the `:disabled` pseudo-class — it only matches real \
         form controls, never a `<div disabled>`; got selector `{selector}`",
    );

    // DOM-level proof: a div carrying the class + the `disabled`
    // attribute (exactly what `set_disabled` sets) matches the rule,
    // and the same div WITHOUT the attribute does not. This is the
    // behavior `:disabled` got wrong.
    let class_name = selector.trim_start_matches('.').trim_end_matches("[disabled]");
    element.set_class_name(class_name);
    assert!(
        !element.matches(&selector).unwrap(),
        "before the disabled attribute is set, the div must not match the overlay rule",
    );
    element.set_attribute("disabled", "").unwrap();
    assert!(
        element.matches(&selector).unwrap(),
        "a `<div class=\"{class_name}\" disabled>` must match the emitted overlay rule \
         `{selector}`; this is the styling the `:disabled` pseudo silently dropped",
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
    use runtime_shared::{
        AssetId, AssetSource, AssetTag, FontStyle, FontWeight, SystemFallback,
        TypefaceFace, TypefaceId,
    };

    install_mount();
    let mut backend = WebBackend::new("#app");

    // Shape 1: pure-web build (`embed-font-bytes` off) → `Bundled`.
    let bundled_id = AssetId(0xB00D);
    backend.register_asset_impl(
        bundled_id,
        AssetTag::Font,
        &AssetSource::Bundled { path: "fonts/Inter-Regular.ttf" },
    );

    // Shape 2: build that also links a byte-consuming backend (e.g. the
    // website's wgpu Simulator) → `BundledEmbedded`. Web must STILL
    // link the path and ignore the embedded bytes.
    let embedded_id = AssetId(0xB33F);
    backend.register_asset_impl(
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
    backend.register_typeface_impl(TypefaceId(0xFACE), "Inter", &faces, SystemFallback::SansSerif);

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
/// Shared setup for the text-batcher tests: mount + scheduler + the
/// global self-handle (`install_text_batcher`), so the batched text /
/// class paths are the ones under test rather than the detached
/// fallbacks.
fn install_for_text_bindings() -> std::rc::Rc<std::cell::RefCell<WebBackend>> {
    install_mount();
    // Scheduler is needed because `schedule_text_flush` calls
    // `runtime_shared::schedule_microtask`. Idempotent — re-running
    // is fine.
    crate::install_scheduler();
    let backend = std::rc::Rc::new(std::cell::RefCell::new(WebBackend::new("#app")));
    crate::install_text_batcher(&backend);
    backend
}


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
    use runtime_shared::{Color, StyleRules, Tokenized};
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

    backend.borrow_mut().apply_style_impl(&node, &rules);

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
// The three f-string JS-binding regressions that lived here
// (`regression_fstring_signal_set_updates_dom_via_js_binding`,
// `..._existing_js_notifier_not_clobbered`, `..._two_bindings_one_signal`)
// drove `runtime_shared::render` + `ui!`, i.e. the deleted walker. Their
// SUBJECT — the per-signal JS text-binding shim on this backend — is
// covered on the surviving path by `newcore.rs`'s battery:
// `regression_newcore_sids_stay_out_of_oldcore_arena_range` (set →
// notifier → DOM, plus the sid namespace),
// `regression_two_text_bindings_on_one_signal_both_update` (the fan-out
// + no-clobber pair) and
// `regression_class_binding_registered_after_text_binding_still_swaps`.

// ---------------------------------------------------------------------------
// Cross-backend @font-face dedup — the lazy-chunk double-download fix.
// ---------------------------------------------------------------------------

/// REGRESSION GUARD: registering the same typeface across TWO live
/// `WebBackend` instances on the same wasm thread must inject the
/// `@font-face` rule exactly ONCE.
///
/// The bug this guards against: when a lazy chunk's `mount_chunk`
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
    use runtime_shared::assets::{AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId};
    use runtime_shared::{FontStyle, FontWeight};

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
    a.register_asset_impl(asset_id, AssetTag::Font, &AssetSource::Bundled { path: url_path });
    a.register_typeface_impl(type_id, family_name, &[face], SystemFallback::SansSerif);
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
    b.register_asset_impl(asset_id, AssetTag::Font, &AssetSource::Bundled { path: url_path });
    b.register_typeface_impl(type_id, family_name, &[face], SystemFallback::SansSerif);
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
    let _wrapper = backend.create_view_impl(&Default::default());
    let input_node = backend.create_text_input_impl(
        "", // initial_value (overridden post-adopt)
        None,
        Rc::new(|_: String| {}),
        None, // on_key_down
        None, // on_blur
        false, // secure
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

    // TEST HYGIENE: `WebBackend::hydrate` arms the scheduler's hydration
    // microtask buffer; only `finish` (never called here) disarms it. A
    // leaked armed buffer swallows LATER tests' scheduled microtasks —
    // the `regression_image_on_load_cached_does_not_reenter_borrow`
    // "deferred on_load must fire" failure under `--features hydrate`.
    crate::scheduler::end_hydration_buffering();
}

// ---------------------------------------------------------------------------
// Divergence-remount must remove the stale SSR node on EVERY attach path.
// ---------------------------------------------------------------------------

/// REGRESSION TEST — the duplicated absolutely-positioned nav.
///
/// A subtree-local hydration remount (SSR/client diverge) records the
/// stale SSR node and swaps the fresh client node in for it. The resync
/// used to live ONLY in `Backend::insert`; the anchorless `when` / `switch`
/// splice (`build_when_spliced`) and keyed `Each` reconcile parent their
/// branch/rows through `Backend::insert_at`, which did NOT run the resync.
///
/// So a top-level `when(...)` nav (no style ⇒ anchorless splice) whose arm
/// root diverged from SSR got its fresh copy inserted via `insert_at` while
/// the stale SSR nav stayed in the DOM — two nav bars, the orphan pinned to
/// `top:0`. This test arms exactly that remount and asserts `insert_at`
/// detaches the stale node (the fix), leaving a single, correct child.
#[cfg(feature = "hydrate")]
#[wasm_bindgen_test]
fn insert_at_removes_stale_ssr_node_on_divergence_remount() {

    install_mount();
    let doc = web_sys::window().unwrap().document().unwrap();
    let app = doc.get_element_by_id("app").unwrap();

    // SSR markup: the app root `<div>` holds one `<span>` — the stale
    // "nav". The client will build a `<div>` there instead (tag mismatch),
    // so that fresh `<div>` becomes the remount root the splice parents
    // via `insert_at`.
    app.set_inner_html(r#"<div class="approot"><span class="ssr-nav">STALE</span></div>"#);

    let mut backend = WebBackend::hydrate("#app");

    // Adopt the app root `<div>`; the cursor descends onto the `<span>`.
    let mut approot = backend.create_view_impl(&Default::default());

    // Client builds a `<div>` where SSR had a `<span>` → `create_view`'s
    // `hydrate_next("div")` mismatches and arms the remount (this fresh
    // `<div>` is the remount root; the `<span>` is the recorded stale).
    let fresh_nav = backend.create_view_impl(&Default::default());

    // The anchorless-splice attach path: `build_when_spliced` calls exactly
    // this to parent its branch node. Before the fix it left the stale in
    // place; after the fix it swaps the stale out.
    backend.insert_at_impl(&mut approot, fresh_nav.clone(), 0);

    // Exactly one element child under `.approot`, and it's the fresh nav
    // `<div>` — the stale `<span>` is gone. Pre-fix this was 2 (span + div).
    let approot_el: web_sys::Element = approot.unchecked_into();
    assert_eq!(
        count_element_children(&approot_el),
        1,
        "after an insert_at remount resync the stale SSR node must be detached; \
         the duplicate-nav bug leaves the stale span alongside the fresh div",
    );
    let only = approot_el
        .first_element_child()
        .expect("one child expected");
    assert!(
        only.is_same_node(Some(fresh_nav.unchecked_ref())),
        "the surviving child must be the fresh client node, not the stale SSR span",
    );
    assert_eq!(
        doc.query_selector_all("#app .ssr-nav").unwrap().length(),
        0,
        "the stale SSR nav must not survive anywhere in the mount",
    );

    // TEST HYGIENE: `WebBackend::hydrate` arms the scheduler's hydration
    // microtask buffer; only `finish` (never called here) disarms it. A
    // leaked armed buffer swallows LATER tests' scheduled microtasks —
    // the `regression_image_on_load_cached_does_not_reenter_borrow`
    // "deferred on_load must fire" failure under `--features hydrate`.
    crate::scheduler::end_hydration_buffering();
}

/// REGRESSION TEST — the SSG navigator remount cascade (every screen
/// subtree of a navigator app logged `[hydrate] SSR/client diverge` and
/// remounted instead of adopting).
///
/// The navigator handlers realize the initial SCREEN before the author
/// layout builds the outlet, but the SSR document nests the screen
/// INSIDE the outlet. Without cursor steering the screen build consumed
/// the outlet's node and every later view adopted its parent's node —
/// a whole-page one-level shift that cascaded into remounts at every
/// span-vs-div boundary.
///
/// This test replays the navigator's exact create order against an
/// SSR-shaped document and asserts every node adopts its true
/// counterpart: begin() steers the cursor to the marked outlet's first
/// child for the screen build, end() restores it, the layout build
/// adopts the outlet WITHOUT descending into its consumed subtree, and
/// post-outlet chrome still adopts.
#[cfg(feature = "hydrate")]
#[wasm_bindgen_test]
fn regression_nav_screen_cursor_steering_adopts_out_of_order_build() {
    install_mount();
    let doc = web_sys::window().unwrap().document().unwrap();
    let app = doc.get_element_by_id("app").unwrap();

    // SSR document for a stack navigator whose layout is
    // [outlet, chrome-after]: the screen (with a text span) nests
    // inside the marked outlet.
    app.set_inner_html(
        r#"<div class="navroot"><div data-iy-nav-outlet=""><div class="screen"><span>hi</span></div></div><div class="chrome"></div></div>"#,
    );
    let ssr_screen = doc.query_selector("#app .screen").unwrap().unwrap();
    let ssr_outlet = doc.query_selector("#app [data-iy-nav-outlet]").unwrap().unwrap();
    let ssr_chrome = doc.query_selector("#app .chrome").unwrap().unwrap();

    let mut backend = WebBackend::hydrate("#app");

    // Navigator mount order: root → (steered) screen → layout.
    let root = backend.create_view_impl(&Default::default());
    backend.hydrate_nav_screen_begin_impl(&root, "");

    // Screen build. Pre-fix this create adopted the OUTLET node (the
    // one-level shift); it must adopt the screen root.
    let screen = backend.create_view_impl(&Default::default());
    assert!(
        screen
            .dyn_ref::<web_sys::Element>()
            .unwrap()
            .is_same_node(Some(ssr_screen.as_ref())),
        "the steered screen build must adopt the server's screen root, \
         not the outlet (the one-level-shift bug)",
    );
    // The screen's text leaf adopts the span inside it.
    let txt = crate::primitives::text::create(&mut backend, "hi");
    assert_eq!(
        txt.unchecked_ref::<web_sys::Element>().tag_name().to_lowercase(),
        "span",
        "screen text adopts the SSR span",
    );

    backend.hydrate_nav_screen_end_impl();

    // Layout build: the outlet adopts its own node…
    let outlet = backend.create_view_impl(&Default::default());
    assert!(
        outlet
            .dyn_ref::<web_sys::Element>()
            .unwrap()
            .is_same_node(Some(ssr_outlet.as_ref())),
        "the layout build must adopt the outlet node after the restore",
    );
    // …and the cursor SKIPS the outlet's consumed subtree, so the
    // chrome sibling adopts its own node instead of the screen's.
    let chrome = backend.create_view_impl(&Default::default());
    assert!(
        chrome
            .dyn_ref::<web_sys::Element>()
            .unwrap()
            .is_same_node(Some(ssr_chrome.as_ref())),
        "post-outlet chrome must adopt its own node — descending into the \
         consumed outlet subtree would re-shift everything after it",
    );

    // Clean adoption throughout: no remount armed anywhere.
    assert!(
        !backend.hydration_pending_fresh,
        "a fully-adopting navigator mount must not arm any remount",
    );

    // TEST HYGIENE: disarm the hydration microtask buffer (see the
    // note in `text_input_create_adopts_ssr_input_during_hydration`).
    crate::scheduler::end_hydration_buffering();
}

/// Degraded-mode companion: a document WITHOUT the outlet marker (an
/// SSR build predating it) must not shift-adopt. `begin` parks the
/// cursor so the screen builds fresh, `end` restores it, and the layout
/// build still adopts the outlet node — the screen is swapped in by the
/// navigator's `show_in_outlet` (clear + insert), chrome still adopts.
#[cfg(feature = "hydrate")]
#[wasm_bindgen_test]
fn nav_screen_steering_without_marker_builds_screen_fresh_and_layout_adopts() {
    install_mount();
    let doc = web_sys::window().unwrap().document().unwrap();
    let app = doc.get_element_by_id("app").unwrap();

    app.set_inner_html(
        r#"<div class="navroot"><div class="outlet"><div class="screen"></div></div></div>"#,
    );
    let ssr_outlet = doc.query_selector("#app .outlet").unwrap().unwrap();

    let mut backend = WebBackend::hydrate("#app");

    let root = backend.create_view_impl(&Default::default());
    backend.hydrate_nav_screen_begin_impl(&root, "");
    // No marker → parked cursor → fresh build (NOT an adoption of the
    // outlet, which is the shift this whole fix removes).
    let screen = backend.create_view_impl(&Default::default());
    assert!(
        !screen
            .dyn_ref::<web_sys::Element>()
            .unwrap()
            .is_same_node(Some(ssr_outlet.as_ref())),
        "without the marker the screen must build fresh, never consume the outlet",
    );
    backend.hydrate_nav_screen_end_impl();

    let outlet = backend.create_view_impl(&Default::default());
    assert!(
        outlet
            .dyn_ref::<web_sys::Element>()
            .unwrap()
            .is_same_node(Some(ssr_outlet.as_ref())),
        "the layout build adopts the outlet after the restore",
    );

    crate::scheduler::end_hydration_buffering();
}

/// Count element children of `el` via the sibling chain (`.children()` /
/// `child_element_count()` aren't in this crate's enabled web-sys feature
/// set; `first_element_child` / `next_element_sibling` are).
#[cfg(feature = "hydrate")]
fn count_element_children(el: &web_sys::Element) -> u32 {
    let mut n = 0;
    let mut cur = el.first_element_child();
    while let Some(c) = cur {
        n += 1;
        cur = c.next_element_sibling();
    }
    n
}

/// REGRESSION TEST — same divergence-remount invariant for the batched
/// `insert_many` attach path (the `Repeat` fallback collects rows then
/// hands the lot here). A remount root in the batch must be swapped in for
/// its stale SSR node; the rest of the batch inserts normally.
#[cfg(feature = "hydrate")]
#[wasm_bindgen_test]
fn insert_many_removes_stale_ssr_node_on_divergence_remount() {

    install_mount();
    let doc = web_sys::window().unwrap().document().unwrap();
    let app = doc.get_element_by_id("app").unwrap();

    // SSR: app root with a stale `<span>` at the cursor.
    app.set_inner_html(r#"<div class="approot"><span class="ssr-row">STALE</span></div>"#);

    let mut backend = WebBackend::hydrate("#app");
    let mut approot = backend.create_view_impl(&Default::default());

    // Fresh `<div>` where SSR had `<span>` → arms the remount.
    let fresh_row = backend.create_view_impl(&Default::default());

    backend.insert_many_impl(&mut approot, vec![fresh_row.clone()]);

    let approot_el: web_sys::Element = approot.unchecked_into();
    assert_eq!(
        count_element_children(&approot_el),
        1,
        "insert_many must swap the remount root in for its stale SSR node",
    );
    assert_eq!(
        doc.query_selector_all("#app .ssr-row").unwrap().length(),
        0,
        "the stale SSR row must not survive after an insert_many remount resync",
    );

    // TEST HYGIENE: `WebBackend::hydrate` arms the scheduler's hydration
    // microtask buffer; only `finish` (never called here) disarms it. A
    // leaked armed buffer swallows LATER tests' scheduled microtasks —
    // the `regression_image_on_load_cached_does_not_reenter_borrow`
    // "deferred on_load must fire" failure under `--features hydrate`.
    crate::scheduler::end_hydration_buffering();
}
/// the fresh node in for the stale host and resumes the cursor at the host's
/// next sibling — so the sibling adopts cleanly.
#[cfg(feature = "hydrate")]
#[wasm_bindgen_test]
fn create_external_consumes_stale_ssr_host_when_handler_builds_fresh() {
    use std::any::{Any, TypeId};
    use std::rc::Rc;

    struct CanvasLike;

    install_mount();
    let doc = web_sys::window().unwrap().document().unwrap();
    let app = doc.get_element_by_id("app").unwrap();

    // SSR: external host (empty div) then a sibling span.
    app.set_inner_html(
        r#"<div class="approot"><div class="ssr-ext"></div><span class="sib">SIB</span></div>"#,
    );

    let mut backend = WebBackend::hydrate("#app");
    // Runtime v2: `create_external` always builds FRESH (it is the
    // placeholder path — SDK handlers live on the scene registry now), so
    // it is permanently on the non-adopting side of this contract, which
    // is exactly what this regression guards.

    // Adopt approot; cursor descends onto the stale `ssr-ext` div.
    let mut approot = backend.create_view_impl(&Default::default());

    let payload: Rc<dyn Any> = Rc::new(CanvasLike);
    let ext = backend.create_external_impl(
        TypeId::of::<CanvasLike>(),
        "canvas-like",
        &payload,
        &Default::default(),
    );
    // The walker parents the external via insert_at → resync swaps the fresh
    // node in for the stale host.
    backend.insert_at_impl(&mut approot, ext.clone(), 0);

    // Stale SSR host detached — not left orphaned next to the fresh node.
    assert_eq!(
        doc.query_selector_all("#app .ssr-ext").unwrap().length(),
        0,
        "unconsumed SSR external host must be detached (the stale-canvas-div bug)",
    );
    // approot holds exactly [fresh-external, sibling] — the fresh node took
    // the host's slot, the sibling is untouched.
    let approot_el: web_sys::Element = approot.unchecked_into();
    assert_eq!(count_element_children(&approot_el), 2, "expected [external, sibling]");
    assert!(
        approot_el.first_element_child().unwrap().is_same_node(Some(ext.unchecked_ref())),
        "the external's fresh node must occupy the stale host's position",
    );
    // Cursor resumed at the sibling span, so it adopts cleanly instead of
    // mismatching (the divergence cascade the bug caused).
    let span = doc.query_selector("#app .sib").unwrap().unwrap();
    assert!(
        backend
            .hydration_cursor
            .as_ref()
            .map(|c| c.is_same_node(Some(span.as_ref())))
            .unwrap_or(false),
        "cursor must resume at the external's next sibling for clean adoption",
    );

    // TEST HYGIENE: `WebBackend::hydrate` arms the scheduler's hydration
    // microtask buffer; only `finish` (never called here) disarms it. A
    // leaked armed buffer swallows LATER tests' scheduled microtasks —
    // the `regression_image_on_load_cached_does_not_reenter_borrow`
    // "deferred on_load must fire" failure under `--features hydrate`.
    crate::scheduler::end_hydration_buffering();
}
// `create_external_drains_lazily_deferred_handler_before_dispatch` and
// `create_external_adopting_handler_reuses_ssr_host` are gone with the
// backend-side External registry (runtime v2: third-party primitives
// register a payload handler on the scene `Registry`, which dispatches
// before any backend cap, so there is no per-backend handler table and
// no `defer_external_registration` drain). The SDK-side successors are
// each SDK's `tests/newcore.rs` op-log suite; the "unregistered payload"
// behaviour is the scene contract, pinned in runtime-scene/vocabulary.

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
    use runtime_shared::{Color, FlexDirection, StyleRules, Tokenized};
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
    backend.apply_styled_states_impl(&node1, &color_rules, &[]);
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
    backend.apply_styled_states_impl(&node2, &flex_rules, &[]);
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

use runtime_shared::primitives::icon::{FillRule, IconData};
use runtime_shared::Color;

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

// ---------------------------------------------------------------------------
// Portal focus-trap re-entrancy
// ---------------------------------------------------------------------------

/// REGRESSION TEST.
///
/// The portal focus trap installs a `focusin` listener on `document`
/// that bounces escaping focus back into the portal via `.focus()`.
/// That `.focus()` SYNCHRONOUSLY re-dispatches `focusin`, re-entering
/// the very same listener before the outer call returns.
///
/// When the listener was a `Closure<dyn FnMut>`, wasm-bindgen's
/// exclusive-borrow guard threw "closure invoked recursively or after
/// being dropped" at the FFI boundary on that re-entrant call —
/// BEFORE the Rust body (and its `in_progress` short-circuit) could
/// run. The throw escaped as an uncaught exception inside the inner
/// `focusin` dispatch and surfaced as a console error on every focus
/// bounce (and on teardown, when removing a focused portal moves
/// focus through the trap). The fix makes the listener a
/// `Closure<dyn Fn>`, which carries no re-entrancy guard (re-entry is
/// memory-safe through the `RefCell`), so the inner call runs the body
/// and `in_progress` bails it cleanly with no throw.
///
/// This test drives a focus escape through a real trap and asserts (a)
/// no uncaught error is reported to `window`, and (b) the trap still
/// works (focus lands back on the portal's first focusable child).
/// Against the old `FnMut` listener, assertion (a) fails; the fix
/// makes both hold.
#[wasm_bindgen_test]
fn portal_focus_trap_bounce_does_not_throw_on_reentrant_focusin() {
    use std::cell::Cell;
    use std::rc::Rc;
    use wasm_bindgen::closure::Closure;

    install_mount();
    let doc = web_sys::window().unwrap().document().unwrap();
    let body = doc.body().expect("body");

    // A portal subtree with two focusable children, plus an element
    // OUTSIDE the portal to focus first (so the next focus escape
    // triggers the bounce).
    let portal_root = doc.create_element("div").expect("portal root");
    let inside_a = doc.create_element("button").expect("inside a");
    let inside_b = doc.create_element("button").expect("inside b");
    portal_root.append_child(&inside_a).unwrap();
    portal_root.append_child(&inside_b).unwrap();
    let outside = doc.create_element("button").expect("outside");
    body.append_child(&portal_root).unwrap();
    body.append_child(&outside).unwrap();

    // Count uncaught errors reported to `window`. The re-entrant
    // FnMut throw escapes the inner `focusin` dispatch and is
    // reported here synchronously ("report the exception"). We
    // `prevent_default` so a captured error doesn't also trip the
    // test runner's own handler — our assertion below is the signal.
    let errors: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let errors_for_cb = errors.clone();
    let on_error: Closure<dyn FnMut(web_sys::Event)> =
        Closure::wrap(Box::new(move |ev: web_sys::Event| {
            errors_for_cb.set(errors_for_cb.get() + 1);
            ev.prevent_default();
        }) as Box<dyn FnMut(web_sys::Event)>);
    let window = web_sys::window().unwrap();
    window
        .add_event_listener_with_callback_and_bool(
            "error",
            on_error.as_ref().unchecked_ref(),
            true, // capture phase — run before any runner-installed handler
        )
        .unwrap();

    // Arm the trap (keep the returned Closure alive for the duration).
    let _trap = crate::primitives::portal::install_focus_trap(&doc, portal_root.clone())
        .expect("install_focus_trap returns the listener closure");

    // Move focus OUTSIDE the portal. This fires `focusin`
    // (target = outside) → the trap calls `.focus()` on `inside_a`
    // → that synchronously re-dispatches `focusin` (target = inside_a),
    // re-entering the listener. The old `FnMut` listener threw here.
    let outside_html: web_sys::HtmlElement = outside.unchecked_into();
    outside_html.focus().expect("focus outside");

    // (a) No uncaught error must have been reported.
    assert_eq!(
        errors.get(),
        0,
        "focus-trap bounce re-entered its own `focusin` listener and threw \
         (closure invoked recursively) — the listener must be `Fn`, not `FnMut`",
    );

    // (b) The trap actually worked: focus landed back inside the portal.
    let active = doc.active_element();
    let landed_inside = active
        .as_ref()
        .map(|a| portal_root.contains(Some(a.as_ref())))
        .unwrap_or(false);
    assert!(
        landed_inside,
        "focus trap must bounce focus back into the portal subtree; active element was {:?}",
        active.map(|a| a.tag_name()),
    );

    // Cleanup so later tests don't inherit the document `error`
    // listener.
    let _ = window.remove_event_listener_with_callback_and_bool(
        "error",
        on_error.as_ref().unchecked_ref(),
        true,
    );
    drop(on_error);
}

// ---------------------------------------------------------------------------
// Touch responder model — regression for the web backend ignoring
// `TouchResponse::CONSUMED` when deciding event propagation.
//
// The framework's responder contract (crates/runtime/core/src/touch/mod.rs)
// says: whichever ancestor consumes the `Began` keeps the gesture and the
// event does NOT bubble to further ancestors; an unconsumed `Began` bubbles
// up to retry one level higher. The web backend delivers touches via native
// DOM Pointer Event listeners and relies on DOM bubbling to walk the
// ancestor chain — so honoring "consumed" means calling
// `stop_propagation()` on the consumed event. Before the fix it never did,
// and a child's CONSUMED still let the parent's `on_touch` fire (e.g. an
// overlay closing on every interior button press).
// ---------------------------------------------------------------------------

/// Build a `pointerdown` that actually bubbles, dispatch it on `target`,
/// and let it walk the ancestor chain like a real pointer press.
fn dispatch_bubbling_pointerdown(target: &web_sys::Element) {
    let init = web_sys::PointerEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    let ev = web_sys::PointerEvent::new_with_event_init_dict("pointerdown", &init)
        .expect("construct bubbling pointerdown");
    target.dispatch_event(&ev).expect("dispatch pointerdown");
}

/// REGRESSION TEST.
///
/// A child whose `on_touch` returns `CONSUMED` must stop the `Began` from
/// also reaching an ancestor's `on_touch`. Before the fix the web backend
/// never called `stop_propagation`, so both handlers fired — the
/// overlay-tap-to-close-on-interior-press bug.
#[wasm_bindgen_test]
fn regression_web_touch_consumed_child_stops_ancestor_on_touch() {
    use runtime_shared::{TouchResponse};
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    // Parent wraps child; both are connected to the document so DOM
    // bubbling is live (listeners are registered bubble-phase, so child
    // fires before parent).
    let parent = doc.create_element("div").unwrap();
    let child = doc.create_element("div").unwrap();
    parent.append_child(&child).unwrap();
    doc.body().unwrap().append_child(&parent).unwrap();

    let parent_fired = Rc::new(Cell::new(false));
    let child_fired = Rc::new(Cell::new(false));

    let pf = parent_fired.clone();
    backend.install_touch_handler_impl(
        &parent.clone().unchecked_into(),
        Rc::new(move |_| {
            pf.set(true);
            TouchResponse::CONSUMED
        }),
    );
    let cf = child_fired.clone();
    backend.install_touch_handler_impl(
        &child.clone().unchecked_into(),
        Rc::new(move |_| {
            cf.set(true);
            TouchResponse::CONSUMED
        }),
    );

    dispatch_bubbling_pointerdown(&child);

    assert!(child_fired.get(), "child on_touch must fire on its own press");
    assert!(
        !parent_fired.get(),
        "child CONSUMED must stop the Began from bubbling to the parent on_touch",
    );
}

/// REGRESSION TEST (companion).
///
/// The fix must NOT break the bubble-up retry: an `IGNORED` (unconsumed)
/// child press must still reach the parent, since the responder model
/// re-tries one level up until someone consumes.
#[wasm_bindgen_test]
fn web_touch_ignored_child_still_bubbles_to_ancestor() {
    use runtime_shared::{TouchResponse};
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    let parent = doc.create_element("div").unwrap();
    let child = doc.create_element("div").unwrap();
    parent.append_child(&child).unwrap();
    doc.body().unwrap().append_child(&parent).unwrap();

    let parent_fired = Rc::new(Cell::new(false));
    let child_fired = Rc::new(Cell::new(false));

    let pf = parent_fired.clone();
    backend.install_touch_handler_impl(
        &parent.clone().unchecked_into(),
        Rc::new(move |_| {
            pf.set(true);
            TouchResponse::CONSUMED
        }),
    );
    let cf = child_fired.clone();
    backend.install_touch_handler_impl(
        &child.clone().unchecked_into(),
        Rc::new(move |_| {
            cf.set(true);
            TouchResponse::IGNORED
        }),
    );

    dispatch_bubbling_pointerdown(&child);

    assert!(child_fired.get(), "child on_touch must fire on its own press");
    assert!(
        parent_fired.get(),
        "an IGNORED child press must still bubble up to the parent on_touch",
    );
}

/// REGRESSION TEST.
///
/// A `Pressable` (idea-ui `Button` / `IconButton`) inside a view that carries
/// `on_touch` — the "clickable table row with buttons in it" pattern — must
/// swallow the press so the ancestor's `on_touch` tap does NOT also fire.
/// A pressable activates through a native `click`, a different event channel
/// from the pointer-based `on_touch` responder chain, so before the fix the
/// button's `pointerdown` bubbled straight to the row and BOTH fired. The
/// pressable now installs a `pointerdown`-swallowing listener
/// (`touch::swallow_ancestor_touch`) to match native's single-view delivery.
///
/// Contrast with `web_touch_ignored_child_still_bubbles_to_ancestor` above: a
/// plain child bubbles; an interactive-leaf child does not.
#[wasm_bindgen_test]
fn regression_web_pressable_swallows_ancestor_on_touch() {
    use runtime_shared::{TouchResponse};
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    // The ancestor stands in for a clickable table row's `<td>`: it carries an
    // `on_touch` that would fire the row callback.
    let row = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&row).unwrap();
    let row_fired = Rc::new(Cell::new(false));
    let rf = row_fired.clone();
    backend.install_touch_handler_impl(
        &row.clone().unchecked_into(),
        Rc::new(move |_| {
            rf.set(true);
            TouchResponse::CONSUMED
        }),
    );

    // A real Pressable, parented into the row.
    let pressable: web_sys::Node =
        backend.create_pressable_impl(Rc::new(|| {}), &Default::default());
    row.append_child(&pressable).unwrap();
    let pressable_el: web_sys::Element = pressable.unchecked_into();

    dispatch_bubbling_pointerdown(&pressable_el);

    assert!(
        !row_fired.get(),
        "a press on a Pressable must NOT reach the ancestor row's on_touch",
    );
}

/// REGRESSION TEST.
///
/// A `mark_preserves_focus` region (a combobox's anchored option menu) must
/// cancel the `pointerdown` default for presses ANYWHERE inside it — the
/// browser's focus move is `mousedown`'s default action, so the canceled
/// default is what keeps the anchoring input focused while a row is
/// clicked. The press target being a Pressable row is the hard case: the
/// row's own bubble-phase `stopPropagation` (the ancestor-touch swallow,
/// asserted above) would starve a bubble-phase listener on the marked
/// ancestor — the mark's listener must run in the CAPTURE phase to see the
/// press at all.
#[wasm_bindgen_test]
fn regression_web_preserves_focus_cancels_pointerdown_through_pressable_swallow() {
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    // The marked ancestor stands in for the combobox menu panel.
    let panel = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&panel).unwrap();
    backend.mark_preserves_focus_impl(&panel.clone().unchecked_into());

    // A real Pressable row inside it — installs the pointerdown swallow.
    let row: web_sys::Node = backend.create_pressable_impl(Rc::new(|| {}), &Default::default());
    panel.append_child(&row).unwrap();
    let row_el: web_sys::Element = row.unchecked_into();

    let init = web_sys::PointerEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    let ev = web_sys::PointerEvent::new_with_event_init_dict("pointerdown", &init)
        .expect("construct bubbling pointerdown");
    row_el.dispatch_event(&ev).expect("dispatch pointerdown");

    assert!(
        ev.default_prevented(),
        "a press inside a preserves_focus region must cancel the pointerdown \
         default (the focus steal), even when the press target is a Pressable \
         whose swallow stops bubble-phase propagation",
    );
}

/// REGRESSION TEST.
///
/// The cancel above must NOT extend to a text-entry control inside the
/// region. A menu panel with a pinned `header` slot (idea-ui's `Menu` /
/// `SubMenu` search field) is marked so its ROWS don't blur the field —
/// but the browser's focus move is `mousedown`'s default action, so a
/// blanket cancel made the field impossible to focus by clicking it, which
/// is the one thing a search slot exists for. AppKit's half of the mark
/// already lets a press reach an `NSTextField` inside the region; the
/// exemption is what keeps the two backends observably the same.
#[wasm_bindgen_test]
fn regression_web_preserves_focus_lets_a_press_focus_a_text_field_inside_it() {
    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    // The marked ancestor stands in for the slotted menu panel.
    let panel = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&panel).unwrap();
    backend.mark_preserves_focus_impl(&panel.clone().unchecked_into());

    // The header slot's search field.
    let input = doc.create_element("input").unwrap();
    panel.append_child(&input).unwrap();

    let init = web_sys::PointerEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    let ev = web_sys::PointerEvent::new_with_event_init_dict("pointerdown", &init)
        .expect("construct bubbling pointerdown");
    input.dispatch_event(&ev).expect("dispatch pointerdown");

    assert!(
        !ev.default_prevented(),
        "a press on a text-entry control inside a preserves_focus region must \
         keep its default (the focus move) — otherwise the pinned search field \
         can never be focused by clicking it",
    );
}

/// REGRESSION TEST.
///
/// A node's tracked listeners must be DETACHED when its teardown record is
/// dropped, not merely orphaned. Dropping a `Closure` only invalidates its
/// JS shim — the listener stays registered — and node teardown runs the
/// style effect's cleanup (`on_node_unstyled`, which clears the record)
/// BEFORE the DOM removal. Removing a still-focused `<input>` makes the
/// browser fire `blur` during that removal, so closing a menu or a modal
/// while a field inside it had focus threw "closure invoked recursively or
/// after being dropped" on every close.
#[wasm_bindgen_test]
fn regression_web_node_teardown_detaches_listeners_before_dropping_them() {
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    let input = doc.create_element("input").unwrap();
    doc.body().unwrap().append_child(&input).unwrap();
    let node: web_sys::Node = input.clone().unchecked_into();
    let id = backend.node_id(&node);

    let fired = Rc::new(Cell::new(0));
    let counter = fired.clone();
    let closure = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::Event)>::new(
        move |_e: web_sys::Event| counter.set(counter.get() + 1),
    );
    backend.track_listener(id, &input, "blur", false, closure);

    let ev = web_sys::Event::new("blur").expect("construct blur");
    input.dispatch_event(&ev).expect("dispatch blur");
    assert_eq!(fired.get(), 1, "a tracked listener fires while the node is live");

    // Teardown: what `on_node_unstyled` does to the node's record.
    backend.state_listeners.remove(&id);

    let ev = web_sys::Event::new("blur").expect("construct blur");
    input.dispatch_event(&ev).expect("dispatch blur");
    assert_eq!(
        fired.get(),
        1,
        "after teardown the listener must be GONE from the element — a \
         still-registered listener over a dropped closure throws when the \
         removal of a focused input fires blur",
    );
}

// ---------------------------------------------------------------------------
// Pointer capture is bound to CONSUME, not to CLAIM.
//
// The responder model promises that whichever handler consumes the `Began`
// keeps every later event for that `TouchId`. Every native backend gives that
// for free; the DOM does not — an uncaptured `pointermove` is dispatched to
// whatever is under the cursor, so an element only heard about motion that
// stayed inside its own rect. Capture used to wait for `claim: true`, which no
// slop-gated recognizer (pan, drag, pinch) can return until it has measured
// travel — travel it can only measure from events it is no longer being sent.
// A normal-speed flick off a small handle therefore produced a `Began` and
// then silence, and the press became a native text selection instead.
// ---------------------------------------------------------------------------

/// Build a bubbling, cancelable pointer event of `kind` for `pointer_id` and
/// dispatch it on `target`. Wider than [`dispatch_bubbling_pointerdown`]: the
/// capture tests need a specific pointer id, and the selection tests need the
/// matching `pointerup` to close the gesture back down.
fn dispatch_bubbling_pointer(target: &web_sys::Element, kind: &str, pointer_id: i32) {
    let init = web_sys::PointerEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    init.set_pointer_id(pointer_id);
    let ev = web_sys::PointerEvent::new_with_event_init_dict(kind, &init)
        .unwrap_or_else(|_| panic!("construct bubbling {kind}"));
    target.dispatch_event(&ev).expect("dispatch pointer event");
}

/// Dispatch the bubbling, cancelable `selectstart` a browser fires when a
/// press starts anchoring a highlight, and report whether it was cancelled.
fn selectstart_was_suppressed(target: &web_sys::Element) -> bool {
    let init = web_sys::EventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    let ev = web_sys::Event::new_with_event_init_dict("selectstart", &init)
        .expect("construct selectstart");
    target.dispatch_event(&ev).expect("dispatch selectstart");
    ev.default_prevented()
}

/// REGRESSION TEST.
///
/// A handler that CONSUMES the `Began` and never claims must still get the
/// pointer captured, because capture is how the DOM keeps delivering. Before
/// the fix capture was gated on `claim: true`, so a `DragRecognizer` at the
/// default 8 px slop was unrecognizable on any handle smaller than ~2× the
/// slop: the first coalesced `pointermove` of a real flick lands outside the
/// handle and is dispatched somewhere else entirely.
///
/// The browser refuses `setPointerCapture` for a pointer id that matches no
/// *active* pointer, which a synthesized event never does, so what is
/// observable here is that the backend ASKED — which is the decision the bug
/// was in.
#[wasm_bindgen_test]
fn regression_web_consumed_press_captures_pointer_without_a_claim() {
    use runtime_shared::TouchResponse;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    let el = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();
    backend.install_touch_handler_impl(
        &el.clone().unchecked_into(),
        // CONSUMED, never CLAIMED — exactly what every slop-gated recognizer
        // returns for its whole tracking phase.
        Rc::new(move |_| TouchResponse::CONSUMED),
    );

    let _ = crate::primitives::touch::take_capture_attempts();
    dispatch_bubbling_pointer(&el, "pointerdown", 41);

    assert_eq!(
        crate::primitives::touch::take_capture_attempts(),
        vec![41],
        "a consumed Began must capture the pointer so the gesture keeps \
         receiving moves once the cursor leaves the element",
    );

    dispatch_bubbling_pointer(&el, "pointerup", 41);
}

/// COMPANION.
///
/// Capture follows ownership: a handler that IGNORED the press does not own
/// the pointer, so the backend must not lock the pointer to it — the `Began`
/// is still bubbling up to look for an ancestor that wants it.
#[wasm_bindgen_test]
fn web_touch_ignored_press_does_not_capture_the_pointer() {
    use runtime_shared::TouchResponse;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    let el = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();
    backend.install_touch_handler_impl(
        &el.clone().unchecked_into(),
        Rc::new(move |_| TouchResponse::IGNORED),
    );

    let _ = crate::primitives::touch::take_capture_attempts();
    dispatch_bubbling_pointer(&el, "pointerdown", 42);

    assert!(
        crate::primitives::touch::take_capture_attempts().is_empty(),
        "an unconsumed press must not be captured — the Began is still \
         looking for an ancestor to own it",
    );
}

/// REGRESSION TEST.
///
/// The second-order damage of a gesture press: the browser treats it as a
/// selection anchor, so a press that fails to become a drag sweeps a
/// highlight across whatever sits beside the handle. That is the symptom
/// users actually report ("it highlights the text instead of dragging"),
/// and on every native backend the same press selects nothing. While this
/// element owns a press, `selectstart` under it is cancelled.
#[wasm_bindgen_test]
fn regression_web_gesture_press_suppresses_native_text_selection() {
    use runtime_shared::TouchResponse;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    let handle = doc.create_element("div").unwrap();
    let label = doc.create_element("span").unwrap();
    label.set_text_content(Some("drag me"));
    handle.append_child(&label).unwrap();
    doc.body().unwrap().append_child(&handle).unwrap();
    backend.install_touch_handler_impl(
        &handle.clone().unchecked_into(),
        Rc::new(move |_| TouchResponse::CONSUMED),
    );

    assert!(
        !selectstart_was_suppressed(&label),
        "with no press in flight the element must not interfere with \
         ordinary text selection",
    );

    dispatch_bubbling_pointer(&handle, "pointerdown", 43);
    assert!(
        selectstart_was_suppressed(&label),
        "a press this handler consumed is a gesture, not a selection anchor",
    );

    dispatch_bubbling_pointer(&handle, "pointerup", 43);
    assert!(
        !selectstart_was_suppressed(&label),
        "the suppression must last only as long as the gesture — the release \
         hands selection back",
    );
}

/// COMPANION.
///
/// Suppression stops where something else owns the selection: an editable
/// field (its caret IS the selection) and any subtree the author opted back
/// in through `user_select`. Without these a text input inside a draggable
/// card would go un-selectable, and the `UserSelect::Text` prop would be
/// unreachable on any gesture surface.
#[wasm_bindgen_test]
fn web_gesture_press_leaves_owned_selections_alone() {
    use runtime_shared::TouchResponse;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    let card = doc.create_element("div").unwrap();
    let field = doc.create_element("input").unwrap();
    let selectable = doc.create_element("span").unwrap();
    selectable
        .set_attribute("style", "user-select: text; -webkit-user-select: text")
        .unwrap();
    card.append_child(&field).unwrap();
    card.append_child(&selectable).unwrap();
    doc.body().unwrap().append_child(&card).unwrap();
    backend.install_touch_handler_impl(
        &card.clone().unchecked_into(),
        Rc::new(move |_| TouchResponse::CONSUMED),
    );

    dispatch_bubbling_pointer(&card, "pointerdown", 44);

    assert!(
        !selectstart_was_suppressed(&field),
        "an editable field owns its own selection even inside a gesture surface",
    );
    assert!(
        !selectstart_was_suppressed(&selectable),
        "an explicit user-select opt-in must survive a gesture press",
    );

    dispatch_bubbling_pointer(&card, "pointerup", 44);
}

/// REGRESSION TEST.
///
/// A `Toggle` inside a view that carries `on_touch` must swallow the press,
/// like `Button` / `Link` / `Pressable` already do. The double-fire is the
/// same bug they had, but for a checkbox it is worse than cosmetic: a
/// checkbox activates on the synthesized `click`, which is dispatched at the
/// element the pointer was targeted at — and an ancestor that captures the
/// pointer (which it now does the moment its handler consumes the `Began`)
/// moves that target off the checkbox, so the toggle silently stops toggling.
/// Verified in Chrome against a bare page: with an ancestor capturing at
/// press, a real click on a nested checkbox never fires `change`.
#[wasm_bindgen_test]
fn regression_web_toggle_swallows_ancestor_on_touch() {
    use runtime_shared::TouchResponse;
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    let row = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&row).unwrap();
    let row_fired = Rc::new(Cell::new(false));
    let rf = row_fired.clone();
    backend.install_touch_handler_impl(
        &row.clone().unchecked_into(),
        Rc::new(move |_| {
            rf.set(true);
            TouchResponse::CONSUMED
        }),
    );

    let toggle: web_sys::Node =
        backend.create_toggle_impl(false, Rc::new(|_| {}), &Default::default());
    row.append_child(&toggle).unwrap();
    let toggle_el: web_sys::Element = toggle.unchecked_into();

    let _ = crate::primitives::touch::take_capture_attempts();
    dispatch_bubbling_pointerdown(&toggle_el);

    assert!(
        !row_fired.get(),
        "a press on a Toggle must NOT reach the ancestor row's on_touch",
    );
    assert!(
        crate::primitives::touch::take_capture_attempts().is_empty(),
        "the row never consumed the press, so it must not capture the pointer \
         away from the checkbox's click",
    );
}

// ---------------------------------------------------------------------------
// Secondary-press delivery via `contextmenu` — FRAMEWORK-NOTES #95.
// ---------------------------------------------------------------------------

/// Dispatch a bubbling `pointerdown` with `button == 2` — the shape Safari
/// (macOS Ctrl-click remap) and every browser's two-finger / right-button
/// press produce.
fn dispatch_bubbling_secondary_pointerdown(target: &web_sys::Element) {
    let init = web_sys::PointerEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    init.set_button(2);
    let ev = web_sys::PointerEvent::new_with_event_init_dict("pointerdown", &init)
        .expect("construct secondary pointerdown");
    target.dispatch_event(&ev).expect("dispatch pointerdown");
}

/// Dispatch a bubbling `contextmenu` as a plain `MouseEvent` — the shape
/// Firefox delivers, and (modulo the PointerEvent subclass) what Chrome on
/// macOS delivers for a Ctrl-click after suppressing the `pointerdown`.
/// Returns the event so callers can assert `default_prevented`.
fn dispatch_bubbling_contextmenu(target: &web_sys::Element, ctrl: bool) -> web_sys::MouseEvent {
    let init = web_sys::MouseEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    init.set_ctrl_key(ctrl);
    let ev = web_sys::MouseEvent::new_with_mouse_event_init_dict("contextmenu", &init)
        .expect("construct contextmenu");
    target.dispatch_event(&ev).expect("dispatch contextmenu");
    ev
}

/// Dispatch a bubbling `pointerdown` with `button == 0`, `ctrlKey`, and
/// pointerType "mouse" — the macOS Ctrl-click shape Chrome and Firefox
/// deliver when they don't suppress the pointerdown outright.
fn dispatch_bubbling_ctrl_primary_pointerdown(target: &web_sys::Element) {
    let init = web_sys::PointerEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    init.set_button(0);
    init.set_ctrl_key(true);
    init.set_pointer_type("mouse");
    let ev = web_sys::PointerEvent::new_with_event_init_dict("pointerdown", &init)
        .expect("construct ctrl primary pointerdown");
    target.dispatch_event(&ev).expect("dispatch pointerdown");
}

/// REGRESSION TEST (FRAMEWORK-NOTES #95, follow-up).
///
/// Chrome and Firefox on macOS can also deliver Ctrl-click as a *primary*
/// `pointerdown` with `ctrlKey: true`, followed by `contextmenu`. Delivered
/// as Primary, that Began reaches app code as a Ctrl-modified primary click —
/// in a selection grid, the remove-block modifier — and the (synthesized or
/// real) Secondary then acts on the corrupted state: right-clicking inside a
/// selection deselected it. On macOS the backend must fold `button == 0 &&
/// ctrlKey` (mouse only) into `PointerButton::Secondary` at classification,
/// so the press rides the normal secondary path: one `Began`, Secondary, no
/// re-delivery from the `contextmenu`, and the stray `button == 0` pointerup
/// is ignored (the press never entered the active set).
#[wasm_bindgen_test]
fn regression_web_mac_ctrl_primary_pointerdown_folds_to_secondary() {
    use runtime_shared::{PointerButton, TouchPhase, TouchResponse};
    use std::cell::RefCell;
    use std::rc::Rc;

    crate::primitives::touch::force_ctrl_click_fold(Some(true)); // pin "macOS"

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();

    let seen: Rc<RefCell<Vec<(TouchPhase, PointerButton)>>> = Rc::new(RefCell::new(Vec::new()));
    let s = seen.clone();
    backend.install_touch_handler_impl(
        &el.clone().unchecked_into(),
        Rc::new(move |te| {
            s.borrow_mut().push((te.phase, runtime_shared::pointer_button()));
            TouchResponse::CONSUMED
        }),
    );

    // The full Chrome/Firefox macOS Ctrl-click sequence: primary-with-ctrl
    // pointerdown, contextmenu, then the stray button-0 release.
    dispatch_bubbling_ctrl_primary_pointerdown(&el);
    let ctx = dispatch_bubbling_contextmenu(&el, true);
    {
        let init = web_sys::PointerEventInit::new();
        init.set_bubbles(true);
        init.set_button(0);
        let up = web_sys::PointerEvent::new_with_event_init_dict("pointerup", &init)
            .expect("construct pointerup");
        el.dispatch_event(&up).expect("dispatch pointerup");
    }
    let events = seen.borrow().clone();
    assert_eq!(
        events.first(),
        Some(&(TouchPhase::Began, PointerButton::Secondary)),
        "macOS Ctrl-click must be classified Secondary at pointerdown, not \
         delivered as a Ctrl-modified Primary",
    );
    assert_eq!(
        events.iter().filter(|(_, b)| *b == PointerButton::Secondary).count(),
        1,
        "the contextmenu after the folded pointerdown must not re-deliver the press",
    );
    assert!(ctx.default_prevented(), "the native menu must still be suppressed");
    assert!(
        !events.iter().any(|(p, _)| *p == TouchPhase::Ended),
        "a secondary press is Began-only; the stray button-0 pointerup must be ignored",
    );

    crate::primitives::touch::force_ctrl_click_fold(None);
}

/// COMPANION. On Windows/Linux, Ctrl-click is a genuinely modified *primary*
/// press (the add-to-selection idiom) — the fold is macOS-only and must not
/// reclassify it.
#[wasm_bindgen_test]
fn web_non_mac_ctrl_primary_click_stays_primary() {
    use runtime_shared::{PointerButton, TouchPhase, TouchResponse};
    use std::cell::Cell;
    use std::rc::Rc;

    crate::primitives::touch::force_ctrl_click_fold(Some(false)); // pin "not macOS"

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();

    let saw = Rc::new(Cell::new(None::<(TouchPhase, PointerButton, bool)>));
    let s = saw.clone();
    backend.install_touch_handler_impl(
        &el.clone().unchecked_into(),
        Rc::new(move |te| {
            s.set(Some((
                te.phase,
                runtime_shared::pointer_button(),
                runtime_shared::pointer_modifiers().ctrl,
            )));
            TouchResponse::CONSUMED
        }),
    );

    dispatch_bubbling_ctrl_primary_pointerdown(&el);

    assert_eq!(
        saw.get(),
        Some((TouchPhase::Began, PointerButton::Primary, true)),
        "off macOS, Ctrl-click must stay a Primary Began with the ctrl modifier set",
    );

    crate::primitives::touch::force_ctrl_click_fold(None);
}

/// REGRESSION TEST (FRAMEWORK-NOTES #95).
///
/// Chrome on macOS delivers Ctrl-click as ONLY a `contextmenu` event — the
/// `pointerdown` is suppressed entirely (Safari remaps it to `button == 2`
/// at pointerdown; Firefox sends a primary pointerdown AND contextmenu).
/// Before the fix the web backend's `contextmenu` listener was a bare
/// `preventDefault()`, so a Ctrl-click reached app code as nothing at all:
/// no Secondary `Began`, no primary-with-ctrl, and no native menu either.
/// The listener must synthesize the Secondary `Began` when no pointerdown
/// preceded it.
#[wasm_bindgen_test]
fn regression_web_chrome_ctrl_click_contextmenu_only_delivers_secondary_began() {
    use runtime_shared::{PointerButton, TouchPhase, TouchResponse};
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();

    let fired = Rc::new(Cell::new(0u32));
    let saw = Rc::new(Cell::new(None::<(TouchPhase, PointerButton, bool)>));
    let (f, s) = (fired.clone(), saw.clone());
    backend.install_touch_handler_impl(
        &el.clone().unchecked_into(),
        Rc::new(move |te| {
            f.set(f.get() + 1);
            s.set(Some((
                te.phase,
                runtime_shared::pointer_button(),
                runtime_shared::pointer_modifiers().ctrl,
            )));
            TouchResponse::CONSUMED
        }),
    );

    // The Chrome/macOS Ctrl-click shape: contextmenu with NO pointerdown.
    let ev = dispatch_bubbling_contextmenu(&el, true);

    assert_eq!(fired.get(), 1, "the Ctrl-click must reach the on_touch handler");
    let (phase, button, ctrl) = saw.get().expect("handler recorded the event");
    assert_eq!(phase, TouchPhase::Began, "a secondary press is a complete click: Began only");
    assert_eq!(button, PointerButton::Secondary, "Ctrl-click must surface as Secondary");
    assert!(ctrl, "the modifier state must come from the contextmenu event");
    assert!(ev.default_prevented(), "the native menu must still be suppressed");
}

/// COMPANION. When `pointerdown` DID deliver the secondary press (Safari
/// Ctrl-click, any browser's right-button press), the `contextmenu` that
/// follows must NOT re-deliver it — one press, one `Began`. The
/// check-and-clear must not leave the flag stale either: a subsequent
/// pointerdown-less `contextmenu` (same element, Chrome shape) must
/// synthesize again.
#[wasm_bindgen_test]
fn web_contextmenu_after_secondary_pointerdown_is_not_redelivered() {
    use runtime_shared::TouchResponse;
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();

    let fired = Rc::new(Cell::new(0u32));
    let f = fired.clone();
    backend.install_touch_handler_impl(
        &el.clone().unchecked_into(),
        Rc::new(move |_| {
            f.set(f.get() + 1);
            TouchResponse::CONSUMED
        }),
    );

    // Safari / two-finger shape: pointerdown(button=2) then contextmenu.
    dispatch_bubbling_secondary_pointerdown(&el);
    let ev = dispatch_bubbling_contextmenu(&el, false);
    assert_eq!(fired.get(), 1, "one press must deliver exactly one Began");
    assert!(ev.default_prevented(), "the native menu must still be suppressed");

    // Flag was consumed above — the next pointerdown-less contextmenu is a
    // fresh press and must synthesize.
    dispatch_bubbling_contextmenu(&el, true);
    assert_eq!(fired.get(), 2, "a later Chrome-shape Ctrl-click must still be delivered");
}

/// COMPANION. The synthesis must honor the responder model across nesting:
/// when a child's handler CONSUMED the secondary `pointerdown` (which
/// `stop_propagation`s, so the ancestor's pointerdown listener never marks
/// its own flag), the independently-bubbling `contextmenu` must not reach
/// the ancestor and synthesize a press the child already consumed. The
/// flag therefore records the consumed outcome, and `contextmenu` mirrors
/// it: consumed → stop propagation.
#[wasm_bindgen_test]
fn web_contextmenu_consumed_secondary_does_not_leak_to_ancestor() {
    use runtime_shared::TouchResponse;
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let parent = doc.create_element("div").unwrap();
    let child = doc.create_element("div").unwrap();
    parent.append_child(&child).unwrap();
    doc.body().unwrap().append_child(&parent).unwrap();

    let parent_fired = Rc::new(Cell::new(0u32));
    let child_fired = Rc::new(Cell::new(0u32));
    let pf = parent_fired.clone();
    backend.install_touch_handler_impl(
        &parent.clone().unchecked_into(),
        Rc::new(move |_| {
            pf.set(pf.get() + 1);
            TouchResponse::CONSUMED
        }),
    );
    let cf = child_fired.clone();
    backend.install_touch_handler_impl(
        &child.clone().unchecked_into(),
        Rc::new(move |_| {
            cf.set(cf.get() + 1);
            TouchResponse::CONSUMED
        }),
    );

    dispatch_bubbling_secondary_pointerdown(&child);
    dispatch_bubbling_contextmenu(&child, false);

    assert_eq!(child_fired.get(), 1, "the child gets the one Began");
    assert_eq!(
        parent_fired.get(),
        0,
        "a secondary press the child consumed must not be re-synthesized on the ancestor \
         from the independently-bubbling contextmenu",
    );
}

/// REGRESSION TEST (the clickable-row context menu).
///
/// The reported failure, end to end: a table cell made clickable by
/// `TableRow(on_row_click = …)` installs a tap recognizer on EVERY cell.
/// A recognizer consumes from `Began` — it has to, to be sure of hearing
/// the motion it measures — and consuming is what commits the bubble
/// decision. So a right-click on a data row was swallowed by the cell,
/// found not to be a tap, and dropped; the ancestor's context-menu
/// handler never heard it. The header row of the same table, which has no
/// `on_row_click` and therefore no recognizer, opened the menu fine —
/// which is what made it look like a table bug rather than a tap one.
///
/// `Recognizer::drive` now ignores a non-primary `Began`, so it bubbles.
/// This drives the real DOM path, with a real `tap()` on the cell.
#[wasm_bindgen_test]
fn regression_web_secondary_press_on_a_tappable_cell_reaches_the_row_menu() {
    use runtime_shared::{tap, PointerButton, TapRecognizer, TouchPhase, TouchResponse};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let row = doc.create_element("div").unwrap();
    let cell = doc.create_element("div").unwrap();
    row.append_child(&cell).unwrap();
    doc.body().unwrap().append_child(&row).unwrap();

    // The row's context-menu handler: answers a secondary press, ignores
    // everything else so a primary press still falls through to the cell.
    let menu_opens: Rc<RefCell<Vec<TouchPhase>>> = Rc::new(RefCell::new(Vec::new()));
    let mo = menu_opens.clone();
    backend.install_touch_handler_impl(
        &row.clone().unchecked_into(),
        Rc::new(move |ev| {
            if runtime_shared::pointer_button().opens_context_menu() {
                mo.borrow_mut().push(ev.phase);
                return TouchResponse::CONSUMED;
            }
            TouchResponse::IGNORED
        }),
    );

    // The cell is clickable — exactly what `set_cell_interaction` installs.
    let taps = Rc::new(Cell::new(0u32));
    let t = taps.clone();
    backend.install_touch_handler_impl(
        &cell.clone().unchecked_into(),
        tap(TapRecognizer::new(), move || t.set(t.get() + 1)),
    );

    // Right-click the CELL.
    dispatch_bubbling_secondary_pointerdown(&cell);
    let menu = dispatch_bubbling_contextmenu(&cell, false);

    assert_eq!(
        *menu_opens.borrow(),
        vec![TouchPhase::Began],
        "the row's menu handler must hear the secondary press the cell ignored — once",
    );
    assert_eq!(taps.get(), 0, "and a right-click is never a tap");
    assert!(menu.default_prevented(), "the native menu is still suppressed");

    // The cell is still clickable: a primary press taps it and never
    // reaches the row.
    let init = web_sys::PointerEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    init.set_button(0);
    init.set_pointer_id(7);
    let down = web_sys::PointerEvent::new_with_event_init_dict("pointerdown", &init).unwrap();
    cell.dispatch_event(&down).unwrap();
    let up = web_sys::PointerEvent::new_with_event_init_dict("pointerup", &init).unwrap();
    cell.dispatch_event(&up).unwrap();
    assert_eq!(taps.get(), 1, "a left-click still taps the cell");
    assert_eq!(
        menu_opens.borrow().len(),
        1,
        "and the tap does not reach the row",
    );
}

/// COMPANION. An interactive leaf's ancestor-touch swallow must extend to
/// `contextmenu`: right-clicking a Pressable inside an `on_touch` row stops
/// the `pointerdown` at the control, which leaves the row's flag unset —
/// exactly the state that triggers synthesis. Without the swallow covering
/// `contextmenu`, the row would receive a secondary press its control
/// swallowed.
#[wasm_bindgen_test]
fn web_contextmenu_synthesis_respects_pressable_swallow() {
    use runtime_shared::TouchResponse;
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    let row = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&row).unwrap();
    let row_fired = Rc::new(Cell::new(0u32));
    let rf = row_fired.clone();
    backend.install_touch_handler_impl(
        &row.clone().unchecked_into(),
        Rc::new(move |_| {
            rf.set(rf.get() + 1);
            TouchResponse::CONSUMED
        }),
    );

    let pressable: web_sys::Node =
        backend.create_pressable_impl(Rc::new(|| {}), &Default::default());
    row.append_child(&pressable).unwrap();
    let pressable_el: web_sys::Element = pressable.unchecked_into();

    // Chrome-shape right-press on the control: contextmenu with no
    // pointerdown reaching the row.
    let ev = dispatch_bubbling_contextmenu(&pressable_el, false);

    assert_eq!(
        row_fired.get(),
        0,
        "a right-press on a Pressable must not synthesize a Secondary Began on the ancestor row",
    );
    assert!(
        ev.default_prevented(),
        "the native menu over the control must stay suppressed (pre-#95 behavior)",
    );
}

/// COMPANION. Touchscreen long-press also raises `contextmenu` (Chrome ships
/// it as a PointerEvent with pointerType "touch") — but that press already
/// reached app code as a *primary* gesture that may still be in flight, so
/// no Secondary must be injected; suppress-only is preserved for touch.
#[wasm_bindgen_test]
fn web_touch_longpress_contextmenu_does_not_synthesize_secondary() {
    use runtime_shared::TouchResponse;
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();

    let fired = Rc::new(Cell::new(0u32));
    let f = fired.clone();
    backend.install_touch_handler_impl(
        &el.clone().unchecked_into(),
        Rc::new(move |_| {
            f.set(f.get() + 1);
            TouchResponse::CONSUMED
        }),
    );

    let init = web_sys::PointerEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    init.set_pointer_type("touch");
    let ev = web_sys::PointerEvent::new_with_event_init_dict("contextmenu", &init)
        .expect("construct touch contextmenu");
    el.dispatch_event(&ev).expect("dispatch contextmenu");

    assert_eq!(fired.get(), 0, "touch long-press contextmenu must not inject a Secondary");
    assert!(ev.default_prevented(), "the native menu must still be suppressed");
}

// ---------------------------------------------------------------------------
// The window-level release safety net — regression for it being installed
// PER SUBSCRIBED ELEMENT.
//
// `install` needs a `pointerup` / `pointercancel` net on `window` so a release
// that never reaches the element still finishes the gesture. It used to add
// that pair inside `install`, i.e. element-lifetime state on a page-lifetime
// target: an element's own listeners are collected with the element, but
// `window`'s are not, and nothing on the touch path ever removed them. Any app
// that mounts `on_touch` elements dynamically — a virtualized list, a table, a
// grid re-slicing as the user scrolls — therefore added two permanent `window`
// listeners per cell per slice, and every later `pointerup` anywhere on the
// page was dispatched into all of them (plus the closures leaked). One shared
// pair keyed by live pointer id is O(1) in elements ever mounted.
// ---------------------------------------------------------------------------

/// Build a bubbling, cancelable pointer event of `kind` carrying `pointer_id`.
fn pointer_event_with_id(kind: &str, pointer_id: i32) -> web_sys::PointerEvent {
    let init = web_sys::PointerEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    init.set_pointer_id(pointer_id);
    web_sys::PointerEvent::new_with_event_init_dict(kind, &init)
        .expect("construct bubbling pointer event")
}

/// REGRESSION TEST.
///
/// Subscribing many elements must not grow `window`'s listener list. Before the
/// fix this was `2 * elements`; now it is exactly 2 for the whole page, however
/// many elements subscribe and however many gestures run.
#[wasm_bindgen_test]
fn regression_web_touch_window_net_is_shared_not_per_element() {
    use runtime_shared::TouchResponse;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    // 25 independently subscribed elements, each with a live gesture (the net
    // is armed at `pointerdown`, so this is the state that used to have 50
    // window listeners attached).
    const N: i32 = 25;
    // Deltas, not absolutes: the registry is thread-local and shared with the
    // other tests in this module, some of which press without ever releasing.
    let (_, before) = crate::primitives::touch::window_net_stats();
    let mut elements = Vec::new();
    for i in 0..N {
        let el = doc.create_element("div").unwrap();
        doc.body().unwrap().append_child(&el).unwrap();
        backend.install_touch_handler_impl(
            &el.clone().unchecked_into(),
            Rc::new(move |_| TouchResponse::CONSUMED),
        );
        el.dispatch_event(&pointer_event_with_id("pointerdown", 500 + i))
            .expect("dispatch pointerdown");
        elements.push(el);
    }

    let (listeners, armed) = crate::primitives::touch::window_net_stats();
    assert_eq!(
        listeners, 2,
        "the window safety net must be ONE shared pointerup/pointercancel pair \
         for the page, not a pair per subscribed element",
    );
    assert_eq!(
        armed,
        before + N as usize,
        "each live gesture arms the shared net exactly once",
    );

    // Release every gesture and detach, so the shared registry is empty again
    // for other tests (and to prove the registry itself doesn't accumulate).
    for (i, el) in elements.iter().enumerate() {
        el.dispatch_event(&pointer_event_with_id("pointerup", 500 + i as i32))
            .expect("dispatch pointerup");
        el.remove();
    }
    let (_, remaining) = crate::primitives::touch::window_net_stats();
    assert_eq!(
        remaining, before,
        "every released gesture must be unregistered from the shared net",
    );
}

/// COMPANION. The net still does its job: a release that lands off the element
/// (capture didn't hold, the pointer went up over something else) must finish
/// the gesture, exactly once.
#[wasm_bindgen_test]
fn web_touch_off_element_release_ends_gesture_via_shared_net() {
    use runtime_shared::{TouchPhase, TouchResponse};
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();

    let ended = Rc::new(Cell::new(0u32));
    let e = ended.clone();
    backend.install_touch_handler_impl(
        &el.clone().unchecked_into(),
        Rc::new(move |ev| {
            if ev.phase == TouchPhase::Ended {
                e.set(e.get() + 1);
            }
            TouchResponse::CONSUMED
        }),
    );

    // Deltas, not absolutes: other tests share the thread-local registry.
    let (_, before) = crate::primitives::touch::window_net_stats();
    el.dispatch_event(&pointer_event_with_id("pointerdown", 601))
        .expect("dispatch pointerdown");
    let (_, armed) = crate::primitives::touch::window_net_stats();
    assert_eq!(armed, before + 1, "a live gesture arms the shared net");

    // The release never touches the element — it goes straight to `window`.
    let win = web_sys::window().unwrap();
    win.dispatch_event(&pointer_event_with_id("pointerup", 601))
        .expect("dispatch pointerup on window");

    assert_eq!(
        ended.get(),
        1,
        "an off-element release must still deliver Ended through the shared net",
    );
    let (_, after) = crate::primitives::touch::window_net_stats();
    assert_eq!(after, before, "finishing the gesture must disarm the net");

    // A second release for the same pointer must not re-deliver.
    win.dispatch_event(&pointer_event_with_id("pointerup", 601))
        .expect("dispatch second pointerup");
    assert_eq!(ended.get(), 1, "a disarmed pointer must not deliver Ended twice");
    el.remove();
}

/// COMPANION. When the release DOES reach the element, the element's own
/// listener finishes the gesture and the shared net must not double-deliver —
/// both routes run the same `finish`, whose `active` check makes the second a
/// no-op.
#[wasm_bindgen_test]
fn web_touch_on_element_release_disarms_shared_net() {
    use runtime_shared::{TouchPhase, TouchResponse};
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();

    let ended = Rc::new(Cell::new(0u32));
    let e = ended.clone();
    backend.install_touch_handler_impl(
        &el.clone().unchecked_into(),
        Rc::new(move |ev| {
            if ev.phase == TouchPhase::Ended {
                e.set(e.get() + 1);
            }
            TouchResponse::CONSUMED
        }),
    );

    let (_, before) = crate::primitives::touch::window_net_stats();
    el.dispatch_event(&pointer_event_with_id("pointerdown", 602))
        .expect("dispatch pointerdown");
    // Bubbles element → … → window, so both routes see this one release.
    el.dispatch_event(&pointer_event_with_id("pointerup", 602))
        .expect("dispatch pointerup");

    assert_eq!(ended.get(), 1, "the release must deliver exactly one Ended");
    let (_, after) = crate::primitives::touch::window_net_stats();
    assert_eq!(after, before, "the element route must disarm the shared net too");
    el.remove();
}

/// Build a `keydown` for `key` that bubbles and is cancelable, dispatch it on
/// `target`, and return whether its default action ended up prevented.
fn dispatch_bubbling_keydown(target: &web_sys::Element, key: &str) -> bool {
    let init = web_sys::KeyboardEventInit::new();
    init.set_key(key);
    init.set_bubbles(true);
    init.set_cancelable(true);
    let ev = web_sys::KeyboardEvent::new_with_keyboard_event_init_dict("keydown", &init)
        .expect("construct bubbling keydown");
    target.dispatch_event(&ev).expect("dispatch keydown");
    ev.default_prevented()
}

/// REGRESSION TEST.
///
/// A `Space`/`Enter` keydown that originates on a focused DESCENDANT of a
/// pressable (the "text_input inside a Modal" case — the modal's card layer is
/// a no-op pressable) must NOT be treated as an activation. The pressable's
/// bubble-phase `keydown` listener would otherwise call `prevent_default()`,
/// swallowing the space character (or suppressing Enter submit) in the input.
///
/// Only a keydown whose target IS the pressable itself activates it.
#[wasm_bindgen_test]
fn regression_web_pressable_ignores_descendant_key() {
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    // A pressable card layer with a text input nested inside it — the Modal
    // shape: `pressable > ... > <input>`.
    let pressed = Rc::new(Cell::new(false));
    let p = pressed.clone();
    let pressable: web_sys::Node =
        backend.create_pressable_impl(Rc::new(move || p.set(true)), &Default::default());
    let pressable_el: web_sys::Element = pressable.clone().unchecked_into();
    doc.body().unwrap().append_child(&pressable_el).unwrap();

    let input = doc.create_element("input").unwrap();
    pressable_el.append_child(&input).unwrap();

    // Space typed into the descendant input must survive — not prevented,
    // and it must not fire the card-layer press.
    let space_prevented = dispatch_bubbling_keydown(&input, " ");
    assert!(
        !space_prevented,
        "Space in a descendant input must NOT be prevent_default()'d by the ancestor pressable",
    );
    assert!(
        !pressed.get(),
        "Space in a descendant input must NOT activate the ancestor pressable",
    );

    // Enter likewise.
    let enter_prevented = dispatch_bubbling_keydown(&input, "Enter");
    assert!(
        !enter_prevented,
        "Enter in a descendant input must NOT be prevent_default()'d by the ancestor pressable",
    );
    assert!(
        !pressed.get(),
        "Enter in a descendant input must NOT activate the ancestor pressable",
    );

    // But Space ON the pressable itself still activates it (keyboard a11y).
    let self_prevented = dispatch_bubbling_keydown(&pressable_el, " ");
    assert!(
        self_prevented,
        "Space on the pressable itself must still be prevent_default()'d (button a11y)",
    );
    assert!(
        pressed.get(),
        "Space on the pressable itself must still activate it",
    );
}

/// Dispatch a non-bubbling pointer enter/leave event directly at `target`,
/// optionally tagging the pointer type (`""` = unspecified/mouse-like).
fn dispatch_pointer_typed(target: &web_sys::Element, kind: &str, pointer_type: &str) {
    let init = web_sys::PointerEventInit::new();
    if !pointer_type.is_empty() {
        init.set_pointer_type(pointer_type);
    }
    let ev = web_sys::PointerEvent::new_with_event_init_dict(kind, &init)
        .expect("construct pointer event");
    target.dispatch_event(&ev).expect("dispatch pointer event");
}

fn dispatch_pointer(target: &web_sys::Element, kind: &str) {
    dispatch_pointer_typed(target, kind, "");
}

/// `on_hover` fires `true` on `pointerenter` and `false` on `pointerleave`.
/// This is the web wiring behind `idea-ui`'s hover-driven `Tooltip`.
#[wasm_bindgen_test]
fn web_on_hover_fires_true_on_enter_false_on_leave() {
    use std::cell::RefCell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();

    let states: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let s = states.clone();
    backend.install_hover_handler_impl(
        &el.clone().unchecked_into(),
        Rc::new(move |entering: bool| s.borrow_mut().push(entering)),
    );

    dispatch_pointer(&el, "pointerenter");
    dispatch_pointer(&el, "pointerleave");

    assert_eq!(
        *states.borrow(),
        vec![true, false],
        "on_hover must fire true on pointerenter then false on pointerleave",
    );
}

/// REGRESSION: `on_hover` must IGNORE touch pointers. On a touch device the
/// browser fires `pointerenter` on touch-DOWN (the finger "enters" as it
/// lands), so firing the hover handler there would pop a hover tooltip the
/// instant the user presses — defeating the long-press affordance for
/// wrapping buttons. Hover is mouse/pen only; touch goes through `on_touch`
/// (the `long_press` path) instead.
#[wasm_bindgen_test]
fn web_on_hover_ignores_touch_pointers() {
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();

    let fired = Rc::new(Cell::new(0u32));
    let f = fired.clone();
    backend.install_hover_handler_impl(
        &el.clone().unchecked_into(),
        Rc::new(move |_entering: bool| f.set(f.get() + 1)),
    );

    // Touch enter/leave must NOT reach the hover handler.
    dispatch_pointer_typed(&el, "pointerenter", "touch");
    dispatch_pointer_typed(&el, "pointerleave", "touch");
    assert_eq!(fired.get(), 0, "touch pointers must not fire on_hover");

    // A mouse pointer still does.
    dispatch_pointer_typed(&el, "pointerenter", "mouse");
    assert_eq!(fired.get(), 1, "mouse pointers must still fire on_hover");
}

/// `on_load` fires when the `<img>` dispatches its `load` event, carrying
/// the (natural) dimensions read off the element. A synthetic `load`
/// event on a src-less `<img>` reports `0×0` (no real bitmap), which is
/// enough to prove the wiring fires and reads the element — the real
/// dimensions flow the same way once a bitmap decodes.
#[wasm_bindgen_test]
fn web_on_load_fires_on_img_load_event() {
    use runtime_shared::{ImageLoadEvent};
    use std::cell::RefCell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let img = doc.create_element("img").unwrap();
    doc.body().unwrap().append_child(&img).unwrap();

    let seen: Rc<RefCell<Vec<(f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
    let s = seen.clone();
    backend.install_image_load_handler_impl(
        &img.clone().unchecked_into(),
        Rc::new(move |ev: &ImageLoadEvent| s.borrow_mut().push((ev.width, ev.height))),
    );

    // No src → not `complete` with a bitmap, so nothing fires yet.
    assert!(seen.borrow().is_empty(), "on_load must not fire before load");

    let ev = web_sys::Event::new("load").unwrap();
    img.dispatch_event(&ev).unwrap();
    assert_eq!(seen.borrow().len(), 1, "on_load fires once on the load event");
}

/// `on_error` fires when the `<img>` dispatches its `error` event.
#[wasm_bindgen_test]
fn web_on_error_fires_on_img_error_event() {
    use std::cell::Cell;
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();
    let img = doc.create_element("img").unwrap();
    doc.body().unwrap().append_child(&img).unwrap();

    let fired = Rc::new(Cell::new(0u32));
    let f = fired.clone();
    backend.install_image_error_handler_impl(
        &img.clone().unchecked_into(),
        Rc::new(move || f.set(f.get() + 1)),
    );
    assert_eq!(fired.get(), 0, "on_error must not fire before error");

    let ev = web_sys::Event::new("error").unwrap();
    img.dispatch_event(&ev).unwrap();
    assert_eq!(fired.get(), 1, "on_error fires once on the error event");
}

// ---------------------------------------------------------------------------
// Breakpoint overlay survives class re-mint in cascade order
// ---------------------------------------------------------------------------

/// REGRESSION TEST.
///
/// A dynamic class's base rule and its `@media (min-width: …)` overlay
/// have EQUAL specificity (same class selector); the overlay only wins
/// because it sits later in the sheet (mobile-first source order). When
/// the last node using the class is unstyled (a `switch`-keyed subtree
/// rebuild on a `resource` refetch is the real-world trigger), the
/// class's slots are freed base-first into `free_rule_indices`; the
/// per-rule LIFO recycle in `insert_rule_raw` then handed the re-minted
/// base rule the overlay's (higher) slot and the overlay the base's
/// (lower) slot — physically inverting the pair. From then on the base
/// rule won at EVERY viewport width and the desktop layout silently
/// degraded to the mobile one until a full page reload.
///
/// The fix (`insert_rule_group`) draws the group's recycled slots up
/// front and assigns them in ascending order, so the base always lands
/// physically before its overlays. This test releases + re-mints the
/// same styled content and asserts the physical CSSOM order.
#[wasm_bindgen_test]
fn regression_breakpoint_overlay_survives_class_remint_in_cascade_order() {
    use runtime_shared::{Breakpoint, Length, StyleRules, Tokenized};
    use std::rc::Rc;

    install_mount();
    let mut backend = WebBackend::new("#app");
    let doc = web_sys::window().unwrap().document().unwrap();

    let make_base = || {
        Rc::new(StyleRules {
            flex_basis: Some(Tokenized::Literal(Length::Percent(100.0))),
            ..Default::default()
        })
    };
    let make_overlay = || {
        Rc::new(StyleRules {
            flex_basis: Some(Tokenized::Literal(Length::Px(360.0))),
            ..Default::default()
        })
    };

    // Locate the physical CSSOM positions of the class's base style
    // rule and its @media overlay rule.
    let find_positions = |backend: &mut WebBackend, class: &str| -> (u32, u32) {
        let sheet = backend.sheet();
        let rules = sheet.css_rules().expect("css_rules");
        let selector = format!(".{class}");
        let mut base_pos = None;
        let mut media_pos = None;
        for i in 0..rules.length() {
            let Some(r) = rules.get(i) else { continue };
            if let Some(style_rule) = r.dyn_ref::<web_sys::CssStyleRule>() {
                if style_rule.selector_text() == selector {
                    base_pos = Some(i);
                }
            } else if r.dyn_ref::<web_sys::CssMediaRule>().is_some()
                && r.css_text().contains(class)
            {
                media_pos = Some(i);
            }
        }
        (
            base_pos.expect("base class rule must be in the sheet"),
            media_pos.expect("@media overlay rule must be in the sheet"),
        )
    };


    // Initial mint on node 1: appended in source order, base < media.
    let element1 = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&element1).unwrap();
    let node1: web_sys::Node = element1.unchecked_into();
    backend.apply_styled_variants_impl(
        &node1,
        &make_base(),
        &[],
        &[(Breakpoint::Lg, make_overlay())],
        &[],
    );
    let id1 = backend.node_id(&node1);
    let class = backend
        .dynamic
        .get(&id1)
        .expect("node 1 must hold a dynamic slot")
        .shared
        .class_name
        .clone();
    let (base_before, media_before) = find_positions(&mut backend, &class);
    assert!(
        base_before < media_before,
        "sanity: initial mint must emit base ({base_before}) before @media ({media_before})",
    );

    // Release: last user gone → both slots freed (base-first). This is
    // what a switch-keyed subtree teardown does through
    // `on_node_unstyled`.
    backend.drop_dynamic_slot(id1);

    // Re-mint the SAME content on a fresh node — the resolution cache
    // hands the walker fresh Rcs after a rebuild, so use fresh Rcs
    // here too. Pre-fix, the LIFO recycle inverted the pair.
    let element2 = doc.create_element("div").unwrap();
    doc.body().unwrap().append_child(&element2).unwrap();
    let node2: web_sys::Node = element2.unchecked_into();
    backend.apply_styled_variants_impl(
        &node2,
        &make_base(),
        &[],
        &[(Breakpoint::Lg, make_overlay())],
        &[],
    );
    let id2 = backend.node_id(&node2);
    let reminted = backend
        .dynamic
        .get(&id2)
        .expect("node 2 must hold a dynamic slot")
        .shared
        .class_name
        .clone();
    assert_eq!(reminted, class, "same content must re-mint the same class");

    let (base_after, media_after) = find_positions(&mut backend, &class);
    assert!(
        base_after < media_after,
        "re-minted base rule (index {base_after}) must stay physically BEFORE its @media \
         overlay (index {media_after}); inverted order makes the base win at every viewport \
         width and the breakpoint layout silently stops applying",
    );

    // The re-mint must have recycled the freed slots, not appended —
    // otherwise churn-heavy subtrees grow the sheet without bound.
    assert_eq!(
        (base_after, media_after),
        (base_before, media_before),
        "re-mint must reuse the freed slots at their original indices",
    );
}

// ---------------------------------------------------------------------------
// Image `on_load` re-entrancy — cached-image load must fire on a microtask.
// ---------------------------------------------------------------------------

/// Await an `<img>`'s `load` event so `complete() && naturalWidth > 0`
/// holds deterministically before the test installs its handler — this
/// reproduces the "already cached / already decoded" state the bug needs.
async fn decoded_img() -> web_sys::HtmlImageElement {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;
    let doc = web_sys::window().unwrap().document().unwrap();
    let img: web_sys::HtmlImageElement =
        doc.create_element("img").unwrap().unchecked_into();
    // 1×1 transparent GIF — decodes to naturalWidth == 1.
    let src = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let cb = Closure::once_into_js(move || {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        });
        let _ = img.add_event_listener_with_callback("load", cb.unchecked_ref());
    });
    img.set_src(src);
    wasm_bindgen_futures::JsFuture::from(promise).await.unwrap();
    assert!(
        img.complete() && img.natural_width() > 0,
        "test image must be fully decoded before install",
    );
    img
}

/// REGRESSION — an `on_load` handler that re-enters the backend
/// `RefCell` must not panic when the image is already decoded.
///
/// `walker::image::build` installs the load handler while holding
/// `backend.borrow_mut()`. Before the fix, `install_load` fired the
/// already-decoded notification *inline*, so a handler that writes a
/// signal a reactive style depends on re-entered that same borrow via
/// the style effect and aborted with "RefCell already borrowed"
/// (`walker/style.rs`). The already-decoded fire is now deferred to a
/// microtask, out of the borrow. This test reproduces the exact layer:
/// install under a live `borrow_mut`, with a handler that itself takes
/// `borrow_mut`. Inline firing panics; deferred firing does not.
#[wasm_bindgen_test]
async fn regression_image_on_load_cached_does_not_reenter_borrow() {
    use std::cell::Cell;
    use std::rc::Rc;
    install_mount();
    // `schedule_microtask` needs an installed scheduler; idempotent.
    crate::install_scheduler();
    let backend = Rc::new(std::cell::RefCell::new(WebBackend::new("#app")));

    let img = decoded_img().await;
    let node: web_sys::Node = wasm_bindgen::JsCast::unchecked_into(img);

    // A handler that re-enters the backend borrow — the shape a
    // signal-writing `on_load` produces once its style effect runs.
    let fired = Rc::new(Cell::new(false));
    let handler: runtime_shared::ImageLoadHandler = {
        let backend = backend.clone();
        let fired = fired.clone();
        Rc::new(move |_ev| {
            // Re-entering `borrow_mut` here is exactly what the style
            // effect did in the field crash. Must not be live during
            // this call.
            let _b = backend.borrow_mut();
            fired.set(true);
        })
    };

    // Install exactly as the walker does: under a live `borrow_mut`.
    {
        let mut b = backend.borrow_mut();
        b.install_image_load_handler_impl(&node, handler);
    }

    // Inline firing would already have panicked above. It must not have
    // run yet — it's deferred to a microtask.
    assert!(
        !fired.get(),
        "cached-image on_load must be deferred, not fired inline inside install",
    );

    // Yield one microtask turn; now the deferred notification runs, and
    // the re-entrant borrow is safe because `install` has returned.
    let yield_promise = js_sys::Promise::resolve(&wasm_bindgen::JsValue::NULL);
    wasm_bindgen_futures::JsFuture::from(yield_promise).await.unwrap();
    assert!(
        fired.get(),
        "deferred on_load must fire on the next microtask",
    );
}
