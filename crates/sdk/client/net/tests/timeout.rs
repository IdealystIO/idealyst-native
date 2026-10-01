//! `RequestBuilder::timeout` / `ClientBuilder::timeout` is a deadline on the
//! WHOLE exchange — connect, request, response head and body — and expiry
//! resolves `Error::Timeout`. Runs against whichever native transport this
//! target compiles: reqwest on Linux/Windows, NSURLSession on macOS (the
//! same `ios.rs` arm iOS and tvOS use; this binary also runs on the iOS
//! simulator via `xcrun simctl spawn`).
//!
//! Regression: the NSURLSession arm ignored the timeout. A server that
//! accepted the connection and never answered held the request for
//! NSURLSession's 60 s idle default, then failed as `Error::Network`; a
//! body trickling a byte at a time never timed out at all, because an idle
//! timeout restarts on every byte.

// Native-only: the stalling server is tokio, which cannot build for wasm32.
#![cfg(not(target_arch = "wasm32"))]

use std::time::{Duration, Instant};

use net::{Client, Error};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// How the test server treats each request after reading its head.
#[derive(Clone, Copy)]
enum Server {
    /// Accept, read the request, never write a byte (hold the socket).
    NeverRespond,
    /// Send a 200 head promising 100 kB, then one byte every 100 ms — a
    /// response that is never idle and never finishes.
    TrickleBody,
    /// Answer immediately with `200 OK` and the body `hi`.
    Prompt,
}

/// Boot a server with the given behavior on a random loopback port and
/// return its base URL.
async fn serve(behavior: Server) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                // Read the request head (up to the blank line). A body,
                // if any, is irrelevant here.
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                match behavior {
                    Server::NeverRespond => {
                        // Park on the socket until the client gives up.
                        let _ = stream.read(&mut buf).await;
                    }
                    Server::TrickleBody => {
                        let head = "HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\n";
                        if stream.write_all(head.as_bytes()).await.is_err() {
                            return;
                        }
                        loop {
                            if stream.write_all(b"x").await.is_err() {
                                return;
                            }
                            let _ = stream.flush().await;
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                    Server::Prompt => {
                        let reply = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\
                                     Connection: close\r\n\r\nhi";
                        let _ = stream.write_all(reply.as_bytes()).await;
                    }
                }
            });
        }
    });
    format!("http://{addr}")
}

/// How long a test waits before declaring the request's own deadline
/// ignored. Far above every timeout used here, far below NSURLSession's
/// 60 s idle default, so an ignored timeout fails fast instead of hanging.
const IGNORED_AFTER: Duration = Duration::from_secs(10);

/// Upper bound on how late a timeout may land. Generous: CI machines and
/// simulators stall, and the point is "bounded", not "precise".
const SLACK: Duration = Duration::from_secs(4);

async fn send_with_deadline(
    client: &Client,
    url: &str,
    timeout: Option<Duration>,
) -> Result<String, Error> {
    let mut req = client.get(url);
    if let Some(t) = timeout {
        req = req.timeout(t);
    }
    let send = async move { req.send().await?.text().await };
    tokio::time::timeout(IGNORED_AFTER, send)
        .await
        .unwrap_or_else(|_| {
            panic!("request ignored its timeout: still pending after {IGNORED_AFTER:?}")
        })
}

#[tokio::test]
async fn regression_timeout_fires_when_server_never_responds() {
    let url = serve(Server::NeverRespond).await;
    let timeout = Duration::from_millis(500);
    let start = Instant::now();
    let result = send_with_deadline(&Client::new(), &url, Some(timeout)).await;
    let elapsed = start.elapsed();
    assert!(
        matches!(result, Err(Error::Timeout)),
        "expected Error::Timeout, got {result:?}"
    );
    assert!(
        elapsed >= timeout - Duration::from_millis(50),
        "fired early: {elapsed:?}"
    );
    assert!(elapsed < timeout + SLACK, "fired late: {elapsed:?}");
}

/// The deadline covers the body: a response that keeps trickling bytes is
/// never idle, so only a whole-exchange deadline ends it.
#[tokio::test]
async fn regression_timeout_covers_a_body_that_never_finishes() {
    let url = serve(Server::TrickleBody).await;
    let timeout = Duration::from_millis(1_000);
    let start = Instant::now();
    let result = send_with_deadline(&Client::new(), &url, Some(timeout)).await;
    let elapsed = start.elapsed();
    assert!(
        matches!(result, Err(Error::Timeout)),
        "expected Error::Timeout, got {result:?}"
    );
    assert!(elapsed < timeout + SLACK, "fired late: {elapsed:?}");
}

#[tokio::test]
async fn regression_client_default_timeout_applies() {
    let url = serve(Server::NeverRespond).await;
    let client = Client::builder()
        .timeout(Duration::from_millis(500))
        .build();
    let result = send_with_deadline(&client, &url, None).await;
    assert!(
        matches!(result, Err(Error::Timeout)),
        "expected Error::Timeout, got {result:?}"
    );
}

#[tokio::test]
async fn timeout_longer_than_the_exchange_does_not_fire() {
    let url = serve(Server::Prompt).await;
    let body = send_with_deadline(&Client::new(), &url, Some(Duration::from_secs(5))).await;
    assert_eq!(body.unwrap(), "hi");
}
