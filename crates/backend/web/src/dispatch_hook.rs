//! Post-dispatch hook: a thread-local `fn()` slot fired by backend
//! infrastructure **after** it has invoked a callback that may run
//! author code (scheduler timers, one-shot frames, rAF-loop
//! iterations, executor-spawned future polls).
//!
//! # Why this exists (the bug prevented)
//!
//! The new core ([`crate::newcore`]) stages signal writes; nothing is
//! observable until the flush driver calls `World::flush`. DOM-event
//! callbacks get their flush from the dispatch-site glue in
//! `newcore.rs` (every author callback the backend installs is wrapped
//! to schedule a flush when it returns). But author code also runs
//! from surfaces that are *not* DOM events:
//!
//! - `runtime_shared::scheduling::after_ms` timers (e.g. a debounce that
//!   sets a signal),
//! - one-shot `after_animation_frame` callbacks and `raf_loop`
//!   iterations (animation ticks that stage writes),
//! - futures spawned through [`crate::install_async_executor`]
//!   (resource/server-call completions that set signals).
//!
//! Without this hook, a write staged from any of those would sit
//! uncommitted until some unrelated DOM event happened to trigger a
//! flush — i.e. "my `after_ms` callback ran but the UI never updated".
//! The old-core reactive system applies writes synchronously and never
//! needs this, so the slot defaults to `None` and every fire site is a
//! single thread-local read — a no-op unless `newcore::start` installed
//! the flush driver.
//!
//! # What deliberately does NOT fire the hook
//!
//! - **Scheduled microtasks** (`schedule_microtask`). The flush itself
//!   is dispatched as a scheduled microtask; firing the hook after
//!   every microtask would re-schedule a flush from inside the flush's
//!   own dispatch and spin the microtask queue forever. No author code
//!   reaches the microtask queue outside an already-hooked surface
//!   (events, timers, frames, future polls), so nothing is lost.
//! - **`render_loop.rs`** (the old-core animation clock) and
//!   `viewport_observer.rs` (old-core viewport signal): both feed
//!   old-core state that the new-core boot path does not install.

use std::cell::Cell;

thread_local! {
    static HOOK: Cell<Option<fn()>> = const { Cell::new(None) };
}

/// Install the post-dispatch hook (replaces any previous one).
/// `newcore::start_in` installs `newcore::schedule_flush` here.
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
