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
pub(crate) async fn enqueue(path: &str, args: Value) -> Result<Value, TransportError> {
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
        let result = send_single(&call.path, &call.args, call.cancel).await;
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
    cancel: Option<CancelToken>,
) -> Result<Value, TransportError> {
    use crate::client::{net_client, snapshot_config};

    let config = snapshot_config()?;
    let url = format!("{}/_srv/{}", config.base_url.trim_end_matches('/'), path);

    // Conditional `.cancel_on(...)` — we don't want to chain it
    // unconditionally because that would always allocate a token
    // slot in the builder even for non-cancellable solo calls.
    let mut request = net_client().post(url).body(net::Json(args));
    if let Some(token) = cancel {
        request = request.cancel_on(token);
    }

    let response = request
        .send()
        .await
        .map_err(crate::client::map_net_error)?;

    if !response.is_success() {
        let status = response.status();
        let message = response.text().await.unwrap_or_default();
        return Err(TransportError::Server { status, message });
    }

    response
        .json::<Value>()
        .await
        .map_err(|e| TransportError::Codec(e.to_string()))
}

// -----------------------------------------------------------------------------
// Batch wire (N > 1).
// -----------------------------------------------------------------------------

/// Serializable shape of one entry in the batch request body.
#[derive(serde::Serialize)]
struct BatchEntry<'a> {
    path: &'a str,
    args: &'a Value,
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
        })
        .collect();

    let response = net_client()
        .post(url)
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
