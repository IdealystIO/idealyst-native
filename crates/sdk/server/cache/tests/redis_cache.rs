//! Live host tests for `RedisCache`. `#[ignore]` by default — they need
//! a running Redis. Run with:
//!
//! ```sh
//! CACHE_TEST_REDIS_URL=redis://127.0.0.1:6379 \
//!   cargo test -p cache --features redis --test redis_cache -- --ignored
//! ```
#![cfg(feature = "redis")]

use std::time::Duration;

use cache::{Cache, CacheExt, RedisCache};

fn store(ns: &str) -> RedisCache {
    let url = std::env::var("CACHE_TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let client = redis::Client::open(url).expect("redis url");
    RedisCache::new(client).namespace(ns.to_string())
}

fn fresh_ns(tag: &str) -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("cachetest:{tag}:{nonce}")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live Redis (CACHE_TEST_REDIS_URL)"]
async fn set_get_delete_round_trip() {
    let c = store(&fresh_ns("rt"));
    assert_eq!(c.get("k").await.unwrap(), None);
    c.set("k", b"v".to_vec(), None).await.unwrap();
    assert_eq!(c.get("k").await.unwrap(), Some(b"v".to_vec()));
    c.delete("k").await.unwrap();
    assert_eq!(c.get("k").await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live Redis (CACHE_TEST_REDIS_URL)"]
async fn ttl_expires_entries() {
    let c = store(&fresh_ns("ttl"));
    // 1s is the floor the backend enforces (SET EX takes whole seconds).
    c.set("k", b"v".to_vec(), Some(Duration::from_secs(1))).await.unwrap();
    assert_eq!(c.get("k").await.unwrap(), Some(b"v".to_vec()));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(c.get("k").await.unwrap(), None, "entry must expire after its TTL");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live Redis (CACHE_TEST_REDIS_URL)"]
async fn json_round_trip_and_instances_share_data() {
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Dash {
        count: u32,
    }
    let ns = fresh_ns("shared");
    let a = store(&ns);
    let b = store(&ns);
    a.set_json("d", &Dash { count: 7 }, None).await.unwrap();
    // A second instance (≈ another server process) reads the same entry.
    assert_eq!(b.get_json::<Dash>("d").await.unwrap(), Some(Dash { count: 7 }));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live Redis (CACHE_TEST_REDIS_URL)"]
async fn unreachable_redis_errors_fast() {
    let client = redis::Client::open("redis://127.0.0.1:1").expect("url parses");
    let c = RedisCache::new(client);
    let err = tokio::time::timeout(Duration::from_secs(10), c.get("k"))
        .await
        .expect("cache failure must be bounded seconds")
        .expect_err("unreachable redis must error");
    assert!(err.0.contains("redis"), "got: {}", err.0);
}
