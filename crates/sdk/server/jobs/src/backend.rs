//! The queue abstraction every broker implements.
//!
//! The trait is deliberately narrow and non-blocking: `reserve` returns
//! `Ok(None)` immediately when nothing is ready rather than blocking, so the
//! worker loop owns all pacing/backoff and a single implementation shape
//! (poll → dispatch) covers Redis, SQS, Postgres and the in-memory backend
//! alike. Retry/backoff *arithmetic* lives in the worker (via [`Backoff`]);
//! the backend only needs to re-queue with a delay or move to dead-letter.
//!
//! [`Backoff`]: crate::Backoff

use crate::{JobId, OutgoingJob, QueueError, ReserveOpts, ReservedJob};
use async_trait::async_trait;
use std::time::Duration;

/// A pluggable job queue. `Send + Sync` so a single instance is shared behind
/// an `Arc` across the enqueue side and every worker task.
#[async_trait]
pub trait QueueBackend: Send + Sync {
    /// Push a job onto its queue. Honors `job.delay` (not reservable until the
    /// delay elapses).
    async fn enqueue(&self, job: OutgoingJob) -> Result<JobId, QueueError>;

    /// Lease the next ready job from one of `opts.queues`, or `Ok(None)` if
    /// none is ready. The returned job is invisible to other workers until
    /// `opts.visibility` elapses or it is acked/retried/dead-lettered.
    async fn reserve(&self, opts: &ReserveOpts) -> Result<Option<ReservedJob>, QueueError>;

    /// Mark a leased job as successfully completed — remove it for good.
    async fn ack(&self, job: &ReservedJob) -> Result<(), QueueError>;

    /// Return a leased job to its queue to be retried after `delay`. The
    /// attempt counter carried by the job persists, so the next reservation
    /// reports `attempt + 1`.
    async fn retry(&self, job: &ReservedJob, delay: Duration) -> Result<(), QueueError>;

    /// Move a leased job to the dead-letter queue — retries are exhausted (or
    /// the failure is non-retryable). `reason` is recorded for diagnostics.
    async fn dead_letter(&self, job: &ReservedJob, reason: &str) -> Result<(), QueueError>;
}
