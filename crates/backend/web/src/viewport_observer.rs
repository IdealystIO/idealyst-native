//! Window-resize observer that pushes `window.innerWidth /
//! innerHeight` into `runtime_core::set_viewport_size`.
//!
//! Author code subscribes via [`runtime_core::viewport_size()`] from
//! inside an effect / derived. The observer fires on initial install
//! (so the signal has a non-zero value before the first paint) and on
//! every `resize` event after that.
//!
//! Idempotent — re-calling [`install_viewport_observer`] replaces the
//! previous listener with a fresh one. The closure is `forget()`-leaked
//! into the JS heap and lives for the page's lifetime; that matches the
//! existing dev-transport resize listener at
//! [`crate::dev_transport`].

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

/// Install a `resize` listener on `window` and push the current viewport
/// once immediately so subscribers see a non-zero value on first read.
///
/// Safe to call without a browser context (worker, SSR) — no-ops if
/// `web_sys::window()` returns `None`.
pub fn install_viewport_observer() {
    let Some(win) = web_sys::window() else { return };

    // Fire once synchronously so the initial value is correct by the
    // time the framework's first render runs.
    push_current_viewport(&win);

    let closure: Closure<dyn FnMut(web_sys::Event)> =
        Closure::new(move |_: web_sys::Event| {
            if let Some(win) = web_sys::window() {
                push_current_viewport(&win);
            }
        });
    let _ = win.add_event_listener_with_callback(
        "resize",
        closure.as_ref().unchecked_ref(),
    );
    // Listener outlives the install call. The dev-transport sibling
    // uses the same forget-strategy; we don't track a handle because
    // page-lifetime is the intended scope.
    closure.forget();
}

fn push_current_viewport(win: &web_sys::Window) {
    let w = win.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let h = win.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    runtime_core::set_viewport_size(runtime_core::ViewportSize {
        width: w,
        height: h,
    });
}
