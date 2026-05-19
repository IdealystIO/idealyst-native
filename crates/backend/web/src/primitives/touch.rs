//! Raw touch delivery for the web backend.
//!
//! Implements [`framework_core::Backend::install_touch_handler`] and
//! [`framework_core::Backend::claim_touch`] using the Pointer Events
//! API. One DOM element receives four listeners — `pointerdown`,
//! `pointermove`, `pointerup`, `pointercancel` — and translates each
//! into a [`TouchEvent`] for the framework's handler.
//!
//! Pointer Events unify mouse, touch, and pen on the web; the
//! `pointerType` distinction is not surfaced through the framework
//! today (`force` is filled when the device reports it; otherwise
//! `None`).
//!
//! Native scroll / pinch on the subscribed element is suppressed via
//! `touch-action: none`. Once a handler returns `claim: true`, we
//! call `setPointerCapture` so subsequent events stay locked to this
//! element even if the finger / cursor leaves its bounds — that's
//! the web-side implementation of the claim protocol.

use crate::WebBackend;
use framework_core::{TouchEvent, TouchHandler, TouchId, TouchPhase, TouchPoint};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Element, Node, PointerEvent};

/// Install the four pointer listeners on `node`. Storage of the
/// resulting [`Closure`]s is shared with the existing event-listener
/// vec so the JS side keeps them alive for the element's lifetime.
pub(crate) fn install(b: &mut WebBackend, node: &Node, handler: TouchHandler) {
    // The framework only installs a touch handler on primitives that
    // map to real DOM elements; if the downcast fails we'd be
    // looking at a text node or a fragment, which shouldn't carry
    // an `on_touch` slot. Bail silently rather than panic — the
    // framework treats a missing impl as best-effort.
    let element: Element = match node.clone().dyn_into::<Element>() {
        Ok(e) => e,
        Err(_) => return,
    };

    // Suppress native scroll/pinch so the browser doesn't preempt
    // our events with pan-to-scroll or pinch-to-zoom. Touch-action:
    // none is the Pointer Events knob for "I want all the events
    // myself"; CSS-cascadable so existing stylesheet rules can
    // override per-element if needed.
    if let Ok(html) = element.clone().dyn_into::<web_sys::HtmlElement>() {
        let _ = html.style().set_property("touch-action", "none");
    }

    // `pointermove` fires for hover-only motion too — for mouse the
    // pointer can move over an element with no button down. We
    // only deliver `Moved` for pointers that are currently "down"
    // on this element, which we track here. Touch never hovers, so
    // this filter is effectively a no-op for finger input.
    //
    // We also track captured pointers (those for which a handler
    // returned `claim: true`) so future logic — e.g. an element
    // suppressing scroll while claimed — can read it.
    let active: Rc<RefCell<HashSet<i32>>> = Rc::new(RefCell::new(HashSet::new()));
    let captured: Rc<RefCell<HashSet<i32>>> = Rc::new(RefCell::new(HashSet::new()));

    // pointerdown — Began.
    {
        let handler = handler.clone();
        let active = active.clone();
        let captured = captured.clone();
        let element_for_capture = element.clone();
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let local = local_position(&ev);
            let touch_id = TouchId(ev.pointer_id() as u64);
            let te = TouchEvent {
                id: touch_id,
                phase: TouchPhase::Began,
                position: TouchPoint::new(local.0, local.1),
                window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
                timestamp_ns: timestamp_ns(&ev),
                force: pressure_to_force(ev.pressure()),
            };
            let response = (handler)(&te);
            if response.consumed {
                active.borrow_mut().insert(ev.pointer_id());
                if response.claim {
                    capture_pointer(&element_for_capture, ev.pointer_id(), &captured);
                }
            }
        });
        let _ = element.add_event_listener_with_callback(
            "pointerdown",
            closure.as_ref().unchecked_ref(),
        );
        b._touch_closures
            .push(closure.into_js_value().unchecked_into());
    }

    // pointermove — Moved (only when this pointer is in `active`).
    {
        let handler = handler.clone();
        let active = active.clone();
        let captured = captured.clone();
        let element_for_capture = element.clone();
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let pid = ev.pointer_id();
            if !active.borrow().contains(&pid) {
                return;
            }
            let local = local_position(&ev);
            let touch_id = TouchId(pid as u64);
            let te = TouchEvent {
                id: touch_id,
                phase: TouchPhase::Moved,
                position: TouchPoint::new(local.0, local.1),
                window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
                timestamp_ns: timestamp_ns(&ev),
                force: pressure_to_force(ev.pressure()),
            };
            let response = (handler)(&te);
            if response.claim && !captured.borrow().contains(&pid) {
                capture_pointer(&element_for_capture, pid, &captured);
            }
        });
        let _ = element.add_event_listener_with_callback(
            "pointermove",
            closure.as_ref().unchecked_ref(),
        );
        b._touch_closures
            .push(closure.into_js_value().unchecked_into());
    }

    // pointerup — Ended.
    {
        let handler = handler.clone();
        let active = active.clone();
        let captured = captured.clone();
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let pid = ev.pointer_id();
            if !active.borrow_mut().remove(&pid) {
                return;
            }
            captured.borrow_mut().remove(&pid);
            let local = local_position(&ev);
            let touch_id = TouchId(pid as u64);
            let te = TouchEvent {
                id: touch_id,
                phase: TouchPhase::Ended,
                position: TouchPoint::new(local.0, local.1),
                window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
                timestamp_ns: timestamp_ns(&ev),
                force: pressure_to_force(ev.pressure()),
            };
            let _ = (handler)(&te);
        });
        let _ = element.add_event_listener_with_callback(
            "pointerup",
            closure.as_ref().unchecked_ref(),
        );
        b._touch_closures
            .push(closure.into_js_value().unchecked_into());
    }

    // pointercancel — Cancelled.
    {
        let handler = handler.clone();
        let active = active.clone();
        let captured = captured.clone();
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            let pid = ev.pointer_id();
            if !active.borrow_mut().remove(&pid) {
                return;
            }
            captured.borrow_mut().remove(&pid);
            let local = local_position(&ev);
            let touch_id = TouchId(pid as u64);
            let te = TouchEvent {
                id: touch_id,
                phase: TouchPhase::Cancelled,
                position: TouchPoint::new(local.0, local.1),
                window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
                timestamp_ns: timestamp_ns(&ev),
                force: pressure_to_force(ev.pressure()),
            };
            let _ = (handler)(&te);
        });
        let _ = element.add_event_listener_with_callback(
            "pointercancel",
            closure.as_ref().unchecked_ref(),
        );
        b._touch_closures
            .push(closure.into_js_value().unchecked_into());
    }
}

/// Implementation of [`framework_core::Backend::claim_touch`] —
/// external claim invoked when a handler returned `claim: true` via
/// any route other than the local `pointerdown` / `pointermove`
/// callback we wired above (today there's no such route on web, but
/// the trait method exists for symmetry with iOS / Android where the
/// framework dispatches and the backend claims).
///
/// In practice on web, claims happen inline in the listener closure
/// (where we have the live `PointerEvent` to pass to
/// `setPointerCapture`). This method is a no-op fallback.
#[allow(dead_code)]
pub(crate) fn claim(_b: &mut WebBackend, _node: &Node, _touch_id: TouchId) {
    // No-op on web; see doc comment.
}

/// Translate `client` coordinates (viewport-relative) into element-
/// local coordinates by subtracting the element's bounding rect.
/// Falls back to client coordinates if the cast fails — better to
/// hand the handler a same-frame approximation than nothing.
fn local_position(ev: &PointerEvent) -> (f32, f32) {
    let target = match ev.current_target() {
        Some(t) => t,
        None => return (ev.client_x() as f32, ev.client_y() as f32),
    };
    let el: web_sys::Element = match target.dyn_into() {
        Ok(e) => e,
        Err(_) => return (ev.client_x() as f32, ev.client_y() as f32),
    };
    let rect = el.get_bounding_client_rect();
    (
        ev.client_x() as f32 - rect.x() as f32,
        ev.client_y() as f32 - rect.y() as f32,
    )
}

/// Convert `PointerEvent.timeStamp` (DOMHighResTimeStamp, ms with
/// fractional precision) to nanoseconds. Web exposes only ms-with-
/// fractions; the conversion preserves the fractional part by
/// multiplying before casting.
fn timestamp_ns(ev: &PointerEvent) -> u64 {
    (ev.time_stamp() * 1_000_000.0) as u64
}

/// Map the Pointer Events `pressure` field (0.0..=1.0 if reported)
/// onto our `force` slot. The DOM reports `0.5` for buttons that
/// don't track pressure but are active; we treat that as "no
/// information" by returning `None`. Pen / 3D-touch devices report
/// finer-grained values which pass through.
fn pressure_to_force(pressure: f32) -> Option<f32> {
    // The Pointer Events spec says non-pressure-sensitive sources
    // emit either 0.0 (no button) or 0.5 (button down). Both
    // values are sentinels rather than real measurements.
    if pressure == 0.0 || (pressure - 0.5).abs() < f32::EPSILON {
        None
    } else {
        Some(pressure)
    }
}

/// Call `Element.setPointerCapture(pointer_id)` and record the
/// capture in `captured`. Suppresses the call on browsers that
/// haven't implemented it (we fall back to whatever
/// `add_event_listener` plus `touch-action: none` give us).
fn capture_pointer(element: &Element, pointer_id: i32, captured: &Rc<RefCell<HashSet<i32>>>) {
    let _ = element.set_pointer_capture(pointer_id);
    captured.borrow_mut().insert(pointer_id);
}
