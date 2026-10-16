//! Post-dispatch hook: a thread-local `fn()` slot fired by host
//! infrastructure **after** it has invoked a callback that may run
//! author code.
//!
//! Same shape as `backend_terminal::dispatch_hook` — see that module for
//! why scheduler-driven author code needs a post-dispatch flush. Here the
//! only wrapped author callback is the click handler
//! [`crate::ClickOutcome::HandlerFired`] hands back.
//!
//! # The embedder contract (the CPU backend owns NO scheduler)
//!
//! Unlike the terminal backend (whose `host-terminal` crate ships a
//! tick scheduler with first-party fire sites), the CPU backend has no
//! first-party fire site: the host decides render cadence (see the
//! crate docs — "the host calls `render` when it wants a frame"), and
//! the host decides whether to install a `runtime_shared::scheduling`
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
//! # What deliberately does NOT fire the hook
//!
//! - **Scheduled microtasks** — same infinite-reschedule trap as the
//!   terminal hook documents.
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
