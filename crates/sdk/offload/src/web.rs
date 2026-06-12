//! Web backend: dispatch the job to a `wasmworker` Web Worker pool.
//!
//! The handle (built by `offload::handle!` == `wasmworker::webworker!`) is a
//! [`wasmworker::func::WebWorkerFn`] carrying the function's name + pointer. The
//! global pool is created on first use (`worker_pool()`); it spins up
//! `navigator.hardwareConcurrency` workers, each running its own instance of the
//! same app wasm, and routes the call by name. The job runs on a real worker
//! thread, so the main thread never blocks.

use serde::{Deserialize, Serialize};

use crate::OffloadError;

/// Run `func` with `arg` on a worker thread and await the result.
///
/// `wasmworker`'s `run` is infallible at this layer (a worker-side failure
/// surfaces as a panic in its own context), so the `Result` is `Ok` on success;
/// the `OffloadError` variant exists to keep the signature identical to the
/// native backend, where the worker thread can genuinely drop the channel.
pub async fn run<T, R>(
    func: wasmworker::func::WebWorkerFn<T, R>,
    arg: &T,
) -> Result<R, OffloadError>
where
    T: Serialize + for<'de> Deserialize<'de>,
    R: Serialize + for<'de> Deserialize<'de>,
{
    Ok(wasmworker::worker_pool().await.run(func, arg).await)
}
