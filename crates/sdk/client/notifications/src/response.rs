//! The response inbox: how a tap, an action button, a reply or a dismissal
//! travels from the platform callback to the app's [`on_response`]
//! handlers.
//!
//! Platform callbacks arrive on whatever thread the OS picks (a
//! UNUserNotificationCenter delegate queue, a WinRT thread-pool thread, the
//! Linux D-Bus watcher thread, the Android main thread, the JS event loop),
//! and some arrive before any app code has subscribed — the tap that
//! cold-started the app is delivered while the app is still booting. So
//! [`deliver`] only queues: the response goes into a process-wide queue and
//! the UI-thread pump is woken. The pump runs the handlers on the UI thread
//! (where they may write signals), and holds the queue until the first
//! handler is registered, so the launch tap reaches a handler subscribed at
//! boot instead of being lost.
//!
//! The pump is a task on the host's async executor waiting on a channel
//! (the same bridge the `connectivity` SDK uses). Without an executor (a
//! bare test process) a task that waits on a channel would block its
//! caller forever, so the queue is drained synchronously instead, to the
//! handlers of whichever thread delivers or subscribes.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use crate::NotificationResponse;

type Handler = Rc<dyn Fn(&NotificationResponse)>;

/// Responses delivered but not yet handed to a handler, oldest first.
static PENDING: Mutex<VecDeque<NotificationResponse>> = Mutex::new(VecDeque::new());

/// The UI-thread pump's wake channel. Set once by [`start_pump`]; `None`
/// inside when no async executor was installed, and [`wake`] drains on the
/// calling thread instead.
static PUMP: OnceLock<Option<async_channel::Sender<()>>> = OnceLock::new();

thread_local! {
    static HANDLERS: RefCell<Vec<(u64, Handler)>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
    /// Set while the handlers run, so a handler that delivers a response
    /// itself (a robot test, a handler re-posting) queues it for the next
    /// drain instead of re-entering the loop.
    static DRAINING: Cell<bool> = const { Cell::new(false) };
}

/// Queue `r` for the app's [`on_response`] handlers. Callable from any
/// thread; the handlers run later, on the UI thread.
pub(crate) fn deliver(r: NotificationResponse) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push_back(r);
    wake();
}

fn wake() {
    match PUMP.get() {
        Some(Some(tx)) => {
            let _ = tx.try_send(());
        }
        // No executor: the handlers live in this thread's list, if any.
        Some(None) => drain(),
        // Not started: `init` has not run, so nothing is subscribed yet.
        None => {}
    }
}

/// Start the UI-thread pump. Idempotent; call it on the UI thread, where
/// the executor polls the pump task.
pub(crate) fn start_pump() {
    if PUMP.get().is_some() {
        return;
    }
    let wake = if runtime_core::driver::async_executor_installed() {
        let (tx, rx) = async_channel::unbounded::<()>();
        runtime_core::driver::spawn_detached(async move {
            while rx.recv().await.is_ok() {
                drain();
            }
        });
        Some(tx)
    } else {
        None
    };
    let _ = PUMP.set(wake);
}

/// Hand every queued response to every handler, unless there are none yet
/// — then the queue waits for the first [`on_response`].
fn drain() {
    if DRAINING.with(Cell::get) {
        return;
    }
    DRAINING.with(|d| d.set(true));
    loop {
        let handlers: Vec<Handler> = HANDLERS.with(|h| h.borrow().iter().map(|(_, f)| f.clone()).collect());
        if handlers.is_empty() {
            break;
        }
        let next = PENDING.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
        let Some(r) = next else { break };
        runtime_vocabulary::robot_only! {
            crate::robot_controls::record_response(&r);
        }
        for h in &handlers {
            h(&r);
        }
    }
    DRAINING.with(|d| d.set(false));
}

/// Run `handler` for every response to this app's notifications — a tap on
/// the body ([`ResponseAction::Open`](crate::ResponseAction::Open)), an
/// action button, a reply, or a dismissal — on the UI thread.
///
/// Responses that arrive before the first handler is registered are kept
/// and handed to it, so a handler registered at boot receives the tap that
/// launched the app. Every registered handler sees every response, in
/// registration order. Dropping the returned [`ResponseSubscription`]
/// unregisters the handler.
///
/// Calls [`init`](crate::init) if the app has not: on iOS and macOS the
/// launch tap only reaches the app if that happened at boot, so register
/// the handler (or call `init`) before the first screen mounts.
pub fn on_response(handler: impl Fn(&NotificationResponse) + 'static) -> ResponseSubscription {
    crate::init();
    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    HANDLERS.with(|h| h.borrow_mut().push((id, Rc::new(handler))));
    // Hand over what queued before this handler existed — through the pump,
    // so with an executor the handler never runs inside the caller's frame.
    wake();
    ResponseSubscription { id }
}

/// The registration [`on_response`] returns. Dropping it unregisters the
/// handler.
#[must_use = "dropping the subscription immediately unregisters the handler"]
pub struct ResponseSubscription {
    id: u64,
}

impl Drop for ResponseSubscription {
    fn drop(&mut self) {
        let _ = HANDLERS.try_with(|h| h.borrow_mut().retain(|(id, _)| *id != self.id));
    }
}

#[cfg(test)]
pub(crate) fn pending_len() -> usize {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).len()
}

/// Serializes the tests that go through the process-wide queue (they run
/// on parallel threads, each with its own handlers) and empties it.
#[cfg(test)]
pub(crate) fn exclusive_queue() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).clear();
    guard
}
