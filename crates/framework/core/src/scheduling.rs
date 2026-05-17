//! Platform-agnostic scheduling primitives.
//!
//! All three helpers solve a single shape of bug:
//!
//! > A user-provided closure is queued with the browser (microtask,
//! > `requestAnimationFrame`, `setTimeout`). The closure's owner is
//! > dropped before the browser fires it. The browser still
//! > dispatches. wasm-bindgen sees a destroyed `Closure` and panics
//! > "closure invoked recursively or after being dropped".
//!
//! The pattern is easy to write incorrectly. The helpers here own
//! both the closure handle AND the browser handle, and on `Drop`:
//!
//! 1. Cancel the browser-side scheduling (via
//!    `cancelAnimationFrame` / `clearTimeout`). The browser drops
//!    its queued reference.
//! 2. Drop the wasm-bindgen `Closure`. No spurious invocations.
//!
//! For native (non-wasm32) targets these helpers run their bodies
//! synchronously (or in the case of `RafLoop`, do nothing — there's
//! no concept of "next animation frame" without a windowing backend
//! to ask). The web-relevant cancellation hazard is wasm-specific.
//!
//! # Quick reference
//!
//! | Helper                                | Fires           | Cancel on drop |
//! |---------------------------------------|-----------------|----------------|
//! | [`schedule_microtask`]                | once, next tick | n/a (one-shot, fire-and-forget) |
//! | [`after_animation_frame`]             | once, next frame| ✓ |
//! | [`after_ms`]                          | once, after delay | ✓ |
//! | [`raf_loop`]                          | every frame     | ✓ stops the loop |

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

// ---------------------------------------------------------------------------
// schedule_microtask — used by `build_switch` to defer screen swaps so the
// triggering click closure returns before any of its captured state is
// torn down.
// ---------------------------------------------------------------------------

/// Schedule `f` to run after the current synchronous stack unwinds —
/// a "microtask" in browser terms. Used by the framework to break
/// synchronous chains that would otherwise re-enter wasm-bindgen
/// `FnMut` closures (e.g. a click handler that triggers a screen
/// swap which drops the click's own button tree, then continues to
/// execute inside the now-destroyed closure).
///
/// Platform behavior:
/// - **wasm32:** uses `Promise.resolve().then(...)` so `f` runs on
///   the same event-loop turn but outside the current call stack.
/// - **Native (Android/iOS/desktop):** runs `f` synchronously. The
///   underlying re-entry hazard is wasm-specific (wasm-bindgen's
///   FnMut single-borrow check); on native there's no equivalent
///   trap and synchronous execution preserves ordering.
///
/// One-shot; no cancellation. Use [`after_animation_frame`] or
/// [`after_ms`] when you need a cancellable handle.
pub fn schedule_microtask<F: FnOnce() + 'static>(f: F) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::closure::Closure;
        let mut once: Option<F> = Some(f);
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
    #[cfg(not(target_arch = "wasm32"))]
    {
        f();
    }
}

// ---------------------------------------------------------------------------
// after_animation_frame — one-shot rAF with cancel-on-drop.
// ---------------------------------------------------------------------------

/// A scheduled one-shot callback. Cancels the pending browser
/// dispatch on `Drop`, then drops the closure. Hold the handle
/// somewhere as long as you want the callback alive; let it drop
/// to cancel.
///
/// On native this is an empty marker — the body ran synchronously
/// at construction.
pub struct ScheduledTask {
    #[cfg(target_arch = "wasm32")]
    inner: Option<Rc<RefCell<ScheduledTaskInner>>>,
}

#[cfg(target_arch = "wasm32")]
struct ScheduledTaskInner {
    handle: i32,
    kind: ScheduledKind,
    /// The Closure must outlive its scheduled dispatch. We hold it
    /// here so Drop can release it after the browser has been told
    /// to cancel.
    _closure: wasm_bindgen::closure::Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
enum ScheduledKind {
    AnimationFrame,
    Timeout,
}

#[cfg(target_arch = "wasm32")]
impl Drop for ScheduledTaskInner {
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
        // `_closure` drops here, after cancellation has been told.
    }
}

impl ScheduledTask {
    /// Manually cancel ahead of `Drop`. After calling, the task is
    /// a no-op handle. Convenient when the cancel point is
    /// elsewhere in the code (e.g. an `on_lost` callback) and
    /// dropping the field isn't the natural shape.
    pub fn cancel(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            self.inner = None;
        }
    }
}

/// Schedule `f` to run on the next animation frame. Returns a
/// handle whose `Drop` cancels the pending callback if it hasn't
/// fired yet.
///
/// Use this for: deferred mount work that depends on layout (e.g.
/// reading `clientWidth` after a freshly-inserted node has been
/// sized), one-frame-late effect application, etc.
///
/// **Cancellation matters even after the frame has fired** if you
/// dropped the handle in the same tick — the browser may have
/// already queued the closure for invocation but not yet
/// dispatched it. The wasm-bindgen `Closure` would otherwise be
/// destroyed while a queued dispatch is pending. See `RafLoop` for
/// the recurring variant.
///
/// On native: runs `f` synchronously and returns a no-op handle.
pub fn after_animation_frame<F: FnOnce() + 'static>(f: F) -> ScheduledTask {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;
        let window = match web_sys::window() {
            Some(w) => w,
            None => {
                f();
                return ScheduledTask { inner: None };
            }
        };
        let mut once: Option<F> = Some(f);
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            if let Some(g) = once.take() {
                g();
            }
        });
        let handle = match window
            .request_animation_frame(closure.as_ref().unchecked_ref())
        {
            Ok(h) => h,
            Err(_) => return ScheduledTask { inner: None },
        };
        ScheduledTask {
            inner: Some(Rc::new(RefCell::new(ScheduledTaskInner {
                handle,
                kind: ScheduledKind::AnimationFrame,
                _closure: closure,
            }))),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        f();
        ScheduledTask {}
    }
}

/// Schedule `f` to run after `delay_ms` milliseconds. Returns a
/// handle whose `Drop` cancels the pending callback.
///
/// On native: runs `f` synchronously (no real timer; the
/// cancellation contract is wasm-specific).
pub fn after_ms<F: FnOnce() + 'static>(delay_ms: i32, f: F) -> ScheduledTask {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;
        let window = match web_sys::window() {
            Some(w) => w,
            None => {
                f();
                return ScheduledTask { inner: None };
            }
        };
        let mut once: Option<F> = Some(f);
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
            Err(_) => return ScheduledTask { inner: None },
        };
        ScheduledTask {
            inner: Some(Rc::new(RefCell::new(ScheduledTaskInner {
                handle,
                kind: ScheduledKind::Timeout,
                _closure: closure,
            }))),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = delay_ms;
        f();
        ScheduledTask {}
    }
}

// ---------------------------------------------------------------------------
// raf_loop — recurring requestAnimationFrame with cancel-on-drop.
// ---------------------------------------------------------------------------

/// A live animation-frame loop. Each frame the user's closure runs;
/// returning, the helper auto-requests the next frame. Dropping
/// the handle cancels the currently-pending frame **and** stops
/// the auto-rearm — no further callbacks fire.
///
/// This is what the gradient demo's render loop wanted but had to
/// hand-roll. Author code now writes:
///
/// ```ignore
/// let loop_handle = framework_core::raf_loop(move || {
///     paint_one_frame();
/// });
/// // …store loop_handle in your renderer state…
/// ```
///
/// and the cancellation discipline (cancel rAF, drop closure) is
/// handled automatically.
///
/// On native: never fires; returns an inert handle. Hooking native
/// vsync would be a backend concern — the Graphics primitive's
/// platform-specific support is the natural place to wire that.
pub struct RafLoop {
    #[cfg(target_arch = "wasm32")]
    inner: Option<Rc<RefCell<RafLoopInner>>>,
}

#[cfg(target_arch = "wasm32")]
struct RafLoopInner {
    /// Pending frame handle, if any. `None` between the time the
    /// closure starts running and the time it requests the next
    /// frame at the end.
    pending: Option<i32>,
    /// The wasm-bindgen wrapper for the per-frame callback.
    /// Allocated lazily by the kickoff path; reused for every
    /// frame thereafter.
    closure: Option<wasm_bindgen::closure::Closure<dyn FnMut()>>,
    /// Set to `true` from Drop. The per-frame closure short-circuits
    /// on this flag so a callback already pulled off the JS queue
    /// becomes a no-op.
    cancelled: bool,
}

#[cfg(target_arch = "wasm32")]
impl Drop for RafLoopInner {
    fn drop(&mut self) {
        self.cancelled = true;
        if let (Some(h), Some(window)) = (self.pending.take(), web_sys::window()) {
            let _ = window.cancel_animation_frame(h);
        }
        // `closure` drops with `self`.
    }
}

impl RafLoop {
    /// Manually stop the loop ahead of `Drop`.
    pub fn cancel(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            self.inner = None;
        }
    }
}

/// Start a recurring animation-frame loop. The closure receives no
/// arguments; if it needs frame timing, it can read
/// `performance.now()` itself.
///
/// The closure is `FnMut` so it can hold mutable state across
/// frames (e.g. accumulated time).
pub fn raf_loop<F: FnMut() + 'static>(f: F) -> RafLoop {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;
        let window = match web_sys::window() {
            Some(w) => w,
            None => return RafLoop { inner: None },
        };
        let state = Rc::new(RefCell::new(RafLoopInner {
            pending: None,
            closure: None,
            cancelled: false,
        }));
        // Per-frame closure: invoke the user fn (under `RefCell`
        // tricks so we can re-arm without re-entering the borrow),
        // then request the next frame and stash its handle on
        // `state.pending` for cancellation.
        let weak_state = Rc::downgrade(&state);
        let user_fn = Rc::new(RefCell::new(f));
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            let Some(state) = weak_state.upgrade() else { return };
            {
                let s = state.borrow();
                if s.cancelled {
                    return;
                }
            }
            // Browser is about to fire the next frame; record that
            // there's no longer a pending callback.
            {
                let mut s = state.borrow_mut();
                s.pending = None;
            }
            // Invoke the user function outside any borrow on
            // `state` so the user is free to drop the RafLoop
            // handle from inside their own frame body if they want
            // to.
            {
                let mut f_borrow = user_fn.borrow_mut();
                (&mut *f_borrow)();
            }
            // Re-arm. If the user dropped the loop from within
            // their callback, `cancelled` is set and we skip.
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
        // Stash the closure on state so the per-frame body can
        // re-request itself.
        state.borrow_mut().closure = Some(closure);
        // Kick off the first frame. We can't hold a `state.borrow()`
        // across the request_animation_frame call AND then take a
        // `state.borrow_mut()` to record the handle — the immutable
        // borrow's temporary lifetime spans the whole if-let body
        // and overlaps the mut borrow, panicking deterministically.
        // Pull the JS function reference out into a local so the
        // borrow drops before we hit the schedule + write.
        let raf_fn = state
            .borrow()
            .closure
            .as_ref()
            .map(|c| c.as_ref().clone());
        if let Some(raf_fn) = raf_fn {
            if let Ok(h) = window.request_animation_frame(raf_fn.unchecked_ref()) {
                state.borrow_mut().pending = Some(h);
            }
        }
        RafLoop { inner: Some(state) }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = f;
        RafLoop {}
    }
}
