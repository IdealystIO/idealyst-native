//! Browser regression: a JS socket / stream must never be left holding
//! event closures that have already been dropped.
//!
//! Dropping a `wasm_bindgen::Closure` invalidates the JS shim that
//! forwards into wasm, but it does not unregister the shim from the
//! object it was assigned to. The web `WebSocket` and `EventSource` arms
//! both dropped their closures on the connect-failure path (`result?`)
//! while the underlying JS object was still live and still about to
//! emit — `close` for the socket, an auto-retry `error` for the stream.
//! The browser then invoked the dead shim, and wasm-bindgen threw
//!
//! ```text
//! closure invoked recursively or after being dropped
//! ```
//!
//! into the event loop. It does not trap the module, so nothing
//! user-visible breaks; it buries the console in exceptions on exactly
//! the connections someone is debugging. Both arms now detach their
//! handler slots from the JS object in the closures' own `Drop`.
//!
//! Browser-only, because the whole mechanism is: run with
//! `wasm-pack test --headless --chrome --package net`.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// The substring wasm-bindgen throws when a dead shim is invoked.
const DROPPED: &str = "after being dropped";

/// Loopback port 1 refuses immediately on both schemes, so a connect
/// failure needs no test server.
const REFUSED_WS: &str = "ws://127.0.0.1:1";
const REFUSED_SSE: &str = "http://127.0.0.1:1/events";

/// An exception thrown inside an event handler is uncaught, so it
/// surfaces as a window `error` event. This collects them so a test can
/// assert on what the console would have shown.
struct ErrorSpy {
    seen: Rc<RefCell<Vec<String>>>,
    cb: Closure<dyn FnMut(web_sys::ErrorEvent)>,
}

impl ErrorSpy {
    fn install() -> Self {
        let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        let cb = Closure::<dyn FnMut(web_sys::ErrorEvent)>::new(move |e: web_sys::ErrorEvent| {
            sink.borrow_mut().push(e.message());
        });
        web_sys::window()
            .expect("window")
            .add_event_listener_with_callback("error", cb.as_ref().unchecked_ref())
            .expect("listen for uncaught errors");
        ErrorSpy { seen, cb }
    }

    /// Uncaught messages naming a dropped closure.
    fn dropped_closure_errors(&self) -> Vec<String> {
        self.seen
            .borrow()
            .iter()
            .filter(|m| m.contains(DROPPED))
            .cloned()
            .collect()
    }
}

impl Drop for ErrorSpy {
    fn drop(&mut self) {
        if let Some(w) = web_sys::window() {
            let _ = w.remove_event_listener_with_callback("error", self.cb.as_ref().unchecked_ref());
        }
    }
}

/// Yield to the event loop for `ms`, so the browser can deliver the
/// events that follow a failed connect.
async fn settle(ms: i32) {
    let p = js_sys::Promise::new(&mut |resolve, _reject| {
        web_sys::window()
            .expect("window")
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms)
            .expect("setTimeout");
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
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
