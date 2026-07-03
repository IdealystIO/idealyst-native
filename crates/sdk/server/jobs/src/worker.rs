//! The worker runtime: reserve → dispatch → ack/retry/dead-letter.
//!
//! Compiled only in the worker build (`feature = "worker"`). Builds a name →
//! handler map from the `inventory` of `#[job]` registrations, then loops
//! pulling ready jobs from the backend and running their handlers under a
//! bounded concurrency limit. A handler that returns `Err` (or panics) is
//! retried with [`Backoff`] until `max_attempts`, then dead-lettered.
//!
//! Two entry points, both driving the same loop:
//! - [`Worker::run`] — a dedicated process; blocks until SIGINT/SIGTERM, then
//!   drains in-flight jobs before returning.
//! - [`Worker::spawn`] — an in-process background task (call before
//!   `server::serve(addr).await`); stop it via [`WorkerHandle::shutdown`].
//!
//! [`Backoff`]: crate::Backoff

use crate::{configured_backend, JobError, QueueBackend, QueueError, ReserveOpts, ReservedJob};
use futures_util::FutureExt;
use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinHandle;

type HandlerFn =
    fn(Vec<u8>) -> Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;

/// Start configuring a worker. Finish with [`Worker::run`] or [`Worker::spawn`].
pub fn worker() -> Worker {
    Worker::default()
}

/// A configured (not yet running) worker.
pub struct Worker {
    backend: Option<Arc<dyn QueueBackend>>,
    queues: Vec<String>,
    concurrency: usize,
    visibility: Duration,
    poll_interval: Duration,
}

impl Default for Worker {
    fn default() -> Self {
        let concurrency = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self {
            backend: None,
            queues: vec!["default".into()],
            concurrency,
            visibility: Duration::from_secs(30),
            poll_interval: Duration::from_millis(500),
        }
    }
}

impl Worker {
    /// Use a specific backend instead of the one from [`crate::configure`].
    pub fn backend<B: QueueBackend + 'static>(mut self, backend: B) -> Self {
        self.backend = Some(Arc::new(backend));
        self
    }

    /// Queues to drain, in priority order (default: `["default"]`).
    pub fn queues<I, S>(mut self, queues: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.queues = queues.into_iter().map(Into::into).collect();
        if self.queues.is_empty() {
            self.queues.push("default".into());
        }
        self
    }

    /// Maximum jobs running at once (default: available parallelism).
    pub fn concurrency(mut self, n: usize) -> Self {
        self.concurrency = n.max(1);
        self
    }

    /// Lease duration handed to `reserve` (default: 30s).
    pub fn visibility(mut self, d: Duration) -> Self {
        self.visibility = d;
        self
    }

    /// How long to wait after an empty poll before asking again (default: 500ms).
    pub fn poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    fn resolve_backend(&self) -> Result<Arc<dyn QueueBackend>, QueueError> {
        self.backend
            .clone()
            .or_else(configured_backend)
            .ok_or(QueueError::NotConfigured)
    }

    /// Run as a dedicated process. Blocks until SIGINT (Ctrl-C) or, on Unix,
    /// SIGTERM; then stops reserving and drains in-flight jobs before returning.
    pub async fn run(self) -> Result<(), QueueError> {
        self.run_until(shutdown_signal()).await
    }

    /// Run as an in-process background task. Returns a handle to stop it.
    pub fn spawn(self) -> WorkerHandle {
        let notify = Arc::new(Notify::new());
        let trigger = notify.clone();
        let task = tokio::spawn(async move {
            let _ = self.run_until(async move { trigger.notified().await }).await;
        });
        WorkerHandle { notify, task }
    }

    async fn run_until(
        self,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<(), QueueError> {
        let backend = self.resolve_backend()?;
        let handlers = handler_map();
        let opts = ReserveOpts {
            queues: self.queues.clone(),
            visibility: self.visibility,
        };
        let sem = Arc::new(Semaphore::new(self.concurrency));

        tokio::pin!(shutdown);
        loop {
            // Wait for a free concurrency slot (or shutdown).
            let permit = tokio::select! {
                biased;
                _ = &mut shutdown => break,
                p = sem.clone().acquire_owned() => p.expect("worker semaphore never closes"),
            };

            let reserved = tokio::select! {
                _ = &mut shutdown => { drop(permit); break; }
                r = backend.reserve(&opts) => r,
            };

            match reserved {
                Ok(Some(job)) => {
                    let backend = backend.clone();
                    let handler = handlers.get(job.name.as_str()).copied();
                    tokio::spawn(async move {
                        run_one(backend, job, handler).await;
                        drop(permit);
                    });
                }
                Ok(None) => {
                    drop(permit);
                    tokio::select! {
                        _ = &mut shutdown => break,
                        _ = tokio::time::sleep(self.poll_interval) => {}
                    }
                }
                Err(e) => {
                    eprintln!("[jobs worker] reserve error: {e}");
                    drop(permit);
                    tokio::select! {
                        _ = &mut shutdown => break,
                        _ = tokio::time::sleep(self.poll_interval) => {}
                    }
                }
            }
        }

        // Drain: every in-flight task holds one permit; reacquiring them all
        // means they've finished (acked/retried/dead-lettered).
        let _ = sem.acquire_many(self.concurrency as u32).await;
        Ok(())
    }
}

/// Run a single reserved job to completion, then settle it with the backend.
async fn run_one(
    backend: Arc<dyn QueueBackend>,
    job: ReservedJob,
    handler: Option<HandlerFn>,
) {
    let Some(handler) = handler else {
        // A job with no registered handler can never succeed — dead-letter it
        // immediately rather than churning retries.
        let _ = backend
            .dead_letter(&job, "no handler registered for this job name")
            .await;
        return;
    };

    // Treat a panic in the handler as a failed attempt, not a worker crash.
    let outcome = AssertUnwindSafe(handler(job.payload.clone()))
        .catch_unwind()
        .await;

    let reason = match outcome {
        Ok(Ok(())) => {
            let _ = backend.ack(&job).await;
            return;
        }
        Ok(Err(e)) => e.0,
        Err(_panic) => "job handler panicked".to_string(),
    };

    if job.is_last_attempt() {
        let _ = backend.dead_letter(&job, &reason).await;
    } else {
        let delay = job.backoff.delay_for(job.attempt);
        let _ = backend.retry(&job, delay).await;
    }
}

/// Build the process-wide name → handler map from the `inventory` of `#[job]`
/// registrations. Cheap to rebuild, but callers only need it once per run.
fn handler_map() -> HashMap<&'static str, HandlerFn> {
    let mut map = HashMap::new();
    for entry in inventory::iter::<crate::__private::JobEntry> {
        if map.insert(entry.name, entry.handler).is_some() {
            eprintln!(
                "[jobs worker] WARNING: two #[job]s registered the name `{}`; \
                 the later registration wins",
                entry.name
            );
        }
    }
    map
}

/// Resolve when the process receives SIGINT (Ctrl-C) or, on Unix, SIGTERM.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).ok();
        let term_fut = async {
            match term.as_mut() {
                Some(t) => {
                    t.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term_fut => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Handle to an in-process worker started with [`Worker::spawn`].
pub struct WorkerHandle {
    notify: Arc<Notify>,
    task: JoinHandle<()>,
}

impl WorkerHandle {
    /// Signal the worker to stop reserving and drain in-flight jobs, then wait
    /// for it to finish.
    pub async fn shutdown(self) {
        self.notify.notify_one();
        let _ = self.task.await;
    }

    /// Stop the worker immediately without draining (aborts the task).
    pub fn abort(self) {
        self.task.abort();
    }
}
