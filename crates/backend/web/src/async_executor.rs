//! Web `AsyncExecutor`: routes `spawn` to
//! `wasm_bindgen_futures::spawn_local` (the JS event loop).

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use runtime_shared::driver::AsyncExecutor;

/// Register this backend's executor with `runtime-core`. Idempotent —
/// first install wins.
pub fn install_async_executor() {
    runtime_shared::driver::install_async_executor(Box::new(WasmAsyncExecutor));
}

struct WasmAsyncExecutor;

impl AsyncExecutor for WasmAsyncExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        wasm_bindgen_futures::spawn_local(HookedFuture(future));
    }
}

/// Fires the post-dispatch hook after every poll of a spawned future.
/// Each `.await` resume runs author code that may stage new-core signal
/// writes (a server-call completion setting a resource signal); without
/// the per-poll hook those writes would sit uncommitted until an
/// unrelated event flushed them. No-op under the old core (hook slot
/// empty) — see `dispatch_hook` module docs.
struct HookedFuture(Pin<Box<dyn Future<Output = ()> + 'static>>);

impl Future for HookedFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let result = self.0.as_mut().poll(cx);
        crate::dispatch_hook::fire_dispatch_hook();
        result
    }
}
