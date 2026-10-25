//! Debug-build warnings for scoped APIs that drop work by design.
//!
//! `spawn_then` drops its result when the scope it was called in dies
//! first, and `after_ms_scoped` / `raf_loop_scoped` do nothing outside an
//! effect, a build or a scoped timer. Both are the documented semantics, and
//! both fail as a request that visibly succeeded and a handler that never
//! ran, with nothing logged — a pressed control rebuilt by the server's own
//! live event dropped a join call's ticket; a reconnect retry scheduled from
//! a `spawn_then` callback never fired. So a debug build says so, once per
//! call site (`#[track_caller]` gives the author's line), through the
//! platform logger (`console.warn` / `NSLog` / logcat).
//!
//! Release builds compile all of this away.

use std::panic::Location;

#[cfg(debug_assertions)]
mod imp {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::panic::Location;

    runtime_tls::thread_local! {
        static SEEN: RefCell<HashSet<(&'static str, u32, u32)>> = RefCell::new(HashSet::new());
        static EMITTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn warn_once_at(at: &'static Location<'static>, message: impl FnOnce() -> String) {
        let first = SEEN.with(|s| s.borrow_mut().insert((at.file(), at.line(), at.column())));
        if !first {
            return;
        }
        let message = message();
        runtime_shared::log_warn!("{message}");
        EMITTED.with(|e| e.borrow_mut().push(message));
    }

    pub fn take_emitted() -> Vec<String> {
        EMITTED.with(|e| std::mem::take(&mut *e.borrow_mut()))
    }
}

/// A `spawn_then` result was dropped because its scope ended first.
#[cfg(feature = "async-driver")]
#[inline]
pub(crate) fn spawn_then_dropped(at: &'static Location<'static>) {
    #[cfg(debug_assertions)]
    imp::warn_once_at(at, || {
        format!(
            "spawn_then at {at}: the scope it was called in ended before the task \
             finished, so the result was dropped and its callback never ran. Expected \
             if that view was closed. If the scope was a control that rebuilt itself (a \
             busy flag on its own button) or a view a live update replaced, take \
             `ScopeAlive::current()` in the component body and use \
             `spawn_then_in(&alive, …)`."
        )
    });
    #[cfg(not(debug_assertions))]
    let _ = at;
}

/// A scoped timer was registered with no scope to own it, so it is inert.
#[inline]
pub(crate) fn scoped_timer_inert(api: &'static str, at: &'static Location<'static>) {
    #[cfg(debug_assertions)]
    imp::warn_once_at(at, || {
        format!(
            "{api} at {at} was called outside any effect, component build or scoped \
             timer, so it does nothing. A `spawn_then` callback or other async \
             continuation is none of those: use `after_ms_detached` there, guarded by a \
             `ScopeAlive` taken in the component body."
        )
    });
    #[cfg(not(debug_assertions))]
    let _ = (api, at);
}

/// Drain the warnings this thread has emitted. For tests — a debug-build
/// seam, absent from release builds.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn take_emitted_for_test() -> Vec<String> {
    imp::take_emitted()
}
