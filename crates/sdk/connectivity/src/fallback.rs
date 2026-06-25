//! Inert reachability impl for native targets with no platform monitor
//! (Windows / Linux / CI hosts).
//!
//! There's no cheap, dependency-free synchronous reachability query that's
//! correct across these platforms, so [`current`] returns the documented
//! best-effort snapshot ([`Connectivity::ASSUME_ONLINE`]) and [`watch`] never
//! fires. Assuming reachable is the safe default for a "should I attempt the
//! request?" check — the request itself reports the real failure.

use crate::{Connectivity, WatchCallback};

/// Best-effort: assume reachable. See the module docs.
pub(crate) fn current() -> Connectivity {
    Connectivity::ASSUME_ONLINE
}

/// Register a watcher. On these platforms there's no change source, so the
/// callback is simply held (and dropped with the subscription) and never
/// invoked.
pub(crate) fn watch(callback: WatchCallback) -> Subscription {
    Subscription { _callback: callback }
}

/// Inert subscription — owns the callback so its lifetime matches the other
/// backends', and drops it on teardown. Never fires.
pub(crate) struct Subscription {
    _callback: WatchCallback,
}
