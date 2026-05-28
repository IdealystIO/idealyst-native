//! Web `AsyncExecutor`: routes `spawn` to
//! `wasm_bindgen_futures::spawn_local` (the JS event loop).

use std::future::Future;
use std::pin::Pin;

use runtime_core::driver::AsyncExecutor;

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
}
