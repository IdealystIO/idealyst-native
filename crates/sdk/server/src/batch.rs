//! Microtask-coalesced batching for client-side server-fn calls.
//!
//! When the macro's client stub fires, it doesn't immediately POST —
//! it enqueues into a process-global pending queue and (if not already
//! scheduled) spawns a "flusher" task. The flusher yields once to let
//! sibling calls land in the same batch, then either:
//!
//! - **N = 1**: sends a normal `POST /_srv/<path>` (the existing
//!   single-call wire). Solo calls pay only the cost of a single
//!   task yield.
//! - **N > 1**: sends a single `POST /_srv/_batch` with the array of
//!   `{path, args}` pairs and distributes the array of `Result`s
//!   back to each call's `oneshot` channel.
//!
//! All transparent to the author — `add(2,3).await` looks the same.
//! On an app-load fan-out (`use_query(get_user)` + `use_query(list_todos)`
//! + ...), what used to be 10 HTTP requests becomes one.
//!
//! Compiled only when the `server` feature is OFF (client builds);
//! the server build never enqueues, it receives.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::task::{Context, Poll};

use futures_channel::oneshot;
use serde_json::Value;

use crate::error::ServerError;

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
    response: oneshot::Sender<Result<Value, ServerError>>,
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

/// Enqueue a call, schedule a flush, and await this call's slot.
///
/// The returned `Result<Value, ServerError>` is whatever the server
/// produced for this slot:
/// - `Ok(value)` on success — the caller still needs to
///   `serde_json::from_value::<Ret>` it.
/// - `Err(ServerError)` on transport / codec / 4xx-5xx failure.
pub(crate) async fn enqueue(path: &str, args: Value) -> Result<Value, ServerError> {
    let (tx, rx) = oneshot::channel();
    queue().lock().unwrap().push(PendingCall {
        path: path.to_string(),
        args,
        response: tx,
    });

    // Schedule a flusher if none is in flight. Lost races (two
    // threads both swap-false-to-true) are impossible because `swap`
    // is atomic — exactly one caller observes the prior `false`.
    if !FLUSH_SCHEDULED.swap(true, Ordering::AcqRel) {
        runtime_core::driver::spawn_async(async {
            // Yield once so sibling calls enqueued in the same
            // executor tick land in the same flush. After yielding,
            // we clear the scheduled flag *before* taking the queue
            // — any further enqueues after this point will spawn a
            // fresh flusher.
            yield_once().await;
            FLUSH_SCHEDULED.store(false, Ordering::Release);
            flush().await;
        });
    }

    match rx.await {
        Ok(r) => r,
        // The sender was dropped without sending — the flusher
        // panicked or was cancelled. Surface as a Network error;
        // it's not a codec problem and the user's domain error
        // never got constructed.
        Err(_) => Err(ServerError::Network(
            "batch flusher dropped without responding".into(),
        )),
    }
}

// -----------------------------------------------------------------------------
// Flush
// -----------------------------------------------------------------------------

async fn flush() {
    let pending: Vec<PendingCall> = std::mem::take(&mut *queue().lock().unwrap());
    if pending.is_empty() {
        return;
    }

    if pending.len() == 1 {
        // Solo path: existing single-call wire (POST /_srv/<path>).
        // Preserves backward compatibility and avoids the array
        // overhead for a lone call.
        let call = pending.into_iter().next().unwrap();
        let result = send_single(&call.path, &call.args).await;
        let _ = call.response.send(result);
    } else {
        // Batch path: one HTTP request for N calls.
        let outcome = send_batch(&pending).await;
        match outcome {
            Ok(results) => {
                // Server is contractually required to return `results.len() == pending.len()`.
                // If it doesn't, treat missing slots as a server error so each call still
                // resolves (rather than silently dropping the oneshot sender).
                let mut results = results.into_iter();
                for call in pending {
                    let r = results.next().unwrap_or_else(|| {
                        Err(ServerError::Codec(
                            "batch response missing entry for this call".into(),
                        ))
                    });
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

async fn send_single(path: &str, args: &Value) -> Result<Value, ServerError> {
    use crate::client::{net_client, snapshot_config};

    let config = snapshot_config()?;
    let url = format!("{}/_srv/{}", config.base_url.trim_end_matches('/'), path);

    let response = net_client()
        .post(url)
        .body(net::Json(args))
        .send()
        .await
        .map_err(crate::client::map_net_error)?;

    if !response.is_success() {
        let status = response.status();
        let message = response.text().await.unwrap_or_default();
        return Err(ServerError::Server { status, message });
    }

    response
        .json::<Value>()
        .await
        .map_err(|e| ServerError::Codec(e.to_string()))
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
) -> Result<Vec<Result<Value, ServerError>>, ServerError> {
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
        return Err(ServerError::Server { status, message });
    }

    // Each slot in the response is the standard serde-serialised
    // `Result<Value, ServerError>` produced by the server's handler
    // (i.e. `{"Ok": value}` or `{"Err": {...}}`).
    response
        .json::<Vec<Result<Value, ServerError>>>()
        .await
        .map_err(|e| ServerError::Codec(e.to_string()))
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
