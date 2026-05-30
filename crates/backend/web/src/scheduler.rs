//! Web `Scheduler`: `Promise.then` for microtasks, `requestAnimationFrame`
//! for one-shot frames + the recurring loop, `setTimeout` for delayed
//! callbacks. Each cancellable variant owns both the browser handle
//! and the wasm-bindgen `Closure` so `Drop` cancels the browser-side
//! dispatch *before* releasing the closure — avoiding the
//! "closure invoked after being dropped" panic.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use runtime_core::scheduling::{ScheduleHandle, Scheduler};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

#[cfg(feature = "hydrate")]
thread_local! {
    /// SSR-hydration microtask buffer. `None` normally (dispatch via
    /// `Promise.then`). While hydrating, microtasks buffer here and
    /// `mount` drains them synchronously inside the adoption window, so
    /// the navigator SDK's deferred chrome/screen builds adopt the
    /// server's DOM. Set by [`begin_hydration_buffering`], cleared by
    /// [`end_hydration_buffering`].
    static HYDRATION_BUFFER: RefCell<Option<VecDeque<Box<dyn FnOnce() + 'static>>>> =
        const { RefCell::new(None) };
}

/// Begin buffering microtasks for the hydration window (called by
/// `WebBackend::hydrate` before `mount`).
#[cfg(feature = "hydrate")]
pub(crate) fn begin_hydration_buffering() {
    HYDRATION_BUFFER.with(|b| {
        let mut slot = b.borrow_mut();
        if slot.is_none() {
            *slot = Some(VecDeque::new());
        }
    });
}

/// Stop buffering (called by `WebBackend::finish`). Any still-buffered
/// tasks flush via the normal async path so none are dropped.
#[cfg(feature = "hydrate")]
pub(crate) fn end_hydration_buffering() {
    let leftover = HYDRATION_BUFFER.with(|b| b.borrow_mut().take());
    if let Some(tasks) = leftover {
        for task in tasks {
            dispatch_via_promise(task);
        }
    }
}

#[cfg(feature = "hydrate")]
fn drain_hydration_buffer() {
    loop {
        let next =
            HYDRATION_BUFFER.with(|b| b.borrow_mut().as_mut().and_then(|q| q.pop_front()));
        match next {
            Some(task) => task(),
            None => break,
        }
    }
}

fn dispatch_via_promise(f: Box<dyn FnOnce() + 'static>) {
    let mut once: Option<Box<dyn FnOnce() + 'static>> = Some(f);
    let cb: Closure<dyn FnMut(wasm_bindgen::JsValue)> =
        Closure::new(move |_: wasm_bindgen::JsValue| {
            if let Some(g) = once.take() {
                g();
            }
        });
    let promise = js_sys::Promise::resolve(&wasm_bindgen::JsValue::UNDEFINED);
    let _ = promise.then(&cb);
    cb.forget();
}

/// Register this backend's scheduler with `runtime-core`. Idempotent —
/// first install wins.
pub fn install_scheduler() {
    runtime_core::scheduling::install_scheduler(Box::new(WebScheduler));
}

struct WebScheduler;

impl Scheduler for WebScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        #[cfg(feature = "hydrate")]
        {
            let buffering = HYDRATION_BUFFER.with(|b| b.borrow().is_some());
            if buffering {
                HYDRATION_BUFFER.with(|b| {
                    if let Some(q) = b.borrow_mut().as_mut() {
                        q.push_back(f);
                    }
                });
                return;
            }
        }
        dispatch_via_promise(f);
    }

    fn drain_buffered_microtasks(&self) {
        #[cfg(feature = "hydrate")]
        drain_hydration_buffer();
    }

    fn after_animation_frame(
        &self,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        let Some(window) = web_sys::window() else {
            f();
            return Box::new(InertHandle);
        };
        let mut once: Option<Box<dyn FnOnce() + 'static>> = Some(f);
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            if let Some(g) = once.take() {
                g();
            }
        });
        let handle = match window.request_animation_frame(closure.as_ref().unchecked_ref())
        {
            Ok(h) => h,
            Err(_) => return Box::new(InertHandle),
        };
        Box::new(OneShotHandle {
            inner: Some(Rc::new(RefCell::new(OneShotInner {
                handle,
                kind: ScheduledKind::AnimationFrame,
                _closure: closure,
            }))),
        })
    }

    fn after_ms(
        &self,
        delay_ms: i32,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        let Some(window) = web_sys::window() else {
            f();
            return Box::new(InertHandle);
        };
        let mut once: Option<Box<dyn FnOnce() + 'static>> = Some(f);
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            if let Some(g) = once.take() {
                g();
            }
        });
        let handle = match window.set_timeout_with_callback_and_timeout_and_arguments_0(
            closure.as_ref().unchecked_ref(),
            delay_ms,
        ) {
            Ok(h) => h,
            Err(_) => return Box::new(InertHandle),
        };
        Box::new(OneShotHandle {
            inner: Some(Rc::new(RefCell::new(OneShotInner {
                handle,
                kind: ScheduledKind::Timeout,
                _closure: closure,
            }))),
        })
    }

    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        let Some(window) = web_sys::window() else {
            return Box::new(InertHandle);
        };
        let state = Rc::new(RefCell::new(RafLoopInner {
            pending: None,
            closure: None,
            cancelled: false,
        }));
        let weak_state = Rc::downgrade(&state);
        let user_fn = Rc::new(RefCell::new(f));
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            if state.borrow().cancelled {
                return;
            }
            // Browser is about to fire the next frame; record that
            // there's no longer a pending callback.
            state.borrow_mut().pending = None;
            // Invoke the user function outside any borrow on `state`
            // so the user is free to drop the handle from inside
            // their own frame body.
            {
                let mut f_borrow = user_fn.borrow_mut();
                (&mut *f_borrow)();
            }
            let mut s = state.borrow_mut();
            if s.cancelled {
                return;
            }
            if let Some(window) = web_sys::window() {
                if let Some(c) = s.closure.as_ref() {
                    if let Ok(h) =
                        window.request_animation_frame(c.as_ref().unchecked_ref())
                    {
                        s.pending = Some(h);
                    }
                }
            }
        });
        state.borrow_mut().closure = Some(closure);
        // Kick off the first frame. Pull the JS function ref out
        // into a local so the immutable borrow drops before we hit
        // the borrow_mut below.
        let raf_fn = state.borrow().closure.as_ref().map(|c| c.as_ref().clone());
        if let Some(raf_fn) = raf_fn {
            if let Ok(h) = window.request_animation_frame(raf_fn.unchecked_ref()) {
                state.borrow_mut().pending = Some(h);
            }
        }
        Box::new(RafLoopHandle { inner: Some(state) })
    }
}

// ---------------------------------------------------------------------------
// One-shot handle (after_animation_frame, after_ms)
// ---------------------------------------------------------------------------

struct OneShotHandle {
    inner: Option<Rc<RefCell<OneShotInner>>>,
}

struct OneShotInner {
    handle: i32,
    kind: ScheduledKind,
    /// The Closure must outlive its scheduled dispatch. We hold it
    /// here so Drop can release it *after* the browser has been
    /// told to cancel.
    _closure: Closure<dyn FnMut()>,
}

enum ScheduledKind {
    AnimationFrame,
    Timeout,
}

impl Drop for OneShotInner {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            match self.kind {
                ScheduledKind::AnimationFrame => {
                    let _ = window.cancel_animation_frame(self.handle);
                }
                ScheduledKind::Timeout => {
                    window.clear_timeout_with_handle(self.handle);
                }
            }
        }
    }
}

impl ScheduleHandle for OneShotHandle {
    fn cancel(&mut self) {
        self.inner = None;
    }
}

// ---------------------------------------------------------------------------
// rAF-loop handle
// ---------------------------------------------------------------------------

struct RafLoopHandle {
    inner: Option<Rc<RefCell<RafLoopInner>>>,
}

struct RafLoopInner {
    pending: Option<i32>,
    closure: Option<Closure<dyn FnMut()>>,
    cancelled: bool,
}

impl Drop for RafLoopInner {
    fn drop(&mut self) {
        self.cancelled = true;
        if let (Some(h), Some(window)) = (self.pending.take(), web_sys::window()) {
            let _ = window.cancel_animation_frame(h);
        }
    }
}

impl ScheduleHandle for RafLoopHandle {
    fn cancel(&mut self) {
        self.inner = None;
    }
}

// ---------------------------------------------------------------------------
// Inert fallback handle
// ---------------------------------------------------------------------------

struct InertHandle;

impl ScheduleHandle for InertHandle {
    fn cancel(&mut self) {}
}
