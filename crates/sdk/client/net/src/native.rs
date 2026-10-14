//! Native (non-mobile, non-wasm) transport built on `reqwest`.
//!
//! `reqwest::Client` is itself an `Arc<...>`, so we hold one per
//! [`Client`](crate::Client) without further wrapping. Each `send` call
//! lowers our platform-agnostic `Method` / `Headers` / `Vec<u8>` into
//! reqwest's types, runs the exchange, and lifts everything back.
//!
//! # Execution model: a process-wide I/O runtime thread
//!
//! reqwest's DNS and socket I/O need a tokio reactor, but `net` callers
//! run on whatever executor they have — the framework's `spawn_async`
//! driver, a park/unpark loop on a worker thread, a platform event loop
//! — and none of those is tokio. Awaiting reqwest directly on the caller's
//! executor panicked with "there is no reactor running" on every Linux /
//! Windows backend app (`tests/foreign_executor.rs`).
//!
//! So the exchange always runs on ONE lazily-started, process-wide
//! current-thread tokio runtime parked on its own `net-io` thread
//! ([`io_runtime`]); the caller awaits the result through a
//! `futures-channel` oneshot, which any executor can poll. Chosen over:
//!
//! - **`reqwest::blocking` on worker threads** — a blocked thread cannot be
//!   interrupted, so `CancelToken` / a dropped future could only abandon
//!   the request, leaving the socket and thread busy until the server or
//!   the timeout let go. Here cancel aborts the task, which drops the
//!   reqwest future and closes the connection immediately.
//! - **an executor-agnostic HTTP client** (`ureq`, …) — the agnostic ones
//!   are blocking (same cancellation problem) and would add a second HTTP
//!   + TLS stack beside the reqwest that `eventsource.rs` already uses.
//! - **using the caller's runtime when one happens to exist** — two paths
//!   to test, and reqwest's pooled connections are tasks on the runtime
//!   that opened them, so a `Client` shared across runtimes (one
//!   `#[tokio::test]` runtime per test, or a runtime torn down while the
//!   `Client` lives on) hands out connections whose driver is gone. One
//!   runtime for the whole process has neither problem.
//!
//! tokio is already in the dependency graph through reqwest, so this adds
//! one parked thread, not a dependency. The runtime is never shut down: it
//! lives as long as the process, like the connection pool it drives.

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::Poll;
use std::time::Duration;

use futures_channel::oneshot;

use crate::cancel::CancelToken;
use crate::error::Error;
use crate::headers::Headers;
use crate::method::Method;
use crate::response::Response;

/// The process-wide runtime every request's I/O runs on, started on first
/// use. Built on the calling thread (building one never requires or
/// conflicts with an ambient runtime) and then moved onto a dedicated
/// thread that drives it forever; spawned tasks run there because a
/// current-thread runtime executes its tasks inside `block_on`.
fn io_runtime() -> &'static tokio::runtime::Handle {
    static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .thread_name("net-io-blocking")
            .build()
            .expect("net: build the I/O runtime");
        let handle = rt.handle().clone();
        std::thread::Builder::new()
            .name("net-io".into())
            .spawn(move || rt.block_on(std::future::pending::<()>()))
            .expect("net: spawn the I/O runtime thread");
        handle
    })
}

/// Aborts the I/O task when the caller stops waiting — the request future
/// was dropped, or the cancel token won the race. Aborting drops the
/// reqwest future on the I/O thread, which is how reqwest closes an
/// in-flight connection. Aborting a finished task is a no-op.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

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

    // Run the exchange on the I/O runtime; the oneshot is the only thing
    // the caller's executor polls. `_abort` lives across the await, so
    // returning early (cancel) or being dropped tears the request down.
    let (tx, rx) = oneshot::channel();
    let task = io_runtime().spawn(async move {
        let _ = tx.send(request_future.await);
    });
    let _abort = AbortOnDrop(task.abort_handle());
    let bridged = async move {
        // `Canceled` = the task ended without sending: it panicked inside
        // reqwest (the runtime contained it) — there is no other abort
        // path while `_abort` is held.
        rx.await
            .unwrap_or_else(|_| Err(Error::Other("net: request task ended without a result".into())))
    };

    match cancel {
        None => bridged.await,
        Some(token) => race_with_cancel(bridged, token).await,
    }
}

/// Race `request_future` against `token.cancelled()`. Whichever
/// resolves first wins; if cancel wins, `send` returns and its
/// [`AbortOnDrop`] aborts the I/O task, dropping the reqwest future
/// (reqwest's `Drop` closes the connection without sending more bytes).
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
