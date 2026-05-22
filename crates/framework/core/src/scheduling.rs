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
//! Without an installed [`Scheduler`]:
//! - On native targets, the helpers run their bodies synchronously
//!   (the wasm re-entry hazard is wasm-specific; on native there's
//!   no equivalent trap).
//! - On wasm32, the helpers panic with a configuration error.
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
/// Without an installed scheduler: synchronous on native, panic on
/// wasm32 (the re-entry hazard is wasm-specific; running
/// synchronously there would risk the very bug this exists to
/// avoid).
pub fn schedule_microtask<F: FnOnce() + 'static>(f: F) {
    if let Some(s) = SCHEDULER.get() {
        s.schedule_microtask(Box::new(f));
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        f();
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = f;
        panic!(
            "framework_core::scheduling::schedule_microtask: no Scheduler installed. \
             On wasm32 a backend must register one before this is called \
             — typically `backend_web::install_scheduler()` during host init."
        );
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
/// Without an installed scheduler: synchronous on native, panic on
/// wasm32.
pub fn after_animation_frame<F: FnOnce() + 'static>(f: F) -> ScheduledTask {
    if let Some(s) = SCHEDULER.get() {
        return ScheduledTask {
            inner: Some(s.after_animation_frame(Box::new(f))),
        };
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        f();
        ScheduledTask { inner: None }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = f;
        panic!(
            "framework_core::scheduling::after_animation_frame: no Scheduler installed. \
             Call `backend_web::install_scheduler()` during host init."
        );
    }
}

/// Schedule `f` to run after `delay_ms` milliseconds. Returns a
/// handle whose `Drop` cancels the pending callback.
///
/// Without an installed scheduler: synchronous on native (delay
/// ignored), panic on wasm32.
pub fn after_ms<F: FnOnce() + 'static>(delay_ms: i32, f: F) -> ScheduledTask {
    if let Some(s) = SCHEDULER.get() {
        return ScheduledTask {
            inner: Some(s.after_ms(delay_ms, Box::new(f))),
        };
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = delay_ms;
        f();
        ScheduledTask { inner: None }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (delay_ms, f);
        panic!(
            "framework_core::scheduling::after_ms: no Scheduler installed. \
             Call `backend_web::install_scheduler()` during host init."
        );
    }
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
pub fn raf_loop<F: FnMut() + 'static>(f: F) -> RafLoop {
    if let Some(s) = SCHEDULER.get() {
        return RafLoop {
            inner: Some(s.raf_loop(Box::new(f))),
        };
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = f;
        RafLoop { inner: None }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = f;
        panic!(
            "framework_core::scheduling::raf_loop: no Scheduler installed. \
             Call `backend_web::install_scheduler()` during host init."
        );
    }
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
/// lifetime (e.g., to cancel ahead of scope teardown, or to outlive
/// the scope by `mem::forget`'ing the handle).
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
/// plain [`after_ms`] and manage the handle yourself.
pub fn after_ms_scoped<F: FnOnce() + 'static>(delay_ms: i32, f: F) {
    let ctx = crate::reactive::capture_reactive_ctx();
    let task = after_ms(delay_ms, move || {
        crate::reactive::with_reactive_ctx(&ctx, f);
    });
    crate::reactive::on_cleanup(move || drop(task));
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
    let loop_handle = raf_loop(move || {
        crate::reactive::with_reactive_ctx(&ctx, || f());
    });
    crate::reactive::on_cleanup(move || drop(loop_handle));
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    //! Native-fallback path tests for the scheduling helpers.
    //!
    //! These tests exercise the path the framework takes on
    //! non-wasm targets when no platform [`Scheduler`] has been
    //! installed (which is the test-binary configuration —
    //! `install_scheduler` is a `OnceLock` and we deliberately
    //! never call it here so the unit tests see a clean state).
    //!
    //! The wasm-target panic branches aren't reachable on native
    //! and aren't covered by these tests; they're verified by
    //! eyeballing the matching `panic!` messages with the
    //! installer's call-site docs.

    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn schedule_microtask_runs_synchronously_without_scheduler() {
        // No scheduler installed -> synchronous on native. The
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
        // The wasm raf_loop is FnMut+self-rearming; on native
        // without a scheduler the body must NEVER fire (we have
        // no frame source).
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
        let mut task = after_animation_frame(|| {});
        // First cancel is a no-op (no inner handle on native).
        task.cancel();
        // Second cancel must not panic.
        task.cancel();
        // And drop must not panic either.
        drop(task);
    }

    #[test]
    fn raf_loop_cancel_is_idempotent_on_native_fallback() {
        let mut handle = raf_loop(|| {});
        handle.cancel();
        handle.cancel();
        drop(handle);
    }

    #[test]
    fn schedule_microtask_with_capture_runs_body_with_captured_values() {
        let cell = Rc::new(Cell::new(0));
        let cell_clone = cell.clone();
        schedule_microtask(move || {
            cell_clone.set(42);
        });
        assert_eq!(cell.get(), 42);
    }

    #[test]
    fn drop_of_scheduled_task_without_inner_does_not_panic() {
        // ScheduledTask construction on native (no scheduler)
        // never builds an inner handle; verify the Drop path is
        // benign.
        let task = after_ms(0, || {});
        drop(task); // should not panic
    }
}
