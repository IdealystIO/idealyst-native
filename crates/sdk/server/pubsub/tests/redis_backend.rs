//! Live host tests for the Redis pub/sub backend. `#[ignore]` — needs a Redis.
//!
//! ```sh
//! PUBSUB_TEST_REDIS_URL=redis://127.0.0.1:6379 \
//!   cargo test -p pubsub --features redis -- --ignored
//! ```
#![cfg(feature = "redis")]

use futures_util::StreamExt;
use pubsub::{PubSubBackend, RedisBackend};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

fn url() -> String {
    std::env::var("PUBSUB_TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

static TOPIC_SEQ: AtomicU32 = AtomicU32::new(0);

/// A per-test topic so a shared Redis doesn't cross-talk between tests.
fn topic() -> String {
    format!(
        "pubsubtest:{}:{}",
        std::process::id(),
        TOPIC_SEQ.fetch_add(1, Ordering::SeqCst)
    )
}

#[tokio::test]
#[ignore = "requires a live Redis (PUBSUB_TEST_REDIS_URL)"]
async fn redis_publish_fans_out_across_connections() {
    let b = RedisBackend::connect(&url()).await.unwrap();
    let t = topic();

    // Two independent subscribers (each its own Redis connection).
    let mut a = b.subscribe(&t).await.unwrap();
    let mut c = b.subscribe(&t).await.unwrap();
    // Redis SUBSCRIBE is asynchronous server-side; give it a beat to register.
    tokio::time::sleep(Duration::from_millis(100)).await;

    b.publish(&t, b"hello").await.unwrap();

    let ra = tokio::time::timeout(Duration::from_secs(2), a.next()).await.unwrap();
    let rc = tokio::time::timeout(Duration::from_secs(2), c.next()).await.unwrap();
    assert_eq!(ra.unwrap(), b"hello".to_vec());
    assert_eq!(rc.unwrap(), b"hello".to_vec());
}

#[tokio::test]
#[ignore = "requires a live Redis (PUBSUB_TEST_REDIS_URL)"]
async fn redis_topics_are_isolated() {
    let b = RedisBackend::connect(&url()).await.unwrap();
    let t = topic();
    let other = topic();

    let mut on_t = b.subscribe(&t).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    b.publish(&other, b"nope").await.unwrap();
    b.publish(&t, b"yes").await.unwrap();

    let got = tokio::time::timeout(Duration::from_secs(2), on_t.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, b"yes".to_vec(), "should only see its own topic");
}
