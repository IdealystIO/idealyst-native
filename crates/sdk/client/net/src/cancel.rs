//! Cancellation primitives for in-flight requests.
//!
//! Crafted to be self-contained — no `tokio`, no `runtime-core`
//! dependency — so the `net` SDK stays independently usable. Bridges
//! to richer cancellation systems (e.g. `runtime_core::ResourceCancel`)
//! are one-liners on the consumer side: register
//! `move || handle.cancel()` as the system's cancel callback.
//!
//! # Shape
//!
//! `cancel_token()` returns a paired `(CancelHandle, CancelToken)`:
//!
//! - The **handle** is what holders of the cancel decision call. Cheap
//!   to clone and `Send + Sync` so it can be shipped across threads /
//!   stored on a parent component.
//! - The **token** is what gets attached to one or more in-flight
//!   requests via [`RequestBuilder::cancel_on`](crate::RequestBuilder::cancel_on).
//!
//! Calling `handle.cancel()` is idempotent and signals every token
//! sharing the same `Arc<Inner>`. The request transport observes the
//! signal via [`CancelToken::cancelled`], which returns a `Future`
//! that resolves once the token has fired.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// Internal state shared by a `CancelHandle` and every `CancelToken`
/// it was paired with. `Arc`-shared so cloning either side just bumps
/// a refcount.
///
/// `pub(crate)` (not `pub`) — the type is referenced by the public
/// `CancelHandle` / `CancelToken` field through the `Arc`, so it
/// needs to be at-least crate-visible to satisfy the
/// private-interfaces lint, but it's not part of the public API.
pub(crate) struct Inner {
    cancelled: AtomicBool,
    /// Wakers registered by polling `CancelToken::cancelled()` futures
    /// that haven't yet seen the cancel flag flip. Drained + woken on
    /// the first `cancel()` call.
    wakers: Mutex<Vec<Waker>>,
}

impl Inner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            cancelled: AtomicBool::new(false),
            wakers: Mutex::new(Vec::new()),
        })
    }
}

/// Construct a paired `(handle, token)`.
///
/// The handle is the cancel button; the token is what an in-flight
/// request listens on. Tokens can be cloned and attached to multiple
/// requests so a single handle aborts a whole fan-out.
pub fn cancel_token() -> (CancelHandle, CancelToken) {
    let inner = Inner::new();
    (
        CancelHandle {
            inner: inner.clone(),
        },
        CancelToken { inner },
    )
}

/// Sender side of a cancel signal. Calling [`Self::cancel`] fires
/// every paired token's listeners. Cheap to clone; can be stored
/// anywhere the cancel decision lives (a parent component, an
/// `on_cancel` callback on some other cancellation system, a button's
/// press handler).
#[derive(Clone)]
pub struct CancelHandle {
    inner: Arc<Inner>,
}

/// Pointer identity — a `CancelHandle` is the sender end of ONE cancel
/// signal, so clones of it are equal and handles from two separate
/// [`cancel_token`] calls never are.
///
/// Identity is the question a guarded `set` asks ("is this the same
/// instance?"), and it is the only one with an answer here: the payload is
/// an `AtomicBool` plus a waker list, and comparing those would make two
/// unrelated not-yet-fired handles look interchangeable — so replacing the
/// handle for request A with the one for request B would silently fail to
/// notify. `Signal<T>` is bounded on `T: PartialEq` at creation and `get`,
/// and an app parking the in-flight request's cancel button in state is the
/// motivating case; the orphan rule means only this crate can supply the
/// impl. Mirrors `MediaStream`.
impl PartialEq for CancelHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for CancelHandle {}

impl CancelHandle {
    /// Mark the paired tokens as cancelled and wake any task waiting
    /// on a `token.cancelled()` future. Idempotent — subsequent calls
    /// are no-ops.
    pub fn cancel(&self) {
        // `swap` returns the previous value; if it was already true
        // we've already done the wake pass.
        if self.inner.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        let wakers = std::mem::take(&mut *self.inner.wakers.lock().unwrap());
        for w in wakers {
            w.wake();
        }
    }

    /// True if [`Self::cancel`] has been called (on this handle or
    /// any clone).
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }
}

/// Receiver side of a cancel signal. Attach via
/// [`RequestBuilder::cancel_on`](crate::RequestBuilder::cancel_on) or
/// poll directly via [`Self::cancelled`].
#[derive(Clone)]
pub struct CancelToken {
    pub(crate) inner: Arc<Inner>,
}

/// Pointer identity, for the same reason as [`CancelHandle`] — the token
/// is the receiver end of the same shared `Inner`, is equally cloneable,
/// and is equally likely to be parked in app state next to the request it
/// guards. Note this compares the SIGNAL, so a token equals another token
/// paired with the same handle (they cancel together) — which is exactly
/// the grouping a subscriber cares about.
impl PartialEq for CancelToken {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for CancelToken {}

impl CancelToken {
    /// Snapshot — true once any paired handle has fired.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Future that resolves once cancellation fires. Resolving with
    /// `()` rather than an error keeps it composable in `select`
    /// patterns where the caller decides what an early resolution
    /// means.
    pub fn cancelled(&self) -> Cancelled {
        Cancelled {
            inner: self.inner.clone(),
            registered: false,
        }
    }
}

/// The future returned by [`CancelToken::cancelled`].
pub struct Cancelled {
    inner: Arc<Inner>,
    /// `false` until the future has pushed its waker into the shared
    /// list. Tracked to avoid pushing duplicate wakers on every poll.
    registered: bool,
}

impl Future for Cancelled {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // Fast path: already cancelled.
        if self.inner.cancelled.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        if !self.registered {
            self.inner
                .wakers
                .lock()
                .unwrap()
                .push(cx.waker().clone());
            self.registered = true;
            // Race-close: cancel() could have fired between the first
            // load and pushing the waker. Re-check; if it did fire,
            // resolve immediately (and our pushed waker is a no-op).
            if self.inner.cancelled.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::{RawWaker, RawWakerVTable, Waker};

    /// Minimal noop waker for sync poll tests.
    fn noop_waker() -> Waker {
        fn vtable() -> &'static RawWakerVTable {
            &RawWakerVTable::new(
                |_| RawWaker::new(std::ptr::null(), vtable()),
                |_| {},
                |_| {},
                |_| {},
            )
        }
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), vtable())) }
    }

    #[test]
    fn cancel_handle_flips_token_is_cancelled() {
        let (h, t) = cancel_token();
        assert!(!h.is_cancelled());
        assert!(!t.is_cancelled());
        h.cancel();
        assert!(h.is_cancelled());
        assert!(t.is_cancelled());
    }

    #[test]
    fn cancel_is_idempotent_across_multiple_calls() {
        let (h, t) = cancel_token();
        h.cancel();
        h.cancel(); // must not panic, must not double-wake
        assert!(t.is_cancelled());
    }

    #[test]
    fn cancelled_future_resolves_immediately_when_already_cancelled() {
        let (h, t) = cancel_token();
        h.cancel();
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut fut = Box::pin(t.cancelled());
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(()) => {}
            Poll::Pending => panic!("must resolve immediately when token already cancelled"),
        }
    }

    #[test]
    fn cancelled_future_pending_until_handle_fires() {
        let (h, t) = cancel_token();
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut fut = Box::pin(t.cancelled());
        assert!(matches!(fut.as_mut().poll(&mut cx), Poll::Pending));
        h.cancel();
        // Next poll observes the flip and resolves.
        assert!(matches!(fut.as_mut().poll(&mut cx), Poll::Ready(())));
    }

    #[test]
    fn token_clones_share_cancel_state() {
        let (h, t) = cancel_token();
        let t2 = t.clone();
        h.cancel();
        assert!(t.is_cancelled());
        assert!(t2.is_cancelled());
    }

    // -- PartialEq: identity, so the handle can live in a `Signal` -----------
    //
    // `Signal<T>` is bounded on `T: PartialEq`, and only THIS crate can
    // supply the impl (the orphan rule blocks an app crate). These pin the
    // semantics, not merely the existence.

    #[test]
    fn cancel_handle_clones_compare_equal() {
        let (h, _t) = cancel_token();
        assert!(h == h.clone(), "clones address the same cancel signal");
    }

    #[test]
    fn independently_created_cancel_handles_compare_unequal() {
        let (a, _) = cancel_token();
        let (b, _) = cancel_token();
        assert!(a != b, "two separate cancel signals are never the same handle");
        // …and stay unequal once both have fired, i.e. the compare is not
        // secretly reading the cancelled flag.
        a.cancel();
        b.cancel();
        assert!(a != b, "identity does not collapse once state matches");
    }

    #[test]
    fn cancel_token_clones_compare_equal_and_pairs_are_distinct() {
        let (_ha, ta) = cancel_token();
        let (_hb, tb) = cancel_token();
        assert!(ta == ta.clone());
        assert!(ta != tb);
    }
}
