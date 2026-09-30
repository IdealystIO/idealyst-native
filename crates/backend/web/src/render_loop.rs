//! Web `RenderLoopDriver`: a `requestAnimationFrame` chain on web-glue.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::driver::{
    install_render_loop_driver, RenderLoopDriver, RenderLoopHandle,
};
use web_glue::Closure;

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
    window: web_glue::dom::Window,
    /// Browser's rAF handle for the currently-queued frame.
    pending: Option<i32>,
    /// The per-frame callback. We own it so we can drop it after telling
    /// the browser to cancel — never the other way around.
    closure: Option<Closure>,
    /// Set from `Drop`. The per-frame closure short-circuits on this
    /// flag so a callback already pulled off the JS queue becomes a
    /// no-op.
    cancelled: bool,
}

impl Drop for State {
    fn drop(&mut self) {
        self.cancelled = true;
        if let Some(h) = self.pending.take() {
            self.window.cancel_animation_frame(h);
        }
    }
}

impl RenderLoopHandle for WebHandle {
    fn cancel(&mut self) {
        self.state = None;
    }
}

fn start_inner(mut user_fn: Box<dyn FnMut(f32) + 'static>) -> WebHandle {
    let Some(window) = web_glue::dom::window() else {
        return WebHandle { state: None };
    };
    let started = web_glue::dom::date_now();
    let state = Rc::new(RefCell::new(State {
        window: window.clone(),
        pending: None,
        closure: None,
        cancelled: false,
    }));
    let weak = Rc::downgrade(&state);
    let closure = Closure::new(move |_| {
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
        let elapsed = ((web_glue::dom::date_now() - started) / 1000.0) as f32;
        user_fn(elapsed);
        let mut s = strong.borrow_mut();
        if s.cancelled {
            return;
        }
        if let Some(c) = s.closure.as_ref() {
            let h = s.window.request_animation_frame(c);
            s.pending = Some(h);
        }
    });
    let h = window.request_animation_frame(&closure);
    {
        let mut s = state.borrow_mut();
        s.closure = Some(closure);
        s.pending = Some(h);
    }
    WebHandle { state: Some(state) }
}
