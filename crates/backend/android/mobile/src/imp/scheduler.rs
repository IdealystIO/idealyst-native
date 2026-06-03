//! Android `Scheduler`: `Handler(Looper.getMainLooper()).postDelayed`
//! for `after_ms` / `after_animation_frame` / `raf_loop`, `Handler.post`
//! (delay 0) for microtasks.
//!
//! The layout-pass scheduler is the one exception: its INITIAL attempt
//! runs on a `Choreographer.FrameCallback` (`postFrameCallback`) so the
//! Taffy frames it writes land BEFORE the next frame's view traversal —
//! a `postDelayed` message would run after the traversal and a
//! dynamically-mounted subtree would paint unlaid-out for one frame. See
//! `schedule_layout_pass_retry` / `schedule_frame_callback` below.
//!
//! `runtime_core::scheduling` falls back to synchronous execution
//! on native when no scheduler is installed — fine for
//! `schedule_microtask` (immediate dispatch is correct semantics on
//! a single-threaded native target), but **wrong for `after_ms`**:
//! firing the callback at call time defeats the delay. The
//! long-press touch recognizer trips over this; presence animations
//! and any other timer-driven feature follow.
//!
//! Hosts call [`install_scheduler`] once at startup, before the
//! first `runtime_core::render(...)`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

use runtime_core::scheduling::{
    install_scheduler as install, ScheduleHandle, Scheduler,
};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jlong;
use jni::JNIEnv;

use super::with_env;

// Thread-local registry of scheduled callbacks. Both `nativeInvoke`
// (from the JVM Runnable) and `ScheduledHandle::cancel` (from Rust)
// take ownership via `HashMap::remove`, so whichever runs first
// fires-or-drops the closure and the other gets a no-op. Safe by
// construction — no leaked pointers, no double-free.
//
// History: the original design leaked `Box<dyn FnOnce>` via
// `Box::into_raw` and handed the raw pointer to both sides. The
// JNI invoke and the Rust `cancel` each tried `Box::from_raw` on
// the same address; cancel-after-fire then double-freed and
// SIGSEGV'd with `fault addr 0x72702d676e6f6c5b` (ASCII
// "rp-gnol[" — a string fragment interpreted as a pointer).
thread_local! {
    static CALLBACKS: RefCell<HashMap<i64, Box<dyn FnOnce() + 'static>>> =
        RefCell::new(HashMap::new());
    static NEXT_ID: Cell<i64> = const { Cell::new(1) };
}

/// Register this backend's scheduler with `runtime-core`.
/// Idempotent — first install wins.
///
/// Also installs the cooperative async executor
/// ([`crate::imp::async_executor::install_async_executor`]) so
/// `runtime_core::driver::spawn_async` polls futures on the main looper
/// instead of falling back to `pollster::block_on` (which would FREEZE the
/// main thread — a hard ANR for any future that needs the looper to make
/// progress, e.g. the `camera` SDK's main-thread Camera2 setup). Matches the
/// Apple scheduler, which installs its executor from `install_scheduler` too.
pub fn install_scheduler() {
    install(Box::new(AndroidScheduler));
    // Gated on `async-driver` (the feature that brings `runtime_core::driver`
    // into scope); mirrors the Apple scheduler installing its executor here.
    #[cfg(feature = "async-driver")]
    crate::imp::async_executor::install_async_executor();
}

/// Post a `RustAsyncPoll(id)` to the cached main-looper `Handler` so
/// `async_executor::poll_task(id)` re-runs on the main thread. Exposed for
/// the async executor's `TaskWaker`, which fires on a BACKGROUND thread and
/// needs the thread-safe `Handler.post` marshal. Lives here because the
/// cached `Handler` (`main_handler`) is private to this module; the runnable
/// construction itself lives next to its consumer in `async_executor`.
#[cfg(feature = "async-driver")]
pub(crate) fn post_async_poll_to_main(env: &mut JNIEnv, id: u64) {
    let mh = main_handler();
    crate::imp::async_executor::construct_and_post_async_poll(env, mh.handler.as_obj(), id);
}

struct AndroidScheduler;

unsafe impl Send for AndroidScheduler {}
unsafe impl Sync for AndroidScheduler {}

impl Scheduler for AndroidScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        // Microtasks are fire-and-forget — the caller has no handle
        // to keep alive. `ScheduledHandle::Drop` cancels (it removes
        // the runnable from the looper and drops the closure), so
        // we must leak the handle so the underlying `Handler.post`
        // actually fires. The closure's entry in `CALLBACKS` is
        // removed by the JVM-side invoke when it runs, so the only
        // leaked storage is the empty handle shell.
        std::mem::forget(schedule_runnable(0, f));
    }

    fn after_animation_frame(
        &self,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        // One frame ≈ 16 ms. Choreographer.postFrameCallback would
        // be more accurate but the API takes a `FrameCallback`
        // class, an extra Kotlin shim. The 16ms Handler.postDelayed
        // path matches what the iOS scheduler does and what the
        // existing `RenderLoopDriver`s on both platforms accept as
        // "near enough to vsync."
        Box::new(schedule_runnable(16, f))
    }

    fn after_ms(
        &self,
        delay_ms: i32,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        Box::new(schedule_runnable(delay_ms.max(0), f))
    }

    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        // Self-reposting Handler.postDelayed at ~16ms gives a
        // 60Hz tick on the main thread. Choreographer would be more
        // accurate but needs a separate Kotlin FrameCallback class.
        // The existing iOS scheduler and `RenderLoopDriver`s on both
        // platforms accept the 16ms timer as "near enough to vsync."
        //
        // Cancellation: the wrapped `Rc<Cell<bool>>` flips to false
        // on drop, and the next scheduled tick checks it before
        // re-posting. Worst case the loop runs one extra tick after
        // cancel — acceptable for a 16ms cadence.
        Box::new(start_raf_loop(f))
    }
}

/// Schedule a Taffy layout pass with retry. The host view's
/// `getWidth()/getHeight()` read back 0×0 at `finish` time (Android
/// lays out asynchronously on the next vsync frame). The first
/// scheduled pass usually sees 0×0; on `retry_count == 0` we then
/// re-schedule with a longer delay so the SECOND pass runs after
/// the host has been measured. Two attempts is enough for the
/// initial mount in practice; further retries land at 32/64/128ms
/// in case the activity stack still hasn't laid out (rare — e.g.
/// resuming a backgrounded process). Stops once the host reports a
/// non-zero size and the layout pass actually applies frames.
thread_local! {
    /// Coalescing flag: set when a layout pass is queued (via
    /// `Handler.postDelayed`) and cleared at the start of the queued
    /// pass. Mirrors the iOS `LAYOUT_PASS_QUEUED` pattern — see
    /// [[project_ios_schedule_layout_coalesce]]. Without coalescing,
    /// any future code path that calls `schedule_layout_pass_retry`
    /// in a loop (per-insert, per-style-change, per-frame) would
    /// post N runnables that each fire a full-tree layout. Today the
    /// callsites are limited (`finish`, `swap_body`,
    /// `notify_config_changed`) and don't loop, but the flag costs
    /// ~nothing and prevents the iOS-style regression from sneaking
    /// in later.
    static LAYOUT_PASS_QUEUED: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

pub(crate) fn schedule_layout_pass_retry(retry_count: u32) {
    // Retry attempts deliberately bypass the coalescing flag: the
    // retry exists specifically because the previous pass ran with
    // `viewport_is_ready == false` (host still measuring), so the
    // outer "queued" state is already cleared by the time we reach
    // here. The retry's job is to schedule ANOTHER pass after a
    // backoff delay. Skipping it would drop the retry chain entirely
    // and leave the initial 0×0 mount stuck.
    if retry_count == 0 {
        if LAYOUT_PASS_QUEUED.with(|q| q.replace(true)) {
            // First-attempt call but a pass is already queued.
            // The pending pass will see whatever state our caller
            // just produced; drop this redundant post.
            return;
        }
    }
    let weak = super::ANDROID_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else {
        // Backend already gone — clear the flag so a later install +
        // schedule cycle isn't permanently jammed.
        LAYOUT_PASS_QUEUED.with(|q| q.set(false));
        return;
    };

    let run_pass = move || {
        // Clear the queued flag BEFORE running so any
        // `schedule_layout_pass_retry(0)` arriving during the pass
        // re-arms a fresh post — matches iOS's coalescing semantics.
        if retry_count == 0 {
            LAYOUT_PASS_QUEUED.with(|q| q.set(false));
        }
        let Some(rc) = weak.upgrade() else { return };
        let mut viewport_ok = false;
        if let Ok(mut b) = rc.try_borrow_mut() {
            b.run_layout_pass();
            // `run_layout_pass` bails when viewport is 0×0; we
            // detect that by re-reading after the call. Re-reading
            // via the same `viewport_size` helper is cheap (one JNI
            // call pair).
            viewport_ok = b.viewport_is_ready();
        };
        if !viewport_ok && retry_count < 4 {
            schedule_layout_pass_retry(retry_count + 1);
        }
    };

    if retry_count == 0 {
        // INITIAL attempt: run on a `Choreographer` frame callback, NOT
        // `Handler.postDelayed`. A frame callback fires at the START of
        // the next frame — in the animation/input callback phase, BEFORE
        // that frame's measure/layout/draw traversal — so the Taffy
        // frames this pass writes onto the views' LayoutParams are in
        // place before the views are drawn. `postDelayed(0/16ms)` posts a
        // looper message that typically runs AFTER the traversal, so a
        // dynamically-mounted subtree (a modal/portal mounted when its
        // open signal flips during input dispatch) paints once
        // UNLAID-OUT (the card at the origin / top-left) and visibly
        // snaps into place. Choreographer is the principled fix for that
        // jank and helps every dynamic mount, not just modals. See
        // [[project_android_layout_before_paint_choreographer]].
        schedule_frame_callback(Box::new(run_pass));
    } else {
        // RETRY attempts: the previous pass ran while the host was still
        // 0×0 (process resume, activity tree not yet measured). A frame
        // callback would just re-fire next frame still-unmeasured; we
        // need to back off in wall-clock time to give the host a chance
        // to be measured. Exponential backoff: 32, 64, 128 ms then give
        // up. `Handler.postDelayed` is correct here (we WANT a delay, and
        // there is no paint to beat — nothing is laid out yet).
        let delay = 16i32 << retry_count.min(3);
        // `ScheduledHandle::Drop` cancels the runnable (and removes the
        // closure from CALLBACKS), so we must leak the handle for the
        // post to actually fire. See [[project_android_scheduler_handle_leak]].
        std::mem::forget(schedule_runnable(delay, Box::new(run_pass)));
    }
}

/// Register `f` and post a `Choreographer.FrameCallback` that fires it
/// at the start of the next frame, before the frame's view traversal.
/// Reuses the same id→`CALLBACKS`→`nativeInvoke` registry as
/// `schedule_runnable` (so the closure is owned, fire-or-drop, no leaked
/// pointer). Frame callbacks are one-shot and we never cancel this one,
/// so there is no `ScheduleHandle`/`removeCallbacks` path.
fn schedule_frame_callback(f: Box<dyn FnOnce() + 'static>) {
    let id = NEXT_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    CALLBACKS.with(|m| m.borrow_mut().insert(id, f));
    with_env(|env| {
        let class = env
            .find_class("io/idealyst/runtime/RustFrameCallback")
            .expect("RustFrameCallback class missing — bundle the kotlin runtime");
        let cb = env
            .new_object(&class, "(J)V", &[JValue::Long(id as jlong)])
            .expect("new RustFrameCallback failed");
        // `RustFrameCallback.post()` calls
        // `Choreographer.getInstance().postFrameCallback(this)` on the
        // current (UI) thread. We must hold no extra ref — the
        // Choreographer retains the callback until it fires, and the
        // closure removes itself from CALLBACKS on `nativeInvoke`.
        let _ = env.call_method(&cb, "post", "()V", &[]);
    });
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Cached `Handler` bound to the main `Looper`, plus the `Method`
/// lookups for `postDelayed` / `removeCallbacks`. We hold a single
/// shared instance for the process lifetime — `Handler` is cheap to
/// allocate but caching avoids a method-resolution dance per
/// scheduled callback.
struct MainHandler {
    handler: GlobalRef,
}

static MAIN_HANDLER: OnceLock<MainHandler> = OnceLock::new();

fn main_handler() -> &'static MainHandler {
    MAIN_HANDLER.get_or_init(|| {
        with_env(|env| {
            // Looper.getMainLooper()
            let looper_class = env
                .find_class("android/os/Looper")
                .expect("Looper class missing");
            let looper = env
                .call_static_method(
                    &looper_class,
                    "getMainLooper",
                    "()Landroid/os/Looper;",
                    &[],
                )
                .expect("Looper.getMainLooper() call failed")
                .l()
                .expect("Looper.getMainLooper() returned null");
            // new Handler(Looper)
            let handler_class = env
                .find_class("android/os/Handler")
                .expect("Handler class missing");
            let handler = env
                .new_object(
                    &handler_class,
                    "(Landroid/os/Looper;)V",
                    &[JValue::Object(&looper)],
                )
                .expect("new Handler(Looper) failed");
            let global = env
                .new_global_ref(handler)
                .expect("new_global_ref(Handler)");
            MainHandler { handler: global }
        })
    })
}

/// One scheduled callback. The closure lives in the thread-local
/// `CALLBACKS` map keyed by `id`; whichever side runs first
/// (`nativeInvoke` from the JVM Runnable, or `cancel` from Rust)
/// removes the entry and the other gets a no-op. The `runnable`
/// `GlobalRef` is held so `cancel` can `Handler.removeCallbacks(it)`
/// — without it, a cancel after we'd already returned the handle
/// to the caller could leak the JVM Runnable.
struct ScheduledHandle {
    id: i64,
    runnable: Option<GlobalRef>,
}

impl ScheduleHandle for ScheduledHandle {
    fn cancel(&mut self) {
        let Some(runnable) = self.runnable.take() else {
            return;
        };
        // Remove the closure first. After this the JNI invoke (if
        // already in flight) will see `None` and no-op — protects
        // us from the cancel-races-Handler-dispatch case.
        CALLBACKS.with(|m| m.borrow_mut().remove(&self.id));
        let mh = main_handler();
        with_env(|env| {
            let _ = env.call_method(
                mh.handler.as_obj(),
                "removeCallbacks",
                "(Ljava/lang/Runnable;)V",
                &[JValue::Object(runnable.as_obj())],
            );
        });
    }
}

impl Drop for ScheduledHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// `raf_loop` self-reposts via this — each tick:
/// 1. Checks the `running` flag (was the handle dropped/cancelled?
///    Then bail out and don't re-post).
/// 2. Calls the user's `FnMut`.
/// 3. Schedules the next `Handler.postDelayed(16ms)` with a fresh
///    one-shot closure that does (1)+(2)+(3) again.
///
/// The wrapped `Rc<Cell<bool>>` is the cancellation channel — its
/// `Cell<bool>` flips false in `RafLoopHandle::cancel` and the next
/// tick reads false then exits the loop without re-posting.
fn start_raf_loop(f: Box<dyn FnMut() + 'static>) -> RafLoopHandle {
    use std::rc::Rc;
    let running = Rc::new(Cell::new(true));
    let cb: Rc<RefCell<Box<dyn FnMut() + 'static>>> = Rc::new(RefCell::new(f));
    schedule_next_tick(running.clone(), cb);
    RafLoopHandle { running }
}

fn schedule_next_tick(running: std::rc::Rc<Cell<bool>>, cb: std::rc::Rc<RefCell<Box<dyn FnMut() + 'static>>>) {
    if !running.get() {
        return;
    }
    let running_for_closure = running.clone();
    let cb_for_closure = cb.clone();
    let handle = schedule_runnable(16, Box::new(move || {
        if !running_for_closure.get() {
            return;
        }
        // Run the user's tick.
        (cb_for_closure.borrow_mut())();
        // Re-arm.
        schedule_next_tick(running_for_closure, cb_for_closure);
    }));
    // `ScheduledHandle::Drop` calls `cancel()` which removes the
    // runnable from the main looper AND drops the closure from
    // `CALLBACKS`. We need the runnable to *fire* (one-shot), so we
    // mustn't drop the handle here. Leak it — the closure removes
    // itself from `CALLBACKS` when the JVM invoke consumes it, so
    // the underlying memory still cleans up after fire. The leaked
    // shell is just an empty `ScheduledHandle { id, runnable: None }`-
    // worth of bytes per tick, freed by process exit.
    std::mem::forget(handle);
}

struct RafLoopHandle {
    running: std::rc::Rc<Cell<bool>>,
}

impl ScheduleHandle for RafLoopHandle {
    fn cancel(&mut self) {
        self.running.set(false);
    }
}

impl Drop for RafLoopHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Post a one-shot runnable and leak its handle so it actually fires
/// (dropping the handle cancels the post — see
/// [[project_android_scheduler_handle_leak]]). Use when the caller has no
/// reason to cancel. Keeps `ScheduledHandle` private to this module.
///
/// Currently unused — the only caller was the old Dialog-portal
/// deferred-`show()` band-aid, removed when portals became view
/// overlays. Kept as generic fire-and-forget scheduling infra.
#[allow(dead_code)]
pub(crate) fn post_runnable(delay_ms: i32, f: Box<dyn FnOnce() + 'static>) {
    std::mem::forget(schedule_runnable(delay_ms, f));
}

fn schedule_runnable(
    delay_ms: i32,
    f: Box<dyn FnOnce() + 'static>,
) -> ScheduledHandle {
    let id = NEXT_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    CALLBACKS.with(|m| m.borrow_mut().insert(id, f));
    let mh = main_handler();
    let runnable = with_env(|env| {
        let class = env
            .find_class("io/idealyst/runtime/RustScheduledRunnable")
            .expect("RustScheduledRunnable class missing — bundle the kotlin runtime");
        let local = env
            .new_object(&class, "(J)V", &[JValue::Long(id as jlong)])
            .expect("new RustScheduledRunnable failed");
        let global = env
            .new_global_ref(local)
            .expect("new_global_ref(RustScheduledRunnable)");
        let _ = env.call_method(
            mh.handler.as_obj(),
            "postDelayed",
            "(Ljava/lang/Runnable;J)Z",
            &[JValue::Object(global.as_obj()), JValue::Long(delay_ms as i64)],
        );
        global
    });
    ScheduledHandle {
        id,
        runnable: Some(runnable),
    }
}

// ---------------------------------------------------------------------------
// JNI exports — invoked by `RustScheduledRunnable.run()`
// ---------------------------------------------------------------------------

/// `RustScheduledRunnable.run` → `nativeInvoke(id)`. Removes the
/// closure from the thread-local registry and runs it. If `cancel`
/// already removed it (cancel-races-dispatch), this is a silent
/// no-op.
///
/// `id` is the registry key, not a pointer — see the doc on
/// `CALLBACKS` for why we abandoned the leaked-pointer design.
///
/// Wrapped in `catch_unwind` because Rust panics across the JNI
/// boundary are UB. Caught panics log to `idealyst` logcat so they
/// don't disappear silently.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustScheduledRunnable_nativeInvoke(
    _env: JNIEnv,
    _this: JObject,
    id: jlong,
) {
    let cb = CALLBACKS.with(|m| m.borrow_mut().remove(&(id as i64)));
    if let Some(f) = cb {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f()));
        if let Err(payload) = result {
            // Log first so the message hits logcat, then abort. JNI
            // boundary makes a Rust unwind UB; project policy is
            // crash-loud so we never keep running on the partially-
            // invariant state that produced the panic.
            log::error!(
                "scheduled callback (id={}) panicked: {}",
                id,
                panic_message(&payload),
            );
            std::process::abort();
        }
    }
}

/// `RustFrameCallback.doFrame` → `nativeInvoke(id)`. Identical
/// semantics to the `RustScheduledRunnable` export: remove the closure
/// from the thread-local registry and run it inside `catch_unwind`
/// (panic across JNI is UB → log + abort, crash-loud). Frame callbacks
/// are one-shot, so once consumed the id is dead.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustFrameCallback_nativeInvoke(
    _env: JNIEnv,
    _this: JObject,
    id: jlong,
) {
    let cb = CALLBACKS.with(|m| m.borrow_mut().remove(&(id as i64)));
    if let Some(f) = cb {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f()));
        if let Err(payload) = result {
            log::error!(
                "frame callback (id={}) panicked: {}",
                id,
                panic_message(&payload),
            );
            std::process::abort();
        }
    }
}

/// `RustScheduledRunnable.nativeDrop(id)` — fallback drop path
/// exported for symmetry. Rust's `ScheduledHandle::cancel` already
/// handles the live cancel case via the registry; this is here so
/// the Kotlin side can call it from `finalize` once we wire that.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustScheduledRunnable_nativeDrop(
    _env: JNIEnv,
    _this: JObject,
    id: jlong,
) {
    CALLBACKS.with(|m| m.borrow_mut().remove(&(id as i64)));
}

/// Best-effort `&dyn Any → String` for panic payloads. The standard
/// library's `panic::Location` info doesn't reach us here (we're
/// inside `catch_unwind`); the payload is what `panic!()` passed,
/// usually a `&'static str` or a `String`. Falls back to a
/// placeholder when neither fits.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "<panic payload not a string>".to_string()
}
