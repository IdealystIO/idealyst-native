//! Tick-coalesced batching for client-side server-fn calls.
//!
//! When the macro's client stub fires, it doesn't immediately POST —
//! it enqueues into a process-global pending queue. The **first**
//! caller to observe an empty queue becomes the *flusher*: it yields
//! once (giving sibling calls a chance to enqueue into the same
//! batch), then drains the queue and dispatches:
//!
//! - **N = 1**: a normal `POST /_srv/<path>` (the existing
//!   single-call wire). Solo calls pay only the cost of one task
//!   yield.
//! - **N > 1**: one `POST /_srv/_batch` with the array of
//!   `{path, args}` pairs; the response is an array of `Result`s
//!   the flusher distributes back to each call's `oneshot` channel.
//!
//! All transparent to the author — `add(2,3).await` looks the same.
//! On an app-load fan-out (`use_query(get_user)` + `use_query(list_todos)`
//! + ...), what used to be 10 HTTP requests becomes one.
//!
//! # Why "inline flusher" rather than `spawn_async`
//!
//! On native, `runtime_core::driver::spawn_async` falls back to
//! `pollster::block_on` when no executor is installed — which is
//! synchronous and deadlocks if the caller is already inside a
//! tokio runtime (the common case for `net`'s reqwest transport).
//! Driving the flush inline from the first enqueuer's await chain
//! avoids that entirely and stays runtime-agnostic.
//!
//! Compiled only when the `server` feature is OFF (client builds);
//! the server build never enqueues, it receives.

use std::cell::Cell;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::task::{Context, Poll};

use futures_channel::oneshot;
use net::CancelToken;
use serde_json::Value;

use crate::error::TransportError;

// -----------------------------------------------------------------------------
// Pending queue
// -----------------------------------------------------------------------------

/// One enqueued call waiting for flush.
struct PendingCall {
    /// Wire path of the server fn (e.g. `"add"`).
    path: String,
    /// Wire schema hash for this call, sent per batch entry so the
    /// server can run its drift diagnostic on each entry independently.
    schema: u64,
    /// Pre-serialised args tuple, as a JSON `Value`. Kept as `Value`
    /// rather than `Vec<u8>` so the batch path can re-emit the whole
    /// array in one `serde_json::to_vec` pass without parsing each
    /// child blob a second time.
    args: Value,
    /// One-shot the flusher resolves with this call's result slot.
    /// The caller awaits the receiver half.
    response: oneshot::Sender<Result<Value, TransportError>>,
    /// Cancellation token observed both before flush (filtered out
    /// of the batch) and during flight (solo path aborts HTTP; batch
    /// path resolves the awaiter early but lets the shared HTTP run).
    cancel: Option<CancelToken>,
}

/// Process-wide queue. `OnceLock` makes it lazy (no init at static
/// time); `Mutex<Vec<_>>` is fine — the queue is touched at human
/// timescales (a handful of enqueues per microtask window), not in a
/// hot loop.
fn queue() -> &'static Mutex<Vec<PendingCall>> {
    static QUEUE: OnceLock<Mutex<Vec<PendingCall>>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(Vec::new()))
}

/// `true` iff a flusher task has been spawned and is still pending
/// its first poll (or is currently draining). Prevents N enqueues
/// from spawning N redundant flushers.
static FLUSH_SCHEDULED: AtomicBool = AtomicBool::new(false);

// -----------------------------------------------------------------------------
// Deliberate batching scope.
//
// Batching is OPT-IN: by default each server-fn call is a direct
// `POST /_srv/<path>`. Coalescing into one `/_srv/_batch` request happens
// only for calls made inside a `server::batch(...)` scope. This makes the
// latency-coupling trade-off (a slow call delaying a fast one in the same
// request) a visible, opted-into choice rather than a silent default.
//
// The scope is a per-poll thread-local, restored across yield points the
// same way `cancel.rs` scopes `CURRENT_CANCEL` — no tokio dependency.
// -----------------------------------------------------------------------------

thread_local! {
    /// `true` while the current poll is inside a `batch(...)` scope.
    static IN_BATCH_SCOPE: Cell<bool> = const { Cell::new(false) };
}

/// `true` if the current call should coalesce (i.e. we're inside a
/// `batch(...)` scope). Read by `client::call_impl`.
pub(crate) fn in_scope() -> bool {
    IN_BATCH_SCOPE.with(|c| c.get())
}

/// Coalesce every `#[server]` call made while `future` is polled into
/// batched `POST /_srv/_batch` requests. Calls made outside this scope
/// are direct single requests.
///
/// ```ignore
/// let (todos, me) = server::batch(async {
///     futures::join!(list_todos(), whoami())   // one HTTP request
/// }).await;
/// ```
pub fn batch<F: Future>(future: F) -> BatchScope<F> {
    BatchScope { future }
}

/// Future returned by [`batch`]. Sets the batch-scope thread-local for
/// the duration of each poll, restoring the previous value on yield /
/// completion (RAII, panic-safe) — mirroring [`crate::cancel::WithCancel`].
pub struct BatchScope<F> {
    future: F,
}

impl<F: Future> Future for BatchScope<F> {
    type Output = F::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        // SAFETY: `future` is structurally pinned (never moved out).
        let this = unsafe { self.get_unchecked_mut() };
        let future = unsafe { Pin::new_unchecked(&mut this.future) };
        let prev = IN_BATCH_SCOPE.with(|c| c.replace(true));
        let _guard = BatchScopeGuard { prev };
        future.poll(cx)
    }
}

struct BatchScopeGuard {
    prev: bool,
}

impl Drop for BatchScopeGuard {
    fn drop(&mut self) {
        IN_BATCH_SCOPE.with(|c| c.set(self.prev));
    }
}

/// Direct single-call dispatch (the default, outside a batch scope).
/// Bypasses the coalescing queue entirely: one call, one
/// `POST /_srv/<path>`, with cancellation honoured via the request's
/// own abort (no shared-HTTP race needed for a solo call).
pub(crate) async fn send_direct(
    path: &str,
    schema: u64,
    args: Value,
) -> Result<(Value, Option<u64>), TransportError> {
    let cancel = crate::cancel::current_cancel();
    if let Some(t) = &cancel {
        if t.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
    }
    send_single(path, &args, schema, cancel).await
}

// -----------------------------------------------------------------------------
// Public entry point — used by `__private::call`.
// -----------------------------------------------------------------------------

/// Enqueue a call. The first caller to observe an empty
/// flush-scheduled flag becomes the flusher: it yields once for
/// siblings, drains the queue, and dispatches. Other callers just
/// await their oneshot slot.
///
/// The returned `Result<Value, TransportError>` is whatever the server
/// produced for this slot:
/// - `Ok(value)` on success — the caller still needs to
///   `serde_json::from_value::<Ret>` it.
/// - `Err(TransportError)` on transport / codec / 4xx-5xx failure
///   (including [`TransportError::Cancelled`] when the call's cancel
///   token fired).
pub(crate) async fn enqueue(
    path: &str,
    schema: u64,
    args: Value,
) -> Result<Value, TransportError> {
    // Pick up the cancel token associated with the current `with_cancel`
    // scope, if any. None means this call is not cancellable.
    let cancel = crate::cancel::current_cancel();

    // Short-circuit: if the token was already fired before we got
    // here, don't even enqueue. Saves the queue insert + flush yield.
    if let Some(t) = &cancel {
        if t.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
    }

    let (tx, rx) = oneshot::channel();
    queue().lock().unwrap().push(PendingCall {
        path: path.to_string(),
        schema,
        args,
        response: tx,
        cancel: cancel.clone(),
    });

    // Atomically claim the flusher role. `swap` returns the prior
    // value; exactly one caller observes `false`, becoming the
    // flusher for this batch. Subsequent callers (`prior == true`)
    // simply enqueue and await — somebody else will dispatch them.
    let am_flusher = !FLUSH_SCHEDULED.swap(true, Ordering::AcqRel);

    if am_flusher {
        // Give sibling calls a turn to enqueue. yield_once returns
        // Pending the first poll (waking itself), Ready the second
        // — between those polls the runtime can drive other ready
        // tasks (including additional `enqueue` calls landing on
        // the same tick).
        yield_once().await;
        // Clear the flag *before* taking the queue. Any further
        // enqueues from this point start a fresh batch.
        FLUSH_SCHEDULED.store(false, Ordering::Release);
        let pending = std::mem::take(&mut *queue().lock().unwrap());
        flush(pending).await;
        // Our own oneshot has been resolved by `flush`; the rx.await
        // below resolves on its next poll without yielding.
    }

    // Awaiter race: settle on whichever of {response received, cancel
    // fired} wins. Without a token this is just `rx.await`.
    let recv_future = async move {
        rx.await.unwrap_or_else(|_| {
            Err(TransportError::Network(
                "batch flusher dropped without responding".into(),
            ))
        })
    };

    match cancel {
        None => recv_future.await,
        Some(token) => race_recv_vs_cancel(recv_future, token).await,
    }
}

/// Race a `recv_future` against the cancel token. If cancel wins,
/// return [`TransportError::Cancelled`] immediately — the flusher /
/// HTTP request continues in the background but the caller has
/// moved on. For solo calls (queue size 1 at flush time) the HTTP
/// is also aborted via `net::RequestBuilder::cancel_on`; for
/// batched calls the shared HTTP runs to completion so other
/// non-cancelled siblings still get their results.
async fn race_recv_vs_cancel<F>(recv_future: F, token: CancelToken) -> Result<Value, TransportError>
where
    F: Future<Output = Result<Value, TransportError>>,
{
    let mut fut = Box::pin(recv_future);
    let mut cancel_fut = Box::pin(token.cancelled());
    poll_fn(|cx| {
        if let Poll::Ready(()) = Pin::new(&mut cancel_fut).poll(cx) {
            return Poll::Ready(Err(TransportError::Cancelled));
        }
        if let Poll::Ready(result) = Pin::new(&mut fut).poll(cx) {
            return Poll::Ready(result);
        }
        Poll::Pending
    })
    .await
}

// -----------------------------------------------------------------------------
// Flush
// -----------------------------------------------------------------------------

async fn flush(pending: Vec<PendingCall>) {
    if pending.is_empty() {
        return;
    }

    // Filter out calls whose cancel token already fired while they
    // were waiting in the queue — those slots resolve as
    // `Cancelled` immediately and don't contribute to the HTTP
    // request (whether solo or batched). Saves bandwidth and
    // avoids dispatching work the caller has already abandoned.
    let pending: Vec<PendingCall> = pending
        .into_iter()
        .filter_map(|call| {
            if call
                .cancel
                .as_ref()
                .map(|t| t.is_cancelled())
                .unwrap_or(false)
            {
                let _ = call.response.send(Err(TransportError::Cancelled));
                None
            } else {
                Some(call)
            }
        })
        .collect();

    if pending.is_empty() {
        return;
    }

    if pending.len() == 1 {
        // Solo path: existing single-call wire (POST /_srv/<path>).
        // Preserves backward compatibility and avoids the array
        // overhead for a lone call. Passes the cancel token through
        // so the HTTP itself aborts if cancel fires mid-flight.
        let call = pending.into_iter().next().unwrap();
        // The queue's oneshot carries only the response `Value`; the
        // server-schema header is dropped here (the batch path doesn't
        // run the client-side return-drift diagnostic).
        let result = send_single(&call.path, &call.args, call.schema, call.cancel)
            .await
            .map(|(value, _schema)| value);
        let _ = call.response.send(result);
    } else {
        // Batch path: one HTTP request for N calls.
        let outcome = send_batch(&pending).await;
        match outcome {
            Ok(slots) => {
                // Each slot is the raw `Result<T, ServerError<E>>` JSON
                // value for that call (the author's typed result, opaque
                // here). Wrap as `Ok(value)` in the oneshot — `call_impl`
                // deserializes the typed `Result<T, ServerError<E>>` from it.
                //
                // Server is contractually required to return
                // `slots.len() == pending.len()`. If it doesn't,
                // treat missing slots as a codec error so each call
                // still resolves (rather than silently dropping the
                // oneshot sender).
                let mut slots = slots.into_iter();
                for call in pending {
                    let r = match slots.next() {
                        Some(v) => Ok(v),
                        None => Err(TransportError::Codec(
                            "batch response missing entry for this call".into(),
                        )),
                    };
                    let _ = call.response.send(r);
                }
            }
            Err(e) => {
                // Whole-batch transport failure: every call gets the
                // same error. Clone so each oneshot has its own copy.
                for call in pending {
                    let _ = call.response.send(Err(e.clone()));
                }
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Single-call wire (N = 1 fast path).
// -----------------------------------------------------------------------------

async fn send_single(
    path: &str,
    args: &Value,
    schema: u64,
    cancel: Option<CancelToken>,
) -> Result<(Value, Option<u64>), TransportError> {
    use crate::client::{net_client, snapshot_config};

    let config = snapshot_config()?;
    let url = format!("{}/_srv/{}", config.base_url.trim_end_matches('/'), path);

    // Advertise our wire schema hash so the server can run its drift
    // diagnostic (and `strict_version` gate), then attach any configured
    // credential headers (bearer token, etc.). Conditional `.cancel_on`
    // avoids allocating a token slot for non-cancellable solo calls.
    let mut request = net_client()
        .post(url)
        .set_header(SCHEMA_HEADER, format!("{schema:x}"));
    if let Some(creds) = &config.credentials {
        for (name, value) in creds.headers() {
            request = request.set_header(name, value);
        }
    }
    request = request.body(net::Json(args));
    if let Some(token) = cancel {
        request = request.cancel_on(token);
    }

    let response = request
        .send()
        .await
        .map_err(crate::client::map_net_error)?;

    // 426 Upgrade Required → the server rejected us on schema grounds
    // (strict mismatch, or an arg-decode failure it attributed to drift).
    if response.status() == 426 {
        let server_schema = parse_schema_header(&response);
        let detail = response
            .json::<crate::error::VersionMismatch>()
            .await
            .ok();
        return Err(TransportError::IncompatibleVersion {
            path: path.to_string(),
            client_schema: schema,
            server_schema: detail.map(|d| d.server_schema).or(server_schema).unwrap_or(0),
        });
    }

    if !response.is_success() {
        let status = response.status();
        let message = response.text().await.unwrap_or_default();
        return Err(TransportError::Server { status, message });
    }

    // Read the server's return-schema header before consuming the body.
    let server_schema = parse_schema_header(&response);
    let value = response
        .json::<Value>()
        .await
        .map_err(|e| TransportError::Codec(e.to_string()))?;
    Ok((value, server_schema))
}

/// The request/response header carrying the wire schema hash (hex).
const SCHEMA_HEADER: &str = "x-srv-schema";

fn parse_schema_header(response: &net::Response) -> Option<u64> {
    response
        .header(SCHEMA_HEADER)
        .and_then(|h| u64::from_str_radix(h, 16).ok())
}

// -----------------------------------------------------------------------------
// Batch wire (N > 1).
// -----------------------------------------------------------------------------

/// Serializable shape of one entry in the batch request body.
#[derive(serde::Serialize)]
struct BatchEntry<'a> {
    path: &'a str,
    args: &'a Value,
    schema: u64,
}

async fn send_batch(
    pending: &[PendingCall],
) -> Result<Vec<Value>, TransportError> {
    use crate::client::{net_client, snapshot_config};

    let config = snapshot_config()?;
    let url = format!("{}/_srv/_batch", config.base_url.trim_end_matches('/'));

    let body: Vec<BatchEntry<'_>> = pending
        .iter()
        .map(|p| BatchEntry {
            path: &p.path,
            args: &p.args,
            schema: p.schema,
        })
        .collect();

    let mut request = net_client().post(url);
    if let Some(creds) = &config.credentials {
        for (name, value) in creds.headers() {
            request = request.set_header(name, value);
        }
    }
    let response = request
        .body(net::Json(&body))
        .send()
        .await
        .map_err(crate::client::map_net_error)?;

    if !response.is_success() {
        let status = response.status();
        let message = response.text().await.unwrap_or_default();
        return Err(TransportError::Server { status, message });
    }

    // Each slot is the raw JSON `Result<T, ServerError<E>>` produced by
    // the server-side handler — e.g. `{"Ok": 5}` or `{"Err": {...}}`.
    // We keep slots as `Value` (rather than deserializing to
    // `Result<Value, ServerError<E>>` here) so the single-call wire
    // path and the batch wire path return symmetric shapes through
    // the queue: both put the full Result JSON into the
    // `oneshot` channel, and `call_impl` deserializes the typed
    // `Result<T, ServerError<E>>` once at the consumer site.
    response
        .json::<Vec<Value>>()
        .await
        .map_err(|e| TransportError::Codec(e.to_string()))
}

// -----------------------------------------------------------------------------
// Yield-once helper.
// -----------------------------------------------------------------------------

/// A future that resolves after one executor tick. Used inside
/// `enqueue` to give sibling calls a chance to land in the same
/// flush before the flusher drains the queue.
///
/// Reimplemented here (rather than reaching for `tokio::task::yield_now`)
/// so the server SDK stays executor-agnostic — any runtime that
/// drives `runtime_core::driver::spawn_async` works.
fn yield_once() -> impl Future<Output = ()> {
    struct YieldOnce {
        yielded: bool,
    }
    impl Future for YieldOnce {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.yielded {
                Poll::Ready(())
            } else {
                self.yielded = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
    YieldOnce { yielded: false }
}
