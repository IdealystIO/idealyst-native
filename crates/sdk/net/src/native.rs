//! Native (non-mobile, non-wasm) transport built on `reqwest`.
//!
//! `reqwest::Client` is itself an `Arc<...>`, so we hold one per
//! [`Client`](crate::Client) without further wrapping. Each `send` call
//! lowers our platform-agnostic `Method` / `Headers` / `Vec<u8>` into
//! reqwest's types, awaits the response, and lifts everything back.

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use crate::cancel::CancelToken;
use crate::error::Error;
use crate::headers::Headers;
use crate::method::Method;
use crate::response::Response;

/// Transport handle owned by the [`ClientInner`](crate::client::ClientInner).
pub(crate) struct Transport {
    client: reqwest::Client,
}

impl Transport {
    pub(crate) fn new() -> Self {
        // The default builder is sufficient for v0. Future knobs
        // (TLS config, proxy, redirect policy) get exposed via
        // `ClientBuilder` on the public side.
        let client = reqwest::Client::builder()
            .build()
            .expect("reqwest::Client builder should not fail with default config");
        Self { client }
    }
}

pub(crate) async fn send(
    transport: &Transport,
    method: Method,
    url: String,
    headers: Headers,
    body: Vec<u8>,
    timeout: Option<Duration>,
    cancel: Option<CancelToken>,
) -> Result<Response, Error> {
    let mut req = transport
        .client
        .request(map_method(method), &url);

    for (name, value) in headers.iter() {
        req = req.header(name, value);
    }
    if !body.is_empty() {
        req = req.body(body);
    }
    if let Some(t) = timeout {
        req = req.timeout(t);
    }

    // The send + body collection are pinned together so a cancel
    // mid-stream (between status arriving and body bytes finishing)
    // still aborts via the outer race below. reqwest cancels on drop.
    let request_future = async move {
        let resp = req.send().await.map_err(map_send_error)?;
        let status = resp.status().as_u16();

        let mut out_headers = Headers::new();
        for (name, value) in resp.headers().iter() {
            if let Ok(v) = value.to_str() {
                out_headers.append(name.as_str(), v);
            }
        }

        let bytes = resp.bytes().await.map_err(map_send_error)?.to_vec();

        Ok::<_, Error>(Response {
            status,
            headers: out_headers,
            body: bytes,
        })
    };

    match cancel {
        None => request_future.await,
        Some(token) => race_with_cancel(request_future, token).await,
    }
}

/// Race `request_future` against `token.cancelled()`. Whichever
/// resolves first wins; if cancel wins the request future is dropped
/// (which is how reqwest aborts an in-flight request — its `Drop`
/// closes the connection without sending further bytes).
///
/// Using `poll_fn` rather than `tokio::select!` keeps `net` independent
/// of tokio's macros feature; correctness is the same.
async fn race_with_cancel<F>(request_future: F, token: CancelToken) -> Result<Response, Error>
where
    F: Future<Output = Result<Response, Error>>,
{
    let mut fut = Box::pin(request_future);
    let mut cancel_fut = Box::pin(token.cancelled());
    poll_fn(|cx| {
        // Poll the cancel future first so a token that fires *before*
        // the request makes any progress short-circuits without a
        // wasted network round-trip.
        if let Poll::Ready(()) = Pin::new(&mut cancel_fut).poll(cx) {
            return Poll::Ready(Err(Error::Cancelled));
        }
        if let Poll::Ready(result) = Pin::new(&mut fut).poll(cx) {
            return Poll::Ready(result);
        }
        Poll::Pending
    })
    .await
}

fn map_method(m: Method) -> reqwest::Method {
    match m {
        Method::Get => reqwest::Method::GET,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Patch => reqwest::Method::PATCH,
        Method::Delete => reqwest::Method::DELETE,
        Method::Head => reqwest::Method::HEAD,
        Method::Options => reqwest::Method::OPTIONS,
    }
}

fn map_send_error(e: reqwest::Error) -> Error {
    if e.is_timeout() {
        Error::Timeout
    } else if e.is_builder() {
        Error::InvalidUrl(e.to_string())
    } else if e.is_connect() || e.is_request() {
        Error::Network(e.to_string())
    } else {
        Error::Other(e.to_string())
    }
}
