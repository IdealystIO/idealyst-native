//! Hydration (adopt-mode) boot for the new core — the SSR/SSG
//! counterpart of [`super::start`] / [`super::start_in`].
//!
//! [`hydrate`] / [`hydrate_in`] mount a `runtime_scene::Element` tree by
//! ADOPTING the server-rendered DOM under the mount instead of creating
//! fresh nodes: the backend is constructed via [`WebBackend::hydrate`],
//! which arms the OLD core's hydration machinery (the pre-order adoption
//! cursor, the mismatch → subtree-remount policy, the microtask
//! buffering window). Because every `runtime_vocabulary::caps` method on
//! [`WebBackend`] delegates UFCS-style to the existing `Backend` impl
//! (see the module docs in `newcore.rs`), the cursor-aware `create_*`
//! paths — and therefore the whole adoption/mismatch policy — are reused
//! verbatim; this module only sequences the boot around them.
//!
//! # Why adoption lines up with new-core realize order
//!
//! The cursor adopts in pre-order **creation** order, so adoption is
//! correct iff the new core calls `create_*` in the same order the old
//! walker did against the same author tree — which is exactly what the
//! scene-parity goldens pin (`crates/dev/scene-parity`): the structural
//! and full-op suites prove the op streams match byte-for-byte, and every
//! sanctioned divergence in that README is a *teardown-window* ordering
//! (release/cleanup interleaving, LIFO-vs-creation-order frees, the
//! skipped virgin-anchor `clear_children`) — none reorders `create_*` at
//! mount, so none can shift the cursor. Divergence #2 (the new drivers
//! skip the first-fire `clear_children` on a fresh anchor) is actively
//! REQUIRED here: under hydration the adopted anchor is NOT empty (it
//! holds the SSR branch), and clearing it would delete the very nodes the
//! branch build is about to adopt — the old walker special-cases this
//! with an explicit `if !hydrating` skip (`when_switch.rs`); the new
//! drivers never clear on first fire, so the same protection holds by
//! construction.
//!
//! # Anchored-mode gate
//!
//! SSR renders with `supports_child_splice() == false`, so the server DOM
//! nests every reactive region (`Dyn` hole, `Keyed` list) under a
//! `display:contents` anchor `<div>`. The old walker forces the anchored
//! path during hydration (`view.rs`: `!is_hydrating()` on the spliced
//! `When`/`Switch` arms) for the same reason; on the new core the
//! equivalent gate lives in `Host::supports_splice` (`newcore.rs`), which
//! reports `false` while `is_hydrating()`. Without it the spliced drivers
//! would adopt the SSR *anchor* as the region's first content node — off
//! by one tree level — and every following sibling would mismatch (the
//! `[hydrate] diverge` remount cascade). Regions first mounted AFTER
//! `finish` clears `is_hydrating` splice as usual; a region adopted
//! anchored keeps its anchor for its lifetime, same as the old core.
//!
//! # Boot sequence (mirrors the old `start_local` hydration path in
//! `tools/build/web` + `walker.rs::mount`)
//!
//! 1. `WebBackend::hydrate(selector)` — cursor at the SSR root, microtask
//!    buffering ON (deferred builds must not escape the adoption window).
//! 2. Seed the SSR-assumed viewport (`data-ssr-viewport`) so anything
//!    that reads `runtime_shared`'s viewport during the build sees the
//!    server's value, not the real one (first client render must equal
//!    the server render — the clean-adoption prerequisite).
//! 3. `world.enter`: build + `realize` (every `create_*` adopts), then
//!    `drain_buffered_microtasks()` — deferred builds run while the
//!    cursor is still live, matching `walker.rs`'s drain-before-`finish`.
//!    Skipping the drain would let deferred subtree builds fire after
//!    `finish` and build FRESH beside the already-adopted DOM
//!    (duplicated content — the bug the buffering exists to prevent).
//! 4. `Backend::finish(root)` — exits hydration mode, ends buffering, and
//!    (root already being the mount's child) leaves the server DOM in
//!    place: no clear, no flash.
//! 5. First `world.flush()` + flush-driver (dispatch hook) install —
//!    identical to [`super::start_in`] from here on.
//!
//! # Mismatch policy (ported, not reinvented)
//!
//! Inherited wholesale from the old machinery: a tag mismatch parks the
//! cursor, `hydrate_note_fresh` records the freshly-built node as a
//! subtree-local remount root (with a `[hydrate] SSR/client diverge`
//! `console.warn`), the remount subtree builds fresh, and the
//! `insert`/`insert_at`/`insert_many` resync swaps it in for the stale
//! SSR node in place — siblings keep adopting. `finish` detaches any
//! leftover stale node as a safety net.
//!
//! # Degraded modes
//!
//! - Mount has no server DOM (`page_is_prerendered` false): falls through
//!   to [`super::start_in`] — a plain fresh boot, the same policy as the
//!   old CLI wrapper's `start_local`.
//! - `hydrate` cargo feature off: `WebBackend::hydrate` stubs to
//!   `WebBackend::new`, so this boot degrades to clear-and-rebuild (same
//!   flicker as a fresh boot, nothing broken).
//!
//! # Known gaps (deliberate, documented)
//!
//! - **Navigator / portal hydration**: the old SDK navigator handlers'
//!   adopt helpers (`hydrate_adopt_container`, `hydrate_enter_region`,
//!   the `idealyst-nav-root` marker walk) are reachable through the
//!   delegated `create_navigator`, but the new-core vocabulary navigator
//!   handler defers screen/chrome builds through `runtime_world`
//!   microtasks that are NOT hydration-aware; navigator screens will take
//!   the mismatch-remount path rather than adopting. Same for portal
//!   content (never server-rendered in place). Covered when the
//!   vocabulary navigator handler grows adoption support.
//! - ~~Viewport reconciliation~~ CLOSED: the boot now creates the
//!   per-world viewport ctx (`runtime_vocabulary::viewport`) seeded from
//!   the SSR viewport, then — post-`finish` — installs the new-core
//!   resize source and pushes the REAL window size through it, so a
//!   client window that differs from the server's assumption reconciles
//!   reactively (mirroring the old boot's post-mount
//!   `install_viewport_observer` ordering). Old-core seeding (step 2)
//!   remains — it both protects old-core value reads during the build
//!   AND is what the world ctx seeds from.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{realize, Element, Registry};
use runtime_world::World;

use super::{schedule_flush, App, APP, FLUSH_WORLD};
use crate::WebBackend;

/// Boot the new core by adopting server-rendered DOM under `#app`.
/// Falls back to a fresh [`super::start`]-style mount when `#app` holds
/// no server DOM. See the module docs for the full sequence.
// `#[inline]` is load-bearing, not a perf hint: as a plain `pub` non-generic
// fn this is codegen'd into backend-web's rlib and survives to the final
// link (the pipeline passes `--keep-lld-exports`, and the wrapper builds
// with `lto = "off"`), which instantiates `register_builtins_with::<_,
// AllBuiltins>` and re-anchors the WHOLE builtin vocabulary even for an app
// that selected a smaller set through the `_with` entry. Measured: a
// `--primitives core` build stayed at 560,997 bytes instead of 357,581.
// `#[inline]` makes it available for cross-crate inlining without emitting a
// standalone symbol, so an unused default set is never instantiated.
#[inline]
pub fn hydrate(build: impl FnOnce() -> Element) {
    hydrate_in("#app", |_| {}, build)
}

/// [`hydrate`] with an explicit mount selector and the same registration
/// seam as [`super::start_in`]: `register` runs after
/// [`runtime_vocabulary::register_builtins`], before the tree realizes.
// `#[inline]` is load-bearing, not a perf hint: as a plain `pub` non-generic
// fn this is codegen'd into backend-web's rlib and survives to the final
// link (the pipeline passes `--keep-lld-exports`, and the wrapper builds
// with `lto = "off"`), which instantiates `register_builtins_with::<_,
// AllBuiltins>` and re-anchors the WHOLE builtin vocabulary even for an app
// that selected a smaller set through the `_with` entry. Measured: a
// `--primitives core` build stayed at 560,997 bytes instead of 357,581.
// `#[inline]` makes it available for cross-crate inlining without emitting a
// standalone symbol, so an unused default set is never instantiated.
#[inline]
pub fn hydrate_in(
    selector: &str,
    register: impl FnOnce(&mut Registry<WebBackend>),
    build: impl FnOnce() -> Element,
) {
    hydrate_in_with::<runtime_vocabulary::AllBuiltins>(selector, register, build)
}

/// [`hydrate_in`], booting only the builtin families `S` selects.
///
/// Must be given the SAME set as the app's [`super::start_in_with`] call.
/// A CLI-generated wrapper compiles in both boot paths and picks one at
/// runtime, so if either still names [`runtime_vocabulary::AllBuiltins`]
/// the full handler set stays reachable from the boot entry and *neither*
/// path shrinks — this was measured: converting `start_in` alone moved a
/// hello-world by 0.4%, because `hydrate_in` was re-anchoring everything.
pub fn hydrate_in_with<S: runtime_vocabulary::BuiltinSet>(
    selector: &str,
    register: impl FnOnce(&mut Registry<WebBackend>),
    build: impl FnOnce() -> Element,
) {
    // Idempotent installs, same as `start_in` — the flush driver and the
    // hydration buffering both ride the scheduler, so it must exist first.
    crate::install_scheduler();
    crate::install_time_source();
    // Route `runtime_shared::driver::spawn` futures through the hooked
    // executor so future polls fire the post-dispatch flush hook.
    #[cfg(feature = "async-driver")]
    crate::install_async_executor();

    // No server DOM → nothing to adopt; plain fresh boot. Same
    // prerendered/fresh dispatch the old CLI wrapper's `start_local` does.
    //
    // MUST forward `S`, not call the non-generic `start_in`. Correctness:
    // the fallback would otherwise register a DIFFERENT builtin set than
    // the path the caller selected. Size: `start_in` is
    // `start_in_with::<AllBuiltins>`, so naming it here instantiates the
    // full vocabulary and re-anchors every handler — a `--primitives core`
    // build measured 560,997 bytes instead of 357,581 because of this one
    // call.
    if !crate::page_is_prerendered(selector) {
        return super::start_in_with::<S>(selector, register, build);
    }

    // Adoption-mode backend: cursor on the SSR root, microtask buffering
    // armed (see module docs step 1).
    let backend = Rc::new(RefCell::new(WebBackend::hydrate(selector)));
    crate::install_global_self(&backend);

    // Seed the SSR-assumed viewport BEFORE the build so the first client
    // render matches the server render (module docs step 2). `None` when
    // the server didn't stamp `data-ssr-viewport` — then there's nothing
    // to diverge from.
    if let Some((w, h)) = crate::ssr_viewport(selector) {
        runtime_shared::set_viewport_size(runtime_shared::ViewportSize::new(w, h));
    }

    let mut registry: Registry<WebBackend> = Registry::new();
    runtime_vocabulary::register_builtins_with::<WebBackend, S>(&mut registry);
    register(&mut registry);
    let registry = Rc::new(registry);

    let world = World::new();
    let (vp_sig, realized) = world.enter(|| {
        let element = build();
        let realized = realize(&backend, &registry, element);
        // Drain deferred builds INSIDE the adoption window (module docs
        // step 3): buffered microtasks may build subtrees, so they run
        // under the world ambient, before `finish` retires the cursor.
        runtime_shared::scheduling::drain_buffered_microtasks();
        // Capture the per-world viewport ctx AFTER the build (same
        // rationale as `start_in`: the derived-bucket memo captures the
        // breakpoint table at creation, and apps install custom tables
        // inside their build). A ctx created mid-build seeded from the
        // SSR viewport written above, so the adopted first render
        // agreed with the server about the breakpoint; the REAL window
        // size is pushed post-finish (below), mirroring the old-core
        // "observer installed post-mount" reconcile ordering.
        let vp_sig = runtime_vocabulary::viewport::viewport_ctx().size_signal();
        (vp_sig, realized)
    });

    // Single-root contract, same as `start_in`.
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_web::newcore::hydrate: the app root must contribute exactly one \
             top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    // Exits hydration mode + ends microtask buffering. The root was
    // adopted (it IS the mount's first child), so `finish` returns
    // without clearing — the server's DOM stays live, no flash.
    WebBackend::finish_impl(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount — AFTER `finish`, so reactive
    // rebuilds triggered by staged writes run off the hydration path
    // (fresh creates), never against a retired cursor.
    world.flush();

    // Install the flush driver — same two reachability points as
    // `start_in`: the author-callback wrappers in the caps impls and the
    // scheduler/executor post-dispatch hook. Installed AFTER `finish`, so
    // nothing can flush inside the adoption window.
    crate::dispatch_hook::install_dispatch_hook(schedule_flush);
    FLUSH_WORLD.with(|w| *w.borrow_mut() = Some(world.clone()));
    APP.with(|slot| {
        *slot.borrow_mut() = Some(App {
            realized,
            _backend: backend,
            _registry: registry,
            world,
        })
    });

    // Live viewport source + one immediate push of the REAL window size:
    // the build ran at the SSR-assumed viewport; if the client window
    // differs, the staged write commits on the scheduled flush and
    // breakpoint-dependent reactivity reconciles (off the hydration
    // path — fresh creates, same posture as any post-boot resize).
    super::install_viewport_source(vp_sig);
    super::push_current_viewport_now(vp_sig);
}

// ===========================================================================
// Browser-side tests. backend-web compiles for wasm32 only (see
// tests.rs) — there is no native-Rust test path for this crate, so ALL
// coverage lives here. Run with:
//   cd crates/backend/web
//   wasm-pack test --headless --chrome -- --features new-core,hydrate
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_scene::Host;
    use runtime_vocabulary::builders::IntoSceneElement;
    use runtime_vocabulary::{button, text, view};
    use runtime_world::signal;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// Recreate a fresh `#app` mount seeded with `inner_html` — the
    /// hand-built stand-in for SSR output (server-shaped DOM: the same
    /// tags, in the same pre-order, that `backend_ssr` emits for the
    /// fixture tree — anchors included, since SSR renders anchored).
    fn setup_prerendered(inner_html: &str) -> web_sys::Element {
        let document = web_sys::window().unwrap().document().unwrap();
        if let Some(prior) = document.get_element_by_id("app") {
            prior.remove();
        }
        let el = document.create_element("div").unwrap();
        el.set_id("app");
        el.set_inner_html(inner_html);
        document.body().unwrap().append_child(&el).unwrap();
        el
    }

    /// Await one microtask checkpoint (lets the flush driver's queued
    /// `Promise.then` run).
    async fn microtask() {
        let promise = js_sys::Promise::resolve(&JsValue::UNDEFINED);
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    /// `el.children[i]` without the `HtmlCollection` web-sys feature
    /// (element-sibling walk; panics past the end — tests only).
    fn nth_child(el: &web_sys::Element, i: usize) -> web_sys::Element {
        let mut cur = el.first_element_child().expect("no element children");
        for _ in 0..i {
            cur = cur.next_element_sibling().expect("index past the end");
        }
        cur
    }

    /// SSR-shaped fixture for the shared test tree below: root view,
    /// static text, reactive text, button, a Dyn hole and a Keyed list —
    /// both under `display:contents` anchors, because SSR renders with
    /// `supports_child_splice() == false`.
    const FIXTURE: &str = "<div>\
        <span>static</span>\
        <span>n=0</span>\
        <button>inc</button>\
        <div style=\"display:contents\"><span>off</span></div>\
        <div style=\"display:contents\"><span>row1</span><span>row2</span></div>\
        </div>";

    /// The matching author tree. Returns the count signal so tests can
    /// drive post-hydration updates.
    fn fixture_tree() -> (runtime_world::Signal<i32>, Element) {
        let count = signal(0i32);
        let on = signal(false);
        let rows = signal(vec![1u32, 2]);
        let tree = view()
            .child(text().content("static"))
            .child(text().content(move || format!("n={}", count.get())))
            .child(
                button()
                    .label("inc")
                    .on_press(move || count.update(|n| n + 1)),
            )
            // Dyn hole (closure child) — anchored during hydration.
            .child(move || {
                if on.get() {
                    text().content("on").into_scene_element()
                } else {
                    text().content("off").into_scene_element()
                }
            })
            // Keyed list — anchored during hydration.
            .child(runtime_scene::keyed(
                move || rows.get(),
                |n| *n,
                |n| text().content(format!("row{n}")).build(),
            ))
            .build();
        (count, tree)
    }

    /// The anchored-mode gate: while the backend is hydrating,
    /// `Host::supports_splice` must report `false` (SSR DOM nests
    /// reactive regions under anchors; splicing would adopt the anchor
    /// as content — off by one level). After `finish`, splice again.
    #[cfg(feature = "hydrate")]
    #[wasm_bindgen_test]
    fn supports_splice_gated_off_while_hydrating() {
        let mount = setup_prerendered("<div></div>");
        let mut b = WebBackend::hydrate("#app");
        assert!(
            !Host::supports_splice(&b),
            "hydrating backend must force the anchored region path"
        );
        let root = mount
            .first_element_child()
            .unwrap()
            .unchecked_into::<web_sys::Node>();
        WebBackend::finish_impl(&mut b, root);
        assert!(
            Host::supports_splice(&b),
            "post-finish regions splice as usual"
        );
    }

    /// Adoption: hydrating against server-shaped DOM reuses the server's
    /// nodes — no duplicates, identical node identity for the root, the
    /// button, the anchors and their contents.
    #[cfg(feature = "hydrate")]
    #[wasm_bindgen_test]
    fn regression_hydrate_adopts_server_dom_without_duplicates() {
        let mount = setup_prerendered(FIXTURE);
        let ssr_root = mount.first_element_child().unwrap();
        let ssr_button = nth_child(&ssr_root, 2);
        let ssr_dyn_anchor = nth_child(&ssr_root, 3);
        let ssr_dyn_content = ssr_dyn_anchor.first_element_child().unwrap();
        let ssr_keyed_anchor = nth_child(&ssr_root, 4);

        hydrate(|| fixture_tree().1);

        // No duplicate mount root, no duplicate children anywhere.
        assert_eq!(mount.child_element_count(), 1, "one root, not a rebuild");
        let live_root = mount.first_element_child().unwrap();
        assert!(
            live_root.is_same_node(Some(ssr_root.as_ref())),
            "root ADOPTED (same node), not recreated"
        );
        assert_eq!(live_root.child_element_count(), 5, "5 children as served");
        assert!(
            nth_child(&live_root, 2).is_same_node(Some(ssr_button.as_ref())),
            "button adopted"
        );
        let live_dyn_anchor = nth_child(&live_root, 3);
        assert!(
            live_dyn_anchor.is_same_node(Some(ssr_dyn_anchor.as_ref())),
            "Dyn anchor adopted (anchored-mode gate held)"
        );
        assert_eq!(
            live_dyn_anchor.child_element_count(),
            1,
            "Dyn branch adopted in place, not rebuilt beside the SSR one"
        );
        assert!(
            live_dyn_anchor
                .first_element_child()
                .unwrap()
                .is_same_node(Some(ssr_dyn_content.as_ref())),
            "Dyn branch content adopted"
        );
        let live_keyed_anchor = nth_child(&live_root, 4);
        assert!(
            live_keyed_anchor.is_same_node(Some(ssr_keyed_anchor.as_ref())),
            "Keyed anchor adopted"
        );
        assert_eq!(live_keyed_anchor.child_element_count(), 2, "2 rows, no dupes");

        super::super::stop();
    }

    /// Interactivity on ADOPTED nodes: the click closure was wired onto
    /// the server's `<button>` during adoption; clicking it stages a
    /// write that the flush driver commits — and the update lands in the
    /// ADOPTED reactive `<span>`, proving the batched-text binding
    /// targeted the server node.
    #[cfg(feature = "hydrate")]
    #[wasm_bindgen_test]
    async fn regression_hydrated_button_click_updates_adopted_dom() {
        let mount = setup_prerendered(FIXTURE);
        let ssr_count_span = nth_child(&mount.first_element_child().unwrap(), 1);

        hydrate(|| fixture_tree().1);

        let button: web_sys::HtmlElement =
            nth_child(&mount.first_element_child().unwrap(), 2).unchecked_into();
        button.click();
        // The adopted button's closure is the caps-impl WRAPPED author
        // callback (`flushing0`): author fn, then `schedule_flush` → one
        // deduped microtask → flush. Two checkpoints to let it land.
        microtask().await;
        microtask().await;

        let live_span = nth_child(&mount.first_element_child().unwrap(), 1);
        assert!(
            live_span.is_same_node(Some(ssr_count_span.as_ref())),
            "reactive span is still the adopted server node"
        );
        assert_eq!(
            live_span.text_content().unwrap(),
            "n=1",
            "staged write committed into the adopted node"
        );

        super::super::stop();
    }

    /// Mismatch policy (ported from the old core, not reinvented): a
    /// diverging server node triggers a subtree-local remount — the stale
    /// SSR node is swapped out for the fresh build IN PLACE, and the
    /// siblings AFTER the divergence still adopt.
    #[cfg(feature = "hydrate")]
    #[wasm_bindgen_test]
    fn regression_hydrate_mismatch_remounts_subtree_siblings_still_adopt() {
        // Server "rendered" a <p> where the client renders a text <span>.
        let bad = FIXTURE.replace("<span>static</span>", "<p>static</p>");
        let mount = setup_prerendered(&bad);
        let ssr_root = mount.first_element_child().unwrap();
        let ssr_button = nth_child(&ssr_root, 2);

        hydrate(|| fixture_tree().1);

        let live_root = mount.first_element_child().unwrap();
        assert!(
            live_root.is_same_node(Some(ssr_root.as_ref())),
            "root still adopted"
        );
        assert_eq!(live_root.child_element_count(), 5, "stale <p> replaced, not doubled");
        let first = nth_child(&live_root, 0);
        assert_eq!(
            first.tag_name().to_lowercase(),
            "span",
            "diverged child remounted fresh as the client's <span>"
        );
        assert!(
            nth_child(&live_root, 2).is_same_node(Some(ssr_button.as_ref())),
            "siblings after the divergence still adopt (cursor resynced)"
        );

        super::super::stop();
    }

    /// Empty mount → `hydrate` falls through to the fresh `start_in`
    /// boot. Works with or without the `hydrate` cargo feature.
    #[wasm_bindgen_test]
    async fn hydrate_on_empty_mount_falls_back_to_fresh_boot() {
        let mount = setup_prerendered("");
        hydrate(|| fixture_tree().1);
        assert_eq!(mount.child_element_count(), 1, "fresh boot mounted a root");
        // Static text is written synchronously at create; REACTIVE text
        // rides the batched-text registry, whose first write flushes in a
        // microtask — await it before asserting the dynamic content.
        assert!(mount.text_content().unwrap().contains("static"));
        microtask().await;
        microtask().await;
        assert!(mount.text_content().unwrap().contains("n=0"));
        super::super::stop();
    }
}
