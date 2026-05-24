//! Web `AsyncExecutor`: routes both `spawn` and `spawn_on_worker` to
//! `wasm_bindgen_futures::spawn_local`. JS is single-threaded so there
//! is no real worker option; the trait keeps the call shape uniform
//! with native targets.

use std::future::Future;
use std::pin::Pin;

use runtime_core::driver::{AsyncExecutor, BoxedWorkerFuture};

/// Register this backend's executor with `runtime-core`. Idempotent —
/// first install wins.
pub fn install_async_executor() {
    runtime_core::driver::install_async_executor(Box::new(WasmAsyncExecutor));
}

struct WasmAsyncExecutor;

impl AsyncExecutor for WasmAsyncExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        wasm_bindgen_futures::spawn_local(future);
    }

    fn spawn_on_worker(&self, future: BoxedWorkerFuture) {
        wasm_bindgen_futures::spawn_local(future);
    }
}
