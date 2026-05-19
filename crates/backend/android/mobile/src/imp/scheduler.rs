//! Android `Scheduler`: `Handler(Looper.getMainLooper()).postDelayed`
//! for `after_ms`, `Handler.post` for microtasks, `Choreographer`
//! (via `postFrameCallback`) for `after_animation_frame` / `raf_loop`.
//!
//! `framework_core::scheduling` falls back to synchronous execution
//! on native when no scheduler is installed — fine for
//! `schedule_microtask` (immediate dispatch is correct semantics on
//! a single-threaded native target), but **wrong for `after_ms`**:
//! firing the callback at call time defeats the delay. The
//! long-press touch recognizer trips over this; presence animations
//! and any other timer-driven feature follow.
//!
//! Hosts call [`install_scheduler`] once at startup, before the
//! first `framework_core::render(...)`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

use framework_core::scheduling::{
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

/// Register this backend's scheduler with `framework-core`.
/// Idempotent — first install wins.
pub fn install_scheduler() {
    install(Box::new(AndroidScheduler));
}

struct AndroidScheduler;

unsafe impl Send for AndroidScheduler {}
unsafe impl Sync for AndroidScheduler {}

impl Scheduler for AndroidScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        let _ = schedule_runnable(0, f);
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
    let _ = schedule_runnable(16, Box::new(move || {
        if !running_for_closure.get() {
            return;
        }
        // Run the user's tick.
        (cb_for_closure.borrow_mut())();
        // Re-arm.
        schedule_next_tick(running_for_closure, cb_for_closure);
    }));
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
            // Surface the panic — without this it disappears into
            // the JNI return and we never see why a scheduled
            // callback misbehaved.
            log::error!(
                "scheduled callback (id={}) panicked: {}",
                id,
                panic_message(&payload),
            );
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
