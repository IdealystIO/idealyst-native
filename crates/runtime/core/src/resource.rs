//! Async data as a first-class reactive primitive.
//!
//! [`resource`] is to async work what [`memo`](crate::memo) is to sync:
//! a reactive cell that re-runs its source closure when tracked deps
//! change, exposes the result (data, error, loading) as a signal, and
//! cleans up after itself on scope drop.
//!
//! Compared to a hand-rolled `Effect` that spawns an async task and
//! pokes a signal on completion, `resource` provides:
//!
//! - **Stale-result protection.** A sequence-number guard ensures only
//!   the most-recently-issued fetch's result is applied — even if an
//!   older fetch's IO completes after a newer one.
//! - **Explicit cancellation.** Fetchers receive a [`ResourceCancel`]
//!   token that fires on dep change or scope drop. Fetchers can poll
//!   it, or register `on_cancel` callbacks to abort underlying IO
//!   (e.g. wire it to a `web_sys::AbortController` on wasm).
//! - **Refetch on demand.** [`Resource::refetch`] re-runs the fetcher
//!   without changing deps, for pull-to-refresh / retry-after-error
//!   patterns.
//! - **Stale-data retention.** When a refetch starts, `data` keeps its
//!   previous value and only `error` clears + `loading` flips. The UI
//!   doesn't flash an empty state on every refetch.
//!
//! Gated behind the `async-driver` Cargo feature because it depends on
//! [`crate::driver::spawn_async`].

use crate::driver::spawn_async;
use crate::reactive::{on_cleanup, untrack, Effect, Signal, Trackable};
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;

// =============================================================================
// ResourceState
// =============================================================================

/// Snapshot of a [`Resource`]'s current state. All three fields can be
/// populated simultaneously:
///
/// - `data: Some, error: None, loading: false` — last fetch succeeded.
/// - `data: Some, error: None, loading: true`  — refetching after a
///   prior success (showing old data while the new request is in
///   flight).
/// - `data: None, error: None, loading: true`  — initial mount before
///   the first fetch resolves.
/// - `data: Some, error: Some, loading: false` — most recent fetch
///   failed but a prior fetch had succeeded; UI can choose to keep
///   showing the stale `data` alongside the error.
/// - `data: None, error: Some, loading: false` — fetch failed and
///   there's no prior successful payload.
#[derive(Clone, Debug)]
pub struct ResourceState<T, E> {
    pub data: Option<T>,
    pub error: Option<E>,
    pub loading: bool,
}

impl<T, E> Default for ResourceState<T, E> {
    fn default() -> Self {
        Self {
            data: None,
            error: None,
            loading: true,
        }
    }
}

// =============================================================================
// ResourceCancel
// =============================================================================

/// Cancellation token passed to a resource's fetcher.
///
/// Fired in two situations:
/// - The resource's deps changed and a fresh fetch is starting (the
///   previous fetch is cancelled).
/// - The resource's owning scope dropped (e.g. the component
///   unmounted).
///
/// Fetchers can poll [`is_cancelled`](Self::is_cancelled) at await
/// points, or register an [`on_cancel`](Self::on_cancel) callback to
/// abort an underlying IO source (e.g. invoke `AbortController::abort`
/// on wasm).
///
/// Cancellation is **advisory** — if a fetcher ignores the token and
/// completes anyway, the resource's stale-result guard ensures its
/// output is still discarded as long as a newer fetch has started.
/// Explicit cancellation only saves wall-clock + bandwidth.
#[derive(Clone)]
pub struct ResourceCancel {
    inner: Rc<ResourceCancelInner>,
}

struct ResourceCancelInner {
    cancelled: Cell<bool>,
    callbacks: RefCell<Vec<Box<dyn FnOnce()>>>,
}

impl ResourceCancel {
    pub(crate) fn new() -> Self {
        Self {
            inner: Rc::new(ResourceCancelInner {
                cancelled: Cell::new(false),
                callbacks: RefCell::new(Vec::new()),
            }),
        }
    }

    /// Has the token been cancelled? Poll this at await points inside
    /// fetchers that don't have native cancellation support.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.get()
    }

    /// Register a callback that runs once when the token is cancelled.
    /// If the token has already been cancelled, the callback fires
    /// immediately.
    ///
    /// Bridges the token to platform-specific abort APIs. Typical
    /// usage on wasm:
    ///
    /// ```ignore
    /// resource(query, |q, cancel| async move {
    ///     let ctrl = web_sys::AbortController::new().unwrap();
    ///     let signal = ctrl.signal();
    ///     cancel.on_cancel(move || ctrl.abort());
    ///     // ... pass `signal` to fetch's init options.
    /// });
    /// ```
    pub fn on_cancel<F: FnOnce() + 'static>(&self, f: F) {
        if self.inner.cancelled.get() {
            f();
            return;
        }
        self.inner.callbacks.borrow_mut().push(Box::new(f));
    }

    /// Fire the token: flip the cancelled flag and run all registered
    /// callbacks. Idempotent — subsequent calls are no-ops.
    pub(crate) fn cancel(&self) {
        if self.inner.cancelled.replace(true) {
            return;
        }
        let callbacks = std::mem::take(&mut *self.inner.callbacks.borrow_mut());
        for cb in callbacks {
            cb();
        }
    }
}

// =============================================================================
// Resource
// =============================================================================

/// Reactive container for an async-computed value.
///
/// Construct via [`resource`]. The returned `Resource` exposes accessors
/// (`data`, `error`, `loading`) that read the underlying state signal
/// — calling any of them from inside a reactive context subscribes
/// that context to state changes, the same way `Signal::get` does.
pub struct Resource<T, E> {
    state: Signal<ResourceState<T, E>>,
    refetch_counter: Signal<u64>,
}

impl<T, E> Copy for Resource<T, E> {}

impl<T, E> Clone for Resource<T, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Clone + 'static, E: Clone + 'static> Resource<T, E> {
    /// The last successful payload, or `None` if no fetch has succeeded
    /// yet (or the resource hasn't held data since the last failure).
    ///
    /// Note: during a refetch, `data` retains its previous value so
    /// the UI doesn't flash empty. Use [`loading`](Self::loading) if
    /// you need to know whether a fetch is in flight.
    pub fn data(&self) -> Option<T> {
        self.state.get().data
    }

    /// The most recent fetch's error, or `None` if the last attempt
    /// succeeded or none has run yet.
    ///
    /// Note: `error` is cleared when a new fetch starts. If you need
    /// "show error until next success" semantics, snapshot the error
    /// yourself in an effect.
    pub fn error(&self) -> Option<E> {
        self.state.get().error
    }

    /// Whether a fetch is currently in flight.
    pub fn loading(&self) -> bool {
        self.state.get().loading
    }

    /// Single-read snapshot of the full state.
    pub fn state(&self) -> ResourceState<T, E> {
        self.state.get()
    }

    /// Collapsed [`NetworkState`](crate::NetworkState) view of the
    /// current state, suitable for direct `match` against in UI code.
    /// Precedence: `Loading > Error > Success > Idle`; refetch-while-
    /// stale collapses to `Loading`. Read the underlying [`Resource::state`]
    /// directly for richer cases.
    pub fn network_state(&self) -> crate::NetworkState<T, E> {
        (&self.state.get()).into()
    }

    /// Re-run the fetcher with the current deps. Useful for pull-to-
    /// refresh, retry-after-error, and external-event-driven
    /// invalidation. Triggers the same cancel-previous + spawn-fresh
    /// path that a dep change does.
    pub fn refetch(&self) {
        let next = untrack(|| self.refetch_counter.get()).wrapping_add(1);
        self.refetch_counter.set(next);
    }
}

// =============================================================================
// resource() — public constructor
// =============================================================================

/// Create a reactive resource.
///
/// - `deps` is a [`Trackable`] dependency set — a single `Signal<T>` or
///   a tuple of trackables, just like [`on`](crate::on). Changes to any
///   tracked input cancel the in-flight fetch and start a fresh one.
/// - `fetcher` is `Fn(D::Value, ResourceCancel) -> Fut` — invoked
///   eagerly on construction and re-invoked on every dep change /
///   refetch. The `ResourceCancel` token fires on dep change, refetch,
///   or scope drop.
///
/// Returns a [`Resource`] handle that's `Copy` — pass it freely to
/// child components.
///
/// ```ignore
/// let query = signal!(String::new());
///
/// let results = resource(query, |q, cancel| async move {
///     if q.is_empty() {
///         return Ok(Vec::<Item>::new());
///     }
///     // On wasm, bridge cancellation to AbortController.
///     let ctrl = AbortController::new().unwrap();
///     let signal = ctrl.signal();
///     cancel.on_cancel(move || ctrl.abort());
///
///     fetch_with_signal(&format!("/api/q={q}"), signal).await
/// });
///
/// // Read reactively:
/// when(|| results.loading(),
///     || text("Searching..."),
///     || text(move || format!("{} hits", results.data().map_or(0, |xs| xs.len()))),
/// )
/// ```
pub fn resource<D, T, E, Fut, F>(deps: D, fetcher: F) -> Resource<T, E>
where
    D: Trackable + 'static,
    D::Value: 'static,
    T: Clone + 'static,
    E: Clone + 'static,
    Fut: Future<Output = Result<T, E>> + 'static,
    F: Fn(D::Value, ResourceCancel) -> Fut + 'static,
{
    let state: Signal<ResourceState<T, E>> = Signal::new(ResourceState::default());
    let refetch_counter = Signal::new(0u64);

    // Shared mutable bookkeeping across effect re-runs.
    let sequence = Rc::new(Cell::new(0u64));
    let active_cancel: Rc<RefCell<Option<ResourceCancel>>> = Rc::new(RefCell::new(None));
    let fetcher = Rc::new(fetcher);

    let sequence_for_effect = sequence.clone();
    let active_cancel_for_effect = active_cancel.clone();
    let fetcher_for_effect = fetcher.clone();

    let e = Effect::new(move || {
        // Subscribe to deps + refetch trigger.
        let inputs = deps.track();
        let _ = refetch_counter.get();

        // Cancel any previously-active fetch. Its in-flight result (if
        // any arrives later) is also guarded out by the sequence check
        // below, so cancellation here is an optimization, not a
        // correctness requirement.
        if let Some(prev) = active_cancel_for_effect.borrow_mut().take() {
            prev.cancel();
        }

        // Issue a fresh sequence number + cancel token for this fetch.
        let my_seq = sequence_for_effect.get().wrapping_add(1);
        sequence_for_effect.set(my_seq);
        let cancel = ResourceCancel::new();
        *active_cancel_for_effect.borrow_mut() = Some(cancel.clone());

        // Mark loading; clear stale error; KEEP previous data so the
        // UI doesn't flash empty on refetch.
        state.update(|s| {
            s.loading = true;
            s.error = None;
        });

        // Cleanup on next re-run or effect drop. Fires the same cancel
        // that the next re-run would fire, so the in-flight fetch
        // sees the token before its await resumes.
        let cancel_for_cleanup = cancel.clone();
        on_cleanup(move || cancel_for_cleanup.cancel());

        // Spawn the fetch. `spawn_async` is the framework's
        // platform-agnostic executor entry point; on native without
        // an installed executor it falls back to a `pollster::block_on`
        // (synchronous), which is fine for tests + simple native
        // hosts. On wasm32 it requires `backend_web::install_async_executor()`.
        let fut = fetcher_for_effect(inputs, cancel.clone());
        let sequence_for_spawn = sequence_for_effect.clone();
        let cancel_for_spawn = cancel.clone();
        let state_for_spawn = state;

        spawn_async(async move {
            let result = fut.await;

            // Stale-result guard: only the most-recently-issued fetch
            // wins. Any older fetch whose IO completes after a newer
            // one was started has its result discarded here.
            if sequence_for_spawn.get() != my_seq {
                return;
            }
            // Cancellation guard. Belt-and-suspenders with the
            // sequence check — if the scope dropped between the
            // fetcher issuing IO and its result arriving, we should
            // not poke a dropped Signal's slot.
            if cancel_for_spawn.is_cancelled() {
                return;
            }

            state_for_spawn.update(|s| {
                s.loading = false;
                match result {
                    Ok(d) => {
                        s.data = Some(d);
                        s.error = None;
                    }
                    Err(e) => {
                        s.error = Some(e);
                    }
                }
            });
        });
    });

    // Match `memo`'s pattern: the active scope (if any) adopts the
    // effect's slot (`owns: false`, drop is no-op). Outside any scope
    // the handle's drop would free the slot — `forget` keeps the
    // resource alive for the lifetime of the thread, the way bare
    // `Signal::new` outside a scope persists.
    std::mem::forget(e);

    Resource {
        state,
        refetch_counter,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::task::Poll;

    /// A future that's ready immediately with a fixed value — useful
    /// when we want `spawn_async`'s native pollster fallback to
    /// complete synchronously inside the effect body.
    struct ImmediateOk<T>(Option<T>);
    impl<T: Unpin> Future for ImmediateOk<T> {
        type Output = Result<T, &'static str>;
        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> Poll<Self::Output> {
            Poll::Ready(Ok(self.0.take().unwrap()))
        }
    }

    /// A future that's ready immediately with an error.
    struct ImmediateErr<E>(Option<E>);
    impl<E: Unpin> Future for ImmediateErr<E> {
        type Output = Result<i32, E>;
        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> Poll<Self::Output> {
            Poll::Ready(Err(self.0.take().unwrap()))
        }
    }

    #[test]
    fn resource_initial_state_resolves_synchronously_via_pollster_fallback() {
        // On native without an installed async executor, `spawn_async`
        // falls back to `pollster::block_on`. That means a future
        // that's ready immediately resolves inside the effect body —
        // by the time `resource(...)` returns, `data` is populated.
        let key = Signal::new(1i32);
        let r = resource(key, |k, _cancel| ImmediateOk(Some(k * 10)));
        assert_eq!(r.data(), Some(10));
        assert_eq!(r.error(), None);
        assert!(!r.loading());
    }

    #[test]
    fn resource_dep_change_triggers_refetch() {
        let key = Signal::new(1i32);
        let r: Resource<i32, &'static str> =
            resource(key, |k, _cancel| ImmediateOk(Some(k * 10)));
        assert_eq!(r.data(), Some(10));

        key.set(7);
        // The dep change re-runs the effect, which spawns a fresh
        // fetch, which under pollster resolves synchronously.
        assert_eq!(r.data(), Some(70));
        assert!(!r.loading());
    }

    #[test]
    fn resource_error_path_populates_error_and_keeps_data() {
        // Sequence: succeed first, then a dep change triggers a fetch
        // that fails. `data` should retain the prior success;
        // `error` should hold the new failure.
        let key = Signal::new(1i32);
        let mode: Rc<RefCell<&'static str>> = Rc::new(RefCell::new("ok"));
        let mode_for_fetcher = mode.clone();
        let r: Resource<i32, &'static str> = resource(key, move |k, _cancel| {
            let m = *mode_for_fetcher.borrow();
            if m == "ok" {
                Box::pin(ImmediateOk(Some(k * 10))) as std::pin::Pin<Box<dyn Future<Output = _>>>
            } else {
                Box::pin(ImmediateErr(Some("boom")))
            }
        });
        assert_eq!(r.data(), Some(10));

        *mode.borrow_mut() = "err";
        key.set(2);
        assert_eq!(
            r.data(),
            Some(10),
            "data should be retained across a failed refetch"
        );
        assert_eq!(r.error(), Some("boom"));
        assert!(!r.loading());
    }

    #[test]
    fn resource_refetch_re_runs_fetcher_without_dep_change() {
        // Counter inside the fetcher proves it's invoked again on
        // refetch even though deps haven't changed.
        let key = Signal::new(0i32);
        let calls = Rc::new(Cell::new(0u32));
        let calls_for_fetcher = calls.clone();
        let r: Resource<u32, &'static str> = resource(key, move |_k, _cancel| {
            let n = calls_for_fetcher.get() + 1;
            calls_for_fetcher.set(n);
            ImmediateOk(Some(n))
        });
        assert_eq!(r.data(), Some(1));
        r.refetch();
        assert_eq!(r.data(), Some(2));
        r.refetch();
        assert_eq!(r.data(), Some(3));
    }

    #[test]
    fn resource_cancellation_token_fires_on_dep_change() {
        // The previous fetch's cancel token must be tripped when a
        // new fetch starts. We record the token and verify
        // `is_cancelled` flips after a dep change.
        let key = Signal::new(0i32);
        let last_token: Rc<RefCell<Option<ResourceCancel>>> = Rc::new(RefCell::new(None));
        let last_for_fetcher = last_token.clone();
        let _r: Resource<i32, &'static str> = resource(key, move |k, cancel| {
            *last_for_fetcher.borrow_mut() = Some(cancel);
            ImmediateOk(Some(k))
        });
        let first = last_token.borrow().clone().unwrap();
        assert!(!first.is_cancelled(), "fresh token is alive");

        key.set(1);
        assert!(
            first.is_cancelled(),
            "previous fetch's token should be cancelled on dep change"
        );
    }

    #[test]
    fn resource_on_cancel_callback_fires_with_cancellation() {
        let key = Signal::new(0i32);
        let cancel_count = Rc::new(Cell::new(0u32));
        let count_for_fetcher = cancel_count.clone();
        let _r: Resource<i32, &'static str> = resource(key, move |k, cancel| {
            let c = count_for_fetcher.clone();
            cancel.on_cancel(move || c.set(c.get() + 1));
            ImmediateOk(Some(k))
        });
        assert_eq!(cancel_count.get(), 0);
        key.set(1);
        assert_eq!(
            cancel_count.get(),
            1,
            "previous fetch's on_cancel callback should fire once"
        );
        key.set(2);
        assert_eq!(cancel_count.get(), 2);
    }

    #[test]
    fn resource_state_reads_subscribe_callers() {
        // A reactive consumer of `data()` should re-fire when the
        // resource's state changes.
        use std::cell::Cell;
        let key = Signal::new(1i32);
        let r: Resource<i32, &'static str> =
            resource(key, |k, _cancel| ImmediateOk(Some(k * 10)));
        let observed = Rc::new(Cell::new(0i32));
        let o = observed.clone();
        let _e = Effect::new(move || {
            if let Some(d) = r.data() {
                o.set(d);
            }
        });
        assert_eq!(observed.get(), 10);
        key.set(5);
        assert_eq!(observed.get(), 50);
    }

    #[test]
    fn resource_handle_is_copy() {
        // Compile-time sanity: the handle is `Copy` so it can be
        // freely captured into multiple child closures without
        // `.clone()` boilerplate.
        let key = Signal::new(1i32);
        let r: Resource<i32, &'static str> =
            resource(key, |k, _cancel| ImmediateOk(Some(k)));
        let a = r;
        let b = r;
        assert_eq!(a.data(), Some(1));
        assert_eq!(b.data(), Some(1));
    }
}
