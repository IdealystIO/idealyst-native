//! `async_reducer()` — async dual of the sync [`reducer()`](crate::reducer).
//!
//! Bridges a caller-owned `Signal<S>` to an async operation:
//!
//! ```ignore
//! let create = async_reducer(
//!     todos,                                                  // Signal<Vec<Todo>>
//!     |input: CreateTodo| async move { create_todo(input).await },
//!     |list, new_todo| list.push(new_todo),
//! );
//!
//! create.trigger(CreateTodo { title: "buy milk".into() });
//! ```
//!
//! On trigger:
//! 1. Bump a sequence counter, write `AsyncStatus::Loading` to the
//!    handle's `status` signal.
//! 2. Spawn `perform(input)` via [`crate::driver::spawn_async`].
//! 3. When the future settles, if this trigger's sequence is still
//!    the latest, fold the response into the caller's state via
//!    `state.update(|s| apply(s, response))` (on `Ok`) or flip the
//!    status to `Error(E)` (on `Err`).
//!
//! Where each piece lives:
//!
//! | Concern | Lives in |
//! |---|---|
//! | The data | `Signal<S>` (caller-owned) |
//! | The transition function | `apply: (&mut S, R) -> ()` |
//! | The async action | `perform: I -> Future<Result<R, E>>` |
//! | Operation lifecycle | `AsyncReducer.status: Signal<AsyncStatus<E>>` |
//!
//! Compared to [`crate::mutation`]: that primitive stores the last
//! response on the handle itself (`MutationState::data`). Most real
//! apps want the response folded into existing application state;
//! `async_reducer` is the cleaner shape for that case. `mutation`
//! still has a place — when the response is purely a notification
//! you don't store (analytics, telemetry pings) its self-contained
//! state is enough.
//!
//! Compared to the sync [`crate::reducer`]: same state-machine
//! shape, but the action is async + fallible.
//!
//! Gated behind the `async-driver` feature like its sync sibling.

use crate::driver::spawn_async;
use crate::reactive::Signal;
use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

// =============================================================================
// AsyncStatus
// =============================================================================

/// Operation lifecycle for an [`AsyncReducer`].
///
/// Notably **no `Success(T)` variant** — successful responses have
/// already been folded into the caller-owned `Signal<S>` by the
/// apply closure. The handle's job is to report what the *operation*
/// is doing, not what the data is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AsyncStatus<E> {
    /// No trigger has fired since construction or the last
    /// [`AsyncReducer::reset`].
    Idle,
    /// A trigger is in flight.
    Loading,
    /// The most recent settled trigger failed.
    Error(E),
}

impl<E> AsyncStatus<E> {
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }
    pub fn error(&self) -> Option<&E> {
        match self {
            Self::Error(e) => Some(e),
            _ => None,
        }
    }
}

impl<E> Default for AsyncStatus<E> {
    fn default() -> Self {
        Self::Idle
    }
}

// =============================================================================
// AsyncReducer
// =============================================================================

/// Handle for an [`async_reducer`]. `Clone` so it can be captured
/// into multiple event handlers; cheap (one `Rc` clone per call).
pub struct AsyncReducer<I, E> {
    status: Signal<AsyncStatus<E>>,
    /// Stale-result guard. Each trigger captures a number; only the
    /// latest applies its result to state. Out-of-order completions
    /// (a slow trigger settling after a fast one) are discarded.
    sequence: Rc<Cell<u64>>,
    /// All work — sequence bump, status flip, perform-and-apply —
    /// lives in this single closure. `trigger` invokes it and
    /// spawns; `run` awaits it inline. Holding only this closure
    /// (rather than the perform/apply/state pieces separately)
    /// type-erases `R` and `S` away from the public handle type.
    run_fn: Rc<dyn Fn(I) -> Pin<Box<dyn Future<Output = Result<(), E>>>>>,
}

impl<I, E> Clone for AsyncReducer<I, E> {
    fn clone(&self) -> Self {
        Self {
            status: self.status,
            sequence: self.sequence.clone(),
            run_fn: self.run_fn.clone(),
        }
    }
}

impl<I: 'static, E: Clone + 'static> AsyncReducer<I, E> {
    /// Fire the action and forget the future. Status flips to
    /// `Loading` immediately; the result (success applied to state,
    /// error stored in status) lands when the future settles.
    pub fn trigger(&self, input: I) {
        let fut = (self.run_fn)(input);
        spawn_async(async move {
            // Ignore the inline result — the side effects (state
            // update, status flip) already happened inside `run_fn`.
            let _ = fut.await;
        });
    }

    /// Fire the action and await the result inline. The state
    /// signal and `status` are updated the same way as
    /// [`Self::trigger`]; the `Result` here is convenience for
    /// callers that want to navigate / commit a follow-up only on
    /// success.
    pub async fn run(&self, input: I) -> Result<(), E> {
        (self.run_fn)(input).await
    }

    /// Invalidate any in-flight trigger and clear status to
    /// `Idle`. The state signal is untouched — `reset` is a
    /// status-only operation. Useful for "dismiss the error
    /// banner" UX.
    pub fn reset(&self) {
        self.sequence.set(self.sequence.get().wrapping_add(1));
        self.status.set(AsyncStatus::Idle);
    }

    /// The status signal. Subscribe via `.get()` from a reactive
    /// context to bind UI to loading / error transitions.
    pub fn status(&self) -> Signal<AsyncStatus<E>> {
        self.status
    }

    /// Snapshot the current status — convenience for non-reactive
    /// reads (event handlers, log lines).
    pub fn status_now(&self) -> AsyncStatus<E> {
        self.status.get()
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.status.get(), AsyncStatus::Loading)
    }

    pub fn error(&self) -> Option<E> {
        match self.status.get() {
            AsyncStatus::Error(e) => Some(e),
            _ => None,
        }
    }
}

// =============================================================================
// async_reducer() — public constructor
// =============================================================================

/// Construct an async reducer over a caller-owned `Signal<S>`.
///
/// See the module docs for the full design rationale.
pub fn async_reducer<S, I, R, E, F, Fut, A>(
    state: Signal<S>,
    perform: F,
    apply: A,
) -> AsyncReducer<I, E>
where
    // `Clone` on `S` comes from `Signal::update`'s impl bound — the
    // method body only needs `&mut T` but `Signal<T>`'s mutating
    // surface only exists in the `impl<T: Clone + 'static>` block.
    // Effectively no real cost: `S` is `Vec<Todo>`-shaped in
    // practice, and Vec/HashMap/etc. all impl `Clone`.
    S: Clone + 'static,
    I: 'static,
    R: 'static,
    E: Clone + 'static,
    F: Fn(I) -> Fut + 'static,
    Fut: Future<Output = Result<R, E>> + 'static,
    A: Fn(&mut S, R) + 'static,
{
    let status: Signal<AsyncStatus<E>> = Signal::new(AsyncStatus::Idle);
    let sequence = Rc::new(Cell::new(0u64));
    let perform = Rc::new(perform);
    let apply = Rc::new(apply);

    let run_fn: Rc<dyn Fn(I) -> Pin<Box<dyn Future<Output = Result<(), E>>>>> = {
        let sequence_outer = sequence.clone();
        let perform_outer = perform;
        let apply_outer = apply;
        Rc::new(move |input: I| {
            // Claim a sequence number BEFORE moving into the async
            // block — the sequence/state/status writes that mark
            // "this trigger is now the latest" must be observable
            // by the next synchronous trigger, even if the future
            // hasn't been polled yet.
            let my_seq = sequence_outer.get().wrapping_add(1);
            sequence_outer.set(my_seq);
            status.set(AsyncStatus::Loading);

            let fut = perform_outer(input);
            let sequence_for_task = sequence_outer.clone();
            let apply_for_task = apply_outer.clone();

            Box::pin(async move {
                let result = fut.await;

                // Stale-result guard. If we've been superseded the
                // newer trigger has already written the canonical
                // status; we must not touch it. We still return the
                // result to the inline caller — they asked, they
                // get it.
                if sequence_for_task.get() != my_seq {
                    return match result {
                        Ok(_) => Ok(()),
                        Err(e) => Err(e),
                    };
                }

                match result {
                    Ok(r) => {
                        state.update(|s| apply_for_task(s, r));
                        status.set(AsyncStatus::Idle);
                        Ok(())
                    }
                    Err(e) => {
                        let e_clone = e.clone();
                        status.set(AsyncStatus::Error(e));
                        Err(e_clone)
                    }
                }
            }) as Pin<Box<dyn Future<Output = Result<(), E>>>>
        })
    };

    AsyncReducer {
        status,
        sequence,
        run_fn,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive::Effect;
    use std::cell::RefCell;

    /// Future that's ready immediately with an `Ok` — under
    /// `spawn_async`'s native pollster fallback this resolves
    /// synchronously inside `trigger`, which keeps the tests
    /// non-async.
    struct ImmediateOk<T>(Option<T>);
    impl<T: Unpin> Future for ImmediateOk<T> {
        type Output = Result<T, &'static str>;
        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            std::task::Poll::Ready(Ok(self.0.take().unwrap()))
        }
    }

    struct ImmediateErr;
    impl Future for ImmediateErr {
        type Output = Result<i32, &'static str>;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            std::task::Poll::Ready(Err("boom"))
        }
    }

    #[test]
    fn async_reducer_initial_status_is_idle() {
        let state: Signal<Vec<i32>> = Signal::new(Vec::new());
        let r: AsyncReducer<i32, &'static str> =
            async_reducer(state, |x| ImmediateOk(Some(x)), |s, x| s.push(x));
        assert_eq!(r.status_now(), AsyncStatus::Idle);
    }

    #[test]
    fn async_reducer_trigger_applies_response_to_state() {
        let state: Signal<Vec<i32>> = Signal::new(Vec::new());
        let r: AsyncReducer<i32, &'static str> =
            async_reducer(state, |x| ImmediateOk(Some(x * 2)), |s, x| s.push(x));
        r.trigger(7);
        assert_eq!(state.get(), vec![14]);
        assert_eq!(r.status_now(), AsyncStatus::Idle);
    }

    #[test]
    fn async_reducer_trigger_failure_lands_in_status_state_untouched() {
        let state: Signal<Vec<i32>> = Signal::new(vec![1, 2, 3]);
        let r: AsyncReducer<i32, &'static str> =
            async_reducer(state, |_x| ImmediateErr, |s, x| s.push(x));
        r.trigger(99);
        assert_eq!(
            state.get(),
            vec![1, 2, 3],
            "state must not be touched on error"
        );
        assert_eq!(r.status_now(), AsyncStatus::Error("boom"));
    }

    #[test]
    fn async_reducer_reset_clears_status_but_not_state() {
        let state: Signal<Vec<i32>> = Signal::new(Vec::new());
        let r: AsyncReducer<i32, &'static str> =
            async_reducer(state, |_x| ImmediateErr, |s, x| s.push(x));
        r.trigger(0);
        assert!(matches!(r.status_now(), AsyncStatus::Error(_)));
        // Push something into state directly, then reset.
        state.update(|s| s.push(42));
        r.reset();
        assert_eq!(r.status_now(), AsyncStatus::Idle);
        assert_eq!(state.get(), vec![42], "state must survive reset");
    }

    #[test]
    fn async_reducer_run_returns_result_inline() {
        let state: Signal<Vec<i32>> = Signal::new(Vec::new());
        let r: AsyncReducer<i32, &'static str> =
            async_reducer(state, |x| ImmediateOk(Some(x)), |s, x| s.push(x));
        let result = pollster::block_on(r.run(5));
        assert_eq!(result, Ok(()));
        assert_eq!(state.get(), vec![5]);
    }

    #[test]
    fn async_reducer_status_reads_subscribe_callers() {
        let state: Signal<Vec<i32>> = Signal::new(Vec::new());
        let r: AsyncReducer<i32, &'static str> =
            async_reducer(state, |x| ImmediateOk(Some(x)), |s, x| s.push(x));
        let r_clone = r.clone();
        // `AsyncStatus<E>` isn't `Copy` (E might not be), so use
        // `RefCell` rather than `Cell` to thread observed values.
        let observed: Rc<RefCell<AsyncStatus<&'static str>>> =
            Rc::new(RefCell::new(AsyncStatus::Idle));
        let observed_for_effect = observed.clone();
        let _e = Effect::new(move || {
            *observed_for_effect.borrow_mut() = r_clone.status_now();
        });
        // Initial: observed should mirror Idle.
        assert_eq!(*observed.borrow(), AsyncStatus::Idle);
        r.trigger(1);
        // After settle: still Idle (success path flips back).
        assert_eq!(*observed.borrow(), AsyncStatus::Idle);
    }

    #[test]
    fn async_reducer_handle_is_clone_and_shares_state() {
        let state: Signal<Vec<i32>> = Signal::new(Vec::new());
        let r: AsyncReducer<i32, &'static str> =
            async_reducer(state, |x| ImmediateOk(Some(x)), |s, x| s.push(x));
        let a = r.clone();
        let b = r.clone();
        a.trigger(1);
        b.trigger(2);
        // Both triggers feed the same state signal.
        assert_eq!(state.get(), vec![1, 2]);
        // Status is shared too.
        assert_eq!(a.status_now(), AsyncStatus::Idle);
        assert_eq!(b.status_now(), AsyncStatus::Idle);
    }

    #[test]
    fn async_status_helpers_work() {
        let idle: AsyncStatus<&'static str> = AsyncStatus::Idle;
        assert!(idle.is_idle());
        assert!(!idle.is_loading());
        assert_eq!(idle.error(), None);

        let loading: AsyncStatus<&'static str> = AsyncStatus::Loading;
        assert!(loading.is_loading());

        let err: AsyncStatus<&'static str> = AsyncStatus::Error("nope");
        assert!(err.is_error());
        assert_eq!(err.error(), Some(&"nope"));
    }
}
