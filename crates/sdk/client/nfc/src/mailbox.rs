//! The hand-off between a backend's tag-discovery callback and
//! [`NfcSession::next_tag`](crate::NfcSession::next_tag).
//!
//! Tags arrive on whatever thread the platform calls back on (a CoreNFC
//! delegate queue, an Android binder thread, a PC/SC poll thread, a browser
//! event). The backend [`push`](Mailbox::push)es each one; `next_tag` awaits
//! [`recv`](Mailbox::recv). A terminal error ([`close`](Mailbox::close)) is
//! sticky: once the user cancels or the session times out, every later
//! `recv` returns that error instead of waiting forever.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::NfcError;

struct Inner<T> {
    queue: VecDeque<T>,
    closed: Option<NfcError>,
    waker: Option<Waker>,
}

/// A queue of discovered tags plus a sticky terminal error.
pub(crate) struct Mailbox<T> {
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T> Clone for Mailbox<T> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<T> Mailbox<T> {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner { queue: VecDeque::new(), closed: None, waker: None })),
        }
    }

    /// Queue a discovered tag. Ignored once closed: a tag that races the
    /// session's end has no session left to be read through.
    pub(crate) fn push(&self, item: T) {
        let waker = {
            let mut inner = self.inner.lock().unwrap();
            if inner.closed.is_some() {
                return;
            }
            inner.queue.push_back(item);
            inner.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    /// End the session with `error`. The first close wins; queued tags are
    /// dropped, since the session they belong to is gone.
    pub(crate) fn close(&self, error: NfcError) {
        let waker = {
            let mut inner = self.inner.lock().unwrap();
            if inner.closed.is_some() {
                return;
            }
            inner.closed = Some(error);
            inner.queue.clear();
            inner.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    #[cfg(any(test, target_os = "ios"))]
    pub(crate) fn is_closed(&self) -> bool {
        self.inner.lock().unwrap().closed.is_some()
    }

    /// The next tag, or the terminal error.
    pub(crate) fn recv(&self) -> Recv<T> {
        Recv { inner: Arc::clone(&self.inner) }
    }
}

pub(crate) struct Recv<T> {
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T> Future for Recv<T> {
    type Output = Result<T, NfcError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(item) = inner.queue.pop_front() {
            return Poll::Ready(Ok(item));
        }
        if let Some(err) = &inner.closed {
            return Poll::Ready(Err(err.clone()));
        }
        inner.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    struct CountWake(AtomicUsize);
    impl Wake for CountWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn poll<T>(f: &mut Recv<T>, waker: &Waker) -> Poll<Result<T, NfcError>> {
        Pin::new(f).poll(&mut Context::from_waker(waker))
    }

    #[test]
    fn push_wakes_a_pending_receiver() {
        let counter = Arc::new(CountWake(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&counter));
        let mb = Mailbox::new();
        let mut rx = mb.recv();
        assert!(poll(&mut rx, &waker).is_pending());
        mb.push(7u8);
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        assert!(matches!(poll(&mut rx, &waker), Poll::Ready(Ok(7))));
    }

    #[test]
    fn close_is_sticky_and_drops_queued_tags() {
        let waker = Waker::from(Arc::new(CountWake(AtomicUsize::new(0))));
        let mb = Mailbox::new();
        mb.push(1u8);
        mb.close(NfcError::Cancelled);
        mb.close(NfcError::Timeout);
        mb.push(2u8);
        for _ in 0..2 {
            assert!(matches!(poll(&mut mb.recv(), &waker), Poll::Ready(Err(NfcError::Cancelled))));
        }
        assert!(mb.is_closed());
    }

    #[test]
    fn queued_tags_are_delivered_in_order() {
        let waker = Waker::from(Arc::new(CountWake(AtomicUsize::new(0))));
        let mb = Mailbox::new();
        mb.push(1u8);
        mb.push(2u8);
        assert!(matches!(poll(&mut mb.recv(), &waker), Poll::Ready(Ok(1))));
        assert!(matches!(poll(&mut mb.recv(), &waker), Poll::Ready(Ok(2))));
    }
}
