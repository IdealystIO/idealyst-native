//! Post-dispatch hook: a thread-local `fn()` slot fired by host
//! infrastructure **after** it has invoked a callback that may run
//! author code.
//!
//! Mirrors `backend_terminal::dispatch_hook` — the SETTLED new-core
//! flush-driver shape. The new core ([`crate::newcore`]) stages signal
//! writes; nothing is observable until the flush driver calls
//! `World::flush`. Author callbacks the backend registers in the
//! [`HandlerTable`](crate::HandlerTable) (button press, pressable
//! click, text-input / toggle / slider `on_change`, portal
//! `on_dismiss`) get their flush from the dispatch-site glue in
//! `newcore.rs` — the wrapped closure IS what lands in the table, so
//! the embedder's plain dispatch call is covered without any hook.
//!
//! This hook exists for the one dispatch surface the wrappers cannot
//! reach: author code invoked by a *runtime scheduler*, if the embedder
//! installs one (`runtime_core::scheduling::install_scheduler`) — an
//! `after_ms` debounce that sets a signal, a `raf_loop` tick. Roku has
//! no first-party host loop, so the fire-site contract is on the
//! embedder: after your scheduler runs a timer / animation-frame
//! callback, call [`fire_dispatch_hook`] (the terminal host's
//! `scheduler::tick` is the precedent). Without a scheduler this crate
//! never needs the hook — `schedule_microtask` falls back to running
//! synchronously off-web, so the deduped flush a wrapped callback
//! queues commits before the callback wrapper even returns.
//!
//! This module is **unconditional** (not `new-core`-gated) because the
//! fire sites live outside this crate (the embedder's scheduler, which
//! cannot see this crate's features). The slot defaults to `None` and
//! every fire is a single thread-local `Cell` read — a no-op unless
//! `newcore::start` installed the flush driver, so the old core never
//! pays for it.
//!
//! # What deliberately does NOT fire the hook
//!
//! - **Scheduled microtasks** (`schedule_microtask`). The flush itself
//!   is dispatched as a scheduled microtask; firing the hook after
//!   every microtask would re-schedule a flush from inside the flush's
//!   own dispatch and spin a drain-until-empty microtask loop forever
//!   (the same trap the terminal/winit hooks document).
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
