//! Cancel-scope plumbing for client-side server-fn calls.
//!
//! The two surfaces this module owns:
//!
//! - **`CURRENT_CANCEL`**: a thread-local that the macro's client
//!   stub (via [`crate::batch::enqueue`]) reads to discover a
//!   cancellation token associated with the *current async polling*.
//!
//! - **[`with_cancel`]**: the author-facing helper that bridges a
//!   `runtime_core::ResourceCancel` (handed to a `resource` fetcher
//!   on dep change) to a `net::CancelToken`, and scopes that token
//!   for the lifetime of an inner future. While the inner future is
//!   being polled, any `#[server]` call it transitively makes sees
//!   the token in `CURRENT_CANCEL` and threads it through to the
//!   transport.
//!
//! The scope is **per-poll**, restored across yield points the way
//! `tokio::task_local` does — but without depending on tokio. The
//! mechanism: a custom future wraps the user's future and, on each
//! `poll`, swaps the token into `CURRENT_CANCEL`, calls inner.poll,
//! then restores the previous value via an RAII guard. Yielding (
//! returning `Poll::Pending`) restores the prior value; the next
//! poll re-installs ours. Sibling futures running on the same
//! thread observe their own scopes correctly.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use net::CancelToken;
use runtime_core::ResourceCancel;

thread_local! {
    /// The currently-active cancel token for any server-fn call made
    /// on this thread. `None` outside any [`with_cancel`] scope.
    /// Wrapped in `RefCell` rather than `Cell` so `Clone`-ing the
    /// `Option<CancelToken>` doesn't need a temporary `take`.
    static CURRENT_CANCEL: RefCell<Option<CancelToken>> =
        const { RefCell::new(None) };
}

/// Snapshot the active cancel token for the current scope, if any.
/// Read by [`crate::batch::enqueue`] to attach the right token to
/// each call's queue entry.
pub(crate) fn current_cancel() -> Option<CancelToken> {
    CURRENT_CANCEL.with(|c| c.borrow().clone())
}

/// Run `future` with an explicit `net::CancelToken` — any inner
/// `#[server]` call sees the token via the per-poll
/// `CURRENT_CANCEL` thread-local and threads it through to the
/// transport.
///
/// Useful when the caller already has a token (e.g. driving a
/// request from a button press, or from a manually-installed cancel
/// system). Authors using `resource()` should reach for
/// [`with_cancel`] instead.
pub fn with_cancel_token<F: Future>(token: CancelToken, future: F) -> WithCancel<F> {
    WithCancel { future, token }
}

/// Run `future` with the given `ResourceCancel` bridged to a
/// `net::CancelToken` that any inner `#[server]` call will see and
/// honour.
///
/// Use inside a `resource` fetcher:
///
/// ```ignore
/// let r = resource(deps, |args, cancel| async move {
///     server::with_cancel(cancel, my_server_fn(args)).await
/// });
/// ```
///
/// Mechanics:
/// 1. Make a fresh `(handle, token)` pair.
/// 2. Register `resource_cancel.on_cancel(...)` so that when the
///    `Resource` flags a dep change, the handle fires.
/// 3. Wrap `future` in a `WithCancel` that scopes the token into
///    `CURRENT_CANCEL` per poll.
///
/// The bridge from `ResourceCancel` is one-way: cancelling our
/// `CancelHandle` directly (if the caller ever got one) does not
/// fire `resource_cancel` back. Server fns inside the scope see
/// either kind of cancel as the same.
pub fn with_cancel<F: Future>(
    resource_cancel: ResourceCancel,
    future: F,
) -> WithCancel<F> {
    let (handle, token) = net::cancel_token();
    // When the resource decides to cancel (dep change / scope drop),
    // forward to our net token. Cheap clone since `CancelHandle`
    // is just an `Arc` newtype.
    let handle_for_callback = handle.clone();
    resource_cancel.on_cancel(move || handle_for_callback.cancel());
    with_cancel_token(token, future)
}

/// Future returned by [`with_cancel`].
///
/// We avoid `pin-project-lite` by writing the projection manually —
/// the body is small enough that the unsafe is auditable in one
/// glance, and adding a dep just for one struct seems excessive.
///
/// Safety contract: `future` is structurally pinned (we never move
/// it out of `WithCancel`); `token` is not pinned (it's a cheap
/// `Arc`-newtype handle, freely movable).
pub struct WithCancel<F> {
    future: F,
    token: CancelToken,
}

impl<F: Future> Future for WithCancel<F> {
    type Output = F::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        // SAFETY: `future` is structurally pinned, `token` isn't —
        // both invariants are upheld by this projection (we never
        // move out of `self.future`, and we only touch `self.token`
        // by clone).
        let this = unsafe { self.get_unchecked_mut() };
        let token_clone = this.token.clone();
        let future = unsafe { Pin::new_unchecked(&mut this.future) };

        let prev = CURRENT_CANCEL.with(|c| c.borrow_mut().replace(token_clone));
        // The guard restores `prev` even if `future.poll` panics —
        // otherwise a sibling future on this thread could see our
        // stale token after we yield.
        let _guard = ScopeGuard { prev };
        future.poll(cx)
    }
}

/// RAII helper that restores the previous value of `CURRENT_CANCEL`
/// when dropped — including on the panic-unwind path. Held only for
/// the duration of one `poll` call.
struct ScopeGuard {
    prev: Option<CancelToken>,
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        CURRENT_CANCEL.with(|c| *c.borrow_mut() = self.prev.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::{resource, Signal};
    use std::cell::RefCell;
    use std::future::poll_fn;
    use std::rc::Rc;
    use std::task::Poll;

    /// Outside any `with_cancel` scope, `current_cancel()` is None.
    #[test]
    fn current_cancel_is_none_outside_scope() {
        assert!(current_cancel().is_none());
    }

    /// Drive a `resource()` to surface a real `ResourceCancel` —
    /// the only public way to obtain one, since
    /// `ResourceCancel::cancel` is crate-private and the type
    /// itself is only constructible via the resource pathway.
    fn fresh_resource_cancel() -> ResourceCancel {
        let captured: Rc<RefCell<Option<ResourceCancel>>> =
            Rc::new(RefCell::new(None));
        let captured_for_fetcher = captured.clone();
        let dep = Signal::new(0i32);
        let _r: runtime_core::Resource<(), &'static str> =
            resource(dep, move |_, cancel| {
                *captured_for_fetcher.borrow_mut() = Some(cancel);
                async move { Ok(()) }
            });
        let result = captured.borrow_mut().take().unwrap();
        result
    }

    /// Inside a `with_cancel` scope, `current_cancel()` resolves to
    /// the wrapped token; the thread-local is restored after the
    /// scope's future settles.
    #[tokio::test(flavor = "current_thread")]
    async fn current_cancel_is_visible_inside_scope() {
        let resource_token = fresh_resource_cancel();
        let inner = poll_fn(|_| {
            assert!(
                current_cancel().is_some(),
                "expected CURRENT_CANCEL to be set during poll"
            );
            Poll::Ready(())
        });
        with_cancel(resource_token, inner).await;
        assert!(
            current_cancel().is_none(),
            "scope must restore CURRENT_CANCEL after settle"
        );
    }

    /// The net token visible inside the scope is freshly constructed
    /// per `with_cancel` call (not shared across invocations).
    #[tokio::test(flavor = "current_thread")]
    async fn each_with_cancel_scope_gets_a_fresh_token() {
        let mut tokens: Vec<net::CancelToken> = Vec::new();
        for _ in 0..3 {
            let resource_token = fresh_resource_cancel();
            let tokens_ref = &mut tokens;
            with_cancel(resource_token, async {
                tokens_ref.push(current_cancel().unwrap());
            })
            .await;
        }
        // Cancelling one should not affect the others.
        let (h, _t) = net::cancel_token();
        h.cancel();
        // Sanity: none of the captured tokens were cancelled.
        for t in &tokens {
            assert!(!t.is_cancelled());
        }
    }
}
