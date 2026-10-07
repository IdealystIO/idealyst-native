//! Soft keyboard on mobile browsers — the web half of `keyboard_avoiding_view`
//! (see `docs/keyboard.md`).
//!
//! The app viewport never changes for the keyboard. Each
//! `keyboard_avoiding_view` element ([`register`]) measures how much of ITS
//! box the keyboard covers and keeps its content clear: `Padding` adds the
//! overlap to its bottom padding (`box-sizing: border-box`, so the content
//! box ends at the keyboard), `Translate` lifts it with the individual
//! `translate` property (compositor-only). Author code sees the keyboard
//! through `keyboard_inset()`.
//!
//! ## Where the size comes from
//!
//! - **VirtualKeyboard API** (Chromium on Android): the page opts into
//!   `navigator.virtualKeyboard.overlaysContent = true` — the browser then
//!   stops resizing/panning for the keyboard and leaves avoidance to the
//!   page — and `geometrychange` reports the keyboard's exact rect when it
//!   starts to move.
//! - **`visualViewport`** (Safari on iOS / iPadOS, older Chromium): the
//!   keyboard shrinks the visual viewport but not the layout viewport, so
//!   the overlap is `documentElement.clientHeight - visualViewport.height ×
//!   scale` (the scale factor keeps a pinch-zoomed page from reading as a
//!   keyboard). It only counts while a text-entry element is focused, so
//!   browser chrome resizing never reads as a keyboard.
//!
//! ## Animation: a best estimate
//!
//! Browsers expose the keyboard's geometry, not its animation, so the
//! avoiders ride a CSS transition with the platform keyboard's typical
//! timing (`runtime_shared::keyboard::WEB_KEYBOARD_ESTIMATE*`) — the one
//! place the motion is not the platform's own. `keyboard_inset()` reports
//! the same timing.

use std::cell::{Cell, RefCell};

use runtime_shared::keyboard::{WEB_KEYBOARD_ESTIMATE, WEB_KEYBOARD_ESTIMATE_CHROMIUM};
use runtime_shared::{KeyboardAvoid, KeyboardAvoidBehavior, KeyboardInset, Transition};

web_glue::import! {
    // The object that reports keyboard geometry: `navigator.virtualKeyboard`
    // (opted into overlaysContent first), else `window.visualViewport`,
    // else null (no browser context / no API).
    fn js_kb_source() -> u32 =
        "() => { if (typeof window === 'undefined') return G.add(null); \
           const vk = navigator.virtualKeyboard; \
           if (vk) { vk.overlaysContent = true; return G.add(vk); } \
           return G.add(window.visualViewport || null); }";
    fn js_kb_uses_virtual_keyboard_api() -> u32 =
        "() => (typeof navigator !== 'undefined' && navigator.virtualKeyboard) ? 1 : 0";
    // The keyboard's overlap of the layout viewport, CSS px.
    fn js_kb_height() -> f64 =
        "() => { if (typeof window === 'undefined') return 0; \
           const vk = navigator.virtualKeyboard; \
           if (vk) return vk.boundingRect.height; \
           const vv = window.visualViewport; if (!vv) return 0; \
           const a = document.activeElement; \
           const editing = !!a && (a.tagName === 'INPUT' || a.tagName === 'TEXTAREA' || a.isContentEditable); \
           if (!editing) return 0; \
           return Math.max(0, document.documentElement.clientHeight - vv.height * vv.scale); }";
    // Move one avoider for a keyboard of `kb` px. Measures the element's
    // UNTRANSLATED bottom (its own lift added back, so a Translate lift never
    // feeds back into its measurement) against the keyboard's top, then
    // applies padding-bottom (author padding read back as the base) or a
    // `translate` lift, with the transition — or none when not animated.
    // Built JS-side so no float formatting links into the wasm. Returns 0
    // when the element has left the document (the caller drops it).
    fn js_kb_avoid(el: u32, kb: f64, translate: u32, animated: u32, ms: u32, x1: f64, y1: f64, x2: f64, y2: f64) -> u32 =
        "(e, kb, tr, an, ms, x1, y1, x2, y2) => { const el = G.get(e); \
           if (!el.isConnected) return 0; const s = el.style; \
           const lift = el.__iyKbLift || 0; \
           const bottom = el.getBoundingClientRect().bottom + lift; \
           const top = document.documentElement.clientHeight - kb; \
           const ov = kb > 0 ? Math.max(0, bottom - top) : 0; \
           const prop = tr ? 'translate' : 'padding-bottom'; \
           s.transition = an ? `${prop} ${ms}ms cubic-bezier(${x1},${y1},${x2},${y2})` : 'none'; \
           if (tr) { el.__iyKbLift = ov; s.translate = ov > 0 ? `0 ${-ov}px` : ''; return 1; } \
           if (!el.__iyKbPad) { s.paddingBottom = ''; \
             el.__iyKbBase = parseFloat(getComputedStyle(el).paddingBottom) || 0; } \
           el.__iyKbPad = ov > 0; s.boxSizing = 'border-box'; \
           s.paddingBottom = ov > 0 ? `${el.__iyKbBase + ov}px` : ''; return 1; }";
}

thread_local! {
    /// The installed geometry listener (detaches on drop).
    static SOURCE: RefCell<Option<web_glue::dom::Listener>> = const { RefCell::new(None) };
    /// Last reported overlap (px), so repeated events with no change do nothing.
    static LAST_HEIGHT: Cell<f32> = const { Cell::new(0.0) };
    /// The timing estimate for this browser (set at install).
    static TRANSITION: Cell<Transition> = const { Cell::new(WEB_KEYBOARD_ESTIMATE) };
    /// Live `keyboard_avoiding_view` elements. Entries whose element left the
    /// document are dropped on the next keyboard move.
    static AVOIDERS: RefCell<Vec<(web_glue::JsValue, KeyboardAvoid)>> = const { RefCell::new(Vec::new()) };
}

/// Start observing the soft keyboard. Called at the end of every new-core
/// boot path, next to the viewport source; replaces any previous source.
pub(crate) fn install_keyboard_source() {
    remove_keyboard_source();
    let source = unsafe { web_glue::JsValue::from_raw(js_kb_source()) };
    if source.is_null() {
        return;
    }
    let vk = unsafe { js_kb_uses_virtual_keyboard_api() } != 0;
    TRANSITION.with(|t| t.set(if vk { WEB_KEYBOARD_ESTIMATE_CHROMIUM } else { WEB_KEYBOARD_ESTIMATE }));
    let ty = if vk { "geometrychange" } else { "resize" };
    let listener = crate::glue_dom::listen(
        &source,
        ty,
        web_glue::dom::ListenerOptions::default(),
        move |_| keyboard_height_changed(unsafe { js_kb_height() } as f32),
    );
    SOURCE.with(|s| *s.borrow_mut() = Some(listener));
}

/// Public seam for boot paths that build their own [`crate::WebBackend`]
/// (the runtime-server dev client, which replays wire commands into a
/// local backend, including `MarkKeyboardAvoiding`). `connect_web`'s frame
/// pump relays the reported inset to the sidecar.
pub fn install_keyboard_avoidance(_backend: &crate::WebBackend) {
    install_keyboard_source();
}

/// Stop listening (boot/stop cycles must not accumulate listeners).
pub(crate) fn remove_keyboard_source() {
    let old = SOURCE.with(|s| s.borrow_mut().take());
    drop(old);
    LAST_HEIGHT.with(|h| h.set(0.0));
    TRANSITION.with(|t| t.set(WEB_KEYBOARD_ESTIMATE));
    AVOIDERS.with(|a| a.borrow_mut().clear());
}

/// Make `el` a `keyboard_avoiding_view`. A keyboard already up is applied
/// on the next frame, unanimated (the element is laid out by then).
pub(crate) fn register(el: &web_glue::JsValue, avoid: KeyboardAvoid) {
    AVOIDERS.with(|a| a.borrow_mut().push((el.clone(), avoid)));
    let el = el.clone();
    crate::glue_dom::next_frame(move || {
        let h = LAST_HEIGHT.with(Cell::get);
        if h > 0.0 {
            avoid_one(&el, avoid.behavior, false, h, TRANSITION.with(Cell::get));
        }
    });
}

/// The keyboard's overlap changed: report it to author code and move every
/// avoider. No-op when the overlap did not change.
pub(crate) fn keyboard_height_changed(height: f32) {
    let height = height.max(0.0);
    if LAST_HEIGHT.with(|h| h.replace(height)) == height {
        return;
    }
    let transition = TRANSITION.with(Cell::get);
    AVOIDERS.with(|a| {
        a.borrow_mut()
            .retain(|(el, avoid)| avoid_one(el, avoid.behavior, avoid.animated, height, transition))
    });
    runtime_vocabulary::keyboard::push(KeyboardInset::new(height, transition));
    crate::newcore::schedule_flush();
}

/// Returns whether `el` is still in the document.
fn avoid_one(
    el: &web_glue::JsValue,
    behavior: KeyboardAvoidBehavior,
    animated: bool,
    keyboard: f32,
    transition: Transition,
) -> bool {
    let [x1, y1, x2, y2] = transition.easing.control_points();
    let translate = matches!(behavior, KeyboardAvoidBehavior::Translate) as u32;
    let in_document = unsafe {
        js_kb_avoid(
            el.raw(),
            keyboard as f64,
            translate,
            animated as u32,
            transition.duration_ms,
            x1 as f64,
            y1 as f64,
            x2 as f64,
            y2 as f64,
        )
    };
    in_document != 0
}

// Browser-side tests (backend-web compiles for wasm32 only). Run per
// `reference_backend_web_browser_tests`:
//   cd crates/backend/web && CHROMEDRIVER=… WASM_BINDGEN_TEST_ONLY_WEB=1 \
//   cargo test --target wasm32-unknown-unknown --lib keyboard_source
#[cfg(test)]
mod tests {
    use super::*;
    use web_glue::JsCast;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// A full-viewport element (bottom = layout viewport bottom), with
    /// author padding-bottom 10px.
    fn full_height_el() -> web_glue::dom::HtmlElement {
        let document = web_glue::dom::window().unwrap().document().unwrap();
        let el = document.create_element("div").unwrap();
        let el = el.dyn_into::<web_glue::dom::HtmlElement>().unwrap();
        let s = el.style();
        s.set_property("position", "fixed").unwrap();
        s.set_property("left", "0").unwrap();
        s.set_property("right", "0").unwrap();
        s.set_property("top", "0").unwrap();
        s.set_property("bottom", "0").unwrap();
        document.body().unwrap().append_child(&el).unwrap();
        el
    }

    /// Padding: the avoider's bottom padding grows by exactly the part of
    /// it the keyboard covers, on top of its own padding, with the
    /// transition; the inset reaches `keyboard_inset()`; closing restores.
    #[wasm_bindgen_test]
    fn padding_avoider_pads_by_its_overlap_and_restores() {
        remove_keyboard_source();
        let el = full_height_el();
        el.style().set_property("padding-bottom", "10px").unwrap();
        // The author padding comes from a class in real apps; here an inline
        // value stands in, so clear it the way a stylesheet would hold it.
        let sheet = web_glue::dom::window().unwrap().document().unwrap().create_element("style").unwrap();
        sheet.set_text_content(Some(".kb-test{padding-bottom:10px}"));
        web_glue::dom::window().unwrap().document().unwrap().head().unwrap().append_child(&sheet).unwrap();
        el.style().remove_property("padding-bottom").unwrap();
        el.set_class_name("kb-test");

        let world = runtime_world::World::new();
        let kb = world.enter(runtime_vocabulary::keyboard::keyboard_inset);
        let el_js: &web_glue::JsValue = el.as_ref();
        AVOIDERS.with(|a| a.borrow_mut().push((el_js.clone(), KeyboardAvoid::default())));

        keyboard_height_changed(300.0);
        let s = el.style();
        assert_eq!(s.get_property_value("padding-bottom").unwrap(), "310px");
        assert_eq!(s.get_property_value("box-sizing").unwrap(), "border-box");
        let t = s.get_property_value("transition").unwrap();
        assert!(t.contains("padding-bottom") && t.contains("cubic-bezier(0.38, 0.7, 0.125, 1)"), "{t}");
        world.flush();
        assert_eq!(world.enter(|| kb.get().height), 300.0);

        keyboard_height_changed(0.0);
        assert_eq!(el.style().get_property_value("padding-bottom").unwrap(), "");
        world.flush();
        assert!(!world.enter(|| kb.get().is_visible()));
        el.remove();
        sheet.remove();
        remove_keyboard_source();
    }

    /// Translate lifts by the overlap, measured against the UNTRANSLATED
    /// box: a second report of a different height doesn't compound the
    /// first lift.
    #[wasm_bindgen_test]
    fn translate_avoider_lifts_without_feedback() {
        remove_keyboard_source();
        let el = full_height_el();
        let el_js: &web_glue::JsValue = el.as_ref();
        AVOIDERS.with(|a| {
            a.borrow_mut().push((
                el_js.clone(),
                KeyboardAvoid { behavior: KeyboardAvoidBehavior::Translate, animated: false },
            ))
        });
        keyboard_height_changed(300.0);
        assert_eq!(el.style().get_property_value("translate").unwrap(), "0px -300px");
        assert_eq!(el.style().get_property_value("transition").unwrap(), "none");
        keyboard_height_changed(250.0);
        assert_eq!(el.style().get_property_value("translate").unwrap(), "0px -250px");
        keyboard_height_changed(0.0);
        assert_eq!(el.style().get_property_value("translate").unwrap(), "");
        el.remove();
        remove_keyboard_source();
    }

    /// Elements that left the document are dropped instead of leaking.
    #[wasm_bindgen_test]
    fn detached_avoiders_are_dropped() {
        remove_keyboard_source();
        let el = full_height_el();
        let el_js: &web_glue::JsValue = el.as_ref();
        AVOIDERS.with(|a| a.borrow_mut().push((el_js.clone(), KeyboardAvoid::default())));
        el.remove();
        keyboard_height_changed(120.0);
        assert!(AVOIDERS.with(|a| a.borrow().is_empty()));
        remove_keyboard_source();
    }
}
