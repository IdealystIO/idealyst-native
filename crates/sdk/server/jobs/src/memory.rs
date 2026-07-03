//! In-process reference [`QueueBackend`]. This is the correctness reference —
//! every other backend must observe the same enqueue → reserve → ack/retry/
//! dead-letter semantics — and the substrate for the crate's unit tests.
//!
//! It is a real queue, not a toy: it honors delay, leases with a visibility
//! timeout (a job whose lease expires becomes reservable again), a persistent
//! attempt counter, and a dead-letter bucket. It shares state across clones
//! (`Arc` inside), so a server enqueuing and an in-process worker draining hold
//! the same queue.
//!
//! Time flows through the [`Clock`] trait so tests can advance it
//! deterministically ([`ManualClock`]) instead of sleeping.

use crate::{Backoff, JobId, OutgoingJob, QueueBackend, QueueError, ReserveOpts, ReservedJob};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A monotonic millisecond clock. `now_ms` need not be wall-clock — only
/// monotonic and consistent within one backend instance.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Real time: milliseconds since the process's first observation.
pub struct SystemClock {
    base: std::time::Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            base: std::time::Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        self.base.elapsed().as_millis() as u64
    }
}

/// A job sitting in a queue, waiting to be reserved.
#[derive(Clone)]
struct Entry {
    id: JobId,
    queue: String,
    name: String,
    payload: Vec<u8>,
    /// Attempts already made (0 until first reservation).
    attempts: u32,
    max_attempts: u32,
    backoff: Backoff,
    /// Not reservable until now_ms >= available_at (delay / retry backoff).
    available_at: u64,
    /// Recorded on dead-letter for diagnostics.
    dead_reason: Option<String>,
}

/// A job currently leased to a worker.
struct Lease {
    entry: Entry,
    /// Reservable again once now_ms >= deadline (visibility timeout).
    deadline: u64,
}

#[derive(Default)]
struct State {
    ready: Vec<Entry>,
    in_flight: HashMap<u64, Lease>,
    dead: Vec<Entry>,
    next_id: u64,
    next_lease: u64,
}

/// The in-process reference backend. Cheap to `clone` — clones share state.
#[derive(Clone)]
pub struct MemoryBackend {
    state: Arc<Mutex<State>>,
    clock: Arc<dyn Clock>,
}

impl Default for MemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryBackend {
    /// A backend driven by the real monotonic clock.
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock::default()))
    }

    /// A backend driven by a custom clock — used by tests to advance time
    /// without sleeping.
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            clock,
        }
    }

    /// Number of jobs waiting (ready or delayed, not counting in-flight).
    pub fn pending_count(&self) -> usize {
        self.state.lock().unwrap().ready.len()
    }

    /// Number of jobs currently leased to a worker.
    pub fn in_flight_count(&self) -> usize {
        self.state.lock().unwrap().in_flight.len()
    }

    /// Number of dead-lettered jobs.
    pub fn dead_count(&self) -> usize {
        self.state.lock().unwrap().dead.len()
    }

    /// Reclaim any leases whose visibility timeout has passed — the job
    /// becomes reservable again. Called at the top of every `reserve`.
    fn reclaim_expired(state: &mut State, now: u64) {
        let expired: Vec<u64> = state
            .in_flight
            .iter()
            .filter(|(_, l)| now >= l.deadline)
            .map(|(k, _)| *k)
            .collect();
        for lease in expired {
            if let Some(l) = state.in_flight.remove(&lease) {
                state.ready.push(l.entry);
            }
        }
    }
}

#[async_trait]
impl QueueBackend for MemoryBackend {
    async fn enqueue(&self, job: OutgoingJob) -> Result<JobId, QueueError> {
        let now = self.clock.now_ms();
        let mut state = self.state.lock().unwrap();
        state.next_id += 1;
        let id = JobId(format!("mem-{}", state.next_id));
        let available_at = now + job.delay.map(|d| d.as_millis() as u64).unwrap_or(0);
        state.ready.push(Entry {
            id: id.clone(),
            queue: job.queue,
            name: job.name,
            payload: job.payload,
            attempts: 0,
            max_attempts: job.max_attempts.max(1),
            backoff: job.backoff,
            available_at,
            dead_reason: None,
        });
        Ok(id)
    }

    async fn reserve(&self, opts: &ReserveOpts) -> Result<Option<ReservedJob>, QueueError> {
        let now = self.clock.now_ms();
        let mut state = self.state.lock().unwrap();
        Self::reclaim_expired(&mut state, now);

        // First ready entry on one of the requested queues whose delay has
        // elapsed. Queue order in `opts.queues` is the priority order.
        let mut pick: Option<usize> = None;
        'outer: for q in &opts.queues {
            for (i, e) in state.ready.iter().enumerate() {
                if &e.queue == q && e.available_at <= now {
                    pick = Some(i);
                    break 'outer;
                }
            }
        }
        let Some(idx) = pick else {
            return Ok(None);
        };

        let mut entry = state.ready.remove(idx);
        entry.attempts += 1;
        let attempt = entry.attempts;

        state.next_lease += 1;
        let lease = state.next_lease;
        let deadline = now + opts.visibility.as_millis() as u64;

        let reserved = ReservedJob {
            id: entry.id.clone(),
            queue: entry.queue.clone(),
            name: entry.name.clone(),
            payload: entry.payload.clone(),
            attempt,
            max_attempts: entry.max_attempts,
            backoff: entry.backoff.clone(),
            receipt: lease.to_string(),
        };
        state.in_flight.insert(lease, Lease { entry, deadline });
        Ok(Some(reserved))
    }

    async fn ack(&self, job: &ReservedJob) -> Result<(), QueueError> {
        let lease: u64 = job.receipt.parse().unwrap_or(0);
        let mut state = self.state.lock().unwrap();
        // Absent lease == already acked or reclaimed: a safe no-op, matching
        // the framework's generational stale-handle discipline.
        state.in_flight.remove(&lease);
        Ok(())
    }

    async fn retry(&self, job: &ReservedJob, delay: Duration) -> Result<(), QueueError> {
        let lease: u64 = job.receipt.parse().unwrap_or(0);
        let now = self.clock.now_ms();
        let mut state = self.state.lock().unwrap();
        if let Some(l) = state.in_flight.remove(&lease) {
            let mut entry = l.entry;
            entry.available_at = now + delay.as_millis() as u64;
            state.ready.push(entry);
        }
        Ok(())
    }

    async fn dead_letter(&self, job: &ReservedJob, reason: &str) -> Result<(), QueueError> {
        let lease: u64 = job.receipt.parse().unwrap_or(0);
        let mut state = self.state.lock().unwrap();
        if let Some(l) = state.in_flight.remove(&lease) {
            let mut entry = l.entry;
            entry.dead_reason = Some(reason.to_string());
            state.dead.push(entry);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A clock the test advances by hand.
    struct ManualClock(AtomicU64);
    impl ManualClock {
        fn new() -> Arc<Self> {
            Arc::new(Self(AtomicU64::new(0)))
        }
        fn advance(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::SeqCst);
        }
    }
    impl Clock for ManualClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn job(name: &str) -> OutgoingJob {
        OutgoingJob::new(name, name.as_bytes().to_vec())
    }

    #[tokio::test]
    async fn enqueue_reserve_ack_roundtrip() {
        let b = MemoryBackend::new();
        b.enqueue(job("a")).await.unwrap();
        assert_eq!(b.pending_count(), 1);

        let r = b.reserve(&ReserveOpts::default()).await.unwrap().unwrap();
        assert_eq!(r.name, "a");
        assert_eq!(r.attempt, 1);
        assert_eq!(b.pending_count(), 0);
        assert_eq!(b.in_flight_count(), 1);

        b.ack(&r).await.unwrap();
        assert_eq!(b.in_flight_count(), 0);
        // Nothing left to reserve.
        assert!(b.reserve(&ReserveOpts::default()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn retry_increments_attempt_and_honors_delay() {
        let clock = ManualClock::new();
        let b = MemoryBackend::with_clock(clock.clone());
        b.enqueue(job("a")).await.unwrap();

        let r1 = b.reserve(&ReserveOpts::default()).await.unwrap().unwrap();
        assert_eq!(r1.attempt, 1);
        // Retry after 5s of backoff.
        b.retry(&r1, Duration::from_secs(5)).await.unwrap();
        assert_eq!(b.in_flight_count(), 0);
        assert_eq!(b.pending_count(), 1);

        // Not yet available: still delayed.
        assert!(b.reserve(&ReserveOpts::default()).await.unwrap().is_none());
        clock.advance(5_000);
        let r2 = b.reserve(&ReserveOpts::default()).await.unwrap().unwrap();
        assert_eq!(r2.attempt, 2, "attempt counter persists across retry");
    }

    #[tokio::test]
    async fn delay_is_respected_on_enqueue() {
        let clock = ManualClock::new();
        let b = MemoryBackend::with_clock(clock.clone());
        let mut j = job("later");
        j.delay = Some(Duration::from_secs(10));
        b.enqueue(j).await.unwrap();

        assert!(b.reserve(&ReserveOpts::default()).await.unwrap().is_none());
        clock.advance(9_999);
        assert!(b.reserve(&ReserveOpts::default()).await.unwrap().is_none());
        clock.advance(1);
        assert!(b.reserve(&ReserveOpts::default()).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn expired_lease_becomes_reservable_again() {
        let clock = ManualClock::new();
        let b = MemoryBackend::with_clock(clock.clone());
        b.enqueue(job("a")).await.unwrap();

        let opts = ReserveOpts {
            queues: vec!["default".into()],
            visibility: Duration::from_secs(30),
        };
        let r1 = b.reserve(&opts).await.unwrap().unwrap();
        assert_eq!(r1.attempt, 1);
        // Worker vanished without ack; lease times out.
        assert!(b.reserve(&opts).await.unwrap().is_none());
        clock.advance(30_000);
        let r2 = b.reserve(&opts).await.unwrap().unwrap();
        assert_eq!(r2.attempt, 2, "reclaimed lease bumps the attempt");
        // Stale ack from the dead worker must not remove the new lease.
        b.ack(&r1).await.unwrap();
        assert_eq!(b.in_flight_count(), 1);
    }

    #[tokio::test]
    async fn dead_letter_moves_out_of_rotation() {
        let b = MemoryBackend::new();
        b.enqueue(job("a")).await.unwrap();
        let r = b.reserve(&ReserveOpts::default()).await.unwrap().unwrap();
        b.dead_letter(&r, "boom").await.unwrap();
        assert_eq!(b.dead_count(), 1);
        assert_eq!(b.in_flight_count(), 0);
        assert!(b.reserve(&ReserveOpts::default()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn queues_are_isolated_and_priority_ordered() {
        let b = MemoryBackend::new();
        let mut high = job("h");
        high.queue = "high".into();
        let mut low = job("l");
        low.queue = "low".into();
        b.enqueue(low).await.unwrap();
        b.enqueue(high).await.unwrap();

        // Only pulling from "low" ignores the "high" job.
        let opts_low = ReserveOpts {
            queues: vec!["low".into()],
            visibility: Duration::from_secs(30),
        };
        assert_eq!(
            b.reserve(&opts_low).await.unwrap().unwrap().name,
            "l"
        );

        // Priority order: "high" before "low".
        let opts_pri = ReserveOpts {
            queues: vec!["high".into(), "low".into()],
            visibility: Duration::from_secs(30),
        };
        assert_eq!(
            b.reserve(&opts_pri).await.unwrap().unwrap().name,
            "h"
        );
    }
}
