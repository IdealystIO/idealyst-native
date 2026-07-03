//! The typed enqueue builder returned by `Job::enqueue(...)`.
//!
//! `#[job]` generates `send_email::enqueue(to, subject)` → [`Enqueue`]. The
//! builder collects options (`queue`, `delay`, `max_attempts`, `backoff`) and
//! only touches the queue when awaited (`impl IntoFuture`), so the call site
//! reads `send_email::enqueue(a, b).delay(d).await?`.

use crate::{Backoff, JobId, OutgoingJob, QueueError};
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::time::Duration;

/// A pending enqueue. Awaiting it pushes the job onto the configured backend
/// and yields its [`JobId`]. Carries any payload-encoding error until `.await`
/// so the call site stays a plain builder chain.
#[must_use = "an Enqueue does nothing until awaited"]
pub struct Enqueue {
    inner: Result<OutgoingJob, QueueError>,
}

impl Enqueue {
    /// Construct from a job name and an already-encoded payload. Called by the
    /// `#[job]`-generated `enqueue`; not part of the stable authoring surface.
    #[doc(hidden)]
    pub fn new(name: &'static str, payload: Result<Vec<u8>, QueueError>) -> Self {
        Self {
            inner: payload.map(|p| OutgoingJob::new(name, p)),
        }
    }

    fn map(mut self, f: impl FnOnce(&mut OutgoingJob)) -> Self {
        if let Ok(job) = self.inner.as_mut() {
            f(job);
        }
        self
    }

    /// Route to a named queue (default: `"default"`).
    pub fn queue(self, queue: impl Into<String>) -> Self {
        let queue = queue.into();
        self.map(move |j| j.queue = queue)
    }

    /// Delay reservation by `delay`.
    pub fn delay(self, delay: Duration) -> Self {
        self.map(move |j| j.delay = Some(delay))
    }

    /// Total attempts before dead-lettering (clamped to ≥ 1).
    pub fn max_attempts(self, n: u32) -> Self {
        self.map(move |j| j.max_attempts = n.max(1))
    }

    /// Override the retry backoff schedule.
    pub fn backoff(self, backoff: Backoff) -> Self {
        self.map(move |j| j.backoff = backoff)
    }
}

impl IntoFuture for Enqueue {
    type Output = Result<JobId, QueueError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let job = self.inner?;
            crate::config::enqueue(job).await
        })
    }
}
