//! Cooperative main-thread async executor for Apple platforms.
//!
//! `runtime_core::driver::spawn_async` falls back to `pollster::block_on`
//! when no executor is installed — it drives a future *to completion on the
//! calling thread*. That's fine for a short one-shot future (an async
//! renderer setup), but it FREEZES the main thread for a long-running
//! future: an SSE / WebSocket `recv` loop never returns, so `block_on`
//! never hands control back to UIKit's run loop and nothing renders (the
//! "white screen" symptom for `use_sse` / `use_socket` on iOS).
//!
//! This executor instead polls each spawned future *cooperatively* on the
//! main run loop: it runs the future only up to its next `.await`, then
//! yields, and re-polls on wake via `dispatch_async(main_q, …)`. Everything
//! stays single-threaded on the main thread — which matches the reactive
//! system — so when a background-thread waker fires (e.g. an `NSURLSession`
//! delegate delivering an SSE event off the main queue), the continuation
//! that touches signals / UIViews is marshalled back to main rather than
//! mutating UIKit off-thread.
//!
//! Mirrors the web backend's `WasmAsyncExecutor` (`spawn_local` on the JS
//! event loop); here the event loop is libdispatch's main queue.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use block2::StackBlock;
use runtime_core::driver::AsyncExecutor;

use crate::scheduler::{dispatch_async, _dispatch_main_q};

thread_local! {
    /// Pending futures keyed by id. Lives on the main thread only — `spawn`
    /// and `poll_task` both run on main, so the `!Send` futures never cross
    /// a thread boundary; the waker carries only the `u64` id.
    static TASKS: RefCell<HashMap<u64, Pin<Box<dyn Future<Output = ()> + 'static>>>> =
        RefCell::new(HashMap::new());
    static NEXT_ID: Cell<u64> = Cell::new(0);
}

/// Register the Apple cooperative executor with `runtime-core`. Idempotent
/// (first install wins). Called from [`crate::scheduler::install_scheduler`].
pub fn install_async_executor() {
    runtime_core::driver::install_async_executor(Box::new(AppleAsyncExecutor));
}

struct AppleAsyncExecutor;

// SAFETY: a unit struct with no fields, so the `AsyncExecutor: Send + Sync`
// bound is satisfied trivially. The `!Send` futures it spawns live only in
// the main-thread `TASKS` thread-local and are never polled off-main.
unsafe impl Send for AppleAsyncExecutor {}
unsafe impl Sync for AppleAsyncExecutor {}

impl AsyncExecutor for AppleAsyncExecutor {
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
fn poll_task(id: u64) {
    // Take the future OUT while polling so a re-entrant `poll_task` (a wake
    // dispatched onto the serial main queue) can't double-borrow or
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

/// Waker carrying just the task id. `wake` may fire from ANY thread (e.g. an
/// `NSURLSession` delegate's background queue); it marshals a re-poll onto
/// the main queue so the continuation runs on main.
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

/// Schedule `poll_task(id)` on the main queue. Background-safe: the block
/// captures only `id` (a `Copy` `u64`) — no `Rc` / `!Send` state crosses the
/// thread boundary (unlike the scheduler's `dispatch_main_async`, which is
/// main-thread-only). Crash-loud on panic, matching the scheduler's blocks.
fn dispatch_poll(id: u64) {
    let block = StackBlock::new(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poll_task(id)));
        if result.is_err() {
            // The block runs through libdispatch's `extern "C"` drain; a Rust
            // panic unwinding past it would abort with no message. Abort
            // ourselves after logging — crash-loud is the project policy.
            eprintln!("[backend-apple-core] async-executor task poll panicked — aborting");
            std::process::abort();
        }
    });
    // libdispatch needs a heap block: `.copy()` promotes the stack block and
    // refcounts it via `_Block_copy`.
    let block = block.copy();
    let block_ptr: *const std::ffi::c_void = &*block as *const _ as *const std::ffi::c_void;
    // SAFETY: `dispatch_async` to the main queue with a heap-copied block is
    // callable from any thread; libdispatch retains the block until it drains.
    unsafe {
        dispatch_async(
            &_dispatch_main_q as *const _ as *const std::ffi::c_void,
            block_ptr,
        );
    }
    // libdispatch owns the heap block until it fires; forget our handle so it
    // isn't released early (same ownership handoff as `dispatch_main_async`).
    std::mem::forget(block);
}
