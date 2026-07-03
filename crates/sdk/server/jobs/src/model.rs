//! The backend-agnostic job data model: what gets enqueued, what a worker
//! reserves, and how retry backoff is computed. Pure data + arithmetic —
//! no I/O — so it's fully host-testable.

use std::time::Duration;

/// A backend-assigned job identifier. Opaque; its shape is the backend's
/// business (Redis stream id, SQS message id, Postgres row id, …).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct JobId(pub String);

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How long to wait before re-running a job that failed.
///
/// `delay_for(attempt)` is called with the number of the attempt that just
/// failed (1-based), and returns the delay before the next attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum Backoff {
    /// Retry immediately.
    None,
    /// Wait a constant duration between every attempt.
    Fixed(Duration),
    /// Exponential: `base * factor^(attempt-1)`, capped at `max`.
    Exponential {
        base: Duration,
        factor: f64,
        max: Duration,
    },
}

impl Default for Backoff {
    /// 1s, 2s, 4s, … capped at 5 minutes — a sane general default.
    fn default() -> Self {
        Backoff::Exponential {
            base: Duration::from_secs(1),
            factor: 2.0,
            max: Duration::from_secs(300),
        }
    }
}

impl Backoff {
    /// Delay before retrying, given the (1-based) number of the attempt that
    /// just failed. `attempt == 1` → the first backoff step.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        match self {
            Backoff::None => Duration::ZERO,
            Backoff::Fixed(d) => *d,
            Backoff::Exponential { base, factor, max } => {
                // First failure (attempt 1) → base * factor^0 == base.
                let steps = attempt.saturating_sub(1);
                let mult = factor.powi(steps as i32);
                let secs = base.as_secs_f64() * mult;
                if secs.is_finite() {
                    Duration::from_secs_f64(secs).min(*max)
                } else {
                    *max
                }
            }
        }
    }
}

/// A job about to be handed to a backend's `enqueue`.
#[derive(Clone, Debug)]
pub struct OutgoingJob {
    /// Logical queue name. Workers subscribe to a set of these.
    pub queue: String,
    /// The `#[job]` name — the dispatch key a worker maps back to a handler.
    pub name: String,
    /// Serialized handler arguments (the job body's input).
    pub payload: Vec<u8>,
    /// Don't make the job reservable until this much time has elapsed.
    pub delay: Option<Duration>,
    /// Total attempts allowed before dead-lettering (1 == no retry).
    pub max_attempts: u32,
    /// Backoff schedule applied between failed attempts.
    pub backoff: Backoff,
}

/// The default number of attempts before a job is dead-lettered. Matches the
/// common "retry for ~a day with exponential backoff" default.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 25;

impl OutgoingJob {
    /// A job on the `default` queue with default retry policy.
    pub fn new(name: impl Into<String>, payload: Vec<u8>) -> Self {
        Self {
            queue: "default".into(),
            name: name.into(),
            payload,
            delay: None,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff: Backoff::default(),
        }
    }
}

/// A job leased to a worker for execution. The worker must eventually `ack`
/// (success), `retry` (transient failure), or `dead_letter` it; if it does
/// none before the lease's visibility timeout expires, the backend makes the
/// job reservable again.
#[derive(Clone, Debug)]
pub struct ReservedJob {
    pub id: JobId,
    pub queue: String,
    pub name: String,
    pub payload: Vec<u8>,
    /// 1-based: this is the Nth time the job has been reserved.
    pub attempt: u32,
    pub max_attempts: u32,
    pub backoff: Backoff,
    /// Opaque backend-owned lease/receipt. The worker passes the whole
    /// `ReservedJob` back to `ack`/`retry`/`dead_letter`, so each backend
    /// reads its own receipt (memory lease id, SQS receipt handle, Redis
    /// stream entry id, …) without the worker ever interpreting it.
    pub receipt: String,
}

impl ReservedJob {
    /// True when this attempt is the last one allowed — a failure here means
    /// dead-letter, not retry.
    pub fn is_last_attempt(&self) -> bool {
        self.attempt >= self.max_attempts
    }
}

/// What a worker asks a backend for when pulling the next job.
#[derive(Clone, Debug)]
pub struct ReserveOpts {
    /// Queues to pull from, in priority order.
    pub queues: Vec<String>,
    /// Lease duration: how long the worker has to finish before the job
    /// becomes reservable again.
    pub visibility: Duration,
}

impl Default for ReserveOpts {
    fn default() -> Self {
        Self {
            queues: vec!["default".into()],
            visibility: Duration::from_secs(30),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_backoff_grows_then_caps() {
        let b = Backoff::Exponential {
            base: Duration::from_secs(1),
            factor: 2.0,
            max: Duration::from_secs(10),
        };
        assert_eq!(b.delay_for(1), Duration::from_secs(1)); // base
        assert_eq!(b.delay_for(2), Duration::from_secs(2));
        assert_eq!(b.delay_for(3), Duration::from_secs(4));
        assert_eq!(b.delay_for(4), Duration::from_secs(8));
        assert_eq!(b.delay_for(5), Duration::from_secs(10)); // capped
        assert_eq!(b.delay_for(50), Duration::from_secs(10)); // no overflow
    }

    #[test]
    fn fixed_and_none_backoff() {
        assert_eq!(Backoff::None.delay_for(7), Duration::ZERO);
        assert_eq!(
            Backoff::Fixed(Duration::from_millis(250)).delay_for(3),
            Duration::from_millis(250)
        );
    }
}
