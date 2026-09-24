//! The `idealyst dev` page stream, proxied same-origin by `server::router`
//! (see `server::dev_stream`).
//!
//! A stand-in for the dev loop's stream (a raw TCP listener answering the
//! way `dev-http` does: a hand-written `Connection: close` head, then SSE
//! frames written and flushed one at a time) sits behind
//! `dev_stream::routes`, and a real HTTP client reads through it.
//!
//! `cargo test -p server --features server --test dev_stream`
#![cfg(feature = "server")]

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// Serve `router` on a random port.
async fn serve(router: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// A dev stream that answers one connection: records the request it got,
/// then answers `/__idealyst/ack` with `204` and anything else with an SSE
/// head and one frame per message on `frames` (ending when it closes).
async fn fake_stream() -> (SocketAddr, mpsc::UnboundedSender<String>, mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<String>();
    let (req_tx, req_rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let mut got = Vec::new();
        // Read the head, then the body its Content-Length announces.
        loop {
            let n = sock.read(&mut buf).await.unwrap();
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                    .unwrap_or(0);
                if got.len() >= end + 4 + len || n == 0 {
                    break;
                }
            }
        }
        let request = String::from_utf8_lossy(&got).to_string();
        let is_ack = request.starts_with("POST /__idealyst/ack");
        req_tx.send(request).unwrap();
        if is_ack {
            sock.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nAccess-Control-Allow-Origin: *\r\n\r\n").await.unwrap();
            return;
        }
        sock.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
        while let Some(frame) = frame_rx.recv().await {
            sock.write_all(frame.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        }
    });
    (addr, frame_tx, req_rx)
}

/// Each SSE frame reaches the page as the stream writes it — the snapshot
/// first, then live events, in order — not when the response ends. A
/// proxy that buffered would hand the page nothing until the dev session
/// quit.
#[tokio::test(flavor = "multi_thread")]
async fn the_proxy_streams_each_frame_as_it_arrives_in_order() {
    let (upstream, frames, mut requests) = fake_stream().await;
    let app = server::dev_stream::routes(axum::Router::new(), upstream);
    let addr = serve(app).await;

    let response = reqwest::get(format!("http://{addr}/__idealyst/reload?since=3")).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()[server::dev_stream::HEADER], "1");
    let request = requests.recv().await.unwrap();
    assert!(request.starts_with("GET /__idealyst/reload?since=3 HTTP/1.1"), "{request}");

    let mut body = response.bytes_stream();
    let mut read = String::new();
    // The snapshot, then one live event after the first was already read.
    frames.send("data: 1\n\nevent: dev-state\ndata: {\"type\":\"session_started\"}\n\n".into()).unwrap();
    while !read.contains("session_started") {
        let chunk = tokio::time::timeout(Duration::from_secs(5), body.next())
            .await
            .expect("the snapshot was held back: the proxy buffers")
            .unwrap()
            .unwrap();
        read.push_str(&String::from_utf8_lossy(&chunk));
    }
    frames.send("event: dev-state\ndata: {\"type\":\"build_started\"}\n\n".into()).unwrap();
    while !read.contains("build_started") {
        let chunk = tokio::time::timeout(Duration::from_secs(5), body.next())
            .await
            .expect("a live event was held back: the proxy buffers")
            .unwrap()
            .unwrap();
        read.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert_eq!(
        read,
        "data: 1\n\nevent: dev-state\ndata: {\"type\":\"session_started\"}\n\n\
         event: dev-state\ndata: {\"type\":\"build_started\"}\n\n"
    );
    // The stream ends: so does the page's response.
    drop(frames);
    let end = tokio::time::timeout(Duration::from_secs(5), body.next()).await.unwrap();
    assert!(end.is_none(), "{end:?}");
}

/// The page's ack POST reaches the dev loop, body intact.
#[tokio::test(flavor = "multi_thread")]
async fn the_ack_is_forwarded() {
    let (upstream, _frames, mut requests) = fake_stream().await;
    let addr = serve(server::dev_stream::routes(axum::Router::new(), upstream)).await;
    let body = r#"{"kind":"hot_patch","redirected":3}"#;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/__idealyst/ack"))
        .header("content-type", "text/plain;charset=UTF-8")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    let request = requests.recv().await.unwrap();
    assert!(request.starts_with("POST /__idealyst/ack HTTP/1.1"), "{request}");
    assert!(request.ends_with(body), "{request}");
}

/// The probe the dev loop uses to learn the page can reach the stream
/// same-origin; and a stream that is gone is a 502, not a hang.
#[tokio::test(flavor = "multi_thread")]
async fn the_probe_answers_and_a_dead_stream_is_a_bad_gateway() {
    // Nothing listens here.
    let dead: SocketAddr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let addr = serve(server::dev_stream::routes(axum::Router::new(), dead)).await;
    let probe = reqwest::get(format!("http://{addr}{}", server::dev_stream::PROBE_PATH)).await.unwrap();
    assert_eq!(probe.status(), 204);
    assert_eq!(probe.headers()[server::dev_stream::HEADER], "1");
    let gone = reqwest::get(format!("http://{addr}/__idealyst/reload")).await.unwrap();
    assert_eq!(gone.status(), 502);
}

/// Without `IDEALYST_DEV_STREAM` — every binary `idealyst dev` did not
/// start, production included — `server::router()` has no such route.
#[tokio::test(flavor = "multi_thread")]
async fn the_route_is_absent_without_the_variable() {
    assert!(std::env::var_os(server::dev_stream::ENV).is_none(), "the test env must not set it");
    let addr = serve(server::router()).await;
    let probe = reqwest::get(format!("http://{addr}{}", server::dev_stream::PROBE_PATH)).await.unwrap();
    assert_eq!(probe.status(), 404);
    assert!(probe.headers().get(server::dev_stream::HEADER).is_none());
}
