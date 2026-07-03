//! Error types for the jobs SDK.
//!
//! Two distinct axes:
//! - [`QueueError`] — infrastructure failures moving jobs through a backend
//!   (broker unreachable, payload codec, no backend configured). Surfaced to
//!   the *enqueuer* and to the worker loop.
//! - [`JobError`] — a failure *inside a job handler body*. Any `JobError`
//!   triggers a retry (with backoff) until `max_attempts` is exhausted, after
//!   which the job is dead-lettered.

use thiserror::Error;

/// A failure moving a job through the queue backend.
#[derive(Debug, Error)]
pub enum QueueError {
    /// The backend (Redis / SQS / Postgres / …) reported an error.
    #[error("queue backend error: {0}")]
    Backend(String),
    /// The job payload could not be (de)serialized.
    #[error("job payload codec error: {0}")]
    Codec(String),
    /// `jobs::configure(...)` was never called, so there is nowhere to enqueue.
    #[error("no queue backend configured; call jobs::configure(...) at startup")]
    NotConfigured,
}

/// A failure returned from a `#[job]` handler body.
///
/// The worker treats any `Err(JobError)` as "this attempt failed" — it
/// re-queues the job with backoff up to `max_attempts`, then dead-letters it.
/// Construct one from any displayable error via `.into()` /
/// [`JobError::new`], e.g. `something().map_err(JobError::new)?`.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct JobError(pub String);

impl JobError {
    /// Build a `JobError` from anything that can render itself.
    pub fn new(msg: impl std::fmt::Display) -> Self {
        Self(msg.to_string())
    }
}

impl From<String> for JobError {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for JobError {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}
