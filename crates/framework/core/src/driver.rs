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
//! use framework_core::driver::{render_loop, spawn_async};
//! use framework_core::primitives::graphics::{graphics, OnReadyEvent};
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
            "framework_core::driver::spawn_async: no AsyncExecutor installed. \
             On wasm32 a backend must register one before `spawn_async` is called \
             — typically `backend_web::install_async_executor()` during host init."
        );
    }
}

/// Drive a future to completion *off* the current thread on native;
/// same as [`spawn_async`] on web (single-threaded by definition).
///
/// Use this when the future does work that mustn't block the calling
/// thread — typically wgpu's `request_adapter` / `request_device` /
/// shader compilation, which the [`Graphics`] primitive fires from
/// `on_ready` on the platform's UI thread. Blocking that thread for
/// seconds (which the wgpu init dance can do on real devices) freezes
/// the app's input handling and triggers Android's "Skipped N frames"
/// warning.
///
/// On **web**: identical to `spawn_async` — the future runs on the
/// single JS event loop. There's no worker option without
/// `web_workers` plumbing the framework doesn't have.
///
/// On **native** (Android / iOS / desktop): spawns a dedicated
/// `std::thread`, runs `pollster::block_on(future)` on it, and
/// returns immediately. The future must be `Send + 'static` — it's
/// moved across the thread boundary. The spawned thread is detached
/// (no join handle returned); typical use is "build a renderer, hand
/// it off via a shared `Arc<Mutex<…>>`, return."
///
/// # Send bound
///
/// On native this takes a `Send` future because it moves to a worker
/// thread. On web the `Send` bound is irrelevant (JS is
/// single-threaded) but uniform call shape across targets means web
/// authors writing `spawn_async_on_worker(async move { ... })` get
/// the same code that compiles on Android — so we keep the bound
/// uniform via a per-target marker trait.
pub fn spawn_async_on_worker<F>(future: F)
where
    F: Future<Output = ()> + WorkerFutureBounds + 'static,
{
    if let Some(exec) = ASYNC_EXECUTOR.get() {
        exec.spawn_on_worker(Box::pin(future));
        return;
    }
    // Fallback: dedicated `std::thread` running `pollster::block_on`.
    // No join handle — typical use is "the future stashes its result
    // in an Arc<Mutex<…>> slot and returns."
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::Builder::new()
            .name("framework-async-worker".into())
            .spawn(move || pollster::block_on(future))
            .expect("framework: failed to spawn async worker thread");
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = future;
        panic!(
            "framework_core::driver::spawn_async_on_worker: no AsyncExecutor installed. \
             On wasm32 a backend must register one before this is called \
             — typically `backend_web::install_async_executor()` during host init."
        );
    }
}

/// Per-target `Send` bound on the future passed to
/// [`spawn_async_on_worker`]. `Send` on native (the future moves to a
/// worker thread), trivial on wasm (single-threaded). Same trick as
/// `RenderLoopClosureBounds` — author code uses one signature that
/// compiles uniformly.
#[cfg(not(target_arch = "wasm32"))]
pub trait WorkerFutureBounds: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> WorkerFutureBounds for T {}

#[cfg(target_arch = "wasm32")]
pub trait WorkerFutureBounds {}
#[cfg(target_arch = "wasm32")]
impl<T> WorkerFutureBounds for T {}

/// Type-erased worker future. Carries the `Send` bound on native (the
/// future moves to a worker thread) and drops it on wasm32 (single-
/// threaded). Mirrors the per-target asymmetry of [`WorkerFutureBounds`].
#[cfg(not(target_arch = "wasm32"))]
pub type BoxedWorkerFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
pub type BoxedWorkerFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;

/// Backend-supplied async runtime. Backends register an instance via
/// [`install_async_executor`] at init; without one, `spawn_async`
/// falls back to `pollster` on native and panics on wasm32.
pub trait AsyncExecutor: Send + Sync {
    /// Drive a future on the calling thread / event loop. Returns
    /// immediately on web (microtask queue), blocks on native.
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>);

    /// Drive a future off the calling thread on native; same as
    /// `spawn` on web.
    fn spawn_on_worker(&self, future: BoxedWorkerFuture);
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
/// On native (Android), the closure runs on a dedicated render
/// thread; `Drop` joins it. On web (rAF) and iOS (NSTimer), the
/// closure runs on the same thread as the caller; `Drop` cancels the
/// next scheduled frame.
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
/// `requestAnimationFrame` on web, `NSTimer` on iOS, a dedicated
/// render thread on Android. If no driver is installed, the returned
/// handle is inert and the closure never fires.
///
/// # Threading — important
///
/// The `Send` bound on the closure is **per-target**:
/// - On Android the closure runs on a dedicated render thread → it
///   must be `Send`, and any state it captures must cross the thread
///   boundary (typically via `Arc<Mutex<…>>` or by moving owned
///   state into the closure).
/// - On web (rAF) and iOS (NSTimer) the closure runs on the same
///   thread as the caller — no `Send` bound, so `Rc<RefCell<…>>`
///   works.
///
/// This is *intentional asymmetry*. Some graphics types (`wgpu` on
/// wasm uses `WasmNotSendSync`) are `!Send` on web and `Send` on
/// native; forcing `Send` everywhere would make those types
/// unusable.
pub fn render_loop<F>(f: F) -> RenderLoop
where
    F: FnMut(f32) + RenderLoopClosureBounds + 'static,
{
    let inner: Box<dyn RenderLoopHandle> = match RENDER_LOOP_DRIVER.get() {
        Some(driver) => driver.start(Box::new(f)),
        None => Box::new(NoopHandle),
    };
    RenderLoop { inner }
}

/// Marker trait that pins the per-target `Send` requirement on the
/// `render_loop` closure. Resolves to `Send` on Android (where the
/// driver runs on a worker thread) and to no extra bound on web /
/// iOS / desktop (where the driver fires on the calling thread).
///
/// Known framework-purity gap: the cfg gate below is the only platform-
/// target switch in framework-core. The audit-preferred fix is a pair
/// of free functions — `render_loop` (same-thread, no `Send`) and
/// `render_loop_on_worker` (`Send` required) — so the choice lives at
/// the call site rather than the target. Implementing that is a
/// breaking change for the web/iOS host callers (they use
/// `Rc<RefCell<…>>` captures that aren't `Send`), so it's tracked
/// separately rather than landing in the framework-purity sweep.
#[cfg(target_os = "android")]
pub trait RenderLoopClosureBounds: Send {}
#[cfg(target_os = "android")]
impl<T: Send> RenderLoopClosureBounds for T {}

#[cfg(not(target_os = "android"))]
pub trait RenderLoopClosureBounds {}
#[cfg(not(target_os = "android"))]
impl<T> RenderLoopClosureBounds for T {}

/// Backend-supplied per-frame driver. Backends implement this trait
/// against their platform's frame source and register an instance via
/// [`install_render_loop_driver`] during init.
///
/// The trait method signature differs per target so the closure can
/// stay `!Send` on web/iOS while being `Send` on Android — driving
/// the same trade-off as [`RenderLoopClosureBounds`].
#[cfg(target_os = "android")]
pub trait RenderLoopDriver: Send + Sync {
    fn start(
        &self,
        closure: Box<dyn FnMut(f32) + Send + 'static>,
    ) -> Box<dyn RenderLoopHandle>;
}

#[cfg(not(target_os = "android"))]
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
