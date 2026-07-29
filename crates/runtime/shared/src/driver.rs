//! Platform-agnostic async + per-frame driver primitives.
//!
//! These exist because the same logical operation has three
//! different shapes across the platforms the framework targets:
//!
//! - **"Drive this future to completion"** — `spawn_local(future)` on
//!   web, `pollster::block_on(future)` on native. The web version is
//!   non-blocking (returns immediately, future progresses on the
//!   microtask queue); the native version blocks the calling thread.
//!   Authors don't want to remember which.
//!
//! - **"Run this closure once per frame"** — `requestAnimationFrame`
//!   on web, `CADisplayLink`/`NSTimer` on iOS, a dedicated render
//!   thread on Android. Same closure shape; very different drivers.
//!   The framework only defines [`RenderLoopDriver`] and a registry;
//!   each backend implements + installs its own driver at init time.
//!
//! Both are gated behind the `async-driver` feature so apps that don't
//! need a frame ticker pay nothing for the wasm-bindgen-futures /
//! pollster deps. Backends that ship a render-loop driver expose a
//! matching `async-driver` feature that forwards here.
//!
//! # Use with the `Graphics` surface primitive
//!
//! ```ignore
//! use runtime_core::driver::{render_loop, spawn_async};
//! use runtime_core::primitives::graphics::{graphics, OnReadyEvent};
//!
//! graphics(move |event: OnReadyEvent| {
//!     spawn_async(async move {
//!         let renderer = build_my_renderer(event.surface, event.size).await;
//!         let _loop = render_loop(move |elapsed| {
//!             renderer.paint_frame(elapsed);
//!         });
//!         // …stash `_loop` in your renderer state so its Drop runs
//!         //   with the rest on `on_lost`.
//!     });
//! })
//! ```
//!
//! The host's init code must call the backend's `install_render_loop`
//! before any author code reaches `render_loop`; otherwise the
//! returned handle is inert.

#![cfg(feature = "async-driver")]

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// spawn_async
// ---------------------------------------------------------------------------

/// Drive a future to completion on the current execution context.
///
/// On **web**: spawns the future on the JS event loop via
/// `wasm_bindgen_futures::spawn_local`. Returns immediately; the
/// future makes progress as its `Wake` notifications fire.
///
/// On **native** (Android / iOS / desktop): blocks the calling thread
/// on the future via `pollster::block_on` and returns once the future
/// completes. If you need the work to happen off the calling thread,
/// use `std::thread::spawn(move || spawn_async(async { ... }))`.
///
/// # Send bound
///
/// On native, the future has to be at least `'static`; it does *not*
/// need to be `Send` because `block_on` polls it on the current
/// thread. On web, futures are inherently `!Send` (the JS event loop
/// is single-threaded) and `wasm_bindgen_futures::spawn_local` accepts
/// `!Send` futures. So the per-target API uses a single non-`Send`
/// bound everywhere, which lines up with what most author code wants
/// to write (closures capturing `Rc`, `Cell`, etc.).
///
/// If you need to spawn on a worker thread, the future *there* has
/// to be `Send` — but that's a `std::thread::spawn` concern, not a
/// `spawn_async` concern.
pub fn spawn_async<F>(future: F)
where
    F: Future<Output = ()> + 'static,
{
    if let Some(exec) = ASYNC_EXECUTOR.get() {
        exec.spawn(Box::pin(future));
        return;
    }
    // Fallback: pollster on the calling thread. Available on every
    // native target without requiring a backend install. On wasm32
    // there is no fallback — `backend_web::install_async_executor()`
    // must run during host init.
    #[cfg(not(target_arch = "wasm32"))]
    {
        pollster::block_on(future);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = future;
        panic!(
            "runtime_core::driver::spawn_async: no AsyncExecutor installed. \
             On wasm32 a backend must register one before `spawn_async` is called \
             — typically `backend_web::install_async_executor()` during host init."
        );
    }
}

/// Backend-supplied async runtime. Backends register an instance via
/// [`install_async_executor`] at init; without one, `spawn_async`
/// falls back to `pollster` on native and panics on wasm32.
pub trait AsyncExecutor: Send + Sync {
    /// Drive a future on the calling thread / event loop. Returns
    /// immediately on web (microtask queue), blocks on native.
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>);
}

static ASYNC_EXECUTOR: OnceLock<Box<dyn AsyncExecutor>> = OnceLock::new();

/// Register the active backend's async executor. First call wins;
/// subsequent calls are silently ignored.
pub fn install_async_executor(executor: Box<dyn AsyncExecutor>) {
    let _ = ASYNC_EXECUTOR.set(executor);
}

// ---------------------------------------------------------------------------
// render_loop — driver-trait dispatch
// ---------------------------------------------------------------------------

/// A live frame-driver handle. Dropping it stops the loop and cancels
/// any pending frame — no further callbacks fire.
///
/// The closure runs on the host's UI thread on every backend — rAF on
/// web, `NSTimer` on iOS, the main looper on Android. `Drop` cancels
/// the next scheduled frame.
pub struct RenderLoop {
    inner: Box<dyn RenderLoopHandle>,
}

impl RenderLoop {
    /// Manually stop the loop ahead of `Drop`. After calling, the
    /// handle is inert.
    pub fn cancel(&mut self) {
        self.inner.cancel();
    }
}

/// Start a per-frame loop. The closure receives `elapsed_seconds`
/// since `render_loop` was called.
///
/// The active backend's [`RenderLoopDriver`] (installed via
/// [`install_render_loop_driver`]) decides the per-frame source:
/// `requestAnimationFrame` on web, `NSTimer` on iOS, the main looper
/// on Android. If no driver is installed, the returned handle is inert
/// and the closure never fires.
///
/// # Threading
///
/// The closure runs on the host's **UI thread** on every backend, so
/// the bound is a uniform `FnMut(f32) + 'static` with no `Send`
/// requirement — it can capture `Rc<RefCell<…>>` and `!Send` graphics
/// types (`wgpu`'s `WasmNotSendSync` on web). A backend that wants to
/// offload GPU work to a worker thread does so *internally*, marshalling
/// across the boundary itself; that never leaks a `Send` bound into this
/// author-facing signature.
pub fn render_loop<F>(f: F) -> RenderLoop
where
    F: FnMut(f32) + 'static,
{
    let inner: Box<dyn RenderLoopHandle> = match RENDER_LOOP_DRIVER.get() {
        Some(driver) => driver.start(Box::new(f)),
        None => Box::new(NoopHandle),
    };
    RenderLoop { inner }
}

/// Backend-supplied per-frame driver. Backends implement this trait
/// against their platform's UI-thread frame source and register an
/// instance via [`install_render_loop_driver`] during init.
///
/// The closure runs on the host's UI thread, so the bound is a uniform
/// `FnMut(f32) + 'static` — no per-target `Send` asymmetry.
pub trait RenderLoopDriver: Send + Sync {
    fn start(
        &self,
        closure: Box<dyn FnMut(f32) + 'static>,
    ) -> Box<dyn RenderLoopHandle>;
}

/// Opaque handle a backend's [`RenderLoopDriver::start`] returns. Its
/// `Drop` impl stops the loop; `cancel` is the explicit method for
/// early teardown (callable multiple times, second call is a no-op).
pub trait RenderLoopHandle: 'static {
    fn cancel(&mut self);
}

static RENDER_LOOP_DRIVER: OnceLock<Box<dyn RenderLoopDriver>> = OnceLock::new();

/// Register the active backend's render-loop driver. First call wins;
/// subsequent calls are silently ignored so multiple backends linked
/// into the same binary don't fight (the host picks the order).
pub fn install_render_loop_driver(driver: Box<dyn RenderLoopDriver>) {
    let _ = RENDER_LOOP_DRIVER.set(driver);
}

struct NoopHandle;

impl RenderLoopHandle for NoopHandle {
    fn cancel(&mut self) {}
}
