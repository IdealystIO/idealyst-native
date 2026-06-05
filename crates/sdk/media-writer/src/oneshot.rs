//! A tiny single-shot, single-threaded async signal — the web `stop()` future
//! awaits it for the `MediaRecorder` `onstop` DOM event.
//!
//! It MUST be waker-based, not a self-rescheduling poll. An earlier version was
//! backed by `std::sync::mpsc` and, when empty, did
//! `cx.waker().wake_by_ref(); Poll::Pending` — re-waking the task on every poll.
//! On the single-threaded wasm executor that's an infinite microtask spin: the
//! task reschedules itself before the browser ever yields to the event loop, so
//! the `onstop` DOM event (which calls [`OneshotTx::send`]) is never dispatched,
//! the receiver never completes, and the TAB FREEZES on stop (the whiteboard
//! "stop screen recording freezes on web" bug).
//!
//! The fix below PARKS the task: `poll` stashes the waker and returns `Pending`
//! without re-waking; `send` (and `Drop`, as a safety net) flips `done` and
//! wakes the stored waker exactly once. `Rc<RefCell<…>>` is intentional — the
//! only consumer is the wasm32 web backend, which is single-threaded.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::Waker;

struct OneshotState {
    done: bool,
    waker: Option<Waker>,
}

/// Construct a connected `(sender, receiver)` pair.
pub(crate) fn futures_oneshot() -> (OneshotTx, OneshotRx) {
    let state = Rc::new(RefCell::new(OneshotState { done: false, waker: None }));
    (OneshotTx(state.clone()), OneshotRx(state))
}

pub(crate) struct OneshotTx(Rc<RefCell<OneshotState>>);

impl OneshotTx {
    /// Complete the receiver and wake any parked waiter (exactly once — `Drop`
    /// sees `done` already set and no-ops).
    pub(crate) fn send(self, _: ()) -> Result<(), ()> {
        let mut s = self.0.borrow_mut();
        s.done = true;
        if let Some(w) = s.waker.take() {
            w.wake();
        }
        Ok(())
    }
}

impl Drop for OneshotTx {
    fn drop(&mut self) {
        // Dropped without an explicit `send` (the recorder/closure was torn down
        // before `onstop`): still release the waiter so `stop()` resolves rather
        // than parking forever — mirrors a channel's Disconnected → Ready.
        let mut s = self.0.borrow_mut();
        if !s.done {
            s.done = true;
            if let Some(w) = s.waker.take() {
                w.wake();
            }
        }
    }
}

pub(crate) struct OneshotRx(Rc<RefCell<OneshotState>>);

impl std::future::Future for OneshotRx {
    type Output = ();
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        let mut s = self.0.borrow_mut();
        if s.done {
            std::task::Poll::Ready(())
        } else {
            // Register (replace) the waker; DON'T re-wake — wait for `send`.
            s.waker = Some(cx.waker().clone());
            std::task::Poll::Pending
        }
    }
}

// ---------------------------------------------------------------------------
// Thread-safe, payload-carrying single-shot — native encoder thread → main
// executor.
//
// The apple/android encoder runs on its own `std::thread`; `RecordingHandle::
// stop` awaits the finalize result on the main (single-threaded) executor. The
// Rx half MUST be AWAITED (parked), never blocked-on with a synchronous
// `recv()`: a blocking recv on the main thread freezes the run loop, so
// AVFoundation's `finishWritingWithCompletionHandler:` completion — dispatched
// onto a queue that needs the run loop pumping — never fires. The encoder then
// waits its full finalize timeout while the UI is frozen: the macOS "stop
// freezes the whole app" bug, the native twin of the wasm busy-waker freeze
// above. Awaiting `SyncRx` parks the task so the run loop keeps pumping.
//
// Same park-don't-spin discipline as the single-thread oneshot, but `Arc<Mutex>`
// so the `Send` Tx can cross to the worker thread, and it carries a `T` payload
// (the `Result` the encoder reports). `Output = Option<T>`: `None` if the sender
// dropped without sending (worker panicked / vanished), so `stop()` resolves to
// an error rather than hanging.
// ---------------------------------------------------------------------------

struct SyncShared<T> {
    value: Option<T>,
    closed: bool, // sender sent, or dropped without sending
    waker: Option<Waker>,
}

/// Construct a connected thread-safe `(sender, receiver)` pair carrying a `T`.
pub(crate) fn sync_oneshot<T>() -> (SyncTx<T>, SyncRx<T>) {
    let shared = Arc::new(Mutex::new(SyncShared { value: None, closed: false, waker: None }));
    (SyncTx(shared.clone()), SyncRx(shared))
}

pub(crate) struct SyncTx<T>(Arc<Mutex<SyncShared<T>>>);

impl<T> SyncTx<T> {
    /// Deliver `value` and wake the parked receiver exactly once.
    pub(crate) fn send(self, value: T) {
        let mut s = self.0.lock().unwrap();
        s.value = Some(value);
        s.closed = true;
        if let Some(w) = s.waker.take() {
            w.wake();
        }
        // `self` drops here; `Drop` sees `closed` set and no-ops.
    }
}

impl<T> Drop for SyncTx<T> {
    fn drop(&mut self) {
        // Dropped without `send`: release the waiter with no value so the
        // receiver resolves to `None` rather than parking forever — mirrors a
        // channel's Disconnected → Ready.
        let mut s = self.0.lock().unwrap();
        if !s.closed {
            s.closed = true;
            if let Some(w) = s.waker.take() {
                w.wake();
            }
        }
    }
}

pub(crate) struct SyncRx<T>(Arc<Mutex<SyncShared<T>>>);

impl<T> std::future::Future for SyncRx<T> {
    type Output = Option<T>;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<T>> {
        let mut s = self.0.lock().unwrap();
        if s.closed {
            std::task::Poll::Ready(s.value.take())
        } else {
            // Register (replace) the waker; DON'T re-wake — wait for `send`/drop.
            s.waker = Some(cx.waker().clone());
            std::task::Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    /// A waker that just counts how many times it was woken, so a test can prove
    /// a pending poll does NOT re-wake itself (the spin-loop bug).
    struct CountWaker(AtomicUsize);
    impl Wake for CountWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn counting_cx() -> (Arc<CountWaker>, Waker) {
        let arc = Arc::new(CountWaker(AtomicUsize::new(0)));
        let waker = Waker::from(arc.clone());
        (arc, waker)
    }

    /// THE REGRESSION: a poll on an unsent receiver must PARK (Pending) WITHOUT
    /// waking itself. The old mpsc version called `wake_by_ref()` here, which
    /// spun the executor and starved the event loop → tab freeze on stop.
    #[test]
    fn pending_poll_does_not_self_wake() {
        let (_tx, rx) = futures_oneshot();
        let (counter, waker) = counting_cx();
        let mut cx = Context::from_waker(&waker);
        let mut rx = Box::pin(rx);

        assert_eq!(rx.as_mut().poll(&mut cx), Poll::Pending);
        // No self-wake: the task is genuinely parked, so the event loop can run.
        assert_eq!(counter.0.load(Ordering::SeqCst), 0, "pending poll must not wake");
        // Polling again (still unsent) likewise must not wake.
        assert_eq!(rx.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(counter.0.load(Ordering::SeqCst), 0);
    }

    /// `send` wakes the parked waiter exactly once and the next poll is Ready.
    #[test]
    fn send_wakes_then_ready() {
        let (tx, rx) = futures_oneshot();
        let (counter, waker) = counting_cx();
        let mut cx = Context::from_waker(&waker);
        let mut rx = Box::pin(rx);

        assert_eq!(rx.as_mut().poll(&mut cx), Poll::Pending);
        tx.send(()).unwrap();
        assert_eq!(counter.0.load(Ordering::SeqCst), 1, "send wakes the waiter once");
        assert_eq!(rx.as_mut().poll(&mut cx), Poll::Ready(()));
    }

    /// Dropping the sender without `send` still releases the waiter (so `stop()`
    /// can't hang if the recorder is torn down before `onstop`).
    #[test]
    fn dropping_sender_releases_waiter() {
        let (tx, rx) = futures_oneshot();
        let (counter, waker) = counting_cx();
        let mut cx = Context::from_waker(&waker);
        let mut rx = Box::pin(rx);

        assert_eq!(rx.as_mut().poll(&mut cx), Poll::Pending);
        drop(tx);
        assert_eq!(counter.0.load(Ordering::SeqCst), 1, "drop wakes the waiter");
        assert_eq!(rx.as_mut().poll(&mut cx), Poll::Ready(()));
    }

    // -- Thread-safe `sync_oneshot` (encoder thread → main executor) ----------

    /// A minimal `block_on` that PARKS the thread between polls (via
    /// `thread::park`/`unpark`) — never spins. Models the real executor: a
    /// blocking `recv()` would freeze it, an awaited park lets it resume when
    /// the worker thread signals.
    fn park_block_on<F: Future>(fut: F) -> F::Output {
        struct ThreadWaker(std::thread::Thread);
        impl Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut fut = Box::pin(fut);
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(v) => return v,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    /// `SyncTx` must be `Send` so it can move to the encoder thread (the whole
    /// point of the thread-safe variant). Compile-time proof.
    #[test]
    fn sync_tx_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<SyncTx<Result<(), String>>>();
    }

    /// THE REGRESSION (native twin of the wasm freeze): the receiver, awaited on
    /// the main thread, PARKS until a value arrives from ANOTHER thread, then
    /// resolves with the payload. A blocking `recv()` here is what froze macOS
    /// on stop — this proves the awaited path parks-and-resumes cross-thread.
    #[test]
    fn sync_cross_thread_send_delivers_payload() {
        let (tx, rx) = sync_oneshot::<u32>();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            tx.send(42);
        });
        assert_eq!(park_block_on(rx), Some(42));
        worker.join().unwrap();
    }

    /// A worker that drops the sender without sending (panicked / vanished)
    /// resolves the receiver to `None` — `stop()` reports an error instead of
    /// hanging the executor forever.
    #[test]
    fn sync_dropped_sender_resolves_none() {
        let (tx, rx) = sync_oneshot::<u32>();
        std::thread::spawn(move || drop(tx)).join().unwrap();
        assert_eq!(park_block_on(rx), None);
    }
}
