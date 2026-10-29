//! Cooperative main-thread async executor for the GTK host.
//!
//! Without an installed executor `runtime_core::driver::spawn_async` falls
//! back to `pollster::block_on`: it drives the future to completion ON THE
//! CALLING THREAD — the GTK main thread. A short request merely stalls the
//! window until it returns; a future that never completes (a `use_socket` /
//! `use_sse` receive loop) owns the main thread for good, and the window
//! stops drawing and responding at 0% CPU with nothing logged.
//!
//! This executor polls each spawned future only up to its next `.await` and
//! returns. Its waker may fire from any thread (a socket's reader thread): it
//! posts a re-poll to the default GLib main context with `idle_add_once`, so
//! every continuation — the code that writes signals and touches widgets —
//! runs on the main thread, which is what the reactive world and GTK require.
//!
//! The same design as the Apple executor (`backend_apple_core::
//! async_executor`, libdispatch's main queue), Android's (the main looper)
//! and Win32's (the message pump); here the event loop is GLib's.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Wake, Waker};

use gtk4::glib;
use runtime_core::driver::AsyncExecutor;

type Task = Pin<Box<dyn Future<Output = ()> + 'static>>;

thread_local! {
    /// Pending futures by id. Main thread only: `spawn` and `poll_task` both
    /// run there, so the `!Send` futures never cross a thread; wakers carry
    /// only the id.
    static TASKS: RefCell<HashMap<u64, Task>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// Register the executor with `runtime-core` (first install wins). Called
/// from [`crate::scheduler::install`].
pub(crate) fn install() {
    runtime_core::driver::install_async_executor(Box::new(GtkAsyncExecutor));
}

struct GtkAsyncExecutor;

impl AsyncExecutor for GtkAsyncExecutor {
    /// Polls once now, so the future runs up to its first `.await` before
    /// `spawn` returns — but never to completion.
    fn spawn(&self, future: Task) {
        // The task table is the main thread's; a future spawned on any other
        // thread would be registered where no re-poll ever looks, and lost.
        // Off the main thread, keep the framework's blocking fallback.
        if !gtk4::is_initialized_main_thread() {
            pollster::block_on(future);
            return;
        }
        let id = NEXT_ID.with(|n| {
            let v = n.get();
            n.set(v.wrapping_add(1));
            v
        });
        TASKS.with(|t| t.borrow_mut().insert(id, future));
        poll_task(id);
    }
}

/// Poll the task with `id`: take it out, poll with a re-scheduling waker,
/// reinsert if still pending.
fn poll_task(id: u64) {
    // Taken OUT while polling, so a poll that spawns, or a wake that lands
    // while this one runs, never double-borrows the table or polls the same
    // future twice — a re-poll for an absent id is a no-op.
    let Some(mut fut) = TASKS.with(|t| t.borrow_mut().remove(&id)) else {
        return; // already completed
    };
    let waker = Waker::from(Arc::new(TaskWaker { id }));
    let mut cx = Context::from_waker(&waker);
    if fut.as_mut().poll(&mut cx).is_pending() {
        TASKS.with(|t| t.borrow_mut().insert(id, fut));
    }
    // Each poll ran author code up to its next `.await` (a server-call or
    // socket continuation that sets signals). GTK has no periodic flush, so
    // fire the post-dispatch hook to commit the staged writes — the web,
    // Apple and Win32 executors do the same per poll.
    backend_linux::dispatch_hook::fire_dispatch_hook();
}

/// Waker carrying only the task id; `wake` may fire from any thread.
struct TaskWaker {
    id: u64,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        post_poll(self.id);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        post_poll(self.id);
    }
}

/// Schedule `poll_task(id)` on the GTK main thread, from any thread.
///
/// `idle_add_once` (the `Send` variant) attaches to the default main context
/// and always defers. Not `MainContext::invoke`: on the owning thread that
/// runs the closure INLINE, so a future that wakes itself during its own poll
/// would re-enter `poll_task` while its slot is empty and lose the wake.
fn post_poll(id: u64) {
    glib::idle_add_once(move || poll_task(id));
}
