//! Browser regression: a JS socket / stream must never be left holding
//! event closures that have already been dropped.
//!
//! Dropping a Rust closure handed to JS revokes the JS function that
//! forwards into wasm, but it does not unregister the function from the
//! object it was assigned to. The web `WebSocket` and `EventSource` arms
//! both dropped their closures on the connect-failure path (`result?`)
//! while the underlying JS object was still live and still about to
//! emit — `close` for the socket, an auto-retry `error` for the stream.
//! The browser then invoked the dead function, which throws into the
//! event loop — wasm-bindgen's
//!
//! ```text
//! closure invoked recursively or after being dropped
//! ```
//!
//! and, since the port to web-glue, `callback #N called after its Rust
//! owner dropped it`. It does not trap the module, so nothing
//! user-visible breaks; it buries the console in exceptions on exactly
//! the connections someone is debugging. Both arms detach their handler
//! slots from the JS object in the closures' own `Drop`.
//!
//! Browser-only, because the whole mechanism is: run with
//! `cargo test -p net --target wasm32-unknown-unknown` (the workspace
//! runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::*;
use web_glue::{JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

/// Substrings of the errors a dead callback throws: web-glue's, and
/// wasm-bindgen's (kept so the test still means something if a binding
/// ever goes back through wasm-bindgen).
const DROPPED: [&str; 2] = ["after its Rust owner dropped it", "after being dropped"];

/// Loopback port 1 refuses immediately on both schemes, so a connect
/// failure needs no test server.
const REFUSED_WS: &str = "ws://127.0.0.1:1";
const REFUSED_SSE: &str = "http://127.0.0.1:1/events";

fn eval(body: &str) -> JsValue {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap()
}

/// An exception thrown inside an event handler is uncaught, so it
/// surfaces as a window `error` event. This collects them so a test can
/// assert on what the console would have shown.
struct ErrorSpy;

impl ErrorSpy {
    fn install() -> Self {
        eval(
            "globalThis.__netErrs = []; \
             globalThis.__netSpy = (e) => __netErrs.push(String(e.message)); \
             window.addEventListener('error', __netSpy);",
        );
        ErrorSpy
    }

    /// Uncaught messages naming a dropped closure.
    fn dropped_closure_errors(&self) -> Vec<String> {
        let joined = eval("return __netErrs.join('\\n');").as_string().unwrap_or_default();
        joined
            .lines()
            .filter(|m| DROPPED.iter().any(|d| m.contains(d)))
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for ErrorSpy {
    fn drop(&mut self) {
        eval("window.removeEventListener('error', __netSpy);");
    }
}

/// Yield to the event loop for `ms`, so the browser can deliver the
/// events that follow a failed connect.
async fn settle(ms: i32) {
    let p = eval(&format!("return new Promise((r) => setTimeout(r, {ms}));"));
    let _ = JsFuture::new(&p).await;
}

/// A refused handshake is followed by a `close` event. Before the fix
/// that event invoked the just-dropped `onclose` shim.
#[wasm_bindgen_test]
async fn regression_failed_ws_connect_leaves_no_dead_handlers() {
    let spy = ErrorSpy::install();

    let res = net::WebSocket::connect(REFUSED_WS).await;
    assert!(res.is_err(), "loopback port 1 must refuse the handshake");

    settle(500).await;

    let errors = spy.dropped_closure_errors();
    assert!(
        errors.is_empty(),
        "the close event after a failed handshake reached a dropped closure: {errors:?}"
    );
}

/// An `EventSource` whose connect fails RETRIES on a browser-chosen
/// timer unless it is closed, so a stale handler slot fires again and
/// again. The wait spans Chrome's default ~3s retry.
#[wasm_bindgen_test]
async fn regression_failed_sse_connect_leaves_no_dead_handlers() {
    let spy = ErrorSpy::install();

    let res = net::EventSource::connect(REFUSED_SSE).await;
    assert!(res.is_err(), "loopback port 1 must refuse the stream");

    settle(4500).await;

    let errors = spy.dropped_closure_errors();
    assert!(
        errors.is_empty(),
        "an auto-retry after a failed stream connect reached a dropped closure: {errors:?}"
    );
}
