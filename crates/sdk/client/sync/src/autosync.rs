//! Auto-sync triggers — the pluggable "**when** should I sync?" source.
//!
//! Sync *mechanism* (cursor pull + outbox flush + merge) never changes; the
//! only question a trigger answers is *when* to run it. That makes the
//! transport choice a swap-in: poll on a timer today, wake on a WebSocket
//! nudge or a push notification tomorrow — all driving the same
//! [`SyncEngine::sync_all`](crate::SyncEngine::sync_all).
//!
//! Implement [`SyncTrigger`] to add a mode; the engine hands it a
//! [`SyncHandle`] it calls whenever a sync should happen. The shipped
//! default is [`PollingTrigger`].
//!
//! ```ignore
//! use std::rc::Rc;
//! // Poll every 15s (and auto-sync immediately on reconnect):
//! engine.start_auto_sync(Rc::new(sync::PollingTrigger::new(15_000)));
//! ```

use std::rc::Rc;

use runtime_core::after_ms_detached;
use runtime_core::driver::spawn_async;

use crate::SyncEngine;

/// What the engine hands a [`SyncTrigger`] so it can request syncs without
/// knowing anything about partitions or the protocol.
#[derive(Clone)]
pub struct SyncHandle {
    engine: SyncEngine,
}

impl SyncHandle {
    pub(crate) fn new(engine: SyncEngine) -> Self {
        SyncHandle { engine }
    }

    /// Ask the engine to sync every partition now (non-blocking; runs on
    /// the async executor). A no-op-ish call when offline — each
    /// partition's flush self-guards, and a pull just fails fast.
    pub fn sync_now(&self) {
        let engine = self.engine.clone();
        spawn_async(async move {
            let _ = engine.sync_all().await;
        });
    }

    /// Whether the engine currently considers itself online — a trigger can
    /// skip work while offline.
    pub fn is_online(&self) -> bool {
        self.engine.is_online()
    }
}

/// A source of "sync now" signals. The engine calls [`start`](Self::start)
/// once at [`start_auto_sync`](crate::SyncEngine::start_auto_sync); the
/// implementation then invokes the [`SyncHandle`] on its own schedule (a
/// timer tick, a socket message, an OS wake, …).
pub trait SyncTrigger {
    /// Begin producing sync signals. Takes ownership via `Rc<Self>` so the
    /// trigger can keep itself alive across its async/timer callbacks.
    fn start(self: Rc<Self>, handle: SyncHandle);
}

/// The always-available default: sync every `interval_ms` while online.
///
/// Simple and dependency-free (uses the framework's `after_ms` scheduler,
/// so it works on web and native). It's the baseline you start with; a
/// push-based trigger later just replaces it — the engine surface is the
/// same.
pub struct PollingTrigger {
    interval_ms: u32,
}

impl PollingTrigger {
    /// Poll every `interval_ms` milliseconds.
    pub fn new(interval_ms: u32) -> Self {
        PollingTrigger { interval_ms }
    }
}

impl SyncTrigger for PollingTrigger {
    fn start(self: Rc<Self>, handle: SyncHandle) {
        arm(self.interval_ms, handle);
    }
}

/// Schedule one tick, then re-arm. `after_ms_detached` is fire-and-forget,
/// so each tick re-arms the next — a self-perpetuating interval that lives
/// for the engine's lifetime.
fn arm(interval_ms: u32, handle: SyncHandle) {
    after_ms_detached(interval_ms as i32, move || {
        if handle.is_online() {
            handle.sync_now();
        }
        arm(interval_ms, handle);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trigger that does nothing on `start` — for testing the engine's
    /// reconnect-auto-sync path without a real timer.
    struct NoopTrigger;
    impl SyncTrigger for NoopTrigger {
        fn start(self: Rc<Self>, _handle: SyncHandle) {}
    }

    #[test]
    fn polling_trigger_constructs() {
        let _t = PollingTrigger::new(1000);
        // (Timer firing is exercised at runtime, not in a unit test — the
        // scheduler isn't installed here. The reconnect + sync_all paths are
        // covered in partition.rs tests.)
    }

    #[test]
    fn noop_trigger_is_object_safe() {
        let t: Rc<dyn SyncTrigger> = Rc::new(NoopTrigger);
        // Just confirm it can be boxed as the engine expects.
        let _ = t;
    }
}
