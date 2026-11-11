//! The `pressable` primitive — a `<div>` with a click handler attached.
//!
//! Unlike the `button` primitive, this is a bare container: no
//! `<button>` element, no UA chrome (no outset border, no system
//! font, no implicit `type=submit`). Everything visual comes from
//! the attached stylesheet + children. The framework's state-bit
//! machinery (`state hovered`, `state pressed`, `state focused`)
//! works through CSS pseudo-classes, which apply to any element
//! including `<div>`.
//!
//! We DO add `role="button"` and `tabindex="0"` so the element is
//! a keyboard-reachable button for assistive tech.
//!
//! Two activations share everything but the pointer path: release
//! (`create`, fires on `click`) and press-down (`create_on_down`, fires at
//! the primary `pointerdown`; see [`wire_press_down`]).

use crate::WebBackend;
use runtime_shared::{PressActivation, PressableHandle, PressableOps, ViewportRect};
use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;
use web_glue::JsCast;
use web_glue::dom::Node;

/// A pressable that fires on click (pointer release, keyboard, AT).
pub(crate) fn create(b: &mut WebBackend, on_click: Rc<dyn Fn()>) -> Node {
    create_with(b, on_click, PressActivation::Release)
}

/// A pressable that fires as the primary pointer goes down
/// (`PressableOps::create_pressable_on_down`). See [`wire_press_down`].
pub(crate) fn create_on_down(b: &mut WebBackend, on_press: Rc<dyn Fn()>) -> Node {
    create_with(b, on_press, PressActivation::Down)
}

fn create_with(b: &mut WebBackend, on_click: Rc<dyn Fn()>, activation: PressActivation) -> Node {
    // HYDRATION: adopt the SSR `<div role=button>` (role/tabindex already
    // set by the SSR `create_pressable`); just wire the handlers below.
    // Its children are adopted separately via cursor descent.
    let adopted = b.hydrate_next("div");
    let el: web_glue::dom::HtmlElement = match adopted {
        Some(el) => el.unchecked_into(),
        None => {
            let el = b
                .doc
                .create_element("div")
                .expect("create pressable")
                .unchecked_into::<web_glue::dom::HtmlElement>();
            // Accessibility: announce as a button + make it Tab-focusable.
            // No inline `cursor` — that's now an author/component-driven
            // style property (`StyleRules::cursor`), so a bare pressable
            // takes the platform default and an author's `cursor` is never
            // shadowed by an un-overridable inline rule.
            let _ = el.set_attribute("role", "button");
            let _ = el.set_attribute("tabindex", "0");
            // A flex column, as on native (`css::VIEW_FLEX_RESET`).
            let _ = el.set_attribute(css::VIEW_MARKER_ATTR, "");
            el
        }
    };
    // Register as a subtree-local remount root if fresh-after-mismatch
    // (no-op when adopted).
    {
        let node: Node = el.clone().unchecked_into();
        b.hydrate_note_fresh(&node);
    }

    match activation {
        PressActivation::Release => {
            let on_click_for_mouse = on_click.clone();
            crate::glue_dom::set_press_onclick(&el, move || (on_click_for_mouse)());
        }
        PressActivation::Down => wire_press_down(&el, on_click.clone()),
    }

    // Keyboard activation. Enter and Space both trigger the press
    // when the element has focus — matching what a real `<button>`
    // does so users on assistive tech / keyboard nav get the same
    // affordance.
    //
    // The listener is bubble-phase, so a keydown originating on a focused
    // DESCENDANT (e.g. a `text_input` inside a Modal, whose card layer is a
    // no-op pressable) bubbles up here too. We must only activate when the
    // pressable ITSELF is the key target — otherwise `prevent_default()` on
    // Space/Enter would cancel the descendant input's character insertion /
    // submit. See `regression_web_pressable_ignores_descendant_key`.
    let on_click_for_key = on_click.clone();
    let el_for_key: Node = el.clone().unchecked_into();
    let el_for_key: web_glue::dom::EventTarget = el_for_key.unchecked_into();
    // Element-lifetime: the element owns the listener, and its closure is
    // released when JS collects the element.
    crate::glue_dom::listen_for_element_lifetime(&el, "keydown", Default::default(), move |ev| {
        let ev: web_glue::dom::KeyboardEvent = web_glue::JsCast::unchecked_into(ev);
        let is_self = ev
            .target()
            .map(|t| t.as_js().strict_eq(el_for_key.as_js()))
            .unwrap_or(false);
        if !is_self {
            return;
        }
        let k = ev.key();
        if k == "Enter" || k == " " {
            ev.prevent_default();
            (on_click_for_key)();
        }
    });

    // Consume the press from ancestor `on_touch` recognizers so a pressable
    // inside a clickable row / tappable card doesn't ALSO trigger the
    // ancestor — matching native's single-view touch delivery. See
    // `touch::swallow_ancestor_touch`.
    super::touch::swallow_ancestor_touch(el.as_ref());

    el.unchecked_into::<Node>()
}

/// The pointer half of a press-down pressable.
///
/// - `pointerdown` with the primary button fires `on_press`. A secondary
///   press (right / middle / macOS Ctrl-click, see
///   `touch::is_mac_ctrl_click`) fires nothing.
/// - The `click` that browsers synthesize after that press's release is
///   claimed (so an enclosing pressable skips it, see
///   `glue_dom::set_press_onclick`) and dropped. A `click` with no fired
///   pointerdown before it — `HTMLElement.click()` from the handle, or
///   assistive-tech activation — fires `on_press`.
///
/// `armed` records "this press already fired at the down". It is set at a
/// firing pointerdown and cleared by the next `click` or `pointercancel`
/// (a touch that turned into a scroll never gets its click). A `click` with
/// `detail == 0` (no pointer behind it: `.click()`, AT) fires even while
/// armed, so a press whose release never produced a click cannot swallow a
/// later programmatic activation; every pointer-made click has
/// `detail >= 1`.
fn wire_press_down(el: &web_glue::dom::HtmlElement, on_press: Rc<dyn Fn()>) {
    let armed = Rc::new(Cell::new(false));

    {
        let armed = armed.clone();
        let on_press = on_press.clone();
        let el_for_down = el.clone();
        crate::glue_dom::listen_for_element_lifetime(el, "pointerdown", Default::default(), move |ev| {
            let ev: web_glue::dom::PointerEvent = web_glue::JsCast::unchecked_into(ev);
            armed.set(false);
            if ev.button() != 0 || super::touch::is_mac_ctrl_click(&ev) {
                return;
            }
            // Focus moves at the compatibility `mousedown`, which runs AFTER
            // this listener — after `on_press` has opened a menu and the menu
            // has moved focus into itself. Left alone, that default focus
            // pulls focus back onto the trigger (a trapping portal then
            // bounces it, a non-trapping one loses it). So the `mousedown`
            // listener below cancels it, and the trigger is focused here
            // instead, BEFORE `on_press`, keeping release-mode focusability.
            // An ancestor that already canceled the press
            // (`InputOps::mark_preserves_focus`) wants focus left where it
            // is, so skip the focus then.
            //
            // Not `ev.prevent_default()` here: canceling `pointerdown` makes
            // the browser withhold the press's `mousedown` AND `mouseup`,
            // which are what the app-level pointer source reports as
            // `Down` / `Up` (`primitives::app_pointer`). The rest of the app
            // would never hear this press end — a menu opened by it could
            // not select the row the pointer is released over.
            if !ev.default_prevented() {
                let _ = el_for_down.focus_prevent_scroll();
            }
            // Keep the rest of the press on this element: with capture,
            // `pointerup` (and, per Pointer Events, the `click`) target the
            // trigger rather than the common ancestor of where the press
            // started and ended — which could be an enclosing pressable
            // whose `onclick` would fire on this press's release. Fails
            // harmlessly for a pointer that is no longer active.
            let _ = el_for_down.set_pointer_capture(ev.pointer_id());
            armed.set(true);
            (on_press)();
        });
    }

    {
        // The default action of the press's `mousedown` is the focus move
        // and the start of a drag text-selection; cancel it for a press
        // that fired (see the `pointerdown` listener). A press that did not
        // fire (secondary button) keeps its defaults.
        let armed = armed.clone();
        crate::glue_dom::listen_for_element_lifetime(el, "mousedown", Default::default(), move |ev| {
            if armed.get() {
                ev.prevent_default();
            }
        });
    }

    {
        let armed = armed.clone();
        crate::glue_dom::listen_for_element_lifetime(el, "pointercancel", Default::default(), move |_| {
            armed.set(false);
        });
    }

    crate::glue_dom::set_press_onclick_ev(el, move |ev| {
        let was_armed = armed.replace(false);
        if click_detail(&ev) == 0 || !was_armed {
            (on_press)();
        }
    });
}

/// `UIEvent.detail` of a `click`: the click count for a pointer-made click,
/// `0` for one with no pointer behind it (`HTMLElement.click()`, AT).
fn click_detail(ev: &web_glue::dom::Event) -> i32 {
    web_glue::js::Reflect::get(ev.as_ref(), &web_glue::JsValue::from_str("detail"))
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32
}

pub(crate) fn make_handle(node: &Node) -> PressableHandle {
    let html: web_glue::dom::HtmlElement = node
        .clone()
        .dyn_into()
        .expect("pressable node is not an HtmlElement");
    PressableHandle::new(Rc::new(html), &WebPressableOps)
}

struct WebPressableOps;
impl PressableOps for WebPressableOps {
    fn click(&self, node: &dyn Any) {
        if let Some(html) = node.downcast_ref::<web_glue::dom::HtmlElement>() {
            html.click();
        }
    }

    fn rect(&self, node: &dyn Any) -> ViewportRect {
        node.downcast_ref::<web_glue::dom::HtmlElement>()
            .map(measure_element_rect)
            .unwrap_or_default()
    }
}

fn measure_element_rect(el: &web_glue::dom::HtmlElement) -> ViewportRect {
    let r = el.get_bounding_client_rect();
    ViewportRect {
        x: r.x() as f32,
        y: r.y() as f32,
        width: r.width() as f32,
        height: r.height() as f32,
    }
}

#[cfg(test)]
mod tests {
    use crate::WebBackend;
    use runtime_shared::TouchResponse;
    use std::cell::Cell;
    use std::rc::Rc;
    use wasm_bindgen_test::*;
    use web_glue::JsCast;

    wasm_bindgen_test_configure!(run_in_browser);

    fn counter() -> (Rc<Cell<u32>>, Rc<dyn Fn()>) {
        let n = Rc::new(Cell::new(0u32));
        let n2 = n.clone();
        (n, Rc::new(move || n2.set(n2.get() + 1)))
    }

    /// A press-down pressable mounted in `#app`, plus its element.
    fn mount_on_down(backend: &mut WebBackend, on_press: Rc<dyn Fn()>) -> web_glue::dom::HtmlElement {
        let node = backend.create_pressable_on_down_impl(on_press, &Default::default());
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        doc.get_element_by_id("app").unwrap().append_child(&node).unwrap();
        node.unchecked_into()
    }

    /// Dispatch a bubbling, cancelable pointer event; returns `defaultPrevented`.
    fn pointer(target: &web_glue::dom::Element, ty: &str, button: i16, ctrl: bool) -> bool {
        let init = web_glue::dom::PointerEventInit::new();
        init.set_bubbles(true);
        init.set_cancelable(true);
        init.set_button(button);
        init.set_ctrl_key(ctrl);
        init.set_pointer_id(1);
        init.set_pointer_type("mouse");
        init.set_is_primary(true);
        let ev = web_glue::dom::PointerEvent::new_with_event_init_dict(ty, &init).unwrap();
        target.dispatch_event(&ev).unwrap();
        ev.default_prevented()
    }

    /// Dispatch a bubbling, cancelable `mousedown` / `mouseup` (the
    /// compatibility events a browser sends after the pointer events);
    /// returns `defaultPrevented`.
    fn mouse(target: &web_glue::dom::Element, ty: &str, button: i16) -> bool {
        let init = web_glue::dom::MouseEventInit::new();
        init.set_bubbles(true);
        init.set_cancelable(true);
        init.set_button(button);
        let ev = web_glue::dom::MouseEvent::new_with_mouse_event_init_dict(ty, &init).unwrap();
        target.dispatch_event(&ev).unwrap();
        ev.default_prevented()
    }

    /// Dispatch a bubbling `click` with the given `detail` (click count;
    /// `0` = no pointer behind it).
    fn click(target: &web_glue::dom::Element, detail: i32) {
        let init = web_glue::dom::MouseEventInit::new();
        init.set_bubbles(true);
        init.set_cancelable(true);
        let _ = web_glue::js::Reflect::set(
            init.as_object(),
            &web_glue::JsValue::from_str("detail"),
            &web_glue::JsValue::from(detail),
        );
        let ev = web_glue::dom::MouseEvent::new_with_mouse_event_init_dict("click", &init).unwrap();
        target.dispatch_event(&ev).unwrap();
    }

    fn key(target: &web_glue::dom::Element, k: &str) {
        let init = web_glue::dom::KeyboardEventInit::new();
        init.set_key(k);
        init.set_bubbles(true);
        init.set_cancelable(true);
        let ev = web_glue::dom::KeyboardEvent::new_with_keyboard_event_init_dict("keydown", &init).unwrap();
        target.dispatch_event(&ev).unwrap();
    }

    /// The whole `PressActivation::Down` contract on one element: the primary
    /// down fires once; that press's release and synthesized click fire
    /// nothing more; secondary presses fire nothing; Enter, Space and the
    /// handle's `click()` each fire once.
    #[wasm_bindgen_test]
    fn press_down_fires_once_at_the_primary_down() {
        crate::tests::install_mount();
        let mut backend = WebBackend::new("#app");
        let (n, on_press) = counter();
        let el = mount_on_down(&mut backend, on_press);

        pointer(&el, "pointerdown", 0, false);
        assert_eq!(n.get(), 1, "primary pointerdown fires");
        pointer(&el, "pointerup", 0, false);
        click(&el, 1);
        assert_eq!(n.get(), 1, "the release and its click fire nothing more");

        pointer(&el, "pointerdown", 2, false);
        pointer(&el, "pointerup", 2, false);
        assert_eq!(n.get(), 1, "right button fires nothing");
        pointer(&el, "pointerdown", 1, false);
        assert_eq!(n.get(), 1, "middle button fires nothing");

        key(&el, "Enter");
        assert_eq!(n.get(), 2, "Enter fires once");
        key(&el, " ");
        assert_eq!(n.get(), 3, "Space fires once");

        super::make_handle(&el.clone().unchecked_into()).click();
        assert_eq!(n.get(), 4, "the handle's click() (detail 0) fires once");

        // A click with no pointerdown before it (AT activation that
        // synthesizes a mouse click) fires.
        click(&el, 1);
        assert_eq!(n.get(), 5, "an unarmed pointer click fires");
    }

    /// A press-down pressable canceled its `pointerdown` to stop the focus
    /// move. Browsers then withhold that press's `mousedown` and `mouseup`,
    /// which are the events the app-level pointer source reports as `Down`
    /// and `Up` — so `on_pointer` listeners never heard the press end, and
    /// a menu it opened could not select the row the pointer was released
    /// over. The `pointerdown` must stay uncanceled; the focus move is
    /// stopped by canceling the `mousedown` of a press that fired.
    #[wasm_bindgen_test]
    fn regression_press_down_leaves_pointerdown_uncanceled_for_the_app_pointer_source() {
        crate::tests::install_mount();
        let mut backend = WebBackend::new("#app");
        let (n, on_press) = counter();
        let el = mount_on_down(&mut backend, on_press);

        assert!(!pointer(&el, "pointerdown", 0, false), "the pointerdown is not canceled");
        assert_eq!(n.get(), 1);
        assert!(mouse(&el, "mousedown", 0), "the fired press's mousedown is canceled (no focus move)");
        assert!(!mouse(&el, "mouseup", 0), "its mouseup is left alone");

        // A press that fires nothing keeps its mousedown default.
        pointer(&el, "pointerup", 0, false);
        click(&el, 1);
        pointer(&el, "pointerdown", 2, false);
        assert!(!mouse(&el, "mousedown", 2), "a secondary press's mousedown is not canceled");
        assert_eq!(n.get(), 1);
    }

    /// On macOS a Ctrl-click is the secondary press even when Chrome/Firefox
    /// report `button == 0`; it must not fire. Off macOS it's a primary press.
    #[wasm_bindgen_test]
    fn press_down_ignores_mac_ctrl_click() {
        crate::tests::install_mount();
        let mut backend = WebBackend::new("#app");
        let (n, on_press) = counter();
        let el = mount_on_down(&mut backend, on_press);

        crate::primitives::touch::force_ctrl_click_fold(Some(true));
        pointer(&el, "pointerdown", 0, true);
        assert_eq!(n.get(), 0, "macOS Ctrl-click is secondary");
        crate::primitives::touch::force_ctrl_click_fold(Some(false));
        pointer(&el, "pointerdown", 0, true);
        assert_eq!(n.get(), 1, "elsewhere Ctrl-click is a modified primary press");
        crate::primitives::touch::force_ctrl_click_fold(None);
    }

    /// A press whose click never arrives (a touch canceled into a scroll)
    /// must not swallow the next real activation.
    #[wasm_bindgen_test]
    fn press_down_pointercancel_disarms() {
        crate::tests::install_mount();
        let mut backend = WebBackend::new("#app");
        let (n, on_press) = counter();
        let el = mount_on_down(&mut backend, on_press);

        pointer(&el, "pointerdown", 0, false);
        pointer(&el, "pointercancel", 0, false);
        assert_eq!(n.get(), 1);
        click(&el, 1);
        assert_eq!(n.get(), 2, "a click after a canceled press is a new activation");
    }

    /// The press must not reach ancestors: neither an enclosing
    /// release-mode pressable (through the release's bubbling `click`) nor an
    /// ancestor `on_touch` (through the `pointerdown`).
    #[wasm_bindgen_test]
    fn press_down_does_not_reach_ancestors() {
        crate::tests::install_mount();
        let mut backend = WebBackend::new("#app");
        let doc = web_glue::dom::window().unwrap().document().unwrap();

        let touched = Rc::new(Cell::new(false));
        let row = doc.create_element("div").unwrap();
        doc.get_element_by_id("app").unwrap().append_child(&row).unwrap();
        let t = touched.clone();
        backend.install_touch_handler_impl(
            &row.clone().unchecked_into(),
            Rc::new(move |_| {
                t.set(true);
                TouchResponse::CONSUMED
            }),
        );
        let (outer_n, outer_press) = counter();
        let outer = backend.create_pressable_impl(outer_press, &Default::default());
        row.append_child(&outer).unwrap();
        let (inner_n, inner_press) = counter();
        let inner = backend.create_pressable_on_down_impl(inner_press, &Default::default());
        outer.append_child(&inner).unwrap();
        let inner: web_glue::dom::Element = inner.unchecked_into();

        pointer(&inner, "pointerdown", 0, false);
        pointer(&inner, "pointerup", 0, false);
        click(&inner, 1);
        assert_eq!(inner_n.get(), 1, "the inner press-down pressable fires once");
        assert_eq!(outer_n.get(), 0, "the enclosing pressable skips the claimed click");
        assert!(!touched.get(), "the ancestor on_touch never sees the press");
    }

    /// The firing pointerdown cancels the compatibility `mousedown`, whose
    /// default would move focus onto the trigger after `on_press` opened a
    /// menu that took focus. The trigger is focused before `on_press` runs,
    /// so focus moves made by `on_press` stand.
    #[wasm_bindgen_test]
    fn press_down_focuses_trigger_before_on_press() {
        crate::tests::install_mount();
        let mut backend = WebBackend::new("#app");
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        let menu_item: web_glue::dom::HtmlElement = doc.create_element("button").unwrap().unchecked_into();
        doc.get_element_by_id("app").unwrap().append_child(&menu_item).unwrap();

        let focused_at_press = Rc::new(Cell::new(false));
        let doc2 = doc.clone();
        let item = menu_item.clone();
        let f = focused_at_press.clone();
        let trigger_slot: Rc<std::cell::RefCell<Option<web_glue::dom::HtmlElement>>> = Default::default();
        let slot = trigger_slot.clone();
        let el = mount_on_down(
            &mut backend,
            Rc::new(move || {
                let active = doc2.active_element();
                let trigger = slot.borrow().clone().unwrap();
                f.set(active.map(|a| a.as_js().strict_eq(trigger.as_js())).unwrap_or(false));
                let _ = item.focus(); // the "menu" takes focus
            }),
        );
        *trigger_slot.borrow_mut() = Some(el.clone());

        pointer(&el, "pointerdown", 0, false);
        assert!(focused_at_press.get(), "the trigger is focused before on_press runs");
        let active = doc.active_element().unwrap();
        assert!(active.as_js().strict_eq(menu_item.as_js()), "on_press's focus move stands");
    }
}
