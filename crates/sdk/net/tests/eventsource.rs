//! Native EventSource (SSE) round-trip: `net::EventSource` (blocking
//! reqwest read on a worker thread + SSE parse) against a minimal raw
//! `text/event-stream` server. Proves connect / recv / frame parsing /
//! close-on-EOF and the futures-channel bridge into async.

#![cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serve one `text/event-stream` response carrying `events` as `data:`
/// frames, then close (EOF ends the stream client-side).
async fn sse_server(events: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            // Drain the request line/headers (don't care about contents).
            let mut scratch = [0u8; 1024];
            let _ = stream.read(&mut scratch).await;

            let mut body = String::from(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            for e in &events {
                body.push_str(&format!("data: {e}\n\n"));
            }
            let _ = stream.write_all(body.as_bytes()).await;
            let _ = stream.flush().await;
            // Drop `stream` → EOF.
        }
    });
    format!("http://{addr}/")
}

#[tokio::test]
async fn reads_events_then_closes_on_eof() {
    let url = sse_server(vec!["one".into(), "two".into(), "three".into()]).await;
    let mut es = net::EventSource::connect(&url).await.expect("connect");

    let mut got = Vec::new();
    while let Some(item) = es.recv().await {
        got.push(item.expect("not an error"));
        if got.len() == 3 {
            break;
        }
    }
    assert_eq!(got, vec!["one", "two", "three"]);

    // After the server closed, recv must terminate (EOF → None), not hang.
    let end = tokio::time::timeout(Duration::from_secs(2), es.recv()).await;
    assert!(matches!(end, Ok(None)), "recv must end on EOF, got {end:?}");
}

#[tokio::test]
async fn connect_error_surfaces() {
    let result = net::EventSource::connect("http://127.0.0.1:1/").await;
    assert!(result.is_err(), "connecting to a dead port must error");
}
