//! Wheel / magnify delivery for the web backend.
//!
//! Implements [`runtime_core::Backend::install_wheel_handler`] using the DOM
//! `wheel` event. One listener on the subscribed element translates each
//! event into a [`WheelEvent`] for the framework's handler.
//!
//! The browser overloads `wheel` for two intents, distinguished by
//! `ctrlKey`:
//! - **`ctrlKey == true`** — a trackpad pinch (the browser synthesizes a
//!   ctrl+wheel) or ctrl+scroll. This is a *zoom* intent. We map `deltaY`
//!   through [`ZOOM_PER_WHEEL_UNIT`] into a normalized incremental
//!   [`WheelEvent::scale`], chosen so a trackpad pinch feels like macOS's
//!   native `magnify:`.
//! - **`ctrlKey == false`** — a plain scroll (two-finger trackpad or mouse
//!   wheel). A *scroll* intent, carried in `delta_x` / `delta_y`.
//!
//! When the handler consumes the event we `preventDefault()` so the page
//! doesn't also scroll or trigger the browser's own pinch-zoom.

use crate::WebBackend;
use runtime_core::{TouchPoint, WheelEvent as FwWheelEvent, WheelHandler, WheelKind};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Element, Node, WheelEvent};

/// Maps one unit of `WheelEvent.deltaY` (under `ctrlKey`) to a fraction of
/// zoom. `scale = exp(-deltaY * k)`: negative deltaY (pinch open / scroll up)
/// zooms in, positive zooms out. The exponential form makes the gesture
/// symmetric and frame-rate independent — composing N events multiplies to
/// the same result as one event of the summed delta. `0.01` lands a typical
/// trackpad-pinch `deltaY` (single-digit per frame) near macOS's per-event
/// magnification magnitude.
const ZOOM_PER_WHEEL_UNIT: f32 = 0.01;

/// Install the `wheel` listener on `node`. The closure is kept alive by
/// pushing it onto the backend's shared listener keep-alive vec (same one the
/// touch listeners use).
pub(crate) fn install(b: &mut WebBackend, node: &Node, handler: WheelHandler) {
    let element: Element = match node.clone().dyn_into::<Element>() {
        Ok(e) => e,
        Err(_) => return,
    };

    let closure = Closure::<dyn FnMut(WheelEvent)>::new(move |ev: WheelEvent| {
        let local = local_position(&ev);
        let zoom = ev.ctrl_key();
        let (kind, delta_x, delta_y, scale) = if zoom {
            // Pinch / ctrl+scroll → zoom. deltaY drives the factor; deltaX is
            // not meaningful for zoom.
            let s = (-(ev.delta_y() as f32) * ZOOM_PER_WHEEL_UNIT).exp();
            (WheelKind::Zoom, 0.0, 0.0, s)
        } else {
            (
                WheelKind::Scroll,
                ev.delta_x() as f32,
                ev.delta_y() as f32,
                1.0,
            )
        };
        let we = FwWheelEvent {
            kind,
            delta_x,
            delta_y,
            scale,
            // Browsers expose no native trackpad rotation, so web never emits
            // `WheelKind::Rotate`; rotation is always zero here.
            rotation: 0.0,
            position: TouchPoint::new(local.0, local.1),
            window_position: TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32),
            timestamp_ns: (ev.time_stamp() * 1_000_000.0) as u64,
        };
        // Batching is automatic via the core `on_wheel` cycle wrapper (see
        // `runtime_core::cycle`): a wheel pan/zoom writes pan_x, pan_y, zoom, + a
        // repaint tick, all coalesced into one reactive flush so web's rAF renders
        // one consistent frame, not the last stray write.
        let response = (handler)(&we);
        if response.consumed {
            // Stop the page from also scrolling / browser-zooming. Must be a
            // non-passive listener (the default for `addEventListener` without
            // `{passive:true}`, which is what `add_event_listener_with_callback`
            // gives us) for preventDefault to take effect on `wheel`.
            ev.prevent_default();
        }
    });
    let _ = element.add_event_listener_with_callback("wheel", closure.as_ref().unchecked_ref());
    b._touch_closures
        .push(closure.into_js_value().unchecked_into());
}

/// Element-local cursor coordinates: `client` minus the element's rect.
fn local_position(ev: &WheelEvent) -> (f32, f32) {
    let target = match ev.current_target() {
        Some(t) => t,
        None => return (ev.client_x() as f32, ev.client_y() as f32),
    };
    let el: Element = match target.dyn_into() {
        Ok(e) => e,
        Err(_) => return (ev.client_x() as f32, ev.client_y() as f32),
    };
    let rect = el.get_bounding_client_rect();
    (
        ev.client_x() as f32 - rect.x() as f32,
        ev.client_y() as f32 - rect.y() as f32,
    )
}
