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
