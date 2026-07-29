//! New-core mirrors of the old root's async-reactive primitives —
//! `resource()` / `mutation()` (`crates/runtime/core/src/{resource,mutation}.rs`).
//!
//! # Why these are reimplementations, not re-exports
//!
//! The old primitives are built ON old-core reactivity: their state
//! lives in old-arena `Signal`s, their lifetime anchoring rides
//! old-core `Effect::new` + scope adoption (`persist`), and their async
//! completions re-enter the graph through `cycle()`. None of that
//! machinery is live on a new-core mount — an aliased crate calling the
//! old `resource()` would create state no world effect can subscribe
//! to (the same class of silent inertness as the old `AnimatedValue::bind`
//! and `after_ms_scoped`; see `glue::animation` / `scoped_scheduling`).
//! So the glue grows semantics-faithful mirrors on the world kernel.
//!
//! # New-core anchoring (the part that differs by necessity)
//!
//! - **Creation requires a reactive context.** State slots live in a
//!   [`World`](runtime_world::World); `resource()`/`mutation()` panic
//!   outside `World::enter` — the same posture as every other glue
//!   creation API (the old core allowed thread-lifetime creation
//!   anywhere; worlds are transient, so "anywhere" has no home).
//! - **Lifetime = the registering scope.** Inside a component build the
//!   ambient collector (`component_scope` / boot `collect_owned`) owns
//!   the slots — the resource dies when the subtree unrealizes, the old
//!   "active scope adopts the effect" contract. Inside an effect run
//!   (`runtime_world::in_effect()`) the slots are collected into a
//!   private `Owned` dropped by that effect's `on_cleanup` — the old
//!   "disposed on the surrounding effect's re-run" contract (the
//!   `scoped_scheduling` anchoring precedent). With no collector the
//!   slots are world-root-owned: they persist for the world's lifetime,
//!   the closest analogue of the old `persist()`-outside-a-scope.
//! - **Async completion is event-boundary work.** The spawned future's
//!   continuation runs OUTSIDE `World::enter`; its `state.update(..)`
//!   *stages* into the signal's own world (handles route by world id)
//!   and commits when the host's post-dispatch flush hook fires
//!   (`backend_web::dispatch_hook` — the executor fires it after every
//!   future poll). No `cycle()` wrap: staging IS the batch on this
//!   kernel. Tests flush explicitly to pin the boundary.
//! - **Teardown drops pending completions safely.** The kernel panics
//!   on writes through a stale handle (live world, freed slot) — a
//!   use-after-unmount diagnostic — so a completion that lands after
//!   the owning scope dropped MUST NOT touch the state signal. The
//!   resource guards with its cancel token (fired by the inner effect's
//!   cleanup at re-run/teardown, exactly like the old core, where the
//!   check was belt-and-suspenders and here is load-bearing); the
//!   mutation carries a liveness sentinel (a dependency-free effect
//!   whose cleanup flips the flag at scope teardown). Completions after
//!   the whole `World` died are additionally covered by the kernel's
//!   "writes to a dead world are silent no-ops" rule.
//!
//! # Documented semantic divergences (kernel policy, not accidents)
//!
//! - `refetch()` / `trigger()` / `reset()` **through an unmounted
//!   handle** panic with the kernel's stale-handle diagnostic (the old
//!   core's stale-set was a silent no-op). The kernel deliberately
//!   surfaces use-after-unmount; only *in-flight completions* get the
//!   silent-drop treatment, because they are not a logic error.
//! - **N `refetch()` calls in one event turn coalesce into one fetch**
//!   (staged counter writes compose; the effect re-runs once per
//!   flush). The old core issued N fetches whose first N−1 results the
//!   stale-guard discarded anyway — observable state is identical.
//! - `ResourceState`/`MutationState` implement `PartialEq` as
//!   **always-`false`** (see the impl comment) — value comparison of
//!   state snapshots is not part of the old surface, and the kernel's
//!   guarded `set` needs the bound.
//! - The old `From<&ResourceState<T,E>> for NetworkState<T,E>` ad-hoc
//!   conversions cannot be mirrored (`NetworkState` stays the shared
//!   runtime-core enum; the impl would be an orphan here). Use
//!   [`Resource::network_state`] / [`Mutation::network_state`] — the
//!   projection rules are reproduced there verbatim.
//!
//! Gated behind the crate's `async-driver` feature (forwards
//! `runtime-core/async-driver`) because both primitives depend on
//! `runtime_shared::driver::spawn_async`, exactly like the old root.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use runtime_shared::driver::spawn_async;
use runtime_shared::NetworkState;
use runtime_world::{collect_owned, in_effect, on_cleanup, signal, Signal};

use crate::glue::Trackable;

// =============================================================================
// Scope anchoring (shared by resource + mutation)
// =============================================================================

/// Run `make` so the slots it creates die with the registering scope,
/// per the module-doc rules. Inside an effect run the creations are
/// collected into a private `Owned` dropped by that effect's cleanup
/// (old-core "scope adopts, disposes on re-run"); everywhere else the
/// ambient collector (or the world root) owns them naturally.
fn anchor_to_scope<R>(make: impl FnOnce() -> R) -> R {
    if in_effect() {
        let (r, owned) = collect_owned(make);
        on_cleanup(move || drop(owned));
        r
    } else {
        make()
    }
}

/// Liveness sentinel: `true` until the registering scope tears down.
/// Implemented as a dependency-free world effect whose returned cleanup
/// flips the flag — it reads no signals, so it never re-runs, and its
/// cleanup fires exactly once, at the owning collector's (or world's)
/// teardown. Async continuations consult it before touching state
/// signals, because a stale-handle write panics by kernel design (see
/// the module docs).
fn liveness_sentinel() -> Rc<Cell<bool>> {
    let live = Rc::new(Cell::new(true));
    let flag = live.clone();
    let _ = runtime_world::effect(move || {
        let f = flag.clone();
        move || f.set(false)
    });
    live
}

// =============================================================================
// ResourceState / MutationState
// =============================================================================

/// Snapshot of a [`Resource`]'s current state — field-for-field mirror
/// of `runtime_shared::ResourceState` (see that type's docs for the five
/// meaningful field combinations: data is retained across refetches so
/// the UI doesn't flash empty, error clears when a new fetch starts).
#[derive(Clone, Debug)]
pub struct ResourceState<T, E> {
    /// The last successful payload (retained during refetch).
    pub data: Option<T>,
    /// The most recent fetch's error (cleared when a new fetch starts).
    pub error: Option<E>,
    /// Whether a fetch is in flight.
    pub loading: bool,
}

impl<T, E> Default for ResourceState<T, E> {
    fn default() -> Self {
        Self { data: None, error: None, loading: true }
    }
}

/// Snapshot of a [`Mutation`]'s current state — mirror of
/// `runtime_shared::MutationState`. Differs from [`ResourceState`] only
/// in its default: a fresh mutation is `loading: false` (nothing has
/// been triggered), a fresh resource `loading: true` (fetches eagerly).
#[derive(Clone, Debug)]
pub struct MutationState<T, E> {
    /// The most recent successful payload, retained across subsequent
    /// triggers (optimistic-UI affordance).
    pub data: Option<T>,
    /// The most recent failure, cleared at the start of every trigger.
    pub error: Option<E>,
    /// Whether a triggered run is in flight.
    pub loading: bool,
}

impl<T, E> Default for MutationState<T, E> {
    fn default() -> Self {
        Self { data: None, error: None, loading: false }
    }
}

// The world kernel requires `T: PartialEq` on every signal (guarded
// `set`). The old state structs carry unbounded `T`/`E` (a GraphQL
// `ResponseData` is `Clone` but rarely `PartialEq`), and the old core
// notified on EVERY state transition (its `update` has no equality
// guard for unbounded payloads). An always-`false` eq preserves both
// at once: no bounds leak onto `T`/`E`, and every committed transition
// notifies — the old contract. This is a notification identity, NOT
// value equality; do not compare state snapshots with `==`.
impl<T, E> PartialEq for ResourceState<T, E> {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}
impl<T, E> PartialEq for MutationState<T, E> {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

/// Shared projection rules for [`Resource::network_state`] /
/// [`Mutation::network_state`] — verbatim `runtime_shared::network_state`
/// precedence: `Loading > Error > Success > `(fallback). The fallback
/// (`data`/`error` both `None`, not loading) is the caller's
/// never-settled/never-triggered state.
fn project<T: Clone, E: Clone>(
    data: &Option<T>,
    error: &Option<E>,
    loading: bool,
    fallback: NetworkState<T, E>,
) -> NetworkState<T, E> {
    if loading {
        return NetworkState::Loading;
    }
    if let Some(e) = error {
        return NetworkState::Error(e.clone());
    }
    if let Some(d) = data {
        return NetworkState::Success(d.clone());
    }
    fallback
}

// =============================================================================
// ResourceCancel
// =============================================================================

/// Cancellation token passed to a resource's fetcher — mirror of
/// `runtime_shared::ResourceCancel` (same fire conditions: dep change /
/// refetch starting a fresh fetch, or the owning scope tearing down).
///
/// On the old core the completion-side `is_cancelled` check was an
/// optimization (the sequence guard already discarded stale results).
/// On the new core it is **load-bearing**: the token is what stops a
/// post-teardown completion from writing through a freed signal slot,
/// which the kernel treats as a panic-worthy use-after-unmount. Pure
/// `Rc` machinery — safe to poll from any continuation.
#[derive(Clone)]
pub struct ResourceCancel {
    inner: Rc<ResourceCancelInner>,
}

struct ResourceCancelInner {
    cancelled: Cell<bool>,
    callbacks: RefCell<Vec<Box<dyn FnOnce()>>>,
}

impl ResourceCancel {
    fn new() -> Self {
        Self {
            inner: Rc::new(ResourceCancelInner {
                cancelled: Cell::new(false),
                callbacks: RefCell::new(Vec::new()),
            }),
        }
    }

    /// Has the token been cancelled? Poll at await points.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.get()
    }

    /// Register a callback that runs once on cancellation (already-
    /// cancelled tokens fire it immediately). Bridge to platform abort
    /// APIs (`AbortController` on wasm) exactly as on the old core.
    pub fn on_cancel<F: FnOnce() + 'static>(&self, f: F) {
        if self.inner.cancelled.get() {
            f();
            return;
        }
        self.inner.callbacks.borrow_mut().push(Box::new(f));
    }

    /// Fire the token. Idempotent.
    fn cancel(&self) {
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

/// Reactive container for an async-computed value — mirror of
/// `runtime_shared::Resource`. Construct via [`resource`]; the handle is
/// `Copy` (two world-signal handles), pass it freely to children.
/// Accessors subscribe the calling reactive context like `Signal::get`.
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
    /// The last successful payload (retained during refetch — check
    /// [`loading`](Self::loading) for in-flight status).
    pub fn data(&self) -> Option<T> {
        self.state.get().data
    }

    /// The most recent fetch's error (cleared when a new fetch starts).
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

    /// Collapsed [`NetworkState`] view (`Loading > Error > Success`;
    /// the never-settled fallback is `Loading`, matching the old
    /// projection — a resource is never `Idle`).
    pub fn network_state(&self) -> NetworkState<T, E> {
        let s = self.state.get();
        project(&s.data, &s.error, s.loading, NetworkState::Loading)
    }

    /// Re-run the fetcher with the current deps (pull-to-refresh /
    /// retry-after-error). Same cancel-previous + spawn-fresh path as a
    /// dep change. Divergence note: multiple `refetch()` calls in one
    /// event turn coalesce into ONE fetch (staged writes compose);
    /// calling through an unmounted handle panics with the kernel's
    /// stale-handle diagnostic (module docs).
    pub fn refetch(&self) {
        // `update` composes on the staged value — the new-core
        // equivalent of the old `untrack(get) + set` (update never
        // tracks, so this is effect-body-safe too).
        self.refetch_counter.update(|n| n.wrapping_add(1));
    }
}

/// Create a reactive resource — mirror of `runtime_shared::resource` (see
/// that fn's docs for the authoring contract: `deps` is a [`Trackable`]
/// re-fetched on change, `fetcher` runs eagerly and receives a
/// [`ResourceCancel`]). New-core specifics — creation context, lifetime
/// anchoring, completion staging, teardown safety — are in the module
/// docs. Panics outside `World::enter`.
pub fn resource<D, T, E, Fut, F>(deps: D, fetcher: F) -> Resource<T, E>
where
    D: Trackable + 'static,
    D::Value: 'static,
    T: Clone + 'static,
    E: Clone + 'static,
    Fut: Future<Output = Result<T, E>> + 'static,
    F: Fn(D::Value, ResourceCancel) -> Fut + 'static,
{
    anchor_to_scope(move || {
        let state: Signal<ResourceState<T, E>> = signal(ResourceState::default());
        let refetch_counter: Signal<u64> = signal(0u64);

        // Shared bookkeeping across effect re-runs — plain `Rc`/`Cell`,
        // never signals: the completion continuation must be able to
        // consult them after the world/scope is gone.
        let sequence = Rc::new(Cell::new(0u64));
        let active_cancel: Rc<RefCell<Option<ResourceCancel>>> = Rc::new(RefCell::new(None));
        let fetcher = Rc::new(fetcher);

        let _ = runtime_world::effect(move || {
            // Subscribe to deps + the refetch trigger.
            let inputs = deps.track();
            let _ = refetch_counter.get();

            // Cancel the previously-active fetch (advisory for IO; the
            // sequence guard below is the correctness backstop).
            if let Some(prev) = active_cancel.borrow_mut().take() {
                prev.cancel();
            }

            let my_seq = sequence.get().wrapping_add(1);
            sequence.set(my_seq);
            let cancel = ResourceCancel::new();
            *active_cancel.borrow_mut() = Some(cancel.clone());

            // Mark loading; clear stale error; KEEP previous data (no
            // empty-flash on refetch — old-core contract).
            state.update(|s| ResourceState {
                data: s.data.clone(),
                error: None,
                loading: true,
            });

            // Fires on the effect's next re-run AND at scope teardown —
            // the load-bearing completion guard (module docs).
            let cancel_for_cleanup = cancel.clone();
            on_cleanup(move || cancel_for_cleanup.cancel());

            let fut = fetcher(inputs, cancel.clone());
            let sequence_for_spawn = sequence.clone();
            let cancel_for_spawn = cancel.clone();

            spawn_async(async move {
                let result = fut.await;

                // Stale-result guard: only the most-recently-issued
                // fetch wins, whatever order IO completes in.
                if sequence_for_spawn.get() != my_seq {
                    return;
                }
                // Teardown guard — MANDATORY here (the kernel panics on
                // stale-handle writes; see module docs).
                if cancel_for_spawn.is_cancelled() {
                    return;
                }

                // Event-boundary write: stages into the state signal's
                // own world; the host's post-dispatch hook flushes.
                state.update(|s| match result {
                    Ok(d) => ResourceState { data: Some(d), error: None, loading: false },
                    Err(e) => ResourceState {
                        data: s.data.clone(),
                        error: Some(e),
                        loading: false,
                    },
                });
            });
        });

        Resource { state, refetch_counter }
    })
}

// =============================================================================
// Mutation
// =============================================================================

/// Reactive container for an externally-triggered async operation —
/// mirror of `runtime_shared::Mutation` (fires only on
/// [`trigger`](Self::trigger)/[`run`](Self::run); the write-side
/// sibling of [`Resource`]). `Clone`, not `Copy` — it owns the handler
/// closure via `Rc`, same as the old core.
pub struct Mutation<I, T, E> {
    state: Signal<MutationState<T, E>>,
    /// Trigger sequence guard: out-of-order completions (slow first
    /// trigger settling after a fast second) are discarded.
    sequence: Rc<Cell<u64>>,
    /// Scope-teardown flag consulted by in-flight completions before
    /// they write (module docs).
    live: Rc<Cell<bool>>,
    handler: Rc<dyn Fn(I) -> Pin<Box<dyn Future<Output = Result<T, E>>>>>,
}

impl<I, T, E> Clone for Mutation<I, T, E> {
    fn clone(&self) -> Self {
        Self {
            state: self.state,
            sequence: self.sequence.clone(),
            live: self.live.clone(),
            handler: self.handler.clone(),
        }
    }
}

impl<I: 'static, T: Clone + 'static, E: Clone + 'static> Mutation<I, T, E> {
    /// Single-read snapshot of the full state (subscribes reactive
    /// callers).
    pub fn state(&self) -> MutationState<T, E> {
        self.state.get()
    }

    /// The last successful payload.
    pub fn data(&self) -> Option<T> {
        self.state.get().data
    }

    /// The most recent error.
    pub fn error(&self) -> Option<E> {
        self.state.get().error
    }

    /// Whether a triggered run is in flight.
    pub fn loading(&self) -> bool {
        self.state.get().loading
    }

    /// Raw access to the backing state signal (derive memos from it,
    /// hand it to children).
    pub fn state_signal(&self) -> Signal<MutationState<T, E>> {
        self.state
    }

    /// Collapsed [`NetworkState`] view; a never-triggered mutation
    /// projects to `Idle` (old-core precedence rules).
    pub fn network_state(&self) -> NetworkState<T, E> {
        let s = self.state.get();
        project(&s.data, &s.error, s.loading, NetworkState::Idle)
    }

    /// Fire the handler with `input`; observe via the state signal.
    /// Stale-result guard: if a newer trigger supersedes this one, its
    /// completion is discarded. The completion write stages and commits
    /// at the host's post-dispatch flush (module docs).
    pub fn trigger(&self, input: I) {
        let my_seq = self.sequence.get().wrapping_add(1);
        self.sequence.set(my_seq);

        // Loading on, stale error cleared, data kept (optimistic-UI).
        self.state.update(|s| MutationState {
            data: s.data.clone(),
            error: None,
            loading: true,
        });

        let fut = (self.handler)(input);
        let sequence = self.sequence.clone();
        let live = self.live.clone();
        let state = self.state;

        spawn_async(async move {
            let result = fut.await;
            if sequence.get() != my_seq {
                return; // superseded by a newer trigger
            }
            if !live.get() {
                return; // owning scope tore down mid-flight (module docs)
            }
            state.update(|s| Self::settled(s, result));
        });
    }

    /// Fire the handler and return the result inline (navigate / toast
    /// after settle). State-signal updates match [`trigger`](Self::trigger),
    /// including the stale guard — a superseded run still RETURNS its
    /// result but does not write it.
    pub async fn run(&self, input: I) -> Result<T, E> {
        let my_seq = self.sequence.get().wrapping_add(1);
        self.sequence.set(my_seq);

        self.state.update(|s| MutationState {
            data: s.data.clone(),
            error: None,
            loading: true,
        });

        let fut = (self.handler)(input);
        let result = fut.await;

        if self.sequence.get() == my_seq && self.live.get() {
            let settled = result.clone();
            self.state.update(move |s| Self::settled(s, settled));
        }
        result
    }

    /// Clear back to the never-triggered default and invalidate any
    /// in-flight trigger. `set_always` for the same reason as the old
    /// core: a reset notifying its subscribers is the contract (and the
    /// state's `PartialEq` is a notification identity anyway).
    pub fn reset(&self) {
        self.sequence.set(self.sequence.get().wrapping_add(1));
        self.state.set_always(MutationState::default());
    }

    /// The settle transition shared by `trigger`/`run`.
    fn settled(prev: &MutationState<T, E>, result: Result<T, E>) -> MutationState<T, E> {
        match result {
            Ok(d) => MutationState { data: Some(d), error: None, loading: false },
            Err(e) => MutationState {
                data: prev.data.clone(),
                error: Some(e),
                loading: false,
            },
        }
    }
}

/// Create a callback-driven async primitive — mirror of
/// `runtime_shared::mutation` (see that fn's docs for the authoring
/// contract). New-core specifics: panics outside `World::enter` (its
/// state signal needs a world), and its lifetime anchors to the
/// registering scope like [`resource`] — clones share the one state
/// slot, so a `Mutation` captured by a longer-lived closure must not be
/// triggered after its creating component unmounts (module docs).
pub fn mutation<I, T, E, Fut, F>(handler: F) -> Mutation<I, T, E>
where
    I: 'static,
    T: Clone + 'static,
    E: Clone + 'static,
    Fut: Future<Output = Result<T, E>> + 'static,
    F: Fn(I) -> Fut + 'static,
{
    anchor_to_scope(move || Mutation {
        state: signal(MutationState::default()),
        sequence: Rc::new(Cell::new(0u64)),
        live: liveness_sentinel(),
        handler: Rc::new(move |input| Box::pin(handler(input))),
    })
}
