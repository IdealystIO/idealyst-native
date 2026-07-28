//! Post-dispatch hook: a thread-local `fn()` slot fired by backend
//! infrastructure **after** it has invoked a callback that may run
//! author code (scheduler timers, one-shot frames, raf-loop
//! iterations, executor-spawned future polls). Android port of
//! `backend-web/src/dispatch_hook.rs` — keep the two in sync.
//!
//! # Why this exists (the bug prevented)
//!
//! The new core ([`crate::newcore`]) stages signal writes; nothing is
//! observable until the flush driver calls `World::flush`. JNI event
//! callbacks (click / touch / text-change / toggle / slider / key /
//! scroll — everything the Kotlin runtime listeners trampoline into)
//! get their flush from the dispatch-site glue in `newcore.rs`: every
//! author callback the backend installs is wrapped to schedule a flush
//! when it returns, so the shared `imp` event closures never change.
//! But author code also runs from surfaces that are *not* input
//! events:
//!
//! - `runtime_core::scheduling::after_ms` timers (a debounce that sets
//!   a signal; the smoke app's self-test),
//! - one-shot `after_animation_frame` callbacks and `raf_loop`
//!   iterations (animation ticks that stage writes),
//! - futures polled by [`crate::imp::async_executor`]
//!   (resource/server-call completions that set signals).
//!
//! Without this hook, a write staged from any of those would sit
//! uncommitted until some unrelated input event happened to trigger a
//! flush — i.e. "my `after_ms` callback ran but the UI never updated".
//! The old-core reactive system applies writes synchronously and never
//! needs this, so the slot defaults to `None` and every fire site is a
//! single thread-local read — a no-op unless `newcore::start` installed
//! the flush driver.
//!
//! # What deliberately does NOT fire the hook
//!
//! - **Scheduled microtasks** (`schedule_microtask`, i.e.
//!   `Handler.post(delay 0)`). The flush itself is dispatched as a
//!   scheduled microtask; firing the hook after every microtask would
//!   re-schedule a flush from inside the flush's own dispatch and
//!   spin the main looper forever (each fire posting a fresh
//!   runnable). No author code reaches the microtask queue outside an
//!   already-hooked surface (events, timers, frames, future polls),
//!   so nothing is lost. On Android this exclusion matters doubly:
//!   microtasks and timers share ONE dispatch trampoline
//!   (`RustScheduledRunnable_nativeInvoke`), so the hook is fired by
//!   wrapping the closure at the `Scheduler` impl's `after_ms` /
//!   `after_animation_frame` / `raf_loop` sites — never inside the
//!   shared trampoline.
//! - **The Choreographer layout-pass frame callback**
//!   (`scheduler::schedule_frame_callback`): it only runs the
//!   backend's own Taffy pass, no author code.
//!
//! # TLS note (bionic 128-key budget)
//!
//! `Cell<Option<fn()>>` with a `const` initializer has no destructor,
//! so this `thread_local!` lowers to a plain `#[thread_local]` ELF-TLS
//! slot — it does NOT consume one of bionic's 128 pthread TLS keys
//! (only dtor-bearing thread-locals register a key; see
//! [[project_android_tls_key_limit_stylesheets]]). Old-core builds
//! therefore pay nothing for this module existing.

use std::cell::Cell;

thread_local! {
    static HOOK: Cell<Option<fn()>> = const { Cell::new(None) };
}

/// Install the post-dispatch hook (replaces any previous one).
/// `newcore::start` installs `newcore::schedule_flush` here.
pub fn install_dispatch_hook(f: fn()) {
    HOOK.with(|h| h.set(Some(f)));
}

/// Remove the hook (used by `newcore::stop`). Fire sites revert to
/// no-ops.
pub fn clear_dispatch_hook() {
    HOOK.with(|h| h.set(None));
}

/// Fire the hook if installed. Called by backend infrastructure right
/// after a potentially-author-code callback returns. Cheap when no hook
/// is installed (one thread-local `Cell` read).
pub(crate) fn fire_dispatch_hook() {
    if let Some(f) = HOOK.with(|h| h.get()) {
        f();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell as StdCell;

    thread_local! {
        static FIRED: StdCell<u32> = const { StdCell::new(0) };
    }

    fn bump() {
        FIRED.with(|f| f.set(f.get() + 1));
    }

    /// Default slot is a no-op; install → fires; clear → no-op again.
    /// This is the exact lifecycle `newcore::start`/`stop` drive.
    #[test]
    fn hook_lifecycle_install_fire_clear() {
        FIRED.with(|f| f.set(0));
        fire_dispatch_hook(); // uninstalled: must not panic, must not fire
        assert_eq!(FIRED.with(|f| f.get()), 0);

        install_dispatch_hook(bump);
        fire_dispatch_hook();
        fire_dispatch_hook();
        assert_eq!(FIRED.with(|f| f.get()), 2, "installed hook fires per call");

        clear_dispatch_hook();
        fire_dispatch_hook();
        assert_eq!(FIRED.with(|f| f.get()), 2, "cleared hook is a no-op");
    }
}
