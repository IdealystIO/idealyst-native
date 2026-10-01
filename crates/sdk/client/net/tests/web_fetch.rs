//! Browser tests for the web HTTP arm (`src/web.rs`): `fetch` through
//! web-glue bindings.
//!
//! Real `fetch` wherever a URL the test controls can produce the case:
//!
//! - **success / binary body / response headers** — a `blob:` URL over
//!   bytes the test minted (the browser answers 200 with the Blob's type as
//!   `Content-Type`), and a `data:` URL for UTF-8 text;
//! - **non-2xx** — a path wasm-bindgen-test-runner's own HTTP server does not
//!   serve (it answers 404);
//! - **network error** — loopback port 1, which refuses immediately.
//!
//! What no browser-side URL can produce deterministically — a server that
//! never answers (cancel, timeout, a dropped future), and seeing what was
//! SENT (method, headers, body) — runs against a stand-in `fetch` installed
//! per test. Like the real one it takes `(input, init)` and normalises them
//! through `new Request(input, init)` (so it does not care how the transport
//! spells the call), answers with a real `Response` object, and honours the
//! request's `AbortSignal` the way the spec's fetch does (rejects with an
//! `AbortError` on abort) — only the network itself is faked.
//!
//! Run with `cargo test -p net --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use net::{cancel_token, Client, Error};
use wasm_bindgen_test::*;
use web_glue::{JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

fn eval(body: &str) -> JsValue {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap()
}

fn global(name: &str) -> JsValue {
    JsValue::global().get(name).unwrap()
}

async fn sleep(ms: u32) {
    let p = eval(&format!("return new Promise((r) => setTimeout(r, {ms}));"));
    JsFuture::new(&p).await.unwrap();
}

/// `fut`, or `None` once `ms` passed without it finishing — so a regression
/// that hangs fails as an assertion instead of stalling the suite.
async fn within<F: Future>(ms: u32, fut: F) -> Option<F::Output> {
    let mut fut = Box::pin(fut);
    let mut timer = Box::pin(sleep(ms));
    poll_fn(|cx| {
        if let Poll::Ready(v) = fut.as_mut().poll(cx) {
            return Poll::Ready(Some(v));
        }
        if let Poll::Ready(()) = Pin::new(&mut timer).poll(cx) {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

/// Replace `globalThis.fetch` until the guard drops.
struct FetchOverride;

impl FetchOverride {
    fn install(fetch_src: &str) -> FetchOverride {
        real_fetch();
        eval(&format!(
            "globalThis.__savedFetch = globalThis.fetch; globalThis.__fetchAborted = false; \
             globalThis.fetch = {fetch_src};"
        ));
        FetchOverride
    }
}

impl Drop for FetchOverride {
    fn drop(&mut self) {
        real_fetch();
    }
}

/// Put the browser's `fetch` back. A failed test panics without running its
/// guard's `Drop` (wasm aborts), so a test that needs the real network calls
/// this first rather than inherit a previous test's stand-in.
fn real_fetch() {
    eval(
        "if (globalThis.__savedFetch) { globalThis.fetch = globalThis.__savedFetch; \
           delete globalThis.__savedFetch; } delete globalThis.__lastReq;",
    );
}

/// A server that never answers: pending until the request's signal aborts,
/// then rejected with the spec's `AbortError`, and the abort recorded.
const HANGING_FETCH: &str = "(input, init) => { const req = new Request(input, init); \
    globalThis.__lastReq = req; \
    return new Promise((_, reject) => { req.signal.addEventListener('abort', () => { \
      globalThis.__fetchAborted = true; \
      reject(new DOMException('The user aborted a request.', 'AbortError')); }); }); }";

/// A blob: URL serving `bytes` with `type`.
fn blob_url(bytes: &[u8], ty: &str) -> String {
    let list = bytes.iter().map(u8::to_string).collect::<Vec<_>>().join(",");
    eval(&format!(
        "return URL.createObjectURL(new Blob([new Uint8Array([{list}])], {{ type: '{ty}' }}));"
    ))
    .as_string()
    .unwrap()
}

#[wasm_bindgen_test]
async fn a_binary_body_and_its_headers_come_back_byte_exact() {
    real_fetch();
    let all: Vec<u8> = (0..=255).collect();
    let url = blob_url(&all, "application/x-test");
    let resp = Client::new().get(&url).send().await.expect("blob fetch succeeds");
    assert_eq!(resp.status(), 200);
    assert!(resp.is_success());
    assert_eq!(resp.header("Content-Type"), Some("application/x-test"));
    assert_eq!(resp.header("content-length"), Some("256"));
    assert_eq!(resp.bytes().await.unwrap(), all);
}

#[wasm_bindgen_test]
async fn a_data_url_body_decodes_as_utf8_text() {
    real_fetch();
    let resp = Client::new()
        .get("data:text/plain;charset=utf-8,h%C3%A9llo%20%E2%9C%93")
        .send()
        .await
        .expect("data: fetch succeeds");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "héllo ✓");
}

#[wasm_bindgen_test]
async fn a_non_2xx_is_a_response_and_error_for_status_maps_it() {
    real_fetch();
    // Relative: resolved against the test page's origin (the runner's server).
    let resp = Client::new()
        .get("/__net_web_fetch_missing")
        .send()
        .await
        .expect("a 404 is a response, not a transport error");
    assert_eq!(resp.status(), 404);
    assert!(!resp.is_success());
    match resp.error_for_status() {
        Err(Error::Status { code: 404, .. }) => {}
        other => panic!("expected Error::Status 404, got {other:?}"),
    }
}

#[wasm_bindgen_test]
async fn a_refused_connection_is_a_network_error() {
    real_fetch();
    match Client::new().get("http://127.0.0.1:1/").send().await {
        Err(Error::Network(msg)) => assert!(!msg.is_empty(), "the browser's message is carried"),
        other => panic!("expected Error::Network, got {other:?}"),
    }
}

#[wasm_bindgen_test]
async fn method_headers_and_a_binary_body_reach_fetch() {
    // Echoes the request body back with a 201 and two response headers,
    // one of them repeated (fetch joins repeats with ", ").
    let _f = FetchOverride::install(
        "async (input, init) => { const req = new Request(input, init); globalThis.__lastReq = req; \
           const h = new Headers({ 'x-reply': 'yes' }); h.append('x-multi', 'a'); h.append('x-multi', 'b'); \
           return new Response(await req.arrayBuffer(), { status: 201, headers: h }); }",
    );
    let body = vec![0u8, 1, 2, 127, 128, 254, 255];
    let resp = Client::builder()
        .default_header("X-Default", "d")
        .build()
        .post("https://example.invalid/upload")
        .body(body.clone())
        .send()
        .await
        .expect("stub fetch answers");

    let req = global("__lastReq");
    assert_eq!(req.get("method").unwrap().as_string().as_deref(), Some("POST"));
    assert_eq!(req.get("url").unwrap().as_string().as_deref(), Some("https://example.invalid/upload"));
    let headers = req.get("headers").unwrap();
    let get = |n: &str| headers.call_method("get", &[&JsValue::from_str(n)]).unwrap().as_string();
    assert_eq!(get("x-default").as_deref(), Some("d"));
    assert_eq!(get("content-type").as_deref(), Some("application/octet-stream"));

    assert_eq!(resp.status(), 201);
    assert_eq!(resp.header("x-reply"), Some("yes"));
    assert_eq!(resp.header("x-multi"), Some("a, b"));
    assert_eq!(resp.bytes().await.unwrap(), body, "the request body crossed byte-exact");
}

/// Regression: gloo-net's `Headers` wrapper only offered `set`, so a header
/// the caller appended twice (`RequestBuilder::header` documents "allows
/// duplicates") reached the browser as its LAST value only — `X-Dup: two`.
/// Every native arm sends both.
#[wasm_bindgen_test]
async fn regression_repeated_request_headers_are_all_sent() {
    let _f = FetchOverride::install(
        "async (input, init) => { globalThis.__lastReq = new Request(input, init); \
           return new Response('', { status: 200 }); }",
    );
    Client::new()
        .get("https://example.invalid/")
        .header("X-Dup", "one")
        .header("X-Dup", "two")
        .send()
        .await
        .unwrap();
    let headers = global("__lastReq").get("headers").unwrap();
    let dup = headers.call_method("get", &[&JsValue::from_str("x-dup")]).unwrap().as_string();
    assert_eq!(dup.as_deref(), Some("one, two"), "appended, not replaced");
}

#[wasm_bindgen_test]
async fn an_empty_body_sends_no_body() {
    let _f = FetchOverride::install(
        "async (input, init) => { globalThis.__lastReq = new Request(input, init); \
           return new Response(null, { status: 204 }); }",
    );
    let resp = Client::new().get("https://example.invalid/").send().await.unwrap();
    assert_eq!(resp.status(), 204);
    assert!(global("__lastReq").get("body").unwrap().is_null(), "no body stream at all");
    assert!(resp.bytes().await.unwrap().is_empty());
}

/// Regression: gloo-net built the request `Headers` with `unwrap_throw`, so
/// a header name the browser refuses threw out of `send` — an uncaught JS
/// exception through the wasm frames, not an `Err`. It is now a
/// `Error::Network`, like every other request the browser will not make.
#[wasm_bindgen_test]
async fn regression_an_invalid_header_name_is_an_error_not_a_throw() {
    real_fetch();
    match Client::new().get("data:,x").header("bad header", "v").send().await {
        Err(Error::Network(msg)) => assert!(!msg.is_empty()),
        other => panic!("expected Error::Network, got {other:?}"),
    }
}

#[wasm_bindgen_test]
async fn a_get_with_a_body_is_refused_as_a_network_error() {
    real_fetch();
    // The browser will not construct it; same mapping as under gloo.
    match Client::new().get("data:,x").body(vec![1u8]).send().await {
        Err(Error::Network(_)) => {}
        other => panic!("expected Error::Network, got {other:?}"),
    }
}

#[wasm_bindgen_test]
async fn cancelling_mid_flight_aborts_the_fetch() {
    let _f = FetchOverride::install(HANGING_FETCH);
    let (handle, token) = cancel_token();
    let req = Client::new().get("https://example.invalid/slow").cancel_on(token).send();
    let canceller = async {
        sleep(10).await;
        assert!(!global("__fetchAborted").as_bool().unwrap(), "not aborted before the cancel");
        handle.cancel();
    };
    let (result, ()) = futures_util::future::join(req, canceller).await;
    assert!(matches!(result, Err(Error::Cancelled)), "got {result:?}");
    assert_eq!(global("__fetchAborted").as_bool(), Some(true), "the browser request was torn down");
}

#[wasm_bindgen_test]
async fn cancelling_a_real_fetch_resolves_cancelled() {
    real_fetch();
    let (handle, token) = cancel_token();
    let req = Client::new().get("/__net_web_fetch_cancelled").cancel_on(token).send();
    let canceller = async { handle.cancel() };
    let (result, ()) = futures_util::future::join(req, canceller).await;
    assert!(matches!(result, Err(Error::Cancelled)), "got {result:?}");
}

/// Regression: the gloo arm ignored `RequestBuilder::timeout` /
/// `ClientBuilder::timeout` (`_timeout`), so a request to a server that never
/// answered hung forever on web while it was `Error::Timeout` natively.
#[wasm_bindgen_test]
async fn regression_a_timeout_aborts_and_resolves_timeout() {
    let _f = FetchOverride::install(HANGING_FETCH);
    let req = Client::new()
        .get("https://example.invalid/never")
        .timeout(Duration::from_millis(20))
        .send();
    let result = within(2_000, req).await.expect("the timeout fired instead of hanging");
    assert!(matches!(result, Err(Error::Timeout)), "got {result:?}");
    assert_eq!(global("__fetchAborted").as_bool(), Some(true));
}

#[wasm_bindgen_test]
async fn a_client_default_timeout_applies_and_a_request_override_wins() {
    let _f = FetchOverride::install(HANGING_FETCH);
    let client = Client::builder().timeout(Duration::from_millis(20)).build();
    let r = within(2_000, client.get("https://example.invalid/a").send()).await.unwrap();
    assert!(matches!(r, Err(Error::Timeout)), "got {r:?}");

    // A longer per-request timeout: still pending when the client default
    // would have fired.
    let r = within(100, client.get("https://example.invalid/b").timeout(Duration::from_secs(30)).send()).await;
    assert!(r.is_none(), "the per-request timeout overrides the client default");
}

#[wasm_bindgen_test]
async fn a_fast_response_inside_its_timeout_succeeds() {
    real_fetch();
    let url = blob_url(b"ok", "text/plain");
    let resp = Client::new().get(&url).timeout(Duration::from_secs(5)).send().await.unwrap();
    assert_eq!(resp.text().await.unwrap(), "ok");
}

/// Regression: dropping the `send` future (a component unmounting, a
/// `select!` losing) left the gloo arm's browser request running to
/// completion, body download included. The reqwest arm aborts on drop; the
/// web arm now does too.
#[wasm_bindgen_test]
async fn regression_dropping_the_send_future_aborts_the_fetch() {
    let _f = FetchOverride::install(HANGING_FETCH);
    let pending = within(10, Client::new().get("https://example.invalid/dropped").send()).await;
    assert!(pending.is_none(), "still in flight when dropped");
    assert_eq!(
        global("__fetchAborted").as_bool(),
        Some(true),
        "dropping the future tore the browser request down"
    );
}

#[wasm_bindgen_test]
async fn a_cancel_after_completion_is_harmless() {
    real_fetch();
    let url = blob_url(b"done", "text/plain");
    let (handle, token) = cancel_token();
    let resp = Client::new().get(&url).cancel_on(token).send().await.unwrap();
    handle.cancel();
    assert_eq!(resp.text().await.unwrap(), "done");
}
