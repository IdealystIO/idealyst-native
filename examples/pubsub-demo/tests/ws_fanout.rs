//! End-to-end proof of the headline feature: a message published server-side
//! (via the `say` `#[server]` fn) fans out to a client connected over the
//! `room_feed` `#[subscription]` WebSocket. Run with `--features server`.
//!
//! This composes the whole path — `pubsub::publish` → the shared backend →
//! `pubsub::subscribe` inside the subscription body → the macro's socket pump →
//! the WebSocket client — against the in-memory backend in one process.
#![cfg(feature = "server")]

use pubsub_demo::{say, ChatMsg};
use std::time::Duration;
use tokio::net::TcpListener;

#[tokio::test]
async fn published_message_reaches_ws_subscriber() {
    // Single shared backend for both the publisher and the subscription body.
    pubsub::configure(pubsub::MemoryBackend::new());

    // Boot the app's router (folds in the #[subscription] WsEntry) on an
    // ephemeral port.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = server::router();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // Connect a client to the subscription WebSocket.
    let mut ws = net::WebSocket::connect(&format!("ws://{addr}/_srv/_ws/room_feed"))
        .await
        .expect("ws connect");
    // Let the server-side subscription register its backend subscription before
    // we publish (memory broadcast only delivers to receivers that exist).
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Publish by invoking the #[server] fn body directly (runs server-side).
    say("alice".to_string(), "hello world".to_string())
        .await
        .expect("say");

    // The subscriber should receive the fanned-out message.
    let frame = tokio::time::timeout(Duration::from_secs(2), ws.recv())
        .await
        .expect("message should arrive")
        .expect("stream open")
        .expect("frame ok");
    let text = match frame {
        net::WsMessage::Text(t) => t,
        other => panic!("expected a text frame, got {other:?}"),
    };
    let got: ChatMsg = serde_json::from_str(&text).unwrap();
    assert_eq!(
        got,
        ChatMsg {
            from: "alice".to_string(),
            text: "hello world".to_string(),
        }
    );
}
