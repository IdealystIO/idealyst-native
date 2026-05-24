//! Web `RenderLoopDriver`: `requestAnimationFrame` chain.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::driver::{
    install_render_loop_driver, RenderLoopDriver, RenderLoopHandle,
};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

/// Register this backend's driver with `runtime-core`. Idempotent —
/// first install wins.
pub fn install_render_loop() {
    install_render_loop_driver(Box::new(WebRenderLoopDriver));
}

struct WebRenderLoopDriver;

impl RenderLoopDriver for WebRenderLoopDriver {
    fn start(
        &self,
        closure: Box<dyn FnMut(f32) + 'static>,
    ) -> Box<dyn RenderLoopHandle> {
        Box::new(start_inner(closure))
    }
}

struct WebHandle {
    // `Option` so `cancel()` can drop the inner state ahead of the
    // outer `Drop`.
    state: Option<Rc<RefCell<State>>>,
}

struct State {
    /// Browser's rAF handle for the currently-queued frame.
    pending: Option<i32>,
    /// The wasm-bindgen wrapper for the per-frame callback. We own
    /// it so we can drop it after telling the browser to cancel —
    /// never the other way around.
    closure: Option<Closure<dyn FnMut()>>,
    /// Set from `Drop`. The per-frame closure short-circuits on this
    /// flag so a callback already pulled off the JS queue becomes a
    /// no-op.
    cancelled: bool,
}

impl Drop for State {
    fn drop(&mut self) {
        self.cancelled = true;
        if let (Some(h), Some(window)) = (self.pending.take(), web_sys::window()) {
            let _ = window.cancel_animation_frame(h);
        }
    }
}

impl RenderLoopHandle for WebHandle {
    fn cancel(&mut self) {
        self.state = None;
    }
}

fn start_inner(mut user_fn: Box<dyn FnMut(f32) + 'static>) -> WebHandle {
    let Some(window) = web_sys::window() else {
        return WebHandle { state: None };
    };
    let started = js_sys::Date::now();
    let state = Rc::new(RefCell::new(State {
        pending: None,
        closure: None,
        cancelled: false,
    }));
    let weak = Rc::downgrade(&state);
    let closure: Closure<dyn FnMut()> = Closure::new(move || {
        let Some(strong) = weak.upgrade() else { return };
        if strong.borrow().cancelled {
            return;
        }
        // Browser is about to fire this frame; clear the pending
        // handle so re-arm logic below sets a fresh one.
        strong.borrow_mut().pending = None;
        // Invoke the user fn outside any borrow on `strong`, so the
        // user is free to drop the RenderLoop handle from inside
        // their own frame body.
        let elapsed = ((js_sys::Date::now() - started) / 1000.0) as f32;
        user_fn(elapsed);
        let mut s = strong.borrow_mut();
        if s.cancelled {
            return;
        }
        if let Some(window) = web_sys::window() {
            if let Some(c) = s.closure.as_ref() {
                if let Ok(h) = window.request_animation_frame(c.as_ref().unchecked_ref())
                {
                    s.pending = Some(h);
                }
            }
        }
    });
    state.borrow_mut().closure = Some(closure);
    // Kick the first frame. We can't hold `state.borrow()` across
    // `request_animation_frame` and then take `borrow_mut` to record
    // the handle (the immutable borrow's temporary lives for the
    // whole if-let). Pull the JS fn ref out first.
    let raf_fn = state.borrow().closure.as_ref().map(|c| c.as_ref().clone());
    if let Some(raf_fn) = raf_fn {
        if let Ok(h) = window.request_animation_frame(raf_fn.unchecked_ref()) {
            state.borrow_mut().pending = Some(h);
        }
    }
    WebHandle { state: Some(state) }
}
