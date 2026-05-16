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
//!   on web, `CADisplayLink` on iOS, a dedicated render thread on
//!   Android. Same closure shape; very different drivers.
//!
//! Both are gated behind the `async-driver` feature so apps that don't
//! need a frame ticker pay nothing for the wasm-bindgen-futures /
//! pollster / objc2 deps.
//!
//! # Use with the `Graphics` surface primitive
//!
//! The intended flow:
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
//! `build_my_renderer` is yours to write — wgpu, vello, raw GL,
//! whatever. The framework's job stops at "here's a surface and a
//! frame ticker."

#![cfg(feature = "async-driver")]

use std::future::Future;

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
    #[cfg(target_arch = "wasm32")]
    {
        wasm_bindgen_futures::spawn_local(future);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        pollster::block_on(future);
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
    #[cfg(target_arch = "wasm32")]
    {
        wasm_bindgen_futures::spawn_local(future);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Detached worker. We don't expose a join handle because the
        // typical use is "the future itself sets up an
        // `Arc<Mutex<…>>` slot and returns" — joining adds nothing.
        std::thread::Builder::new()
            .name("framework-async-worker".into())
            .spawn(move || pollster::block_on(future))
            .expect("framework: failed to spawn async worker thread");
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

// ---------------------------------------------------------------------------
// render_loop
// ---------------------------------------------------------------------------

/// A live frame-driver handle. Dropping it stops the loop and cancels
/// any pending frame — no further callbacks fire.
///
/// On native (Android), the closure runs on a dedicated render
/// thread; `Drop` joins it. On web (rAF) and iOS (CADisplayLink),
/// the closure runs on the same thread as the caller; `Drop` cancels
/// the next scheduled frame.
pub struct RenderLoop {
    inner: imp::RenderLoopInner,
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
/// unusable. Authors who want one shape of state across all
/// platforms should pick a state type that's `Send` on all targets
/// (most things are, on native) and use `Arc<Mutex<…>>` as the
/// outer shell — see the gradient demo for one approach.
///
/// # Timing
///
/// The framework picks the platform's natural per-frame source:
/// - Web: `requestAnimationFrame` — paced by the browser.
/// - Android: dedicated thread; the caller's per-frame work should
///   end with a `present()`-style call whose blocking paces the
///   loop (e.g. wgpu's `PresentMode::Fifo`).
/// - iOS: `NSTimer` at ~60Hz. (CADisplayLink would be more accurate
///   but requires declaring a custom ObjC class; NSTimer's pacing
///   is fine for the typical "animated surface" use case.)
///
/// On desktop targets without a driver yet, the loop never fires
/// and the handle is inert.
pub fn render_loop<F>(f: F) -> RenderLoop
where
    F: FnMut(f32) + RenderLoopClosureBounds + 'static,
{
    RenderLoop { inner: imp::start(f) }
}

/// Marker trait that pins the per-target `Send` requirement on the
/// `render_loop` closure. Resolves to `Send` on Android (where the
/// driver runs on a worker thread) and to no extra bound on web /
/// iOS / desktop (where the driver fires on the calling thread).
#[cfg(target_os = "android")]
pub trait RenderLoopClosureBounds: Send {}
#[cfg(target_os = "android")]
impl<T: Send> RenderLoopClosureBounds for T {}

#[cfg(not(target_os = "android"))]
pub trait RenderLoopClosureBounds {}
#[cfg(not(target_os = "android"))]
impl<T> RenderLoopClosureBounds for T {}

// ---------------------------------------------------------------------------
// Per-platform render_loop implementations.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::cell::RefCell;
    use std::rc::Rc;

    pub(super) struct RenderLoopInner {
        // `Option` so `cancel()` can drop the inner state ahead of
        // the outer `Drop`.
        state: Option<Rc<RefCell<State>>>,
    }

    struct State {
        /// Browser's rAF handle for the currently-queued frame.
        pending: Option<i32>,
        /// The wasm-bindgen wrapper for the per-frame callback. We
        /// own it so we can drop it after telling the browser to
        /// cancel — never the other way around.
        closure: Option<wasm_bindgen::closure::Closure<dyn FnMut()>>,
        /// Set from `Drop`. The per-frame closure short-circuits on
        /// this flag so a callback already pulled off the JS queue
        /// becomes a no-op.
        cancelled: bool,
    }

    impl Drop for State {
        fn drop(&mut self) {
            self.cancelled = true;
            if let (Some(h), Some(window)) = (self.pending.take(), web_sys::window()) {
                let _ = window.cancel_animation_frame(h);
            }
            // `closure` drops with `self`.
        }
    }

    impl RenderLoopInner {
        pub(super) fn cancel(&mut self) {
            self.state = None;
        }
    }

    pub(super) fn start<F: FnMut(f32) + 'static>(mut f: F) -> RenderLoopInner {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;

        let Some(window) = web_sys::window() else {
            return RenderLoopInner { state: None };
        };
        let started = js_sys::Date::now();
        let state = Rc::new(RefCell::new(State {
            pending: None,
            closure: None,
            cancelled: false,
        }));
        let weak = Rc::downgrade(&state);
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            let Some(strong) = weak.upgrade() else { return };
            {
                let s = strong.borrow();
                if s.cancelled {
                    return;
                }
            }
            // Browser is about to fire this frame; clear the pending
            // handle so re-arm logic below sets a fresh one.
            strong.borrow_mut().pending = None;
            // Invoke the user fn outside any borrow on `strong`, so
            // the user is free to drop the RenderLoop handle from
            // inside their own frame body.
            let elapsed = ((js_sys::Date::now() - started) / 1000.0) as f32;
            f(elapsed);
            // Re-arm. If the user dropped the loop from within the
            // callback, `cancelled` is set and we skip.
            let mut s = strong.borrow_mut();
            if s.cancelled {
                return;
            }
            if let Some(window) = web_sys::window() {
                if let Some(c) = s.closure.as_ref() {
                    if let Ok(h) =
                        window.request_animation_frame(c.as_ref().unchecked_ref())
                    {
                        s.pending = Some(h);
                    }
                }
            }
        });
        state.borrow_mut().closure = Some(closure);
        // Kick the first frame. We can't hold a `state.borrow()`
        // across `request_animation_frame` then take a borrow_mut to
        // record the handle (the immutable borrow's temporary lives
        // for the whole if-let). Pull the JS fn ref out first.
        let raf_fn = state
            .borrow()
            .closure
            .as_ref()
            .map(|c| c.as_ref().clone());
        if let Some(raf_fn) = raf_fn {
            if let Ok(h) = window.request_animation_frame(raf_fn.unchecked_ref()) {
                state.borrow_mut().pending = Some(h);
            }
        }
        RenderLoopInner { state: Some(state) }
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
    use std::thread;
    use std::time::{Duration, Instant};

    /// Target frame interval. 60fps is fine for the UI-level animated
    /// surface use case the framework targets; authors who want 120Hz
    /// promotion are better off driving the loop themselves against
    /// the bare `graphics` primitive.
    const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

    pub(super) struct RenderLoopInner {
        tx: Option<Sender<()>>,
        join: Option<thread::JoinHandle<()>>,
    }

    impl RenderLoopInner {
        pub(super) fn cancel(&mut self) {
            // Signal first so the worker breaks out of its
            // `recv_timeout` on its next loop iteration (within
            // FRAME_INTERVAL at worst — typically immediately). Then
            // join so the surrounding `Drop` can't race with a
            // half-running render call.
            if let Some(tx) = self.tx.take() {
                let _ = tx.send(());
            }
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
        }
    }

    impl Drop for RenderLoopInner {
        fn drop(&mut self) {
            self.cancel();
        }
    }

    pub(super) fn start<F: FnMut(f32) + Send + 'static>(mut f: F) -> RenderLoopInner {
        let (tx, rx) = channel::<()>();
        let started = Instant::now();
        let join = thread::Builder::new()
            .name("framework-render-loop".into())
            .spawn(move || {
                // Block on the cancel channel with a frame-length
                // timeout. When the timeout elapses we fire one frame
                // then loop back to wait again — that paces the loop
                // at ~60fps without relying on wgpu's `present()` to
                // block (which it only does for healthy surfaces; a
                // surface-lost / context-recreated state returns
                // immediately, which would otherwise spin the CPU at
                // 100% until cancellation arrives).
                //
                // Cancellation responsiveness: bounded by
                // FRAME_INTERVAL (so ~16ms worst case). The
                // surrounding `cancel()` sends on `tx` and joins;
                // join completes once the next `recv_timeout` returns
                // `Ok(())` and we break out.
                loop {
                    match rx.recv_timeout(FRAME_INTERVAL) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    let elapsed = started.elapsed().as_secs_f32();
                    f(elapsed);
                }
            })
            .expect("framework: failed to spawn render loop thread");
        RenderLoopInner {
            tx: Some(tx),
            join: Some(join),
        }
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Instant;

    pub(super) struct RenderLoopInner {
        // Retained NSTimer (CADisplayLink would be more accurate but
        // requires declaring a custom ObjC class with a target/action
        // selector pair — heavier ObjC runtime story than we want in
        // framework-core. NSTimer at 1/60s is fine for animated
        // surfaces; authors who need 120Hz Promotion can still write
        // a custom CADisplayLink driver against the bare `graphics`
        // primitive.).
        timer: Option<objc2::rc::Retained<objc2_foundation::NSObject>>,
        // Holds the closure alive while the timer fires it. The
        // block inside the timer also holds an Rc clone; the timer's
        // `invalidate` drops that clone, then `cancel()` drops this
        // one and the closure goes with it. No `Send` bound — iOS
        // fires the timer on the same thread that installed it.
        _state: Rc<RefCell<Box<dyn FnMut(f32) + 'static>>>,
    }

    impl RenderLoopInner {
        pub(super) fn cancel(&mut self) {
            if let Some(timer) = self.timer.take() {
                let _: () = unsafe { objc2::msg_send![&timer, invalidate] };
            }
        }
    }

    impl Drop for RenderLoopInner {
        fn drop(&mut self) {
            self.cancel();
        }
    }

    pub(super) fn start<F: FnMut(f32) + 'static>(f: F) -> RenderLoopInner {
        use block2::StackBlock;
        use objc2::msg_send_id;
        use objc2_foundation::NSObject;
        let started = Instant::now();
        // `StackBlock::new` needs a `Clone` closure. Our `FnMut` box
        // isn't `Clone`, so we wrap it in `Rc<RefCell<...>>` —
        // cloning the Rc inside the block is cheap, and we hold one
        // strong reference here in `_state` so the closure outlives
        // any timer fire-and-drop race.
        let state: Rc<RefCell<Box<dyn FnMut(f32) + 'static>>> =
            Rc::new(RefCell::new(Box::new(f)));
        let state_for_block = state.clone();
        let block = StackBlock::new(move |_t: *const NSObject| {
            let elapsed = started.elapsed().as_secs_f32();
            (state_for_block.borrow_mut())(elapsed);
        });
        let block = block.copy();
        let timer: objc2::rc::Retained<NSObject> = unsafe {
            msg_send_id![
                objc2::class!(NSTimer),
                scheduledTimerWithTimeInterval: (1.0 / 60.0) as f64,
                repeats: true,
                block: &*block
            ]
        };
        RenderLoopInner {
            timer: Some(timer),
            _state: state,
        }
    }
}

// Fallback for targets with no specific driver yet (desktop today).
// The loop never fires. Authors who want desktop support can drive
// a loop themselves with `std::thread::spawn` + the existing
// `graphics` primitive.
#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
mod imp {
    pub(super) struct RenderLoopInner;

    impl RenderLoopInner {
        pub(super) fn cancel(&mut self) {}
    }

    pub(super) fn start<F: FnMut(f32) + 'static>(_f: F) -> RenderLoopInner {
        eprintln!(
            "framework_core::driver::render_loop: no driver compiled in for this target — \
             the loop will never fire. Drive your own loop against the bare `graphics` primitive."
        );
        RenderLoopInner
    }
}
