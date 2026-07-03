//! A minimal single-value channel bridging a callback to an `async fn`.
//!
//! The native permission APIs answer through a completion handler or a
//! delegate method that fires on a later run-loop turn (Apple's
//! `requestAuthorizationWithOptions:completion:`, a `CLLocationManager`
//! delegate, Android's `onRequestPermissionsResult`). We can't return their
//! result synchronously, so we hand the producer a [`Sender`] it calls once
//! and `.await` the [`Receiver`] on our side.
//!
//! This is the smallest correct bridge: a shared `Mutex<Inner>` holding the
//! sent value (if any) and the task's [`Waker`]. No external async-channel
//! crate, no `mem::forget`. It's `Send` so the producer half can travel onto
//! the dispatch/JNI thread that will fire it, and it works on the
//! single-threaded wasm run loop too (the value is simply set inline there).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

struct Inner<T> {
    /// `Some` once the producer has sent. The receiver takes it out.
    value: Option<T>,
    /// The awaiting task's waker, parked until a value lands.
    waker: Option<Waker>,
    /// Set when the [`Sender`] is dropped without sending — so a never-fired
    /// callback resolves (to the receiver's default) instead of hanging.
    sender_dropped: bool,
}

/// The send half. Calling [`send`](Sender::send) — at most once — delivers
/// the value and wakes the awaiting task. Dropping it un-sent closes the
/// channel so the receiver doesn't hang.
pub(crate) struct Sender<T> {
    inner: Arc<Mutex<Inner<T>>>,
    sent: bool,
}

/// The receive half. Awaiting it yields the sent value, or the supplied
/// fallback if the [`Sender`] was dropped without sending.
pub(crate) struct Receiver<T> {
    inner: Arc<Mutex<Inner<T>>>,
    /// Returned if the sender drops without sending.
    fallback: Option<T>,
}

/// Create a connected sender/receiver pair. `fallback` is what the receiver
/// resolves to if the sender is dropped without sending (e.g. a callback
/// that never fires) — pick a safe default like `Undetermined`.
pub(crate) fn channel<T>(fallback: T) -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Mutex::new(Inner {
        value: None,
        waker: None,
        sender_dropped: false,
    }));
    (
        Sender {
            inner: inner.clone(),
            sent: false,
        },
        Receiver {
            inner,
            fallback: Some(fallback),
        },
    )
}

impl<T> Sender<T> {
    /// Deliver `value` and wake the receiver. A no-op after the first call.
    pub(crate) fn send(mut self, value: T) {
        self.sent = true;
        let waker = {
            let mut g = self.inner.lock().unwrap();
            g.value = Some(value);
            g.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.sent {
            return;
        }
        // Sender went away without sending: mark closed and wake the
        // receiver so it resolves to its fallback rather than parking forever.
        let waker = {
            let mut g = self.inner.lock().unwrap();
            g.sender_dropped = true;
            g.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }
}

// The values bridged here (`PermissionStatus`, `bool`) are `Unpin`; the
// bound lets `poll` reach `&mut self` through the `Pin` without `unsafe`.
impl<T: Unpin> Future for Receiver<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        // `Receiver` holds only an `Arc` + `Option` — it's `Unpin`, so
        // moving out of the pin is sound.
        let this = self.get_mut();
        let mut g = this.inner.lock().unwrap();
        if let Some(v) = g.value.take() {
            return Poll::Ready(v);
        }
        if g.sender_dropped {
            drop(g);
            // `fallback` is `Some` until first consumed; a Future is only
            // polled to completion once, so this take is sound.
            return Poll::Ready(this.fallback.take().expect("receiver polled after completion"));
        }
        g.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delivers_sent_value() {
        let (tx, rx) = channel::<u8>(0);
        // Send from "another turn" before the await.
        tx.send(42);
        assert_eq!(rx.await, 42);
    }

    #[tokio::test]
    async fn send_after_park_wakes() {
        let (tx, rx) = channel::<u8>(0);
        let handle = tokio::spawn(async move { rx.await });
        // Give the task a turn to park on the receiver.
        tokio::task::yield_now().await;
        tx.send(7);
        assert_eq!(handle.await.unwrap(), 7);
    }

    #[tokio::test]
    async fn dropped_sender_resolves_to_fallback() {
        let (tx, rx) = channel::<u8>(99);
        drop(tx);
        assert_eq!(rx.await, 99);
    }
}
