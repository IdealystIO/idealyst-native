//! Platform-agnostic scheduling primitives.
//!
//! All four helpers solve a single shape of bug:
//!
//! > A user-provided closure is queued with the browser (microtask,
//! > `requestAnimationFrame`, `setTimeout`). The closure's owner is
//! > dropped before the browser fires it. The browser still
//! > dispatches. wasm-bindgen sees a destroyed `Closure` and panics
//! > "closure invoked recursively or after being dropped".
//!
//! The helpers own both the closure handle AND the browser handle,
//! and on `Drop`:
//!
//! 1. Cancel the browser-side scheduling (via `cancelAnimationFrame`
//!    / `clearTimeout`). The browser drops its queued reference.
//! 2. Drop the wasm-bindgen `Closure`. No spurious invocations.
//!
//! The web platform implementation lives in `backend-web`. Hosts
//! register it via `backend_web::install_scheduler()` during init.
//! Without an installed [`Scheduler`], behaviour is keyed on the
//! runtime [`Platform`](crate::Platform) — **not** `#[cfg(target_arch)]`,
//! so this module carries no compile-target switch:
//! - On `Web`, the helpers panic with a configuration error. The
//!   deferral is mandatory there; a synchronous fallback would trip
//!   the wasm-bindgen re-entry hazard described above.
//! - On every other platform, the helpers run their bodies
//!   synchronously (or inert, for [`raf_loop`]) — there's no
//!   equivalent trap off the web.
//!
//! # Quick reference
//!
//! | Helper                                | Fires           | Cancel on drop |
//! |---------------------------------------|-----------------|----------------|
//! | [`schedule_microtask`]                | once, next tick | n/a (one-shot, fire-and-forget) |
//! | [`after_animation_frame`]             | once, next frame| ✓ |
//! | [`after_ms`]                          | once, after delay | ✓ |
//! | [`raf_loop`]                          | every frame     | ✓ stops the loop |

use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Trait + registry
// ---------------------------------------------------------------------------

/// Backend-supplied scheduler. The web backend installs an impl
/// against `Promise.then` / `requestAnimationFrame` / `setTimeout`;
/// hosts register it once at init via [`install_scheduler`].
pub trait Scheduler: Send + Sync {
    /// Fire-and-forget. Schedule `f` to run after the current
    /// synchronous stack unwinds.
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>);

    /// One-shot, cancellable. Schedule `f` on the next animation
    /// frame; the returned handle's `Drop` cancels if not yet fired.
    fn after_animation_frame(
        &self,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle>;

    /// One-shot, cancellable. Schedule `f` after `delay_ms`
    /// milliseconds; the returned handle's `Drop` cancels.
    fn after_ms(
        &self,
        delay_ms: i32,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle>;

    /// Recurring. Run `f` every animation frame; the handle's `Drop`
    /// stops the loop.
    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle>;

    /// Synchronously run every microtask the scheduler has *buffered*
    /// (rather than dispatched), draining until empty. Default no-op;
    /// only the web backend buffers, during the SSR hydration window —
    /// see [`drain_buffered_microtasks`].
    fn drain_buffered_microtasks(&self) {}
}

/// Opaque handle returned by cancellable scheduler methods. Its
/// `Drop` impl cancels the pending dispatch; `cancel` is the
/// explicit method (idempotent — second call is a no-op).
pub trait ScheduleHandle: 'static {
    fn cancel(&mut self);
}

static SCHEDULER: OnceLock<Box<dyn Scheduler>> = OnceLock::new();

/// Register the active backend's scheduler. First call wins;
/// subsequent calls are silently ignored.
pub fn install_scheduler(scheduler: Box<dyn Scheduler>) {
    let _ = SCHEDULER.set(scheduler);
}

/// Returns `true` if a backend has installed a real scheduler. Useful
/// for self-rescheduling helpers (`schedule_periodic_poll`, animation
/// loops) that would otherwise infinite-recurse via the synchronous
/// native fallback when no scheduler is installed — they can bail
/// instead.
pub fn is_scheduler_installed() -> bool {
    SCHEDULER.get().is_some()
}

/// Panic when no [`Scheduler`] is installed *and* we're rendering to
/// `Web`. On Web the deferral these helpers provide is mandatory — a
/// synchronous fallback would re-enter wasm-bindgen `FnMut` closures
/// and trip "closure invoked recursively or after being dropped" — so
/// a missing install is a configuration error surfaced loudly. Off the
/// web there's no such trap, so callers fall through to their
/// synchronous (or inert) fallback.
///
/// Keyed on the runtime [`Platform`](crate::Platform), not
/// `#[cfg(target_arch = "wasm32")]`, so core stays free of a
/// compile-target switch. `platform()` is `Web` only once `mount`
/// installs it from the backend; the web bootstrap registers the real
/// scheduler before `mount`, so by the time a helper runs, `Web` with
/// no scheduler means the host genuinely forgot to register one.
fn panic_if_web_without_scheduler(api: &str) {
    if crate::platform() == crate::Platform::Web {
        panic!(
            "runtime_core::scheduling::{api}: no Scheduler installed. \
             On Web a backend must register one before this is called \
             — typically `backend_web::install_scheduler()` during host init."
        );
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Schedule `f` to run after the current synchronous stack unwinds —
/// a "microtask" in browser terms. Used by the framework to break
/// synchronous chains that would otherwise re-enter wasm-bindgen
/// `FnMut` closures (e.g. a click handler that triggers a screen
/// swap which drops the click's own button tree, then continues to
/// execute inside the now-destroyed closure).
///
/// Without an installed scheduler: panic on `Web`, synchronous on
/// every other platform (the re-entry hazard is web-specific; running
/// synchronously there would risk the very bug this exists to avoid).
pub fn schedule_microtask<F: FnOnce() + 'static>(f: F) {
    if let Some(s) = SCHEDULER.get() {
        s.schedule_microtask(Box::new(f));
        return;
    }
    panic_if_web_without_scheduler("schedule_microtask");
    f();
}

/// Synchronously drain microtasks the installed scheduler buffered (see
/// [`Scheduler::drain_buffered_microtasks`]). No-op without a scheduler
/// or when none are buffered. Called by [`mount`](crate::mount) during
/// SSR hydration to run the navigator SDK's deferred chrome/screen builds
/// *inside* the adoption window — so they adopt the server's DOM instead
/// of firing post-`finish` and rebuilding fresh.
pub fn drain_buffered_microtasks() {
    if let Some(s) = SCHEDULER.get() {
        s.drain_buffered_microtasks();
    }
}

/// A scheduled one-shot callback. Cancels the pending dispatch on
/// `Drop`, then drops the closure. Hold the handle somewhere as long
/// as you want the callback alive; let it drop to cancel.
///
/// On native without an installed scheduler this is an empty marker
/// — the body ran synchronously at construction.
pub struct ScheduledTask {
    inner: Option<Box<dyn ScheduleHandle>>,
}

impl ScheduledTask {
    /// Manually cancel ahead of `Drop`. After calling, the task is a
    /// no-op handle.
    pub fn cancel(&mut self) {
        if let Some(mut h) = self.inner.take() {
            h.cancel();
        }
    }
}

/// Schedule `f` to run on the next animation frame. Returns a
/// handle whose `Drop` cancels the pending callback if it hasn't
/// fired yet.
///
/// Without an installed scheduler: panic on `Web`, synchronous on
/// every other platform.
pub fn after_animation_frame<F: FnOnce() + 'static>(f: F) -> ScheduledTask {
    if let Some(s) = SCHEDULER.get() {
        return ScheduledTask {
            inner: Some(s.after_animation_frame(Box::new(f))),
        };
    }
    panic_if_web_without_scheduler("after_animation_frame");
    f();
    ScheduledTask { inner: None }
}

/// Schedule `f` to run after `delay_ms` milliseconds. Returns a
/// handle whose `Drop` cancels the pending callback.
///
/// Without an installed scheduler: panic on `Web`, synchronous on
/// every other platform (delay ignored).
pub fn after_ms<F: FnOnce() + 'static>(delay_ms: i32, f: F) -> ScheduledTask {
    if let Some(s) = SCHEDULER.get() {
        return ScheduledTask {
            inner: Some(s.after_ms(delay_ms, Box::new(f))),
        };
    }
    panic_if_web_without_scheduler("after_ms");
    let _ = delay_ms; // synchronous fallback ignores the delay
    f();
    ScheduledTask { inner: None }
}

thread_local! {
    /// Parking lot for tasks handed to the runtime via
    /// [`after_ms_detached`]. Each slot holds the [`ScheduledTask`] so its
    /// cancel-on-`Drop` doesn't fire, plus a `done` flag the task sets when
    /// it runs. The list is swept on every `after_ms_detached` call, so it
    /// never grows past the number of *in-flight* detached timers — unlike
    /// `mem::forget`, which leaks an empty handle shell per call forever.
    static DETACHED_TASKS: std::cell::RefCell<Vec<DetachedSlot>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

struct DetachedSlot {
    done: std::rc::Rc<std::cell::Cell<bool>>,
    // Held only to keep the pending dispatch alive; never read.
    _task: ScheduledTask,
}

/// Schedule `f` to run once after `delay_ms`, owned by the runtime
/// instead of the caller. Unlike [`after_ms`] there is no handle to keep
/// alive and nothing to cancel: the task is parked in a runtime registry,
/// fires once, then is swept away.
///
/// This is the right tool for **imperative, off-scope** call sites —
/// global queues, app singletons, event handlers that run outside any
/// component body — where [`after_ms_scoped`] can't apply because there
/// is no surrounding reactive scope to anchor to. It is the named,
/// self-documenting replacement for the `mem::forget(after_ms(..))`
/// idiom: same observable behaviour, but bounded (the registry sweeps
/// fired tasks) and impossible to mistake for a bug. Library and app code
/// should reach for this rather than `mem::forget`.
///
/// If a reactive scope *is* active, prefer [`after_ms_scoped`] so the
/// timer dies with the scope instead of outliving it.
pub fn after_ms_detached<F: FnOnce() + 'static>(delay_ms: i32, f: F) {
    // Sweep tasks that already fired. We're outside any task closure here,
    // so dropping a spent `ScheduledTask` is safe — it never re-enters or
    // drops a handle whose closure is still on the stack (the web hazard).
    DETACHED_TASKS.with(|t| t.borrow_mut().retain(|s| !s.done.get()));

    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    let done_for_cb = done.clone();
    let task = after_ms(delay_ms, move || {
        f();
        // Mark for sweep. This only writes a `Cell` — it drops nothing, so
        // the task's own handle is never freed while its closure runs.
        done_for_cb.set(true);
    });

    // The synchronous-fallback path (no installed scheduler off web) ran
    // `f` already and set `done`; there's nothing to park.
    if done.get() {
        return;
    }
    DETACHED_TASKS.with(|t| t.borrow_mut().push(DetachedSlot { done, _task: task }));
}

/// A live animation-frame loop. Each frame the user's closure runs;
/// returning, the helper auto-requests the next frame. Dropping the
/// handle cancels the currently-pending frame **and** stops the
/// auto-rearm — no further callbacks fire.
///
/// Without an installed scheduler: inert on native (closure never
/// fires; returns a no-op handle), panic on wasm32.
pub struct RafLoop {
    inner: Option<Box<dyn ScheduleHandle>>,
}

impl RafLoop {
    /// Manually stop the loop ahead of `Drop`.
    pub fn cancel(&mut self) {
        if let Some(mut h) = self.inner.take() {
            h.cancel();
        }
    }
}

/// Start a recurring animation-frame loop. The closure receives no
/// arguments; if it needs frame timing, it can read
/// `performance.now()` itself.
///
/// The closure is `FnMut` so it can hold mutable state across
/// frames.
// ---------------------------------------------------------------------------
// Frame-active flag — cooperation between rendering hosts and
// per-frame author code (animation tickers, etc).
// ---------------------------------------------------------------------------
//
// Default `true`. Render hosts (e.g. `host-ios-mobile`'s `draw_frame`)
// set it `false` when their per-frame loop short-circuits because the
// hosted surface isn't visible (e.g. a UIKit ancestor was
// `setHidden:true`). Author-side `raf_loop_scoped` consumers that
// drive animation state read [`is_frame_active`] as a cheap "should I
// do work this frame?" hint and bail early when false — pausing
// CPU-side animation advancement without dropping the reactive scope.
//
// Thread-local, not global: a second wgpu host on the same thread
// would share this flag (acceptable for the website's single-Simulator
// case; revisit when multi-host setups arrive). Lives in this module
// rather than `driver` so it's accessible without the `async-driver`
// feature, which not every embed turns on.

thread_local! {
    static FRAME_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// `true` when render hosts on this thread report their surface as
/// visible / actively painting. Consumers that drive per-frame
/// animation state read this to skip work when nothing's painting —
/// see the module-level comment for the cooperation pattern.
pub fn is_frame_active() -> bool {
    FRAME_ACTIVE.with(|c| c.get())
}

/// Set the per-thread frame-active flag. Render hosts call this at
/// the top of their per-frame closure (`true` before normal paint,
/// `false` before a visibility-gated skip). Default `true` so author
/// code without an installed host sees the optimistic "go" answer.
pub fn set_frame_active(active: bool) {
    FRAME_ACTIVE.with(|c| c.set(active));
}

// NOTE: end of historic diag-counter block — counters removed once
// the renderer-walk-no-op investigation isolated the apple-core
// `after_ms_inner` 0-delay cancellation bug; the fix + regression
// test live in `crates/backend/apple/core/src/scheduler.rs`.

// ---------------------------------------------------------------------------
// Cross-platform debug log shim. Native hosts (apple-core, android-core)
// install a closure that routes through their platform-native log
// channel (`NSLog`, `__android_log_print`) so author-side
// `runtime_core::debug_log(...)` calls surface in `xcrun simctl spawn
// log show` / logcat. Off-host this is a no-op (defaults to a closure
// that discards the message), keeping the call cheap. Intended for
// diagnostic instrumentation that needs to cross author → backend
// boundaries without a per-platform dep at the call site.
// ---------------------------------------------------------------------------

thread_local! {
    static DEBUG_LOG: std::cell::RefCell<Option<Box<dyn Fn(&str)>>> =
        const { std::cell::RefCell::new(None) };
}

pub fn install_debug_log(f: Box<dyn Fn(&str)>) {
    DEBUG_LOG.with(|c| {
        if c.borrow().is_none() {
            *c.borrow_mut() = Some(f);
        }
    });
}

pub fn debug_log(msg: &str) {
    DEBUG_LOG.with(|c| {
        if let Some(ref f) = *c.borrow() {
            f(msg);
        }
    });
}

pub fn raf_loop<F: FnMut() + 'static>(f: F) -> RafLoop {
    if let Some(s) = SCHEDULER.get() {
        return RafLoop {
            inner: Some(s.raf_loop(Box::new(f))),
        };
    }
    panic_if_web_without_scheduler("raf_loop");
    let _ = f; // inert off the web: no frame source without a scheduler
    RafLoop { inner: None }
}

// ---------------------------------------------------------------------------
// Scope-anchored variants
// ---------------------------------------------------------------------------
//
// The plain helpers ([`after_ms`], [`raf_loop`]) return a handle whose
// `Drop` cancels the underlying timer. Callers are expected to keep
// the handle alive for as long as they want the callback firing —
// the most common way is `std::mem::forget(handle)` for page-
// lifetime use, or stashing it in a `Vec` plus
// `on_cleanup(move || drop(vec))` for scope-lifetime use.
//
// The scoped variants below absorb that boilerplate: they install
// the same cleanup automatically against the current reactive scope.
// Use them inside `effect!` / inside a component body / under a
// mounted `Owner` when the animation's natural lifetime is the
// scope's. Outside any scope they degrade to the plain helpers
// (the on_cleanup no-ops, the handle is leaked) — which matches
// `on_cleanup`'s standard behavior.

/// Schedule `f` to fire after `delay_ms`, with the underlying timer
/// anchored to the current reactive scope. When the scope cleans up
/// (the surrounding `effect!` re-runs, or the `Owner` drops) the
/// timer is cancelled — if the callback hasn't fired yet, it never
/// will.
///
/// Use this instead of [`after_ms`] when the timer is part of an
/// animation that should die with the surrounding scope. Use plain
/// [`after_ms`] when you need manual control over the handle's
/// lifetime (e.g., to cancel ahead of scope teardown). To outlive the
/// scope from an off-scope call site, use [`after_ms_detached`] rather
/// than `mem::forget`'ing the handle.
///
/// The deferred callback re-enters the registering scope when it
/// fires, so a nested `*_scoped` helper inside `f` attaches to the
/// same scope as the outer call — otherwise the inner cleanup would
/// see an empty active stack and silently cancel itself before
/// firing.
///
/// **Outside any reactive scope this is a no-op** — the captured
/// task is dropped immediately, mirroring how [`crate::on_cleanup`]
/// silently drops its callback outside a scope. If you need a
/// timer that fires regardless of whether you're in a scope, use
/// [`after_ms_detached`].
pub fn after_ms_scoped<F: FnOnce() + 'static>(delay_ms: i32, f: F) {
    let ctx = crate::reactive::capture_reactive_ctx();
    // Teardown-safety flag (see module doc above `after_ms_scoped` and
    // the `scheduling_scoped` regression). `cancelAnimationFrame` /
    // `clearTimeout` do NOT reliably remove a callback the browser has
    // already dispatched for the current tick, so dropping the handle on
    // scope cleanup is not enough — a queued shot can still fire after
    // the owning scope's signals were dropped (use-after-drop) or while
    // the navigator's mount holds the reactive arena mid-mutation
    // (re-entrancy). The shared flag, set by the `on_cleanup` below and
    // checked at the very top of the callback, guarantees the body never
    // runs once cleanup has happened.
    let cancelled = std::rc::Rc::new(std::cell::Cell::new(false));
    let cancelled_for_cb = cancelled.clone();
    let task = after_ms(delay_ms, move || {
        if cancelled_for_cb.get() {
            return;
        }
        // Re-entrancy guard: if the reactive arena is mid-mutation
        // (an effect body or a `with_signal_mut` window is in flight),
        // touching a signal here would panic. A one-shot can't re-arm,
        // so we drop this late dispatch — the owning context is being
        // mutated out from under us, which in practice means it's about
        // to tear this scope down anyway.
        if crate::reactive::is_reactive_busy() {
            return;
        }
        // One reactive cycle per timer fire: writes inside `f` coalesce
        // into a single fan-out. See `reactive::cycle`.
        crate::reactive::with_reactive_ctx(&ctx, || crate::cycle(f));
    });
    crate::reactive::on_cleanup(move || {
        cancelled.set(true);
        drop(task);
    });
}

/// Recurring animation-frame loop, anchored to the current reactive
/// scope. When the scope cleans up the loop stops; further frames
/// never fire.
///
/// Companion to [`raf_loop`] that doesn't make the caller choose
/// between `mem::forget`'ing the handle (silent page-lifetime leak)
/// and manually wiring `on_cleanup(move || drop(handle))`.
///
/// Per-frame invocations re-enter the registering scope so nested
/// `*_scoped` calls keep their cleanup attachment (see
/// [`after_ms_scoped`] for the rationale).
///
/// **Outside any reactive scope this is a no-op** — see
/// [`after_ms_scoped`] for the rationale.
pub fn raf_loop_scoped<F: FnMut() + 'static>(mut f: F) {
    let ctx = crate::reactive::capture_reactive_ctx();
    // Teardown-safety flag — see `after_ms_scoped` for the full
    // rationale. This is the case the QuillEMR notetaker teardown-race
    // exercised: a persistent per-frame loop reading a screen-scoped
    // signal, where the navigator releases the screen scope while a rAF
    // for THIS frame is already queued. Without the flag the queued
    // frame fires after the scope's signals were dropped (panics #2/#3:
    // "signal used after its scope was dropped" / "signal type
    // mismatch"); without the busy-skip it can re-enter the arena while
    // the navigator mount holds it mid-mutation (panic #1: "RefCell
    // already mutably borrowed" / taken-slot None).
    let cancelled = std::rc::Rc::new(std::cell::Cell::new(false));
    let cancelled_for_cb = cancelled.clone();
    let loop_handle = raf_loop(move || {
        if cancelled_for_cb.get() {
            return;
        }
        // Mid-mutation arena: skip this frame and let the loop re-arm.
        // The next frame, after the in-flight effect/mount has finished,
        // will run the body normally (unless the scope was torn down, in
        // which case `cancelled` is now set and we bail above).
        if crate::reactive::is_reactive_busy() {
            return;
        }
        // One reactive cycle per frame: all writes this frame coalesce
        // into a single fan-out. See `reactive::cycle`.
        crate::reactive::with_reactive_ctx(&ctx, || crate::cycle(|| f()));
    });
    crate::reactive::on_cleanup(move || {
        cancelled.set(true);
        drop(loop_handle);
    });
}

// Gated to non-wasm: these tests use `std::thread`/`std::panic`, and
// the no-scheduler fallback they exercise is now keyed on the runtime
// `Platform` (not the compile target), so they run fully on the host.
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    //! No-scheduler fallback tests for the scheduling helpers.
    //!
    //! The fallback behaviour is keyed on the runtime
    //! [`Platform`](crate::Platform): panic on `Web`, synchronous (or
    //! inert) elsewhere. These tests pin the platform explicitly via
    //! [`non_web`] / set `Web` rather than relying on the compile
    //! target. `install_scheduler` is a process-wide `OnceLock` we
    //! deliberately never fill here so the unit tests see a clean state
    //! (the `Web` panic test skips if some other test in the binary
    //! installed one).

    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// Pin the thread-local platform to a native value. `platform()` is
    /// a thread-local another test on this worker thread may have left
    /// as `Web`; pinning it makes the no-scheduler *sync* fallback
    /// deterministic regardless of test order.
    fn non_web() {
        crate::backend::install_current_platform(crate::Platform::Ios);
    }

    #[test]
    fn no_scheduler_on_web_panics() {
        // The deferral is mandatory on Web; a missing install must fail
        // loudly. Run on a dedicated thread so the `Web` we set can't
        // leak into other tests' thread-locals.
        std::thread::spawn(|| {
            // The panic path needs no installed scheduler; the
            // process-wide OnceLock may already hold one from another
            // test, in which case the call routes there — skip.
            if is_scheduler_installed() {
                return;
            }
            crate::backend::install_current_platform(crate::Platform::Web);
            let r = std::panic::catch_unwind(|| schedule_microtask(|| {}));
            assert!(
                r.is_err(),
                "Web + no scheduler must panic loudly (deferral is mandatory there)",
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn schedule_microtask_runs_synchronously_without_scheduler() {
        non_web();
        // No scheduler installed -> synchronous off the web. The
        // closure must fire by the time `schedule_microtask`
        // returns.
        let fired = Rc::new(Cell::new(false));
        let fired_for_closure = fired.clone();
        schedule_microtask(move || {
            fired_for_closure.set(true);
        });
        assert!(
            fired.get(),
            "schedule_microtask should run synchronously on native without an installed scheduler",
        );
    }

    #[test]
    fn after_animation_frame_runs_synchronously_without_scheduler() {
        non_web();
        let fired = Rc::new(Cell::new(false));
        let fired_for_closure = fired.clone();
        let task = after_animation_frame(move || {
            fired_for_closure.set(true);
        });
        assert!(
            fired.get(),
            "after_animation_frame should fire synchronously on native without a scheduler",
        );
        // The returned ScheduledTask should hold no inner handle
        // since the body already ran.
        assert!(
            task.inner.is_none(),
            "native-fallback ScheduledTask should have no inner handle",
        );
    }

    #[test]
    fn after_ms_runs_synchronously_without_scheduler() {
        non_web();
        let fired = Rc::new(Cell::new(false));
        let fired_for_closure = fired.clone();
        // Big delay — synchronous fallback should ignore it.
        let task = after_ms(10_000, move || {
            fired_for_closure.set(true);
        });
        assert!(
            fired.get(),
            "after_ms should fire synchronously on native without a scheduler (delay ignored)",
        );
        assert!(task.inner.is_none());
    }

    #[test]
    fn raf_loop_is_inert_on_native_without_scheduler() {
        non_web();
        // `raf_loop` is FnMut + self-rearming; off the web without a
        // scheduler the body must NEVER fire (we have no frame source).
        let fired = Rc::new(Cell::new(0u32));
        let fired_for_closure = fired.clone();
        let _loop_handle = raf_loop(move || {
            fired_for_closure.set(fired_for_closure.get() + 1);
        });
        assert_eq!(
            fired.get(),
            0,
            "raf_loop body should not run on native without a scheduler",
        );
    }

    #[test]
    fn scheduled_task_cancel_is_idempotent_on_native_fallback() {
        non_web();
        let mut task = after_animation_frame(|| {});
        // First cancel is a no-op (no inner handle off the web).
        task.cancel();
        // Second cancel must not panic.
        task.cancel();
        // And drop must not panic either.
        drop(task);
    }

    #[test]
    fn raf_loop_cancel_is_idempotent_on_native_fallback() {
        non_web();
        let mut handle = raf_loop(|| {});
        handle.cancel();
        handle.cancel();
        drop(handle);
    }

    #[test]
    fn schedule_microtask_with_capture_runs_body_with_captured_values() {
        non_web();
        let cell = Rc::new(Cell::new(0));
        let cell_clone = cell.clone();
        schedule_microtask(move || {
            cell_clone.set(42);
        });
        assert_eq!(cell.get(), 42);
    }

    #[test]
    fn drop_of_scheduled_task_without_inner_does_not_panic() {
        non_web();
        // ScheduledTask construction without a scheduler never builds
        // an inner handle; verify the Drop path is benign.
        let task = after_ms(0, || {});
        drop(task); // should not panic
    }
}
