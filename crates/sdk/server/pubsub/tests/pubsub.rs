//! Memory-backend fan-out semantics + the typed `Topic<T>` surface over the
//! global backend. Backend-direct tests run in parallel; the `Topic` tests use
//! the process-global `configure`, so they serialize behind one lock.

use futures_util::StreamExt;
use pubsub::{MemoryBackend, PubSubBackend, Topic};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ---- Backend-direct tests (no global state) --------------------------------

#[tokio::test]
async fn publish_fans_out_to_every_subscriber() {
    let b = MemoryBackend::new();
    // Subscribe first — broadcast only delivers messages sent after subscribe.
    let mut a = b.subscribe("room").await.unwrap();
    let mut c = b.subscribe("room").await.unwrap();

    b.publish("room", b"hello").await.unwrap();

    assert_eq!(a.next().await.unwrap(), b"hello".to_vec());
    assert_eq!(c.next().await.unwrap(), b"hello".to_vec());
}

#[tokio::test]
async fn topics_are_isolated() {
    let b = MemoryBackend::new();
    let mut on_a = b.subscribe("a").await.unwrap();

    b.publish("b", b"for-b").await.unwrap();
    b.publish("a", b"for-a").await.unwrap();

    // The "a" subscriber sees only "a"'s message.
    assert_eq!(on_a.next().await.unwrap(), b"for-a".to_vec());
}

#[tokio::test]
async fn late_subscriber_misses_earlier_messages() {
    let b = MemoryBackend::new();
    b.publish("room", b"early").await.unwrap();

    // Subscribing after the publish: at-most-once, no replay.
    let mut late = b.subscribe("room").await.unwrap();
    b.publish("room", b"later").await.unwrap();

    assert_eq!(
        late.next().await.unwrap(),
        b"later".to_vec(),
        "late subscriber should see only messages published after it subscribed"
    );
}

// ---- Typed Topic over the global backend (serialized) ----------------------

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
struct ChatMsg {
    from: String,
    text: String,
}

fn test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[tokio::test]
async fn typed_topic_publish_subscribe_roundtrip() {
    let _guard = test_lock().lock().await;
    pubsub::configure(MemoryBackend::new());

    const CHAT: Topic<ChatMsg> = Topic::new("chat");
    let mut stream = Box::pin(CHAT.subscribe());

    // Prime the lazy stream: the first poll establishes the backend
    // subscription, then parks. A short timeout drives exactly that poll.
    let _ = tokio::time::timeout(Duration::from_millis(50), stream.next()).await;

    let msg = ChatMsg {
        from: "alice".into(),
        text: "hi".into(),
    };
    CHAT.publish(&msg).await.unwrap();

    let got = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("message should arrive")
        .expect("stream should yield");
    assert_eq!(got, msg);
}

#[tokio::test]
async fn free_publish_subscribe_functions_work() {
    let _guard = test_lock().lock().await;
    pubsub::configure(MemoryBackend::new());

    let mut stream = Box::pin(pubsub::subscribe::<u32>("counters"));
    let _ = tokio::time::timeout(Duration::from_millis(50), stream.next()).await;

    pubsub::publish("counters", &7u32).await.unwrap();

    let got = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("message should arrive")
        .expect("stream should yield");
    assert_eq!(got, 7);
}
