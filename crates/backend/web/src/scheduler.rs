//! Web `Scheduler` on web-glue: `web_glue::queue_microtask` for
//! microtasks (one JS `queueMicrotask` per burst, drained in order),
//! `requestAnimationFrame` for one-shot frames + the recurring loop,
//! `setTimeout` for delayed callbacks. Each cancellable variant owns both
//! the browser handle and the glue `Closure` so `Drop` cancels the
//! browser-side dispatch *before* releasing the closure — a revoked
//! function the browser still calls throws "called after its Rust owner
//! dropped it".

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use runtime_shared::scheduling::{ScheduleHandle, Scheduler};
use web_glue::Closure;

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

/// Whether the SSR-hydration window is open (between
/// [`begin_hydration_buffering`] and [`end_hydration_buffering`], i.e. the
/// whole `hydrate → mount → drain → finish` span). Borrow-free (reads a
/// thread-local, not the backend), so navigator SDK code can consult it
/// while it already holds `&mut WebBackend` — unlike `Backend::is_hydrating`.
#[cfg(feature = "hydrate")]
pub(crate) fn is_hydration_active() -> bool {
    HYDRATION_BUFFER.with(|b| b.borrow().is_some())
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
    web_glue::queue_microtask(f);
}

/// Register this backend's scheduler with `runtime-core`. Idempotent —
/// first install wins.
///
/// Deliberately does NOT install the browser URL provider. That used to
/// be piggybacked here ("every web host calls `install_scheduler()`
/// anyway"), which made it unconditional — and
/// `url_provider::install_url_provider`'s popstate listener calls
/// `nav::handle_popstate`, so it kept `NavigatorControl::dispatch` plus
/// its `Rc` drop glue (10,827 bytes measured) alive in bundles that had
/// dropped the navigator primitives entirely. The install now rides
/// `BuiltinSet::nav_services` at the boot seam
/// ([`crate::newcore::start_in_with`] / [`crate::newcore_hydrate::hydrate_in_with`]),
/// next to `newcore_url_sync::install`, so a set without `nav` never
/// names it and LLVM drops the whole chain.
pub fn install_scheduler() {
    runtime_shared::scheduling::install_scheduler(Box::new(WebScheduler));
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
        let Some(window) = web_glue::dom::window() else {
            f();
            crate::dispatch_hook::fire_dispatch_hook();
            return Box::new(InertHandle);
        };
        let closure = Closure::once(move |_| {
            f();
            // One-shot frame callbacks can run author code that
            // stages new-core writes (animation ticks). See
            // `dispatch_hook` module docs.
            crate::dispatch_hook::fire_dispatch_hook();
        });
        let handle = window.request_animation_frame(&closure);
        Box::new(OneShotHandle {
            inner: Some(OneShotInner {
                window,
                handle,
                kind: ScheduledKind::AnimationFrame,
                _closure: closure,
            }),
        })
    }

    fn after_ms(
        &self,
        delay_ms: i32,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        let Some(window) = web_glue::dom::window() else {
            f();
            crate::dispatch_hook::fire_dispatch_hook();
            return Box::new(InertHandle);
        };
        let closure = Closure::once(move |_| {
            f();
            // Timer callbacks are a primary author-code surface
            // (`after_ms` bodies that set signals) — the flush hook
            // is what commits those writes on the new core. See
            // `dispatch_hook` module docs.
            crate::dispatch_hook::fire_dispatch_hook();
        });
        let handle = window.set_timeout(&closure, delay_ms);
        Box::new(OneShotHandle {
            inner: Some(OneShotInner {
                window,
                handle,
                kind: ScheduledKind::Timeout,
                _closure: closure,
            }),
        })
    }

    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        let Some(window) = web_glue::dom::window() else {
            return Box::new(InertHandle);
        };
        let state = Rc::new(RefCell::new(RafLoopInner {
            window: window.clone(),
            pending: None,
            closure: None,
            cancelled: false,
        }));
        let weak_state = Rc::downgrade(&state);
        let mut user_fn = f;
        let closure = Closure::new(move |_| {
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
            // their own frame body. (The glue registry takes this
            // closure out while it runs, so dropping the handle here
            // only defers the closure's own release to the return.)
            user_fn();
            // rAF-loop iterations can run author code that stages
            // new-core writes (frame-paced drag/scroll state). See
            // `dispatch_hook` module docs.
            crate::dispatch_hook::fire_dispatch_hook();
            let mut s = state.borrow_mut();
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
        Box::new(RafLoopHandle { inner: Some(state) })
    }
}

// ---------------------------------------------------------------------------
// One-shot handle (after_animation_frame, after_ms)
// ---------------------------------------------------------------------------

struct OneShotHandle {
    inner: Option<OneShotInner>,
}

struct OneShotInner {
    window: web_glue::dom::Window,
    /// The host's frame / timer id, verbatim (an `f64`: see
    /// `web_glue::dom::Window::request_animation_frame`).
    handle: f64,
    kind: ScheduledKind,
    /// The Closure must outlive its scheduled dispatch. Held here so
    /// Drop can release it *after* the browser has been told to cancel
    /// (fields drop after `Drop::drop` runs).
    _closure: Closure,
}

enum ScheduledKind {
    AnimationFrame,
    Timeout,
}

impl Drop for OneShotInner {
    fn drop(&mut self) {
        match self.kind {
            ScheduledKind::AnimationFrame => self.window.cancel_animation_frame(self.handle),
            ScheduledKind::Timeout => self.window.clear_timeout(self.handle),
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
    window: web_glue::dom::Window,
    pending: Option<f64>,
    closure: Option<Closure>,
    cancelled: bool,
}

impl Drop for RafLoopInner {
    fn drop(&mut self) {
        self.cancelled = true;
        if let Some(h) = self.pending.take() {
            self.window.cancel_animation_frame(h);
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

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;
    use web_glue::js::Function;
    use web_glue::JsValue;

    wasm_bindgen_test_configure!(run_in_browser);

    fn eval(body: &str) -> JsValue {
        Function::new_no_args(body).call0(&JsValue::UNDEFINED).unwrap()
    }

    /// Regression: every cancellable schedule (one-shot frame, timeout,
    /// rAF loop) must reach the host's timer under a fake clock that numbers
    /// its timers from `1e12` — Playwright's `page.clock`, which the
    /// CrewForge kiosk e2e pins with `setFixedTime`. The handles crossed as
    /// `i32`, so cancel named a truncated id, the timer stayed queued, and
    /// it fired into the dropped `Closure` ("called after its Rust owner
    /// dropped it"). Cancelling each must leave the fake clock empty, and
    /// running whatever is left must not throw.
    #[wasm_bindgen_test]
    fn regression_cancel_reaches_fake_clock_timers_past_i32_range() {
        eval(
            "const w = window; \
             const real = { raf: w.requestAnimationFrame, caf: w.cancelAnimationFrame, \
                            st: w.setTimeout, ct: w.clearTimeout }; \
             let next = 1e12; const q = new Map(); \
             const add = (f) => { const id = next++; q.set(id, f); return id; }; \
             const del = (id) => { q.delete(Number(id)); }; \
             w.requestAnimationFrame = add; w.cancelAnimationFrame = del; \
             w.setTimeout = (f) => add(f); w.clearTimeout = del; \
             w.__fakeClock = { \
               pending: () => q.size, \
               run: () => { const errs = []; const due = Array.from(q.values()); q.clear(); \
                            for (const f of due) { try { f(0); } catch (e) { errs.push(String(e.message || e)); } } \
                            return errs.join('\\n'); }, \
               restore: () => { w.requestAnimationFrame = real.raf; w.cancelAnimationFrame = real.caf; \
                                w.setTimeout = real.st; w.clearTimeout = real.ct; delete w.__fakeClock; }, \
             };",
        );
        let ran = Rc::new(std::cell::Cell::new(0));
        let (a, b, c) = (ran.clone(), ran.clone(), ran.clone());
        let mut frame = WebScheduler.after_animation_frame(Box::new(move || a.set(a.get() + 1)));
        let mut timeout = WebScheduler.after_ms(10, Box::new(move || b.set(b.get() + 1)));
        let mut lp = WebScheduler.raf_loop(Box::new(move || c.set(c.get() + 1)));
        let queued = eval("return window.__fakeClock.pending();").as_f64();
        frame.cancel();
        timeout.cancel();
        lp.cancel();
        let pending = eval("return window.__fakeClock.pending();").as_f64();
        let errors = eval("return window.__fakeClock.run();").as_string().unwrap_or_default();
        eval("window.__fakeClock.restore();");

        assert_eq!(queued, Some(3.0), "frame + timeout + loop each queue one timer");
        assert_eq!(pending, Some(0.0), "cancel must remove each from the host's clock");
        assert_eq!(errors, "", "nothing may fire into a dropped closure");
        assert_eq!(ran.get(), 0);
    }
}
