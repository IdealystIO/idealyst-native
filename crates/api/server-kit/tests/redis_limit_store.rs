//! Live host tests for `RedisLimitStore`. `#[ignore]` by default — they
//! need a running Redis. Run with:
//!
//! ```sh
//! KIT_TEST_REDIS_URL=redis://127.0.0.1:6379 \
//!   cargo test -p server-kit --features redis --test redis_limit_store -- --ignored
//! ```
#![cfg(feature = "redis")]

use server_kit::{Admitted, LimitStore, RedisLimitStore};

fn url() -> String {
    std::env::var("KIT_TEST_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

fn store(ns: &str) -> RedisLimitStore {
    let client = redis::Client::open(url()).expect("redis url");
    RedisLimitStore::new(client).namespace(ns.to_string())
}

/// A fresh namespace per test run so reruns don't inherit spent buckets.
fn fresh_ns(tag: &str) -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("kittest:{tag}:{nonce}")
}

/// Build a limit through the same grammar the `limit` tag uses.
fn test_limit(spec: &str) -> server_kit::Limit {
    server_kit::Limit::parse(spec).expect("valid spec")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live Redis (KIT_TEST_REDIS_URL)"]
async fn burst_then_deny_with_retry_after() {
    let store = store(&fresh_ns("burst"));
    let limit = test_limit("2/min");

    assert_eq!(store.admit("send", "user:a", limit).await.unwrap(), Admitted::Allowed);
    assert_eq!(store.admit("send", "user:a", limit).await.unwrap(), Admitted::Allowed);
    match store.admit("send", "user:a", limit).await.unwrap() {
        Admitted::Denied { retry_secs } => {
            assert!((1..=30).contains(&retry_secs), "got retry {retry_secs}");
        }
        other => panic!("third call must be denied, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live Redis (KIT_TEST_REDIS_URL)"]
async fn buckets_isolate_by_route_and_key() {
    let store = store(&fresh_ns("iso"));
    let limit = test_limit("1/min");

    assert_eq!(store.admit("send", "user:a", limit).await.unwrap(), Admitted::Allowed);
    assert!(matches!(
        store.admit("send", "user:a", limit).await.unwrap(),
        Admitted::Denied { .. }
    ));
    // Different key / different route: fresh buckets.
    assert_eq!(store.admit("send", "user:b", limit).await.unwrap(), Admitted::Allowed);
    assert_eq!(store.admit("other", "user:a", limit).await.unwrap(), Admitted::Allowed);
}

/// Two store instances (≈ two server processes) share one bucket — the
/// property the memory store cannot provide.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live Redis (KIT_TEST_REDIS_URL)"]
async fn instances_share_buckets() {
    let ns = fresh_ns("shared");
    let a = store(&ns);
    let b = store(&ns);
    let limit = test_limit("2/min");

    assert_eq!(a.admit("send", "user:x", limit).await.unwrap(), Admitted::Allowed);
    assert_eq!(b.admit("send", "user:x", limit).await.unwrap(), Admitted::Allowed);
    // Capacity 2 is now spent ACROSS instances.
    assert!(matches!(
        a.admit("send", "user:x", limit).await.unwrap(),
        Admitted::Denied { .. }
    ));
    assert!(matches!(
        b.admit("send", "user:x", limit).await.unwrap(),
        Admitted::Denied { .. }
    ));
}

/// Store outage surfaces as StoreUnavailable (which the middleware turns
/// into fail-open) — not a panic, not a hang, and CRUCIALLY not slow:
/// requests stall while the connect attempt runs, so the failure must be
/// bounded. Regression: the manager's default retry schedule took ~7
/// minutes to give up on an unreachable host.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live Redis (KIT_TEST_REDIS_URL)"]
async fn unreachable_redis_fails_unavailable_fast() {
    let client = redis::Client::open("redis://127.0.0.1:1").expect("url parses");
    let store = RedisLimitStore::new(client);
    let limit = test_limit("1/min");
    let err = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        store.admit("send", "user:a", limit),
    )
    .await
    .expect("store failure must be bounded (seconds), not the manager's retry schedule")
    .expect_err("unreachable redis must be StoreUnavailable");
    assert!(err.0.contains("redis"), "got: {}", err.0);
}
