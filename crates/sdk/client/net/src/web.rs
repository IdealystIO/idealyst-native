//! Web (wasm32) transport: the browser's `fetch`, through web-glue bindings
//! (own-web-bindings phase 4 — this arm ran on gloo-net before).
//!
//! One JS module (`net/fetch`, below) owns the whole exchange: it builds the
//! `Headers`, issues `fetch(url, { method, headers, body, signal })`, buffers
//! the body as an `ArrayBuffer`, and settles ONE promise with
//! `{ status, headers, body }` — or rejects with `{ kind, message }`, where
//! `kind` is `"abort"`, `"timeout"` or `"network"`. Doing the classification
//! in JS keeps the Rust side to a single `JsFuture` and one error match.
//!
//! - **Cancellation** — every request carries an `AbortController`. A fired
//!   [`CancelToken`] aborts it (the browser tears the request down, not just
//!   the Rust future) and resolves `Error::Cancelled`; so does dropping the
//!   `send` future mid-flight (the [`InFlight`] guard), which is what the
//!   reqwest arm gets from reqwest's own `Drop`.
//! - **Timeout** — `RequestBuilder::timeout` / `ClientBuilder::timeout` arm a
//!   `setTimeout` that aborts the same controller and marks the exchange
//!   timed out, so it resolves `Error::Timeout`. Like reqwest's, the deadline
//!   covers the whole exchange including the body read. (The gloo arm
//!   ignored the timeout: a request against a server that never answered
//!   hung forever on web.)
//! - **Errors** — a fetch rejection (DNS, refused, CORS, a `Request` the
//!   browser will not construct: bad URL, invalid header, a GET with a body)
//!   is `Error::Network(message)`, as under gloo. A 4xx/5xx is a normal
//!   `Response` (see `Response::error_for_status`).
//! - **Bodies** — the request body is copied out of wasm memory (`slice`)
//!   when the request starts, since `fetch` reads it asynchronously; an empty
//!   body sends none. The response body is copied back into a `Vec<u8>`
//!   once, after it has fully arrived.
//!
//! Threading model: web is single-threaded; web-glue's executor drives
//! everything off the JS microtask queue. The cancel race is the same
//! `poll_fn` race the native transport uses, so `net` stays runtime-agnostic.

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use web_glue::js::{Array, Uint8Array};
use web_glue::{string, JsCast, JsError, JsFuture, JsValue};

use crate::cancel::CancelToken;
use crate::error::Error;
use crate::headers::Headers;
use crate::method::Method;
use crate::response::Response;

web_glue::js_module!(fn fetch_module = "net/fetch", r#"
// Classify a rejection once, here, where the exchange state is visible.
// `st.timedOut` is checked FIRST: the timer aborts the same controller a
// cancel does, so the rejection is an AbortError either way.
const fail = (st, e) => {
  if (st.timedOut) return { kind: 'timeout', message: 'request timed out' };
  if (e != null && e.name === 'AbortError') return { kind: 'abort', message: String(e.message) };
  const m = e != null && e.message !== undefined ? e.message : e;
  return { kind: 'network', message: String(m) };
};
return {
  // Start one exchange. `hs` is a flat [name, value, ...] array; `p/l` is
  // the request body in wasm memory (`l === 0` sends none); `ms < 0` means
  // no timeout. Returns the state object: `.promise` settles as described
  // in the Rust module docs, `abort(st)` tears the request down.
  start(method, url, hs, p, l, ms) {
    const ctl = new AbortController();
    const st = { ctl, timedOut: false, timer: 0, done: false, promise: null };
    // COPY the body now: fetch reads it after this call returns, and a view
    // of wasm memory would not survive a later memory growth.
    const body = l === 0 ? null : G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0));
    if (ms >= 0) st.timer = setTimeout(() => { st.timedOut = true; ctl.abort(); }, ms);
    st.promise = (async () => {
      try {
        // Inside the try: an invalid header name/value throws here and must
        // reject like any other refused request, not throw out of `start`.
        const headers = new Headers();
        for (let i = 0; i + 1 < hs.length; i += 2) headers.append(hs[i], hs[i + 1]);
        const r = await fetch(url, { method, headers, body, signal: ctl.signal });
        const buf = await r.arrayBuffer();
        const out = [];
        r.headers.forEach((v, k) => { out.push(k, v); });
        return { status: r.status, headers: out, body: new Uint8Array(buf) };
      } catch (e) {
        throw fail(st, e);
      } finally {
        st.done = true;
        clearTimeout(st.timer);
      }
    })();
    return st;
  },
  abort(st) {
    if (st.done) return;
    clearTimeout(st.timer);
    st.ctl.abort();
  },
};
"#);

web_glue::import! {
    fn js_fetch_start(mp: usize, ml: usize, up: usize, ul: usize, hs: u32, bp: usize, bl: usize, ms: f64) -> u32 =
        "(mp, ml, up, ul, hs, bp, bl, ms) => G.add(G.m('net/fetch').start(G.str(mp, ml), G.str(up, ul), G.get(hs), bp, bl, ms))";
    fn js_fetch_abort(st: u32) = "(st) => { G.m('net/fetch').abort(G.get(st)); }";
}

pub(crate) struct Transport;

impl Transport {
    pub(crate) fn new() -> Self {
        Self
    }
}

/// An exchange in flight. Dropping it before it settled aborts the request,
/// so neither a cancel nor a dropped `send` future leaves the browser
/// downloading a body nobody will read. (Abort after settling is a no-op on
/// the JS side.)
struct InFlight {
    state: JsValue,
    settled: bool,
}

impl InFlight {
    fn abort(&self) {
        fetch_module();
        // SAFETY: a live state handle `js_fetch_start` minted.
        unsafe { js_fetch_abort(self.state.raw()) }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if !self.settled {
            self.abort();
        }
    }
}

pub(crate) async fn send(
    _transport: &Transport,
    method: Method,
    url: String,
    headers: Headers,
    body: Vec<u8>,
    timeout: Option<Duration>,
    cancel: Option<CancelToken>,
) -> Result<Response, Error> {
    let flat = Array::new();
    for (name, value) in headers.iter() {
        flat.push(&JsValue::from_str(name));
        flat.push(&JsValue::from_str(value));
    }
    let ms = timeout.map_or(-1.0, |d| d.as_secs_f64() * 1000.0);

    fetch_module(); // the anchor: keeps the `net/fetch` record linked
    let (mp, ml) = string::abi(method.as_str());
    let (up, ul) = string::abi(&url);
    // SAFETY: borrowed strings and body bytes for the duration of the call
    // (the module copies the body before returning); the result is a fresh
    // state handle.
    let state = unsafe {
        JsValue::from_raw(js_fetch_start(
            mp,
            ml,
            up,
            ul,
            flat.as_js().raw(),
            body.as_ptr() as usize,
            body.len(),
            ms,
        ))
    };
    drop(body);
    let promise = state.get("promise").map_err(|e| Error::Other(format!("fetch: {}", e.message())))?;
    let mut flight = InFlight { state, settled: false };

    let mut request = Box::pin(JsFuture::new(&promise));
    let mut cancelled = cancel.as_ref().map(|t| Box::pin(t.cancelled()));
    let settled = poll_fn(|cx| {
        if let Some(c) = cancelled.as_mut() {
            if let Poll::Ready(()) = Pin::new(c).poll(cx) {
                return Poll::Ready(None);
            }
        }
        Pin::new(&mut request).poll(cx).map(Some)
    })
    .await;

    let Some(result) = settled else {
        // The token won the race: tear the request down (the guard's drop
        // would too; doing it here keeps the order explicit).
        flight.abort();
        flight.settled = true;
        return Err(Error::Cancelled);
    };
    flight.settled = true;
    match result {
        Ok(v) => response_from(&v),
        Err(e) => Err(error_from(&e)),
    }
}

/// Lift the module's `{ status, headers, body }` into a [`Response`].
fn response_from(v: &JsValue) -> Result<Response, Error> {
    let field = |k: &str| v.get(k).map_err(|e| Error::Other(format!("fetch result: {}", e.message())));
    let status = field("status")?.as_f64().unwrap_or(0.0) as u16;
    let flat = field("headers")?.unchecked_into::<Array>();
    let mut headers = Headers::new();
    let n = flat.length();
    let mut i = 0;
    while i + 1 < n {
        let name = flat.get(i).as_string().unwrap_or_default();
        let value = flat.get(i + 1).as_string().unwrap_or_default();
        headers.append(name, value);
        i += 2;
    }
    let body = field("body")?.unchecked_into::<Uint8Array>().to_vec();
    Ok(Response { status, headers, body })
}

/// Map the module's `{ kind, message }` rejection to an [`Error`].
fn error_from(e: &JsError) -> Error {
    let kind = e.get("kind").ok().and_then(|k| k.as_string());
    let message = e
        .get("message")
        .ok()
        .and_then(|m| m.as_string())
        .unwrap_or_else(|| e.message());
    match kind.as_deref() {
        Some("timeout") => Error::Timeout,
        Some("abort") => Error::Cancelled,
        Some("network") => Error::Network(message),
        // Not the module's shape: something threw before the exchange ran.
        _ => Error::Other(format!("fetch: {}", e.message())),
    }
}
