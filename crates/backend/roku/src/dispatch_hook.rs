//! Post-dispatch hook: a thread-local `fn()` slot fired by host
//! infrastructure **after** it has invoked a callback that may run
//! author code.
//!
//! Same shape as `backend_terminal::dispatch_hook` — see that module for
//! why scheduler-driven author code needs a post-dispatch flush. Author
//! callbacks this backend registers in the
//! [`HandlerTable`](crate::HandlerTable) (button press, pressable
//! click, text-input / toggle / slider `on_change`, portal
//! `on_dismiss`) get their flush from the dispatch-site glue in
//! `newcore.rs` — the wrapped closure IS what lands in the table, so
//! the embedder's plain dispatch call is covered without any hook.
//!
//! This hook exists for the one dispatch surface the wrappers cannot
//! reach: author code invoked by a *runtime scheduler*, if the embedder
//! installs one (`runtime_shared::scheduling::install_scheduler`) — an
//! `after_ms` debounce that sets a signal, a `raf_loop` tick. Roku has
//! no first-party host loop, so the fire-site contract is on the
//! embedder: after your scheduler runs a timer / animation-frame
//! callback, call [`fire_dispatch_hook`] (the terminal host's
//! `scheduler::tick` is the precedent). Without a scheduler this crate
//! never needs the hook — `schedule_microtask` falls back to running
//! synchronously off-web, so the deduped flush a wrapped callback
//! queues commits before the callback wrapper even returns.
//!
//! # What deliberately does NOT fire the hook
//!
//! - **Scheduled microtasks** — same infinite-reschedule trap as the
//!   terminal hook documents.
//! - `HandlerTable` dispatch — already covered by the dispatch-site
//!   wrappers in `newcore.rs` (double-firing is harmless but
//!   redundant; the wrappers are the canonical source).

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

/// Fire the hook if installed. Called by embedder infrastructure (a
/// scheduler the embedder installed) right after a
/// potentially-author-code callback returns. `pub` because the fire
/// sites live outside this crate; cheap when no hook is installed (one
/// thread-local `Cell` read).
pub fn fire_dispatch_hook() {
    if let Some(f) = HOOK.with(|h| h.get()) {
        f();
    }
}
