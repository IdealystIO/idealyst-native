//! Reproduction: a view with an animated (`animated!().bind(ref, AnimProp)`)
//! binding must hydrate cleanly — adopt the SSR node, not diverge/remount and
//! drop or duplicate the subtree.
//!
//! On the portfolio site the animated hero wrapper (opacity/translateY) and the
//! content panel (translateY slide) diverge under hydration, which misaligns
//! the cursor and drops the reactive text that follows. This isolates whether
//! an animated binding alone triggers the divergence.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use backend_web::WebBackend;
use runtime_core::animation::{AnimProp, TweenTo};
use runtime_core::{
    animated, effect, node_ref, rx, scroll_view, signal, text, view, Element, ViewHandle,
};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const MARKER: &str = "ANIM-MARKER";

fn hydrate_tree(tree_fn: impl Fn() -> Element + Copy + 'static, marker: &str) -> usize {
    let ssr_html = backend_ssr::render_path("/", tree_fn).html;
    assert!(ssr_html.contains(marker), "SSR should render marker `{marker}`: {ssr_html}");
    let doc = web_sys::window().unwrap().document().unwrap();
    if let Some(existing) = doc.get_element_by_id("app") {
        existing.remove();
    }
    let app = doc.create_element("div").unwrap();
    app.set_id("app");
    app.set_inner_html(&ssr_html);
    doc.body().unwrap().append_child(&app).unwrap();
    backend_web::install_scheduler();
    let backend = WebBackend::hydrate("#app");
    let rc = Rc::new(RefCell::new(backend));
    backend_web::install_global_self(&rc);
    let _owner = runtime_core::mount(rc, tree_fn);
    app.inner_html().matches(marker).count()
}

// Reactive (i18n-shaped) text: `text(Reactive<String>)` → `TextSource::Bound`.
const RTEXT: &str = "REACTIVE-MARKER";
fn reactive_text_tree() -> Element {
    let sig = signal(RTEXT.to_string());
    view(vec![text(rx!(sig.get())).into()]).into()
}

// Reactive text INSIDE an animated wrapper — the exact hero-name shape.
const ANIM_RTEXT: &str = "ANIM-REACTIVE-MARKER";
fn animated_reactive_tree() -> Element {
    let r = node_ref!(ViewHandle);
    let opacity = animated!(0.0_f32);
    opacity.bind(r, AnimProp::Opacity);
    effect!({
        opacity.animate(TweenTo::new(1.0_f32, Duration::from_millis(50)).ease_out());
    });
    let sig = signal(ANIM_RTEXT.to_string());
    view(vec![text(rx!(sig.get())).into()]).bind(r).into()
}

#[wasm_bindgen_test]
fn reactive_text_hydrates_without_dropping() {
    assert_eq!(hydrate_tree(reactive_text_tree, RTEXT), 1, "reactive text dropped/dup");
}

#[wasm_bindgen_test]
fn reactive_text_in_animated_wrapper_hydrates() {
    assert_eq!(hydrate_tree(animated_reactive_tree, ANIM_RTEXT), 1, "hero-shape reactive text dropped/dup");
}

// Reactive text inside a `scroll_view` — the portfolio's `content_screen`
// shape (`scroll_view > animated panel > reactive text`). `scroll_view::create`
// previously never adopted its SSR node (unconditional `create_element`), so it
// left the cursor parked on the SSR scroll div; the content panel then
// mis-adopted it and the whole scrolled subtree diverged. This guards the
// primitive's hydration adoption.
const SCROLL_RTEXT: &str = "SCROLL-REACTIVE-MARKER";
fn scroll_reactive_tree() -> Element {
    let r = node_ref!(ViewHandle);
    let slide = animated!(16.0_f32);
    slide.bind(r, AnimProp::TranslateY);
    effect!({
        slide.animate(TweenTo::new(0.0_f32, Duration::from_millis(50)).ease_out());
    });
    let sig = signal(SCROLL_RTEXT.to_string());
    let panel = view(vec![text(rx!(sig.get())).into()]).bind(r).into();
    scroll_view(vec![panel]).into()
}

#[wasm_bindgen_test]
fn reactive_text_in_scroll_view_hydrates() {
    assert_eq!(
        hydrate_tree(scroll_reactive_tree, SCROLL_RTEXT),
        1,
        "scroll_view must adopt its SSR node so the scrolled reactive text \
         adopts in place (not diverge → drop/duplicate)"
    );
}

fn animated_tree() -> Element {
    let r = node_ref!(ViewHandle);
    let opacity = animated!(0.0_f32);
    let slide = animated!(16.0_f32);
    opacity.bind(r, AnimProp::Opacity);
    slide.bind(r, AnimProp::TranslateY);
    effect!({
        opacity.animate(TweenTo::new(1.0_f32, Duration::from_millis(50)).ease_out());
        slide.animate(TweenTo::new(0.0_f32, Duration::from_millis(50)).ease_out());
    });
    view(vec![text(MARKER).into()]).bind(r).into()
}

#[wasm_bindgen_test]
fn animated_view_hydrates_without_dropping() {
    // 1. SSR the animated tree.
    let ssr_html = backend_ssr::render_path("/", animated_tree).html;
    assert!(ssr_html.contains(MARKER), "SSR should render the marker: {ssr_html}");

    // 2. Inject + hydrate.
    let doc = web_sys::window().unwrap().document().unwrap();
    if let Some(existing) = doc.get_element_by_id("app") {
        existing.remove();
    }
    let app = doc.create_element("div").unwrap();
    app.set_id("app");
    app.set_inner_html(&ssr_html);
    doc.body().unwrap().append_child(&app).unwrap();

    backend_web::install_scheduler();
    let backend = WebBackend::hydrate("#app");
    let rc = Rc::new(RefCell::new(backend));
    backend_web::install_global_self(&rc);
    let _owner = runtime_core::mount(rc, animated_tree);

    // The marker must survive hydration exactly once — not dropped (divergence
    // remount losing the node) and not duplicated.
    let count = app.inner_html().matches(MARKER).count();
    assert_eq!(
        count, 1,
        "animated view must adopt the SSR node once under hydration \
         (not diverge → drop/duplicate) — got {count}"
    );
}
