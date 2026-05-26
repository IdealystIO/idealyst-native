//! Web (wasm32) transport, built on `gloo-net` fetch.
//!
//! Cancellation is bridged through `web_sys::AbortController` —
//! gloo's `RequestBuilder::abort_signal` plumbs the signal into the
//! underlying `fetch()` call, so calling `controller.abort()` from
//! the cancel watcher actually aborts the request (the resolved
//! response promise rejects with an `AbortError`, which we map to
//! `Error::Cancelled`).
//!
//! Threading model: web is single-threaded; `wasm_bindgen_futures`
//! drives everything off the JS microtask queue. We use the same
//! `poll_fn` race used by the native transport to keep
//! `runtime-agnostic`.

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use gloo_net::http::{Headers as GlooHeaders, Method as GlooMethod, RequestBuilder as GlooBuilder};
use web_sys::AbortController;

use crate::cancel::CancelToken;
use crate::error::Error;
use crate::headers::Headers;
use crate::method::Method;
use crate::response::Response;

pub(crate) struct Transport;

impl Transport {
    pub(crate) fn new() -> Self {
        Self
    }
}

pub(crate) async fn send(
    _transport: &Transport,
    method: Method,
    url: String,
    headers: Headers,
    body: Vec<u8>,
    _timeout: Option<Duration>,
    cancel: Option<CancelToken>,
) -> Result<Response, Error> {
    // Build the gloo request. We bridge our own header / method /
    // body / cancel surface into gloo's builder.
    let gloo_headers = GlooHeaders::new();
    for (name, value) in headers.iter() {
        gloo_headers.set(name, value);
    }

    let controller =
        AbortController::new().map_err(|e| Error::Other(format!("AbortController: {e:?}")))?;
    let signal = controller.signal();

    let builder: GlooBuilder = GlooBuilder::new(&url)
        .method(map_method(method))
        .headers(gloo_headers)
        .abort_signal(Some(&signal));

    // gloo's `body()` is the terminal builder step — it returns the
    // built `Request` (or `Error`) directly, not another builder.
    // `build()` is the no-body equivalent. Branch accordingly.
    let request = if body.is_empty() {
        builder.build().map_err(map_gloo_err)?
    } else {
        // Pass bytes through `Uint8Array` so binary bodies (postcard,
        // protobuf, raw bytes) round-trip cleanly; gloo's text body
        // path would force UTF-8 lossy decoding.
        let array = js_sys::Uint8Array::from(body.as_slice());
        builder.body(array).map_err(map_gloo_err)?
    };

    let request_future = async move {
        let resp = request.send().await.map_err(map_gloo_err)?;
        let status = resp.status();
        let mut out_headers = Headers::new();
        // gloo's Headers iter is `(name, value)` strings.
        for (name, value) in resp.headers().entries() {
            out_headers.append(name, value);
        }
        let bytes = resp.binary().await.map_err(map_gloo_err)?;
        Ok::<_, Error>(Response {
            status,
            headers: out_headers,
            body: bytes,
        })
    };

    match cancel {
        None => request_future.await,
        Some(token) => race_with_cancel(request_future, token, controller).await,
    }
}

/// Race the fetch against the cancel token. When the token wins, we
/// call `controller.abort()` so the in-flight fetch is actually
/// torn down (otherwise the browser keeps the request alive even
/// though the Rust future was dropped).
async fn race_with_cancel<F>(
    request_future: F,
    token: CancelToken,
    controller: AbortController,
) -> Result<Response, Error>
where
    F: Future<Output = Result<Response, Error>>,
{
    let mut fut = Box::pin(request_future);
    let mut cancel_fut = Box::pin(token.cancelled());
    poll_fn(|cx| {
        if let Poll::Ready(()) = Pin::new(&mut cancel_fut).poll(cx) {
            controller.abort();
            return Poll::Ready(Err(Error::Cancelled));
        }
        if let Poll::Ready(result) = Pin::new(&mut fut).poll(cx) {
            return Poll::Ready(result);
        }
        Poll::Pending
    })
    .await
}

fn map_method(m: Method) -> GlooMethod {
    match m {
        Method::Get => GlooMethod::GET,
        Method::Post => GlooMethod::POST,
        Method::Put => GlooMethod::PUT,
        Method::Patch => GlooMethod::PATCH,
        Method::Delete => GlooMethod::DELETE,
        Method::Head => GlooMethod::HEAD,
        Method::Options => GlooMethod::OPTIONS,
    }
}

fn map_gloo_err(e: gloo_net::Error) -> Error {
    // gloo wraps the underlying `JsValue` for fetch errors. An
    // AbortError surfaces as a JsError; sniff the name field so a
    // cancel-triggered abort lands as `Error::Cancelled` rather
    // than a generic Network.
    if let gloo_net::Error::JsError(js) = &e {
        if js.name == "AbortError" {
            return Error::Cancelled;
        }
    }
    match e {
        gloo_net::Error::JsError(js) => Error::Network(js.message),
        gloo_net::Error::SerdeError(err) => Error::Serialize(err.to_string()),
        gloo_net::Error::GlooError(msg) => Error::Other(msg),
    }
}

