//! Tokio-backed async executor for the AAS (runtime-server) sidecar.
//!
//! ## Why this exists
//!
//! `runtime_core::driver::spawn_async` has a native fallback: when no
//! backend has installed an [`AsyncExecutor`](runtime_core::driver::AsyncExecutor),
//! it drives the future to completion with `pollster::block_on` on the
//! calling thread. That fallback is correct for real native backends
//! whose HTTP transport is the platform stack (`NSURLSession` on Apple,
//! `HttpURLConnection` on Android) — those need no Tokio reactor.
//!
//! The AAS sidecar is different. It compiles for a **desktop** target
//! (`not(wasm32 / ios / android)`), where the `net` SDK lowers HTTP to
//! `reqwest`, which is built on `tokio::net::TcpStream`. A `TcpStream`
//! can only be registered with a **live Tokio I/O reactor in the current
//! thread's runtime context**. The session thread is a plain
//! `std::thread` with no runtime, so `pollster::block_on` polls the
//! reqwest future on a thread that has no reactor → the future panics
//! the moment it tries to open a socket:
//!
//! ```text
//! thread 'aas-session-…' panicked at tokio/net/tcp/stream.rs:164:
//!   there is no reactor running, must be called from the context of a
//!   Tokio 1.x runtime
//! ```
//!
//! and every subsequent robot call times out because the session thread
//! is dead. This is purely a dev/AAS artifact: shipped web (wasm
//! `fetch`) and real native both avoid reqwest, so they never hit it.
//!
//! ## The fix
//!
//! Install an executor whose `spawn` drives the future on a
//! **current-thread Tokio runtime** with `enable_all()` (I/O + time
//! drivers). `Runtime::block_on` parks on the runtime's own reactor, so
//! the reqwest `TcpStream` finds the reactor it needs and the request
//! actually completes — same *completes-on-the-calling-thread*
//! semantics as the `pollster` fallback it replaces, just with a reactor
//! underneath.
//!
//! The runtime is a `thread_local!` so each session thread owns its own
//! (the executor handle registered globally is a unit struct, satisfying
//! `AsyncExecutor: Send + Sync`, but the runtime it reaches for is
//! per-thread and never crosses a thread boundary). This mirrors the
//! Apple cooperative executor's unit-struct-plus-thread-local shape.
//!
//! ## Reentrancy
//!
//! Tokio forbids starting a runtime from inside a runtime
//! (`block_on` within `block_on` panics). App code reaching `spawn_async`
//! again *while an outer `spawn_async` is still on the stack* (e.g. an
//! async server-fn whose body kicks off a second `spawn_async`) would
//! trip that. We guard with a thread-local depth flag: the reentrant
//! call falls back to `pollster::block_on`, which is safe because the
//! outer `Runtime::block_on` already entered this thread's runtime
//! context — `Handle::current()` resolves and the reactor is live, so a
//! nested poll of a reqwest future still finds its reactor. The guard
//! only prevents a *second* `Runtime::block_on`, not the reactor access.

use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;

use runtime_core::driver::AsyncExecutor;
use tokio::runtime::Runtime;

thread_local! {
    /// One current-thread Tokio runtime per session thread. Built lazily
    /// on first `spawn`. `enable_all()` turns on the I/O reactor (what
    /// reqwest's `TcpStream` registration needs) and the time driver
    /// (reqwest timeouts, `tokio::time` from author code). Lives for the
    /// thread's lifetime — dropped when the session thread exits.
    static SESSION_RT: Runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread Tokio runtime for AAS session");

    /// Set while a `Runtime::block_on` is on this thread's stack, so a
    /// reentrant `spawn_async` doesn't start a second runtime (which Tokio
    /// rejects with a panic).
    static IN_BLOCK_ON: Cell<bool> = const { Cell::new(false) };
}

/// Install the Tokio-backed executor with `runtime-core`. Idempotent
/// (first install wins — `runtime_core::driver::install_async_executor`
/// is a `OnceLock`). Call once, on each session thread, before `mount`.
///
/// Installing per-thread is harmless: the global handle is the same
/// unit-struct executor, and the runtime it dispatches to is resolved
/// from the *calling* thread's `SESSION_RT` thread-local at `spawn`
/// time, so every session thread drives its own futures on its own
/// runtime.
pub fn install() {
    runtime_core::driver::install_async_executor(Box::new(SidecarAsyncExecutor));
}

struct SidecarAsyncExecutor;

// SAFETY: a fieldless unit struct, so `Send + Sync` is satisfied
// trivially. The `!Send` futures it drives live only inside the
// per-thread `SESSION_RT` `block_on` and never cross a thread boundary;
// the registered handle carries no state.
unsafe impl Send for SidecarAsyncExecutor {}
unsafe impl Sync for SidecarAsyncExecutor {}

impl AsyncExecutor for SidecarAsyncExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        if IN_BLOCK_ON.with(Cell::get) {
            // Reentrant call: a `Runtime::block_on` is already on this
            // thread's stack (so the runtime context — and its reactor —
            // is already entered). Starting a second runtime would panic;
            // `pollster` polls the future on the current thread, and any
            // reqwest socket inside it finds the outer runtime's reactor
            // via `Handle::current()`.
            pollster::block_on(future);
            return;
        }
        SESSION_RT.with(|rt| {
            IN_BLOCK_ON.with(|f| f.set(true));
            // RAII reset so an unwinding future still clears the flag.
            struct Reset;
            impl Drop for Reset {
                fn drop(&mut self) {
                    IN_BLOCK_ON.with(|f| f.set(false));
                }
            }
            let _reset = Reset;
            rt.block_on(future);
        });
    }
}
