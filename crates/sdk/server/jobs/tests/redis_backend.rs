//! Live host tests for the Redis backend. `#[ignore]` by default — they need a
//! reachable Redis. Run with:
//!
//! ```sh
//! JOBS_TEST_REDIS_URL=redis://127.0.0.1:6379 \
//!   cargo test -p jobs --features redis -- --ignored
//! ```
//!
//! Each test uses a unique key namespace, so a shared Redis stays clean and
//! tests don't collide.
#![cfg(feature = "redis")]

use jobs::{Backoff, OutgoingJob, QueueBackend, RedisBackend, ReserveOpts};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

fn url() -> String {
    std::env::var("JOBS_TEST_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

static NS_SEQ: AtomicU32 = AtomicU32::new(0);

async fn backend() -> RedisBackend {
    let ns = format!(
        "jobstest:{}:{}",
        std::process::id(),
        NS_SEQ.fetch_add(1, Ordering::SeqCst)
    );
    RedisBackend::connect_ns(&url(), &ns)
        .await
        .expect("connect to redis")
}

fn job(name: &str) -> OutgoingJob {
    OutgoingJob::new(name, name.as_bytes().to_vec())
}

fn opts() -> ReserveOpts {
    ReserveOpts::default()
}

#[tokio::test]
#[ignore = "requires a live Redis (JOBS_TEST_REDIS_URL)"]
async fn redis_enqueue_reserve_ack_roundtrip() {
    let b = backend().await;
    b.enqueue(job("hello")).await.unwrap();

    let r = b.reserve(&opts()).await.unwrap().expect("a job");
    assert_eq!(r.name, "hello");
    assert_eq!(r.payload, b"hello");
    assert_eq!(r.attempt, 1);

    b.ack(&r).await.unwrap();
    assert!(b.reserve(&opts()).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires a live Redis (JOBS_TEST_REDIS_URL)"]
async fn redis_retry_bumps_attempt_and_delays() {
    let b = backend().await;
    b.enqueue(job("retryme")).await.unwrap();

    let r1 = b.reserve(&opts()).await.unwrap().unwrap();
    assert_eq!(r1.attempt, 1);
    b.retry(&r1, Duration::from_millis(300)).await.unwrap();

    // Still delayed.
    assert!(b.reserve(&opts()).await.unwrap().is_none());
    tokio::time::sleep(Duration::from_millis(400)).await;
    let r2 = b.reserve(&opts()).await.unwrap().unwrap();
    assert_eq!(r2.attempt, 2, "attempt persists across retry");
    b.ack(&r2).await.unwrap();
}

#[tokio::test]
#[ignore = "requires a live Redis (JOBS_TEST_REDIS_URL)"]
async fn redis_expired_lease_is_reclaimed() {
    let b = backend().await;
    b.enqueue(job("leaky")).await.unwrap();

    let short = ReserveOpts {
        queues: vec!["default".into()],
        visibility: Duration::from_millis(200),
    };
    let r1 = b.reserve(&short).await.unwrap().unwrap();
    assert_eq!(r1.attempt, 1);
    // Worker "vanishes": no ack. Lease should time out and re-deliver.
    assert!(b.reserve(&short).await.unwrap().is_none());
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r2 = b.reserve(&short).await.unwrap().unwrap();
    assert_eq!(r2.attempt, 2, "reclaimed lease bumps the attempt");
    b.ack(&r2).await.unwrap();
}

#[tokio::test]
#[ignore = "requires a live Redis (JOBS_TEST_REDIS_URL)"]
async fn redis_dead_letter_removes_from_rotation() {
    let b = backend().await;
    b.enqueue(job("doomed")).await.unwrap();
    let r = b.reserve(&opts()).await.unwrap().unwrap();
    b.dead_letter(&r, "boom").await.unwrap();
    assert!(b.reserve(&opts()).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires a live Redis (JOBS_TEST_REDIS_URL)"]
async fn redis_delay_defers_reservation() {
    let b = backend().await;
    let mut j = job("later");
    j.backoff = Backoff::None;
    j.delay = Some(Duration::from_millis(300));
    b.enqueue(j).await.unwrap();

    assert!(b.reserve(&opts()).await.unwrap().is_none());
    tokio::time::sleep(Duration::from_millis(400)).await;
    let r = b.reserve(&opts()).await.unwrap().unwrap();
    assert_eq!(r.name, "later");
    b.ack(&r).await.unwrap();
}
