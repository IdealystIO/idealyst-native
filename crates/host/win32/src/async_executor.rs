//! Cooperative UI-thread async executor for the Win32 host.
//!
//! Without an installed executor `runtime_core::driver::spawn_async` falls
//! back to `pollster::block_on`: it drives the future to completion ON THE
//! CALLING THREAD — the UI thread. Every async SDK call (a storage write, a
//! clipboard read, an HTTP request, an update check) would freeze the window
//! until it finished, and a future that waits for an event (a socket's next
//! message, a change delivered from another thread) would never return,
//! hanging the app for good.
//!
//! This executor polls each spawned future only up to its next `.await` and
//! returns. Its waker may fire from any thread: it queues the task id and
//! posts the scheduler's wake (`WM_IDEALYST_SCHED`, latched to one pending
//! message — see `scheduler::WakeLatch`), and the UI thread's drain re-polls
//! the ready tasks. So every continuation — the code that writes signals —
//! runs on the UI thread, which is what the reactive world requires.
//!
//! The same design as the Apple executor (`backend_apple_core::
//! async_executor`, libdispatch's main queue) and the web one
//! (`spawn_local` on the JS event loop); here the event loop is the Win32
//! message pump.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use runtime_core::driver::AsyncExecutor;

type Task = Pin<Box<dyn Future<Output = ()> + 'static>>;

thread_local! {
    /// Pending futures by id. UI thread only: `spawn` and the drain both run
    /// there, so the `!Send` futures never cross a thread; wakers carry only
    /// the id.
    static TASKS: RefCell<HashMap<u64, Task>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// Ids woken since the last drain, from any thread.
static READY: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Register the executor with `runtime-core` (first install wins).
pub(crate) fn install() {
    runtime_core::driver::install_async_executor(Box::new(Win32AsyncExecutor));
}

struct Win32AsyncExecutor;

impl AsyncExecutor for Win32AsyncExecutor {
    /// Called on the UI thread. Polls once now, so the future runs up to its
    /// first `.await` before `spawn` returns, but never to completion.
    fn spawn(&self, future: Task) {
        let id = NEXT_ID.with(|n| {
            let v = n.get();
            n.set(v.wrapping_add(1));
            v
        });
        TASKS.with(|t| t.borrow_mut().insert(id, future));
        poll_task(id);
    }
}

/// Re-poll every task woken since the last call. Run by the scheduler's
/// drain on the UI thread.
pub(crate) fn poll_ready() {
    let ready = std::mem::take(&mut *READY.lock().unwrap_or_else(|p| p.into_inner()));
    for id in ready {
        poll_task(id);
    }
}

/// Whether a woken task is waiting for [`poll_ready`].
#[cfg(test)]
pub(crate) fn has_ready() -> bool {
    !READY.lock().unwrap_or_else(|p| p.into_inner()).is_empty()
}

/// Poll task `id`: taken out of the map while it runs, so a wake that lands
/// mid-poll (or a duplicate id in the ready list) finds nothing and no-ops;
/// put back if still pending, dropped if done.
fn poll_task(id: u64) {
    let Some(mut task) = TASKS.with(|t| t.borrow_mut().remove(&id)) else {
        return;
    };
    let waker = Waker::from(Arc::new(TaskWaker { id }));
    if task.as_mut().poll(&mut Context::from_waker(&waker)).is_pending() {
        TASKS.with(|t| t.borrow_mut().insert(id, task));
    }
    // The poll ran author code up to its next `.await` (a continuation that
    // sets signals): fire the dispatch hook so the reactive flush commits
    // those writes, as after any other scheduler callback.
    backend_windows::dispatch_hook::fire_dispatch_hook();
}

/// Carries only the task id; `wake` is safe from any thread.
struct TaskWaker {
    id: u64,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        READY.lock().unwrap_or_else(|p| p.into_inner()).push(self.id);
        crate::scheduler::post_wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    /// A one-shot value handed over from another thread.
    #[derive(Default)]
    struct Slot {
        value: Option<u32>,
        waker: Option<Waker>,
    }

    struct Recv(Arc<Mutex<Slot>>);

    impl Future for Recv {
        type Output = u32;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
            let mut s = self.0.lock().unwrap();
            match s.value.take() {
                Some(v) => Poll::Ready(v),
                None => {
                    s.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
    }

    /// `spawn_async` used `pollster::block_on` on Windows (no executor was
    /// installed), so a future awaiting a value from another thread blocked
    /// the UI thread until that value arrived — the call returned only
    /// after the sender ran. Now it returns at the first `.await` and the
    /// continuation runs on this thread from the scheduler's drain.
    #[test]
    fn regression_spawn_async_does_not_block_the_ui_thread() {
        install();
        let slot = Arc::new(Mutex::new(Slot::default()));
        let sent = Arc::new(AtomicBool::new(false));
        let (s2, sent2) = (slot.clone(), sent.clone());
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let waker = {
                let mut s = s2.lock().unwrap();
                s.value = Some(7);
                s.waker.take()
            };
            sent2.store(true, Ordering::SeqCst);
            if let Some(w) = waker {
                w.wake();
            }
        });

        let got = std::rc::Rc::new(Cell::new(None));
        let g = got.clone();
        let ui_thread = std::thread::current().id();
        let ran_on = std::rc::Rc::new(Cell::new(None));
        let r = ran_on.clone();
        runtime_core::driver::spawn_async(async move {
            let v = Recv(slot).await;
            r.set(Some(std::thread::current().id()));
            g.set(Some(v));
        });
        assert!(!sent.load(Ordering::SeqCst), "spawn_async returned before the value was sent");
        assert_eq!(got.get(), None);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !has_ready() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        crate::scheduler::drain_due();
        assert_eq!(got.get(), Some(7), "the scheduler's drain ran the continuation");
        assert_eq!(ran_on.get(), Some(ui_thread), "on the UI thread");
        sender.join().unwrap();
    }
}
