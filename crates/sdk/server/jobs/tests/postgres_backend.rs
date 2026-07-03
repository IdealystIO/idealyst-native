//! Live host tests for the Postgres backend. `#[ignore]` — they need a
//! reachable Postgres. Run with:
//!
//! ```sh
//! JOBS_TEST_PG_URL=postgres://postgres@127.0.0.1:5432/jobstest \
//!   cargo test -p jobs --features postgres -- --ignored
//! ```
//!
//! Each test uses a unique table name so a shared database stays clean.
#![cfg(feature = "postgres")]

use jobs::{OutgoingJob, PostgresBackend, QueueBackend, ReserveOpts};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

fn url() -> Option<String> {
    std::env::var("JOBS_TEST_PG_URL").ok()
}

static TBL_SEQ: AtomicU32 = AtomicU32::new(0);

async fn backend() -> PostgresBackend {
    let url = url().expect("JOBS_TEST_PG_URL");
    let table = format!(
        "jobstest_{}_{}",
        std::process::id(),
        TBL_SEQ.fetch_add(1, Ordering::SeqCst)
    );
    PostgresBackend::connect_table(&url, &table)
        .await
        .expect("connect to postgres")
}

fn job(name: &str) -> OutgoingJob {
    OutgoingJob::new(name, name.as_bytes().to_vec())
}

fn opts() -> ReserveOpts {
    ReserveOpts::default()
}

#[tokio::test]
#[ignore = "requires a live Postgres (JOBS_TEST_PG_URL)"]
async fn pg_enqueue_reserve_ack_roundtrip() {
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
#[ignore = "requires a live Postgres (JOBS_TEST_PG_URL)"]
async fn pg_retry_bumps_attempt_and_delays() {
    let b = backend().await;
    b.enqueue(job("retryme")).await.unwrap();

    let r1 = b.reserve(&opts()).await.unwrap().unwrap();
    assert_eq!(r1.attempt, 1);
    b.retry(&r1, Duration::from_millis(300)).await.unwrap();

    assert!(b.reserve(&opts()).await.unwrap().is_none());
    tokio::time::sleep(Duration::from_millis(400)).await;
    let r2 = b.reserve(&opts()).await.unwrap().unwrap();
    assert_eq!(r2.attempt, 2);
    b.ack(&r2).await.unwrap();
}

#[tokio::test]
#[ignore = "requires a live Postgres (JOBS_TEST_PG_URL)"]
async fn pg_expired_lease_is_reclaimed() {
    let b = backend().await;
    b.enqueue(job("leaky")).await.unwrap();

    let short = ReserveOpts {
        queues: vec!["default".into()],
        visibility: Duration::from_millis(200),
    };
    let r1 = b.reserve(&short).await.unwrap().unwrap();
    assert_eq!(r1.attempt, 1);
    assert!(b.reserve(&short).await.unwrap().is_none());
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r2 = b.reserve(&short).await.unwrap().unwrap();
    assert_eq!(r2.attempt, 2);
    b.ack(&r2).await.unwrap();
}

#[tokio::test]
#[ignore = "requires a live Postgres (JOBS_TEST_PG_URL)"]
async fn pg_dead_letter_removes_from_rotation() {
    let b = backend().await;
    b.enqueue(job("doomed")).await.unwrap();
    let r = b.reserve(&opts()).await.unwrap().unwrap();
    b.dead_letter(&r, "boom").await.unwrap();
    assert!(b.reserve(&opts()).await.unwrap().is_none());
}
