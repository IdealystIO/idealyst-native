//! Callback-driven async work as a reactive primitive.
//!
//! [`mutation`] is the sibling of [`resource`](crate::resource):
//! `resource` fires on reactive dep changes and is the right tool for
//! reads (load-on-mount, load-on-deps); `mutation` fires only when
//! explicitly triggered and is the right tool for writes
//! (button-click → submit form, swipe → delete, etc.).
//!
//! Like `resource`, a `Mutation` exposes its current state as a signal
//! so UI bindings can react to loading / data / error transitions, and
//! it carries a sequence-number guard so back-to-back triggers don't
//! race — only the most recent trigger's result is applied.
//!
//! Gated behind the `async-driver` Cargo feature because it depends on
//! [`crate::driver::spawn_async`].

use crate::driver::spawn_async;
use crate::reactive::Signal;
use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

// =============================================================================
// MutationState
// =============================================================================

/// Snapshot of a [`Mutation`]'s current state.
///
/// Mirrors [`ResourceState`](crate::ResourceState) field-for-field so
/// downstream code (e.g. `NetworkState` projections, generic state
/// renderers) can treat both primitives uniformly. The only difference
/// is the default: a fresh mutation is `loading: false` because no
/// trigger has happened yet, whereas a fresh resource defaults to
/// `loading: true` (its fetcher runs eagerly).
#[derive(Clone, Debug)]
pub struct MutationState<T, E> {
    /// The most recent successful payload, retained across subsequent
    /// triggers so an optimistic-UI pattern can show the last-good
    /// value while a fresh trigger is in flight.
    pub data: Option<T>,
    /// The most recent failure, cleared at the start of every fresh
    /// trigger.
    pub error: Option<E>,
    /// Whether a triggered run is currently in flight.
    pub loading: bool,
}

impl<T, E> Default for MutationState<T, E> {
    fn default() -> Self {
        Self {
            data: None,
            error: None,
            loading: false,
        }
    }
}

// =============================================================================
// Mutation
// =============================================================================

/// Reactive container for an externally-triggered async operation.
///
/// Construct via [`mutation`]. Authors call [`Self::trigger`] (fire and
/// forget, observe via the state signal) or [`Self::run`] (await the
/// result inline) to invoke the handler.
///
/// Unlike [`Resource`](crate::Resource), `Mutation` is `Clone` rather
/// than `Copy` because it owns the handler closure via `Rc` — there's
/// no arena slot to anchor the closure as `Resource` does for its
/// effect. Clone freely; the inner state is shared.
pub struct Mutation<I, T, E> {
    state: Signal<MutationState<T, E>>,
    /// Sequence guard. Each trigger captures a sequence number; on
    /// completion, the trigger's result is only applied if its number
    /// still matches the current one. Out-of-order completions (a slow
    /// first trigger settling after a fast second one) are discarded.
    sequence: Rc<Cell<u64>>,
    handler: Rc<dyn Fn(I) -> Pin<Box<dyn Future<Output = Result<T, E>>>>>,
}

impl<I, T, E> Clone for Mutation<I, T, E> {
    fn clone(&self) -> Self {
        Self {
            state: self.state,
            sequence: self.sequence.clone(),
            handler: self.handler.clone(),
        }
    }
}

impl<I: 'static, T: Clone + 'static, E: Clone + 'static> Mutation<I, T, E> {
    /// Single-read snapshot of the full state. Subscribes the caller
    /// (if any reactive context is active) to subsequent transitions.
    pub fn state(&self) -> MutationState<T, E> {
        self.state.get()
    }

    /// The last successful payload, or `None` if no trigger has
    /// succeeded yet.
    pub fn data(&self) -> Option<T> {
        self.state.get().data
    }

    /// The most recent error, or `None` if the last trigger succeeded
    /// or no trigger has run.
    pub fn error(&self) -> Option<E> {
        self.state.get().error
    }

    /// Whether a triggered run is currently in flight.
    pub fn loading(&self) -> bool {
        self.state.get().loading
    }

    /// Raw access to the backing state signal — useful when a caller
    /// wants to derive a memo or pass the signal to a child component
    /// without going through the [`Mutation`] handle.
    pub fn state_signal(&self) -> Signal<MutationState<T, E>> {
        self.state
    }

    /// Collapsed [`NetworkState`](crate::NetworkState) view of the
    /// current state, suitable for direct `match` against in UI code.
    /// A never-triggered mutation projects to `Idle`. See
    /// [`crate::NetworkState`] for the full precedence rule.
    pub fn network_state(&self) -> crate::NetworkState<T, E> {
        (&self.state.get()).into()
    }

    /// Fire the handler with `input`. Updates the state signal when
    /// the future resolves. If a previous trigger is still in flight,
    /// its result is discarded (stale-result guard).
    ///
    /// For inline `await` against the result, use [`Self::run`].
    pub fn trigger(&self, input: I) {
        let my_seq = self.sequence.get().wrapping_add(1);
        self.sequence.set(my_seq);

        // Mark loading and clear the stale error. `data` is kept so
        // optimistic-UI patterns continue to show the previous value.
        self.state.update(|s| {
            s.loading = true;
            s.error = None;
        });

        let fut = (self.handler)(input);
        let sequence = self.sequence.clone();
        let state = self.state;

        spawn_async(async move {
            let result = fut.await;
            if sequence.get() != my_seq {
                // A newer trigger has superseded this one.
                return;
            }
            // Async completion is one reactive cycle — see `reactive::cycle`.
            crate::cycle(|| {
                state.update(|s| {
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
    }

    /// Fire the handler with `input` and return the result inline.
    ///
    /// Useful from event handlers that want to navigate / show a
    /// toast / commit a follow-up action only after the operation
    /// settles. The state signal is updated identically to
    /// [`Self::trigger`], so UI bindings still react.
    ///
    /// Stale-result guard still applies — if a newer trigger
    /// supersedes this one before its future resolves, the state
    /// signal is NOT updated from this call's result (the newer one
    /// wins) but the inline `Result` is still returned to the caller.
    pub async fn run(&self, input: I) -> Result<T, E> {
        let my_seq = self.sequence.get().wrapping_add(1);
        self.sequence.set(my_seq);

        self.state.update(|s| {
            s.loading = true;
            s.error = None;
        });

        let fut = (self.handler)(input);
        let result = fut.await;

        if self.sequence.get() == my_seq {
            self.state.update(|s| {
                s.loading = false;
                match &result {
                    Ok(d) => {
                        s.data = Some(d.clone());
                        s.error = None;
                    }
                    Err(e) => {
                        s.error = Some(e.clone());
                    }
                }
            });
        }
        result
    }

    /// Clear all state back to the never-triggered default, and
    /// invalidate any in-flight trigger (its result will be discarded
    /// when it eventually resolves).
    pub fn reset(&self) {
        self.sequence.set(self.sequence.get().wrapping_add(1));
        self.state.set(MutationState::default());
    }
}

// =============================================================================
// mutation() — public constructor
// =============================================================================

/// Create a callback-driven async primitive.
///
/// `handler` is invoked once per call to [`Mutation::trigger`] or
/// [`Mutation::run`]. The returned `Mutation` is `Clone` and can be
/// captured into multiple closures (event handlers, child components).
///
/// ```ignore
/// let save = mutation(|todo: Todo| async move {
///     save_todo(todo).await  // a server-fn, an HTTP call, anything
/// });
///
/// ui! {
///     Button {
///         on_press: {
///             let save = save.clone();
///             move || save.trigger(form.get())
///         },
///     }
///     // Bind UI to state:
///     when(|| save.loading(), || text("Saving..."), || text("Save"))
/// }
/// ```
pub fn mutation<I, T, E, Fut, F>(handler: F) -> Mutation<I, T, E>
where
    I: 'static,
    T: Clone + 'static,
    E: Clone + 'static,
    Fut: Future<Output = Result<T, E>> + 'static,
    F: Fn(I) -> Fut + 'static,
{
    Mutation {
        state: Signal::new(MutationState::default()),
        sequence: Rc::new(Cell::new(0u64)),
        handler: Rc::new(move |input| Box::pin(handler(input))),
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
    /// synchronously inside `trigger`'s body, which keeps the tests
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

    struct ImmediateErr<E>(Option<E>);
    impl<E: Unpin> Future for ImmediateErr<E> {
        type Output = Result<i32, E>;
        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            std::task::Poll::Ready(Err(self.0.take().unwrap()))
        }
    }

    #[test]
    fn mutation_initial_state_is_idle_not_loading() {
        let m: Mutation<i32, i32, &'static str> =
            mutation(|x| ImmediateOk(Some(x * 2)));
        assert!(!m.loading(), "fresh mutation must not report loading");
        assert_eq!(m.data(), None);
        assert_eq!(m.error(), None);
    }

    #[test]
    fn mutation_trigger_populates_data_on_success() {
        let m: Mutation<i32, i32, &'static str> =
            mutation(|x| ImmediateOk(Some(x * 2)));
        m.trigger(21);
        assert_eq!(m.data(), Some(42));
        assert_eq!(m.error(), None);
        assert!(!m.loading());
    }

    #[test]
    fn mutation_trigger_populates_error_and_keeps_prior_data() {
        // Switch handler mode mid-test to drive a success then a
        // failure. After failure: data retained, error populated.
        let mode: Rc<RefCell<&'static str>> = Rc::new(RefCell::new("ok"));
        let mode_for_handler = mode.clone();
        let m: Mutation<i32, i32, &'static str> = mutation(move |x| {
            let m = *mode_for_handler.borrow();
            if m == "ok" {
                Box::pin(ImmediateOk(Some(x * 2)))
                    as Pin<Box<dyn Future<Output = Result<i32, &'static str>>>>
            } else {
                Box::pin(ImmediateErr(Some("boom")))
            }
        });

        m.trigger(5);
        assert_eq!(m.data(), Some(10));

        *mode.borrow_mut() = "err";
        m.trigger(7);
        assert_eq!(
            m.data(),
            Some(10),
            "data must be retained across a failed trigger (optimistic-UI affordance)"
        );
        assert_eq!(m.error(), Some("boom"));
        assert!(!m.loading());
    }

    #[test]
    fn mutation_reset_clears_state_to_idle() {
        let m: Mutation<i32, i32, &'static str> =
            mutation(|x| ImmediateOk(Some(x)));
        m.trigger(9);
        assert_eq!(m.data(), Some(9));
        m.reset();
        assert_eq!(m.data(), None);
        assert_eq!(m.error(), None);
        assert!(!m.loading());
    }

    #[test]
    fn mutation_state_reads_subscribe_callers() {
        // A reactive consumer of `data()` should re-fire when the
        // mutation's state changes.
        let m: Mutation<i32, i32, &'static str> =
            mutation(|x| ImmediateOk(Some(x + 100)));
        let observed = Rc::new(Cell::new(0i32));
        let o = observed.clone();
        let m_for_effect = m.clone();
        let _e = Effect::new(move || {
            if let Some(d) = m_for_effect.data() {
                o.set(d);
            }
        });
        assert_eq!(observed.get(), 0, "no trigger fired yet");
        m.trigger(1);
        assert_eq!(observed.get(), 101);
        m.trigger(2);
        assert_eq!(observed.get(), 102);
    }

    #[test]
    fn mutation_run_returns_result_inline() {
        let m: Mutation<i32, i32, &'static str> =
            mutation(|x| ImmediateOk(Some(x + 1)));
        // pollster's block_on satisfies the await on native.
        let result = pollster::block_on(m.run(41));
        assert_eq!(result, Ok(42));
        assert_eq!(m.data(), Some(42));
    }

    #[test]
    fn mutation_run_propagates_error_inline() {
        let m: Mutation<i32, i32, &'static str> =
            mutation(|_x| ImmediateErr(Some("nope")));
        let result = pollster::block_on(m.run(0));
        assert_eq!(result, Err("nope"));
        assert_eq!(m.error(), Some("nope"));
    }

    #[test]
    fn mutation_handle_is_clone_and_shares_state() {
        let m: Mutation<i32, i32, &'static str> =
            mutation(|x| ImmediateOk(Some(x)));
        let a = m.clone();
        let b = m.clone();
        a.trigger(7);
        assert_eq!(b.data(), Some(7), "clones must share the same state slot");
    }
}
