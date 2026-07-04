//! Live host tests for the Postgres LISTEN/NOTIFY pub/sub backend. `#[ignore]`.
//!
//! ```sh
//! PUBSUB_TEST_PG_URL=postgres://postgres@127.0.0.1/postgres \
//!   cargo test -p pubsub --features postgres -- --ignored
//! ```
#![cfg(feature = "postgres")]

use futures_util::StreamExt;
use pubsub::{PostgresBackend, PubSubBackend};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

fn url() -> String {
    std::env::var("PUBSUB_TEST_PG_URL").expect("PUBSUB_TEST_PG_URL")
}

static TOPIC_SEQ: AtomicU32 = AtomicU32::new(0);

fn topic() -> String {
    format!(
        "pubsubtest:{}:{}",
        std::process::id(),
        TOPIC_SEQ.fetch_add(1, Ordering::SeqCst)
    )
}

#[tokio::test]
#[ignore = "requires a live Postgres (PUBSUB_TEST_PG_URL)"]
async fn pg_notify_fans_out() {
    let b = PostgresBackend::connect(&url()).await.unwrap();
    let t = topic();

    let mut sub = b.subscribe(&t).await.unwrap();
    // LISTEN registers before we NOTIFY; a small delay avoids the race.
    tokio::time::sleep(Duration::from_millis(100)).await;

    b.publish(&t, b"hello").await.unwrap();

    let got = tokio::time::timeout(Duration::from_secs(2), sub.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, b"hello".to_vec());
}

#[tokio::test]
#[ignore = "requires a live Postgres (PUBSUB_TEST_PG_URL)"]
async fn pg_topics_are_isolated() {
    let b = PostgresBackend::connect(&url()).await.unwrap();
    let t = topic();
    let other = topic();

    let mut sub = b.subscribe(&t).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    b.publish(&other, b"nope").await.unwrap();
    b.publish(&t, b"yes").await.unwrap();

    let got = tokio::time::timeout(Duration::from_secs(2), sub.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, b"yes".to_vec());
}
