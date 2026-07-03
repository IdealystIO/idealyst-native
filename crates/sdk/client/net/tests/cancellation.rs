//! Regression tests for [`RequestBuilder::cancel_on`] + the
//! [`CancelToken`](net::CancelToken) primitive.
//!
//! All tests run against a hand-rolled hyper server (rather than the
//! `common` harness in `tests/common/mod.rs`) because some scenarios
//! need the server to hold the connection open indefinitely while the
//! client's cancel races the response — and the simpler harness
//! always replies eagerly.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use net::{cancel_token, Client, Error};
use tokio::net::TcpListener;

/// Spin up a hyper server whose handler sleeps `hold` before
/// responding `200 ""`. Used as a long-running endpoint to cancel
/// against.
async fn slow_server(hold: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hold = Arc::new(hold);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let hold = hold.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |_req: Request<Incoming>| {
                    let hold = *hold;
                    async move {
                        tokio::time::sleep(hold).await;
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .body(Full::new(Bytes::from("late")))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn regression_cancel_mid_flight_returns_cancelled_error() {
    // Server holds for 30s — far longer than the test deadline.
    // Cancellation must beat the sleep and resolve send() promptly.
    let base = slow_server(Duration::from_secs(30)).await;
    let (handle, token) = cancel_token();
    let client = Client::new();
    let request = client.get(format!("{base}/slow")).cancel_on(token);

    let cancel_in = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.cancel();
    });

    let start = std::time::Instant::now();
    let result = request.send().await;
    let elapsed = start.elapsed();

    cancel_in.await.unwrap();
    match result {
        Err(Error::Cancelled) => {}
        other => panic!("expected Error::Cancelled, got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(5),
        "cancel must resolve quickly, took {elapsed:?}"
    );
}

#[tokio::test]
async fn regression_cancel_before_send_short_circuits_with_no_request() {
    // Pre-cancel: the builder's send() should bail immediately
    // without ever opening a connection. We use 127.0.0.1:1 (discard
    // port — nothing listens) so if the request DID try to dial, the
    // test would surface a Network error instead of Cancelled.
    let (handle, token) = cancel_token();
    handle.cancel();
    let client = Client::new();
    let result = client
        .get("http://127.0.0.1:1/nope")
        .cancel_on(token)
        .send()
        .await;
    match result {
        Err(Error::Cancelled) => {}
        other => panic!("expected Error::Cancelled, got {other:?}"),
    }
}

#[tokio::test]
async fn regression_cancel_after_completion_has_no_effect() {
    // Server responds immediately. Cancelling the handle AFTER the
    // future has resolved is a no-op — the result is already in hand.
    let base = slow_server(Duration::from_millis(0)).await;
    let (handle, token) = cancel_token();
    let client = Client::new();
    let result = client
        .get(format!("{base}/fast"))
        .cancel_on(token)
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), 200);

    // Cancel after-the-fact — must not panic / poison anything.
    handle.cancel();
}

#[tokio::test]
async fn regression_single_handle_aborts_multiple_attached_requests() {
    // One CancelHandle attached to N requests via N cloned tokens.
    // Firing the handle once must abort all of them.
    let base = slow_server(Duration::from_secs(30)).await;
    let (handle, token) = cancel_token();
    let client = Client::new();

    let r1 = client.get(format!("{base}/a")).cancel_on(token.clone());
    let r2 = client.get(format!("{base}/b")).cancel_on(token.clone());
    let r3 = client.get(format!("{base}/c")).cancel_on(token);

    let cancel_in = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.cancel();
    });

    let (a, b, c) = tokio::join!(r1.send(), r2.send(), r3.send());
    cancel_in.await.unwrap();

    for (label, result) in [("a", a), ("b", b), ("c", c)] {
        match result {
            Err(Error::Cancelled) => {}
            other => panic!("expected request {label} to be Cancelled, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn regression_unrelated_cancel_does_not_affect_other_request() {
    // Two paired (handle, token)s — cancelling handle A must not
    // abort a request attached to token B.
    let base = slow_server(Duration::from_millis(0)).await;
    let (handle_a, _token_a) = cancel_token();
    let (_handle_b, token_b) = cancel_token();
    let client = Client::new();

    handle_a.cancel(); // unrelated

    let result = client
        .get(format!("{base}/ok"))
        .cancel_on(token_b)
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), 200);
}
