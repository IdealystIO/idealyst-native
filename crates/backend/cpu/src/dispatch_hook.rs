//! Post-dispatch hook: a thread-local `fn()` slot fired by host
//! infrastructure **after** it has invoked a callback that may run
//! author code.
//!
//! Mirrors `backend_terminal::dispatch_hook` — the SETTLED new-core
//! flush-driver shape. The new core ([`crate::newcore`]) stages signal
//! writes; nothing is observable until the flush driver calls
//! `World::flush`. Author callbacks the backend installs (the click
//! handler [`crate::ClickOutcome::HandlerFired`] hands back) get their
//! flush from the dispatch-site glue in `newcore.rs`; author code that
//! runs from a *scheduler* (an `after_ms` debounce that sets a signal,
//! a `raf_loop` animation tick) has no wrapped callback — without this
//! hook a write staged there would sit uncommitted until some unrelated
//! event happened to trigger a flush.
//!
//! # The embedder contract (the CPU backend owns NO scheduler)
//!
//! Unlike the terminal backend (whose `host-terminal` crate ships a
//! tick scheduler with first-party fire sites), the CPU backend has no
//! first-party fire site: the host decides render cadence (see the
//! crate docs — "the host calls `render` when it wants a frame"), and
//! the host decides whether to install a `runtime_core::scheduling`
//! scheduler at all. The contract for embedders:
//!
//! - A host that installs a runtime scheduler MUST call
//!   [`fire_dispatch_hook`] after each timer / frame author callback it
//!   dispatches (`after_ms`, `after_animation_frame`, each `raf_loop`
//!   iteration) — the `host-terminal` scheduler-tick precedent. The
//!   host's per-frame loop should then drain queued microtasks before
//!   painting so the flush commits in the same frame.
//! - A headless host (tests, one-shot render harnesses) with no
//!   scheduler-driven author code settles staged writes explicitly via
//!   [`crate::newcore::flush_sync`] after draining microtasks.
//!
//! This module is **unconditional** (not `new-core`-gated) because the
//! fire sites live in *host crates*, which cannot see this crate's
//! features. The slot defaults to `None` and every fire is a single
//! thread-local `Cell` read — a no-op unless `newcore::start` installed
//! the flush driver, so the old core never pays for it.
//!
//! # What deliberately does NOT fire the hook
//!
//! - **Scheduled microtasks** (`schedule_microtask`). The flush itself
//!   is dispatched as a scheduled microtask; firing the hook after
//!   every microtask would re-schedule a flush from inside the flush's
//!   own dispatch and spin a drain-until-empty microtask loop forever
//!   (the same trap the terminal/winit hooks document).
//! - The render pass ([`crate::CpuBackend::render`]): the paint walk
//!   runs no author code.

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

/// Fire the hook if installed. Called by host infrastructure right
/// after a potentially-author-code callback returns (see the module
/// docs for the embedder contract). `pub` because the fire sites live
/// in host crates; cheap when no hook is installed (one thread-local
/// `Cell` read).
pub fn fire_dispatch_hook() {
    if let Some(f) = HOOK.with(|h| h.get()) {
        f();
    }
}
