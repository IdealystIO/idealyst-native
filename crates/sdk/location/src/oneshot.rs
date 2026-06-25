//! A minimal single-value channel bridging a callback to an `async fn`.
//!
//! [`current`](crate::current)'s native paths answer through a completion
//! handler or a delegate method that fires on a later run-loop turn (the web
//! `getCurrentPosition` success/error closures, a `CLLocationManager`
//! delegate). We can't return their result synchronously, so we hand the
//! producer a [`Sender`] it calls once and `.await` the [`Receiver`].
//!
//! This is the smallest correct bridge: a shared `Mutex<Inner>` holding the
//! sent value (if any) and the task's [`Waker`]. No external async-channel
//! crate, no `mem::forget`. It's `Send` so the producer half can travel onto
//! the dispatch / delegate thread, and it works on the single-threaded wasm
//! run loop too (the value is simply set inline there).
//!
//! Allowed to be unused on backends that don't need it (the desktop stub) —
//! cfg gating compiles the module on every target but only web/apple use it.

#![allow(dead_code)]

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
    /// callback resolves (to the receiver's fallback) instead of hanging.
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
/// resolves to if the sender is dropped without sending (e.g. a callback that
/// never fires) — pick a safe default.
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
        // Sender went away without sending: mark closed and wake the receiver
        // so it resolves to its fallback rather than parking forever.
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

// `Receiver` holds an `Arc` (always `Unpin`) and `Option<T>`; with `T: Unpin`
// the whole future is `Unpin`, letting `poll` reach `&mut self` through the
// `Pin` to take the fallback out. Every value sent through this channel
// (`Result<Position, _>`) is `Unpin`, so the bound never bites a caller.
impl<T: Unpin> Future for Receiver<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut g = self.inner.lock().unwrap();
        if let Some(v) = g.value.take() {
            return Poll::Ready(v);
        }
        if g.sender_dropped {
            drop(g);
            // `fallback` is `Some` until first consumed; a Future is only
            // polled to completion once, so this take is sound.
            return Poll::Ready(
                self.fallback
                    .take()
                    .expect("receiver polled after completion"),
            );
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
        tx.send(42);
        assert_eq!(rx.await, 42);
    }

    #[tokio::test]
    async fn send_after_park_wakes() {
        let (tx, rx) = channel::<u8>(0);
        let handle = tokio::spawn(async move { rx.await });
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
