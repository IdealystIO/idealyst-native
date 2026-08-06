//! Code-splitting handler: `lazy` — the old `walker/lazy.rs` build
//! path re-expressed on the new core, invariant for invariant.
//!
//! # Shape (and why it is NOT a Dyn hole)
//!
//! One container view (`create_view` — the "lazy-boundary wrapper
//! div") whose children are swapped **imperatively** between the three
//! states (loading / chunk body / error), exactly like the old walker.
//! A scene `Dyn` hole would nest the states under a reactive anchor on
//! anchored hosts (`<div style="display: contents">` on SSR/hydrate),
//! which the old core does not emit — the imperative swap keeps the
//! node stream byte-identical across cores (the website SSG parity
//! gate) and keeps a future hydration adoption of lazy boundaries
//! cursor-compatible with old-core SSR output.
//!
//! # Ownership + async discipline
//!
//! - Each state UI is realized as its OWN [`Realized`] held in the
//!   `mounted` slot; swapping drops the outgoing subtree's scope FIRST,
//!   then clears the container, then realizes the incoming one — the
//!   anchored dispose order (`dispose_order_when.anchored.golden`).
//! - The loader future's continuation touches **no nodes and no
//!   world-ambient API**: it stores the outcome in a plain cell and
//!   bumps a `tick` signal (handle-routed staged write — legal from a
//!   naked executor task). The swap effect, owned by the enclosing
//!   subtree, does all construction/realization when the flush driver
//!   commits the tick. Teardown cancellation is therefore free: once
//!   the enclosing `Realized` drops the swap effect, a late-resolving
//!   load bumps a signal nobody observes (the old walker needed an
//!   explicit `cancelled` flag + `ScopedLoad` because its continuation
//!   built directly into the container).
//! - Chunk-load completion reaches the UI via the platform's installed
//!   dispatch hook (web: `HookedFuture` fires the flush after every
//!   executor poll) — the flush-driver discipline; no flush is
//!   triggered from here.
//!
//! # SSR
//!
//! `LifecycleOps::renders_lazy_chunks()` is the same gate the old
//! walker consulted: when `false` (SSR/SSG), the placeholder realizes
//! statically into the container — no signals, no effects, no spawn —
//! so the server bytes are the untouched loading UI, identical to the
//! old core's output.

#[cfg(feature = "async-driver")]
use std::cell::{Cell, RefCell};
#[cfg(feature = "async-driver")]
use std::rc::Rc;

#[cfg(feature = "async-driver")]
use runtime_shared::primitives::lazy::LazyError;
use runtime_shared::primitives::lazy::LazyState;
#[cfg(feature = "async-driver")]
use runtime_scene::{component_scope, realize, Realized};
use runtime_scene::{Element, MountCx, Registry};
#[cfg(feature = "async-driver")]
use runtime_world::{effect, signal, untrack};

use crate::caps::{LifecycleOps, ViewOps};
use crate::prims::{LazyPrim, PrimCell};
use crate::style_attach::{attach_style, StyleServices};

/// Mount a `lazy` boundary — port of `walker/lazy.rs::build`.
pub fn mount_lazy<H>(cx: &mut MountCx<'_, H>, prim: LazyPrim, _children: Vec<Element>) -> H::Node
where
    H: ViewOps + LifecycleOps + StyleServices,
{
    let backend = cx.backend().clone();
    let registry = cx.registry().clone();

    // Container view hosting one state at a time. Old-core order:
    // create_view → attach_style → loading UI.
    let mut container = backend.borrow_mut().create_view(&prim.a11y);
    if let Some(style) = prim.style {
        attach_style(&backend, &container, style);
    }

    let on_state = prim.on_state;
    let placeholder = prim.placeholder;

    // Fire `Loading` synchronously at mount so author UI sees a
    // consistent first event on every backend (walker contract).
    if let Some(cb) = on_state.as_ref() {
        cb(LazyState::Loading);
    }

    // SSR/SSG (and async-driver-less builds): the untouched loading UI
    // is the final output — realize it into the enclosing subtree's
    // scope and stop. No signals, no spawn (module docs).
    #[cfg(feature = "async-driver")]
    let chunks = backend.borrow().renders_lazy_chunks();
    #[cfg(not(feature = "async-driver"))]
    let chunks = false;
    if !chunks {
        #[cfg(not(feature = "async-driver"))]
        let _ = (&prim.loader, &prim.error, &registry);
        if let Some(build) = placeholder.as_ref() {
            cx.realize_children_into(&mut container, vec![build()]);
        }
        return container;
    }

    #[cfg(feature = "async-driver")]
    {
        let loader: Rc<crate::prims::lazy::LazyLoader> = Rc::new(prim.loader);
        let error = prim.error;

        // ---- Off-web fast path: `lazy` is a no-op when there is no chunk ----
        //
        // `wasm_split` emits the chunk body as a PLAIN FN off wasm32
        // (`#[cfg(not(target_arch = "wasm32"))] #default_item`), so there is
        // nothing to fetch and the loader's future is already complete the
        // first time it is polled. Running the full streaming dance for it
        // costs a placeholder paint, an executor round-trip, and a
        // realize → tear-down → realize swap, for zero benefit — and it makes
        // every native screen depend on a per-poll flush hook that only the
        // web and Apple backends install.
        //
        // So: poll once. If the body is already there, realize it directly.
        //
        // The future is POLLED, never re-created. `#[component(lazy)]` panics
        // on a second invocation of a non-`retryable` loader, so a future that
        // comes back `Pending` (or `Err`) is handed to the streaming path
        // below rather than dropped and rebuilt. That also means a host whose
        // loader genuinely is async keeps today's behaviour untouched — the
        // fast path is an optimisation that proves itself, not a cfg guess.
        #[cfg(not(target_arch = "wasm32"))]
        let primed: Option<crate::prims::lazy::LazyFuture> = {
            use std::future::Future;
            let mut fut = (loader)();
            let mut ctx = std::task::Context::from_waker(std::task::Waker::noop());
            match fut.as_mut().poll(&mut ctx) {
                std::task::Poll::Ready(Ok(thunk)) => {
                    cx.realize_children_into(&mut container, vec![component_scope(thunk)]);
                    if let Some(cb) = on_state.as_ref() {
                        cb(LazyState::Rendered);
                    }
                    return container;
                }
                _ => Some(fut),
            }
        };
        #[cfg(target_arch = "wasm32")]
        let primed: Option<crate::prims::lazy::LazyFuture> = None;
        // First attempt reuses the already-polled future; retries call the
        // loader (which `retryable` makes legal).
        let primed = Rc::new(RefCell::new(primed));

        // The currently mounted state UI. Owned by the swap effect's
        // closure (collected into the enclosing subtree) — teardown
        // drops the effect, the slot, and with it the live subtree's
        // cleanups, through the standard drop-as-unmount path.
        let mounted: Rc<RefCell<Option<Realized<H::Node>>>> = Rc::new(RefCell::new(None));

        // Swap the container to `element`. Anchored dispose order: old
        // scope drops FIRST, then the container clears, then the new
        // subtree realizes and inserts. The initial paint skips the
        // clear (freshly empty container — the walker's
        // `show_loading(false)` contract).
        let swap_to: Rc<dyn Fn(Element)> = {
            let backend = backend.clone();
            let registry = registry.clone();
            let container = container.clone();
            let mounted = mounted.clone();
            Rc::new(move |element: Element| {
                let old = mounted.borrow_mut().take();
                if old.is_some() {
                    drop(old);
                    backend.borrow_mut().clear_children(&container);
                }
                let realized = realize(&backend, &registry, element);
                {
                    let mut b = backend.borrow_mut();
                    let mut c = container.clone();
                    for node in realized.collect_nodes() {
                        b.insert(&mut c, node);
                    }
                }
                *mounted.borrow_mut() = Some(realized);
            })
        };

        // (Re-)mount the loading UI — initial paint now, retry later.
        let show_loading: Rc<dyn Fn()> = {
            let on_state = on_state.clone();
            let placeholder = placeholder.clone();
            let swap_to = swap_to.clone();
            Rc::new(move || {
                if let Some(cb) = on_state.as_ref() {
                    cb(LazyState::Loading);
                }
                if let Some(build) = placeholder.as_ref() {
                    let build = build.clone();
                    swap_to(component_scope(move || build()));
                }
            })
        };
        // Initial paint (Loading already fired above — build only).
        if let Some(build) = placeholder.as_ref() {
            let build = build.clone();
            swap_to(component_scope(move || build()));
        }

        // Load outcome mailbox + tick. The continuation only writes
        // these (module docs: no nodes, no ambient API); the swap
        // effect below consumes them under the flush.
        type Outcome = Result<crate::prims::lazy::LazyBodyThunk, String>;
        let pending: Rc<RefCell<Option<Outcome>>> = Rc::new(RefCell::new(None));
        let tick = signal(0u64);

        // Flipped when the enclosing subtree tears down: `tick` was
        // collected into that subtree's `Owned`, so a LATE continuation
        // touching it would trip the kernel's stale-signal-handle
        // diagnostic (freed slot in a live world). The teardown probe
        // flips the flag; the continuation checks it first — the
        // new-core residue of the old walker's `LazyCancelGuard`.
        let cancelled = Rc::new(Cell::new(false));
        {
            let cancelled = cancelled.clone();
            crate::style_attach::on_teardown(move || cancelled.set(true));
        }

        // Spawns one load attempt (initial call + each retry).
        let run_load: Rc<dyn Fn()> = {
            let loader = loader.clone();
            let pending = pending.clone();
            let cancelled = cancelled.clone();
            let primed = primed.clone();
            Rc::new(move || {
                let fut = primed.borrow_mut().take().unwrap_or_else(|| (loader)());
                let pending = pending.clone();
                let cancelled = cancelled.clone();
                runtime_shared::driver::spawn_async(async move {
                    let outcome = fut.await;
                    if cancelled.get() {
                        return; // subtree gone — abandon the load
                    }
                    *pending.borrow_mut() = Some(outcome);
                    tick.update(|n| n + 1);
                });
            })
        };

        // Retry plumbing — same weak-holder shape as the old walker:
        // the error UI's retry handle reaches `retry_holder` weakly, so
        // the cycle `swap effect → retry → show_loading/run_load` can't
        // leak; after teardown a lingering retry upgrades to nothing.
        let retry_holder: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let retry_weak: Rc<dyn Fn()> = {
            let holder = Rc::downgrade(&retry_holder);
            Rc::new(move || {
                if let Some(h) = holder.upgrade() {
                    let f = h.borrow().clone();
                    if let Some(f) = f {
                        f();
                    }
                }
            })
        };
        {
            let show_loading = show_loading.clone();
            let run_load = run_load.clone();
            *retry_holder.borrow_mut() = Some(Rc::new(move || {
                show_loading();
                run_load();
            }));
        }

        // The swap effect: consumes the mailbox when the tick commits.
        // Construction runs inside `component_scope` so state the chunk
        // creates eagerly (an External extension allocating signals in
        // its constructor) is owned by the chunk subtree — the
        // `ScopedLoad` invariant, at the mount site (prims/lazy.rs
        // module docs). Owns `mounted` + `retry_holder` for teardown.
        let _swap = effect(move || {
            let _ = tick.get();
            let _keep = (&retry_holder, &mounted);
            let Some(outcome) = pending.borrow_mut().take() else {
                return;
            };
            untrack(|| match outcome {
                Ok(thunk) => {
                    swap_to(component_scope(thunk));
                    if let Some(cb) = on_state.as_ref() {
                        cb(LazyState::Rendered);
                    }
                }
                Err(message) => {
                    if let Some(cb) = on_state.as_ref() {
                        cb(LazyState::Error(message.clone()));
                    }
                    match error.as_ref() {
                        Some(build_err) => {
                            let err = LazyError::__new(message, retry_weak.clone());
                            let build_err = build_err.clone();
                            swap_to(component_scope(move || build_err(&err)));
                        }
                        None => {
                            runtime_shared::logging::log(
                                runtime_shared::logging::LogLevel::Error,
                                &format!(
                                    "[idealyst] lazy chunk failed to load: {message} \
                                     (no .on_error handler — the loading UI stays visible)"
                                ),
                            );
                        }
                    }
                }
            });
        });

        // Kick off the initial load (loading UI already painted).
        run_load();
    }

    container
}

/// Register the lazy handler. Called by
/// [`register_builtins`](crate::handlers::register_builtins); available
/// separately for backends assembling a custom registry.
pub fn register_lazy<H>(registry: &mut Registry<H>)
where
    H: ViewOps + LifecycleOps + StyleServices + 'static,
{
    registry.register::<PrimCell<LazyPrim>, _>(|cx, p, children| mount_lazy(cx, p.take(), children));
}
