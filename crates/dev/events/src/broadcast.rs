//! Fan-out to consumers that attach and detach at any time.
//!
//! [`Broadcast`] is a [`Sink`] that keeps the session's
//! [`SessionState`] and a list of live subscribers. [`Broadcast::subscribe`]
//! hands a new subscriber the snapshot and a channel of every later
//! event, atomically — taken under one lock, so an event is either in
//! the snapshot or on the channel, never both and never neither. This is
//! what the dev server's `/__idealyst/events` stream serves.
//!
//! Events travel as their JSON lines (the same objects `--events-file`
//! writes), serialized once per event rather than once per subscriber.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::snapshot::SessionState;
use crate::{DevEvent, Envelope, Sink};

/// A subscription: where to start, and what follows.
pub struct Subscription {
    /// The events describing the session as it is now, as JSON lines.
    pub snapshot: Vec<Arc<str>>,
    /// Every later event, as a JSON line. Disconnected when the
    /// broadcast is dropped.
    pub events: mpsc::Receiver<Arc<str>>,
}

struct Inner {
    state: SessionState,
    subscribers: Vec<mpsc::Sender<Arc<str>>>,
}

/// See the module docs.
pub struct Broadcast {
    inner: Mutex<Inner>,
    filter: fn(&DevEvent) -> bool,
}

impl Broadcast {
    /// A broadcast of every event.
    pub fn new() -> Arc<Self> {
        Self::filtered(|_| true)
    }

    /// A broadcast of the events `filter` keeps.
    pub fn filtered(filter: fn(&DevEvent) -> bool) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner { state: SessionState::new(), subscribers: Vec::new() }),
            filter,
        })
    }

    /// Attach a subscriber. See [`Subscription`].
    pub fn subscribe(&self) -> Subscription {
        let (tx, rx) = mpsc::channel();
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let snapshot = inner.state.snapshot().iter().filter_map(line).collect();
        inner.subscribers.push(tx);
        Subscription { snapshot, events: rx }
    }

    /// How many subscribers are attached (tests, diagnostics).
    pub fn subscribers(&self) -> usize {
        self.inner.lock().map(|i| i.subscribers.len()).unwrap_or(0)
    }
}

fn line(envelope: &Envelope) -> Option<Arc<str>> {
    serde_json::to_string(envelope).ok().map(Arc::from)
}

impl Sink for Broadcast {
    fn emit(&self, envelope: &Envelope) {
        if !(self.filter)(&envelope.event) {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.state.apply(envelope);
        if inner.subscribers.is_empty() {
            return;
        }
        let Some(json) = line(envelope) else { return };
        // A subscriber that went away drops out here, on the first event
        // after it did.
        inner.subscribers.retain(|tx| tx.send(json.clone()).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Reporter;

    #[test]
    fn a_subscriber_gets_the_snapshot_then_every_later_event_exactly_once() {
        let r = Reporter::new();
        let b = Broadcast::new();
        r.add_sink(b.clone());
        r.emit(DevEvent::BuildStarted { target: "web".into(), cause: crate::BuildCause::Initial });
        r.log("dev", "history, not state");
        let sub = b.subscribe();
        assert_eq!(sub.snapshot.len(), 1, "{:?}", sub.snapshot);
        r.emit(DevEvent::OverlayPushed { target: "web".into(), sites: 1, ms: 2 });
        let next: Envelope = serde_json::from_str(&sub.events.try_recv().unwrap()).unwrap();
        assert_eq!(next.seq, 3);
        assert!(sub.events.try_recv().is_err());
    }

    #[test]
    fn a_dropped_subscriber_is_forgotten() {
        let r = Reporter::new();
        let b = Broadcast::new();
        r.add_sink(b.clone());
        drop(b.subscribe());
        r.log("dev", "x");
        assert_eq!(b.subscribers(), 0);
    }
}
