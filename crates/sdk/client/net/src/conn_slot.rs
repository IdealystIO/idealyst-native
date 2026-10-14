//! The hand-off between the HttpURLConnection worker thread (`android.rs`)
//! and everyone who may abort its exchange from another thread — the
//! caller's drop guard (the request future was dropped, or its cancel token
//! won) and the deadline [`Watchdog`](crate::timeout_policy::Watchdog).
//! Generic over the connection handle and free of JNI so its decisions are
//! covered on any host; `android.rs` instantiates it with a JNI `GlobalRef`.
//!
//! # Why a state, not an `Option`
//!
//! The worker blocks in `getResponseCode()` / `read()`, and the only way to
//! interrupt it is `disconnect()` from another thread, which needs the
//! connection. The worker publishes it here once it exists. Before that
//! (URL parsing, `openConnection`) there is nothing to disconnect, so an
//! abort that arrives early must instead stop the worker from going on to
//! connect: [`ConnSlot::abandon`] leaves the slot `Abandoned`, and the
//! worker's later [`ConnSlot::publish`] is refused. With a bare `Option`
//! (the earlier shape) an early abort found it empty and the worker then
//! ran the whole exchange with nobody waiting.
//!
//! Every transition happens under one lock, so each connection is handed to
//! exactly one party: the aborter (to disconnect) or the worker (to clean
//! up), never both and never neither.

use std::sync::{Arc, Mutex};

enum State<C> {
    /// Not published yet, or already taken back by the worker.
    Empty,
    /// The worker's connection, available to an aborter.
    Live(C),
    /// The caller stopped waiting; the worker must not start (or keep)
    /// an exchange.
    Abandoned,
}

/// See the module docs. Cloning shares the slot.
pub(crate) struct ConnSlot<C>(Arc<Mutex<State<C>>>);

impl<C> Clone for ConnSlot<C> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<C> ConnSlot<C> {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(State::Empty)))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State<C>> {
        // A poisoned lock only means a holder panicked between two
        // statements that each leave a valid `State`.
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Worker: make `conn` available to aborters. `Err(conn)` when the
    /// exchange was already abandoned — the worker must release `conn` and
    /// stop without connecting.
    pub(crate) fn publish(&self, conn: C) -> Result<(), C> {
        let mut state = self.state();
        match *state {
            State::Abandoned => Err(conn),
            _ => {
                *state = State::Live(conn);
                Ok(())
            }
        }
    }

    /// Worker: take the connection back for its own cleanup (the exchange
    /// finished or failed). `None` when an aborter already took it.
    pub(crate) fn finish(&self) -> Option<C> {
        let mut state = self.state();
        match std::mem::replace(&mut *state, State::Empty) {
            State::Live(conn) => Some(conn),
            State::Abandoned => {
                *state = State::Abandoned;
                None
            }
            State::Empty => None,
        }
    }

    /// Aborter: nobody waits for this exchange any more. Returns the live
    /// connection for the caller to `disconnect()` (unblocking the worker),
    /// or `None` when there is nothing to interrupt — not published yet
    /// (the worker's `publish` will now be refused), already finished, or
    /// already abandoned. Idempotent.
    pub(crate) fn abandon(&self) -> Option<C> {
        match std::mem::replace(&mut *self.state(), State::Abandoned) {
            State::Live(conn) => Some(conn),
            State::Empty | State::Abandoned => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandon_hands_the_live_connection_to_the_aborter_once() {
        let slot = ConnSlot::new();
        slot.publish(7).unwrap();
        assert_eq!(slot.abandon(), Some(7));
        assert_eq!(slot.abandon(), None);
        // The worker's cleanup must not see it a second time.
        assert_eq!(slot.finish(), None);
    }

    #[test]
    fn regression_abort_before_publish_stops_the_worker_from_connecting() {
        // A request future dropped (or cancelled) before the worker had a
        // connection used to find an empty slot, after which the worker went
        // on to run the whole exchange with nobody waiting.
        let slot = ConnSlot::new();
        assert_eq!(slot.abandon(), None);
        assert_eq!(slot.publish(7), Err(7));
        assert_eq!(slot.finish(), None);
    }

    #[test]
    fn finished_exchange_leaves_nothing_to_disconnect() {
        // The caller's drop guard always abandons; after a normal finish it
        // must find nothing (no stray disconnect of a reused handle).
        let slot = ConnSlot::new();
        slot.publish(7).unwrap();
        assert_eq!(slot.finish(), Some(7));
        assert_eq!(slot.abandon(), None);
    }

    #[test]
    fn clones_share_one_slot() {
        let worker = ConnSlot::new();
        let aborter = worker.clone();
        worker.publish(7).unwrap();
        assert_eq!(aborter.abandon(), Some(7));
        assert_eq!(worker.finish(), None);
    }
}
