//! Cooperative main-thread async executor for the Android backend.
//!
//! `runtime_core::driver::spawn_async` falls back to `pollster::block_on`
//! when no executor is installed — it drives a future *to completion on the
//! calling thread*. On Android that calling thread is the JVM main
//! (`Looper`) thread, so `block_on` FREEZES it until the future finishes.
//! For any future that itself needs the main `Looper` to make progress this
//! is a hard DEADLOCK: the `camera` SDK's Kotlin shim posts its Camera2
//! setup to `Looper.getMainLooper()`, but `block_on` is parked ON that
//! looper waiting for the setup to complete — so the setup runnable never
//! runs, the future never resolves, and Android raises an ANR
//! ("Camera Preview isn't responding").
//!
//! This executor instead polls each spawned future *cooperatively* on the
//! main looper: it runs the future only up to its next `.await`, then
//! yields control back to the looper, and re-polls on wake. The futures
//! stay single-threaded on the main thread — which matches the reactive
//! system — so a background-thread waker (a Camera2 callback off-main) is
//! marshalled back onto the looper rather than touching `!Send` state
//! off-thread.
//!
//! Mirrors `backend-apple-core`'s `AppleAsyncExecutor`; the only difference
//! is the cross-thread → main marshal mechanism: Apple uses
//! `dispatch_async(main_q, …)`, we post a `Runnable` to a `Handler` bound to
//! the main `Looper` (`Handler.post` is thread-safe — callable from any
//! thread).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use jni::objects::{JObject, JValue};
use jni::sys::jlong;
use jni::JNIEnv;
use runtime_core::driver::AsyncExecutor;

use super::with_env;
use crate::imp::scheduler::post_async_poll_to_main;

thread_local! {
    /// Pending futures keyed by id. Lives on the main thread only — `spawn`
    /// and `poll_task` both run on the JVM main (`Looper`) thread, so the
    /// `!Send` futures never cross a thread boundary; the waker carries only
    /// the `u64` id (a `Copy`, `Send` value).
    static TASKS: RefCell<HashMap<u64, Pin<Box<dyn Future<Output = ()> + 'static>>>> =
        RefCell::new(HashMap::new());
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// Register the Android cooperative executor with `runtime-core`. Idempotent
/// (first install wins). Called from [`crate::imp::scheduler::install_scheduler`].
pub fn install_async_executor() {
    runtime_core::driver::install_async_executor(Box::new(AndroidAsyncExecutor));
}

struct AndroidAsyncExecutor;

// SAFETY: a unit struct with no fields, so the `AsyncExecutor: Send + Sync`
// bound is satisfied trivially. The `!Send` futures it spawns live only in
// the main-thread `TASKS` thread-local and are never polled off-main — the
// waker marshals every re-poll back onto the main looper before touching
// `TASKS`.
unsafe impl Send for AndroidAsyncExecutor {}
unsafe impl Sync for AndroidAsyncExecutor {}

impl AsyncExecutor for AndroidAsyncExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        // Called on the main thread during render/build. Register, then poll
        // once synchronously so the future advances to its first `.await`
        // before `spawn` returns — but it does NOT drive to completion.
        let id = NEXT_ID.with(|n| {
            let v = n.get();
            n.set(v.wrapping_add(1));
            v
        });
        TASKS.with(|t| t.borrow_mut().insert(id, future));
        poll_task(id);
    }
}

/// Poll the task with `id` on the main thread: take it out, poll with a
/// re-scheduling waker, reinsert if still pending (drop it if it completed).
///
/// Always runs on the JVM main (`Looper`) thread — either synchronously from
/// `spawn`, or from `RustAsyncPoll.run()` which the `Handler` dispatches on
/// the main looper.
pub(crate) fn poll_task(id: u64) {
    // Take the future OUT while polling so a re-entrant `poll_task` (a wake
    // that posted a re-poll onto the looper) can't double-borrow or
    // double-poll the same instance — it sees `None` and no-ops.
    let fut = TASKS.with(|t| t.borrow_mut().remove(&id));
    let Some(mut fut) = fut else {
        return; // already completed or never registered
    };
    let waker = Waker::from(Arc::new(TaskWaker { id }));
    let mut cx = Context::from_waker(&waker);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(()) => { /* done — drop the future */ }
        Poll::Pending => {
            TASKS.with(|t| t.borrow_mut().insert(id, fut));
        }
    }
}

/// Waker carrying just the task id. `wake` may fire from ANY thread (e.g. a
/// Camera2 capture-session callback on a background handler thread); it
/// marshals a re-poll onto the main looper so the continuation runs on main.
struct TaskWaker {
    id: u64,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        dispatch_poll(self.id);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        dispatch_poll(self.id);
    }
}

/// Schedule `poll_task(id)` on the main looper. Background-safe: the only
/// thing that crosses the thread boundary is `id` (a `Copy` `u64`) — no `Rc`
/// / `!Send` state and no thread-local lookup happens off-main.
///
/// CRITICAL: we deliberately do NOT reuse the scheduler's `RustScheduledRunnable`
/// / `CALLBACKS` path. That registry is THREAD-LOCAL (designed for
/// main-thread-only scheduling): a runnable constructed here on the
/// background waker thread would register its closure in the BACKGROUND
/// thread's thread-local `CALLBACKS`, and `nativeInvoke` (running back on
/// main) would look up the id in the MAIN thread's `CALLBACKS`, find
/// nothing, and silently drop the re-poll. So we use a separate, id-only
/// path: `RustAsyncPoll` carries just the `jlong` id across the boundary and
/// `nativePoll` calls `poll_task(id)` directly — nothing is looked up in a
/// thread-local until we're back on main inside `TASKS`.
///
/// `with_env` attaches the calling (possibly background) thread to the JVM
/// so we can construct the runnable; `Handler.post` is thread-safe and does
/// the actual main-thread marshal.
fn dispatch_poll(id: u64) {
    with_env(|env| {
        post_async_poll_to_main(env, id);
    });
}

// ---------------------------------------------------------------------------
// JNI export — invoked by `RustAsyncPoll.run()` on the main looper
// ---------------------------------------------------------------------------

/// `RustAsyncPoll.run` → `nativePoll(id)`. Runs on the JVM main (`Looper`)
/// thread because the `Handler` it was posted to is bound to the main
/// looper. Re-polls the task; `poll_task` no-ops if it already completed.
///
/// Wrapped in `catch_unwind` + log + `abort` because a Rust panic unwinding
/// across the JNI boundary is UB. Crash-loud is the project policy — matches
/// `RustScheduledRunnable_nativeInvoke`.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustAsyncPoll_nativePoll(
    _env: JNIEnv,
    _this: JObject,
    id: jlong,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        poll_task(id as u64)
    }));
    if result.is_err() {
        // Log first so the message hits logcat, then abort. Continuing past
        // a panic across JNI is UB, and crash-loud is the project policy.
        log::error!("async-executor task poll (id={}) panicked — aborting", id);
        std::process::abort();
    }
}

/// Construct a `RustAsyncPoll(id)` and `Handler.post` it to the main looper.
/// Lives here (rather than in `scheduler.rs`) so the runnable-construction
/// detail sits next to its sole consumer; the scheduler only needs to expose
/// the cached main `Handler`. Split into a named fn so the JValue marshalling
/// reads clearly at the one call site in `dispatch_poll`.
pub(crate) fn construct_and_post_async_poll(
    env: &mut JNIEnv,
    handler: &JObject,
    id: u64,
) {
    // `find_app_class` (not `env.find_class`): this runs on whatever thread
    // the `TaskWaker` fired on — often a background worker — where a bare
    // `find_class` resolves against the system classloader and can't see
    // `io.idealyst.*`, aborting the process with `ClassNotFoundException`.
    let class = crate::imp::find_app_class(env, "io/idealyst/runtime/RustAsyncPoll")
        .expect("RustAsyncPoll class missing — bundle the kotlin runtime");
    let runnable = env
        .new_object(&class, "(J)V", &[JValue::Long(id as jlong)])
        .expect("new RustAsyncPoll failed");
    // `Handler.post(Runnable)` is thread-safe — callable from any thread,
    // which is exactly why it's the right cross-thread → main marshal here.
    // We hold no extra ref: the looper's message queue retains the runnable
    // until it drains it on the main thread.
    let _ = env.call_method(
        handler,
        "post",
        "(Ljava/lang/Runnable;)Z",
        &[JValue::Object(&runnable)],
    );
}
