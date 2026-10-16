//! Post-dispatch hook: a thread-local `fn()` slot fired by host
//! infrastructure **after** it has invoked a callback that may run
//! author code (scheduler `after_ms` timers, `after_animation_frame`
//! one-shots, `raf_loop` iterations, and — on this backend — each
//! `WindowsBackend::dispatch_command` return, which also covers SDK
//! command handlers registered outside the caps layer).
//!
//! Same shape as `backend_terminal::dispatch_hook`. The kernel stages
//! signal writes; nothing is observable until the flush driver
//! ([`crate::newcore`]) calls `World::flush`. Author callbacks the backend installs (press,
//! scroll, …) get their flush from the dispatch-site glue in
//! `newcore.rs`; author code that runs from a *host scheduler* (an
//! `after_ms` debounce that sets a signal, a `raf_loop` animation
//! tick) has no wrapped callback — without this hook a write staged
//! there would sit uncommitted until some unrelated event happened to
//! trigger a flush.
//!
//! The scheduler fire sites live in the *host shell* (`host-win32`'s
//! scheduler and async executor), so [`fire_dispatch_hook`] is `pub`.
//! The slot defaults to `None` and every fire is a single thread-local
//! `Cell` read — a no-op unless `newcore::start` installed the flush
//! driver.
//!
//! # What deliberately does NOT fire the hook
//!
//! - **Scheduled microtasks** (`schedule_microtask`). The flush itself
//!   is dispatched as a scheduled microtask; firing the hook after
//!   every microtask would re-schedule a flush from inside the flush's
//!   own dispatch and spin a drain-until-empty microtask loop forever
//!   (the same trap the winit/Android hooks document).
//! - The layout/paint pass (`finish`'s `SetWindowPos` walk): it runs
//!   no author code.

use std::cell::Cell;

thread_local! {
    static HOOK: Cell<Option<fn()>> = const { Cell::new(None) };
}

/// Install the post-dispatch hook (replaces any previous one).
/// `newcore::start` installs `newcore::schedule_flush` here.
pub fn install_dispatch_hook(f: fn()) {
    HOOK.with(|h| h.set(Some(f)));
}

/// Remove the hook (used by [`crate::newcore::NewCoreApp::stop`]). Fire sites revert to
/// no-ops.
pub fn clear_dispatch_hook() {
    HOOK.with(|h| h.set(None));
}

/// Fire the hook if installed. Called by host infrastructure right
/// after a potentially-author-code callback returns. `pub` because the
/// fire sites live in the host shell; cheap when no hook is installed
/// (one thread-local `Cell` read).
pub fn fire_dispatch_hook() {
    if let Some(f) = HOOK.with(|h| h.get()) {
        f();
    }
}
