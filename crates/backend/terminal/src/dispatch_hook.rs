//! Post-dispatch hook: a thread-local `fn()` slot fired by host
//! infrastructure **after** it has invoked a callback that may run
//! author code (the terminal scheduler's `after_ms` timers,
//! `after_animation_frame` one-shots, `raf_loop` iterations).
//!
//! Mirrors `render_wgpu::dispatch_hook` / `backend_web::dispatch_hook`
//! — the SETTLED new-core flush-driver shape. The new core
//! ([`crate::newcore`]) stages signal writes; nothing is observable
//! until the flush driver calls `World::flush`. Author callbacks the
//! backend installs (press, input, toggle, scroll, …) get their flush
//! from the dispatch-site glue in `newcore.rs`; author code that runs
//! from the *scheduler* (an `after_ms` debounce that sets a signal, a
//! `raf_loop` animation tick) has no wrapped callback — without this
//! hook a write staged there would sit uncommitted until some unrelated
//! event happened to trigger a flush.
//!
//! This module is **unconditional** (not `new-core`-gated) because the
//! fire sites live in a *different crate* (`host-terminal`'s scheduler
//! tick, which cannot see this crate's features). The slot defaults to
//! `None` and every fire is a single thread-local `Cell` read — a no-op
//! unless `newcore::start` installed the flush driver, so the old core
//! never pays for it.
//!
//! # What deliberately does NOT fire the hook
//!
//! - **Scheduled microtasks** (`schedule_microtask`). The flush itself
//!   is dispatched as a scheduled microtask; firing the hook after
//!   every microtask would re-schedule a flush from inside the flush's
//!   own dispatch and spin the host's drain-until-empty microtask loop
//!   forever (the same trap the winit/Android hooks document).
//! - The render pass (`render_to_grid`): the paint walk runs no author
//!   code.

use std::cell::Cell;

thread_local! {
    static HOOK: Cell<Option<fn()>> = const { Cell::new(None) };
}

/// Install the post-dispatch hook (replaces any previous one).
/// `newcore::start` installs `newcore::schedule_flush` here.
pub fn install_dispatch_hook(f: fn()) {
    HOOK.with(|h| h.set(Some(f)));
}

/// Remove the hook (used by `NewCoreApp::stop`). Fire sites revert to
/// no-ops.
pub fn clear_dispatch_hook() {
    HOOK.with(|h| h.set(None));
}

/// Fire the hook if installed. Called by host infrastructure (the
/// `host-terminal` scheduler) right after a potentially-author-code
/// callback returns. `pub` (not `pub(crate)`) because the fire sites
/// live in the host crate; cheap when no hook is installed (one
/// thread-local `Cell` read).
pub fn fire_dispatch_hook() {
    if let Some(f) = HOOK.with(|h| h.get()) {
        f();
    }
}
