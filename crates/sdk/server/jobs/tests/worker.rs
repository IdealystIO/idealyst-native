//! End-to-end: `#[job]` registration + typed enqueue + the worker runtime,
//! against the in-memory backend. Exercises the full path the macro generates
//! (payload encode → enqueue → reserve → decode → `State<T>` injection →
//! handler → ack/retry/dead-letter). Requires `--features server` (which enables
//! both the worker runtime and, via the same feature `#[job]` gates on, the
//! handler registration).
#![cfg(feature = "server")]

use jobs::{job, Backoff, JobError, MemoryBackend, QueueBackend};
use server::State;
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
use std::sync::Arc;
use std::time::Duration;

/// A shared counter injected via `server::install_state`, so tests can observe
/// handler execution.
type Counter = Arc<AtomicU32>;

#[job]
async fn add_to_counter(by: u32, amount: State<Counter>) -> Result<(), JobError> {
    amount.fetch_add(by, SeqCst);
    Ok(())
}

/// Fails on its first attempt, succeeds on the second — proves retry works and
/// that the attempt counter advances.
#[job]
async fn flaky(state: State<Counter>) -> Result<(), JobError> {
    let seen = state.fetch_add(1, SeqCst);
    if seen == 0 {
        Err(JobError::new("transient failure"))
    } else {
        Ok(())
    }
}

/// Always fails — used to prove dead-lettering after `max_attempts`.
#[job]
async fn always_fails(_marker: u8) -> Result<(), JobError> {
    Err(JobError::new("nope"))
}

/// `jobs::configure` and `server::install_state` are process-global (by design —
/// they mirror the server SDK's one-backend/one-state-registry model), so these
/// end-to-end tests can't run concurrently. Serialize them behind one lock.
fn test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Poll a condition for up to `timeout`, so tests don't hard-code sleeps.
async fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cond()
}

#[tokio::test]
async fn enqueue_then_worker_runs_the_handler_with_injected_state() {
    let _guard = test_lock().lock().await;
    let backend = MemoryBackend::new();
    jobs::configure(backend.clone());
    let counter: Counter = Arc::new(AtomicU32::new(0));
    server::install_state(counter.clone());

    add_to_counter::enqueue(7).await.unwrap();

    let handle = jobs::worker()
        .backend(backend.clone())
        .poll_interval(Duration::from_millis(5))
        .concurrency(2)
        .spawn();

    assert!(
        wait_until(Duration::from_secs(2), || counter.load(SeqCst) == 7).await,
        "worker should have run the job and incremented the counter"
    );
    handle.shutdown().await;
    assert_eq!(backend.dead_count(), 0);
}

#[tokio::test]
async fn failed_job_retries_then_succeeds() {
    let _guard = test_lock().lock().await;
    let backend = MemoryBackend::new();
    jobs::configure(backend.clone());
    let runs: Counter = Arc::new(AtomicU32::new(0));
    server::install_state(runs.clone());

    // No backoff so the retry is immediate and the test stays fast.
    flaky::enqueue()
        .max_attempts(5)
        .backoff(Backoff::None)
        .await
        .unwrap();

    let handle = jobs::worker()
        .backend(backend.clone())
        .poll_interval(Duration::from_millis(5))
        .spawn();

    // Two runs total: attempt 1 fails, attempt 2 succeeds.
    assert!(
        wait_until(Duration::from_secs(2), || runs.load(SeqCst) == 2).await,
        "job should have run twice (fail then succeed)"
    );
    // Give the ack a beat, then confirm it settled cleanly (not dead-lettered).
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.shutdown().await;
    assert_eq!(backend.dead_count(), 0);
    assert_eq!(backend.pending_count(), 0);
    assert_eq!(backend.in_flight_count(), 0);
}

#[tokio::test]
async fn exhausted_retries_dead_letter() {
    let _guard = test_lock().lock().await;
    let backend = MemoryBackend::new();
    jobs::configure(backend.clone());

    always_fails::enqueue(1u8)
        .max_attempts(3)
        .backoff(Backoff::None)
        .await
        .unwrap();

    let handle = jobs::worker()
        .backend(backend.clone())
        .poll_interval(Duration::from_millis(5))
        .spawn();

    assert!(
        wait_until(Duration::from_secs(2), || backend.dead_count() == 1).await,
        "job should dead-letter after exhausting its attempts"
    );
    handle.shutdown().await;
    assert_eq!(backend.pending_count(), 0);
    assert_eq!(backend.in_flight_count(), 0);
}

#[tokio::test]
async fn unknown_job_name_is_dead_lettered() {
    let _guard = test_lock().lock().await;
    // A job whose name no handler is registered for (e.g. a renamed/removed
    // job still sitting in the queue) must not spin forever.
    let backend = MemoryBackend::new();
    backend
        .enqueue(jobs::OutgoingJob::new("ghost_job", b"[]".to_vec()))
        .await
        .unwrap();
    jobs::configure(backend.clone());

    let handle = jobs::worker()
        .backend(backend.clone())
        .poll_interval(Duration::from_millis(5))
        .spawn();

    assert!(
        wait_until(Duration::from_secs(2), || backend.dead_count() == 1).await,
        "a job with no registered handler should be dead-lettered"
    );
    handle.shutdown().await;
}
