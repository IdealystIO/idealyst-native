//! `Element::Lazy` build path.
//!
//! Mounts the placeholder synchronously, then spawns an async task
//! that drives the loader. When the loader's future resolves with
//! the chunk's `Element`, we build it and replace the
//! placeholder's children with the chunk's content.
//!
//! - **Wasm**: the loader is `wasm-split`'s generated wrapper. Its
//!   future awaits the chunk fetch + the chunk's async fn before
//!   yielding the `Element`.
//! - **Native**: the loader's future resolves synchronously on
//!   first poll because the chunk's async fn is just a regular
//!   async function compiled into the same binary.
//!
//! The on_state callback fires `Loading` synchronously on mount,
//! then `Rendered` when the chunk's primitive is built (or `Error`
//! if the load fails). `Loaded` is skipped — the gap between fetch
//! completion and primitive resolution is below the resolution of
//! a human-observable transition.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::element::Element;
use crate::primitives::lazy::{LazyLoader, LazyState};
use crate::reactive;
use crate::sources::StyleSource;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Dropped when the surrounding scope tears down (it is captured by the
/// `_cleanup_effect`, whose arena slot the surrounding scope owns). Its
/// `Drop` cancels any in-flight load and tears the chunk scope down at the
/// *right moment* — parent teardown — instead of whenever a still-pending
/// load happens to resolve.
///
/// Without the cancel flag, a load that resolves *after* the parent
/// unmounted would call `build_inner` against an orphaned scope and
/// `insert` into a detached container (stale-mount / use-after-teardown).
/// The async continuation checks `cancelled` after its await and bails.
struct LazyCancelGuard {
    cancelled: Rc<Cell<bool>>,
    chunk_scope: Rc<RefCell<Option<Box<reactive::Scope>>>>,
}

impl Drop for LazyCancelGuard {
    fn drop(&mut self) {
        // Signal in-flight loads to abandon their post-await work.
        self.cancelled.set(true);
        // Drop the chunk's reactive scope now so its cleanup effects (e.g.
        // `release_graphics`) run at teardown rather than at late resolution.
        // Taking the `Option` also makes the future's `as_mut()` fail closed.
        let scope = self.chunk_scope.borrow_mut().take();
        drop(scope);
    }
}

/// Wraps the chunk loader future so every poll runs with the chunk's
/// reactive scope active. A lazy chunk constructs its `Element` *inside*
/// this future — the wasm-split chunk fn runs there, and any state the chunk
/// builds eagerly at construction (an `Element::External` extension that
/// allocates signals in its constructor, a component calling `signal()` as it
/// builds) is created during the future's polls, before `build_inner` runs.
/// Without an active scope those creations are unowned and leak until the
/// thread exits (dev builds warn "signal created outside any reactive scope").
///
/// A poll is synchronous — `await` points suspend *between* polls, never
/// within one — so re-entering the scope per poll takes ownership of every
/// synchronous signal/effect the chunk creates while never holding the scope
/// across a suspension. It is the SAME scope `build_inner` runs under, so
/// construction-time and build-time reactive state share one owner and tear
/// down together when the surrounding scope drops.
///
/// Yields `None` if the surrounding scope was torn down mid-load (the
/// [`LazyCancelGuard`] took the scope out of the slot): the load is abandoned,
/// mirroring the post-await `cancelled` check.
#[cfg(any(feature = "async-driver", test))]
struct ScopedLoad {
    scope: Rc<RefCell<Option<Box<reactive::Scope>>>>,
    /// Ambient navigator context captured at mount (see [`build`]).
    /// Re-entered around every poll: the chunk fn constructs its
    /// `Element` inside this future, so any `link` it builds reads
    /// `ambient_navigator()` here — with the screen build long
    /// returned and its guards off the stack.
    nav_ctx: crate::primitives::navigator::shared::AmbientNavContext,
    inner: crate::primitives::lazy::LazyFuture,
}

#[cfg(any(feature = "async-driver", test))]
impl std::future::Future for ScopedLoad {
    /// `None` = surrounding scope torn down mid-load (abandon). `Some(Ok)` =
    /// the chunk's built `Element`. `Some(Err)` = the load failed with a
    /// message (drives the `.error(..)` UI).
    type Output = Option<Result<Element, String>>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        use std::task::Poll;
        // `LazyFuture` is `Pin<Box<..>>` (Unpin), and `scope` is an `Rc`, so
        // `Self: Unpin` and a plain `&mut Self` projection is sound — there is
        // no structurally-pinned field to preserve.
        let this = self.get_mut();
        let mut slot = this.scope.borrow_mut();
        let Some(scope) = slot.as_mut() else {
            // Surrounding scope torn down mid-load; abandon the chunk.
            return Poll::Ready(None);
        };
        // The borrow is held only for this synchronous poll — released when it
        // returns, so a teardown that races an in-flight fetch can take the
        // scope between polls (next poll then sees `None` and bails).
        // Re-establish the ambient nav context for the poll so links the
        // chunk constructs capture the screen's navigator, not `None`.
        let _nav_restore = this.nav_ctx.enter();
        reactive::with_scope(scope.as_mut(), || match this.inner.as_mut().poll(cx) {
            Poll::Ready(result) => Poll::Ready(Some(result)),
            Poll::Pending => Poll::Pending,
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    loader: LazyLoader,
    on_state: Option<Rc<dyn Fn(LazyState)>>,
    placeholder: Option<Box<dyn Fn() -> Element>>,
    error: Option<Rc<dyn Fn(&crate::primitives::lazy::LazyError) -> Element>>,
    style: Option<StyleSource>,
    _ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    // Container view that hosts one of three states at a time: the loading UI
    // (placeholder), the chunk's content on success, or the error UI on
    // failure. Only the container's children swap between states.
    let n = time_backend_create(pkind!(Lazy), || backend.borrow_mut().create_view(&a11y));

    if let Some(s) = style {
        attach_style(backend, &n, s);
    }

    // `Box<dyn Fn>` → `Rc<dyn Fn>` so the loading UI can be re-mounted on every
    // retry, not just the initial paint.
    let placeholder: Option<Rc<dyn Fn() -> Element>> = placeholder.map(Rc::from);
    // The loader is re-invoked on retry, so it lives behind an `Rc`.
    let loader: Rc<LazyLoader> = Rc::new(loader);

    // `show_loading` (re)mounts the loading UI. `clear` is `false` for the
    // initial paint — the container is freshly empty, so clearing would emit a
    // needless `clear_children` (and break the SSR contract that the untouched
    // placeholder is the server's final output). `retry` passes `true` to evict
    // the error UI first.
    let show_loading: Rc<dyn Fn(bool)> = {
        let backend = backend.clone();
        let container = n.clone();
        let on_state = on_state.clone();
        let placeholder = placeholder.clone();
        Rc::new(move |clear: bool| {
            if clear {
                backend.borrow_mut().clear_children(&container);
            }
            // Fire Loading so author UI sees a consistent first event whether
            // the loader is async (web) or resolves on first poll (native).
            if let Some(cb) = on_state.as_ref() {
                cb(LazyState::Loading);
            }
            if let Some(build) = placeholder.as_ref() {
                let child_node = super::build_inner(&backend, build());
                backend.borrow_mut().insert(&mut container.clone(), child_node);
            }
        })
    };

    // Paint the loading state now — this is also the server's output on SSR,
    // where we never spawn the loader (see `renders_lazy_chunks()` below).
    show_loading(false);

    // Track the chunk's mounted node so we can release it on scope
    // drop — the surrounding `Effect` adopts the slot's RAII via
    // capture, so when the parent scope drops, the chunk's backend
    // node releases through the standard cleanup path.
    let chunk_node: Rc<RefCell<Option<B::Node>>> = Rc::new(RefCell::new(None));

    // The chunk's reactive state (Switch/When/Graphics cleanup
    // effects, signals etc.) needs a scope to live in — otherwise
    // every Effect::new called while walking the chunk's primitive
    // tree has `owns = true`, drops immediately at the end of the
    // building function, and cascades a teardown of anything it
    // owned (the canonical symptom: a Graphics primitive inside the
    // chunk gets created, its cleanup Effect is rootless, drop runs
    // before the canvas's first rAF, the rAF then bails because the
    // instance is already released → blank canvas).
    //
    // We synthesize a scope here, run BOTH the loader future and
    // `build_inner` inside it (see `ScopedLoad`), and tie its lifetime
    // to `_cleanup_effect` below so the surrounding scope's drop tears
    // the chunk down at the right moment. Scoping the loader — not just
    // the build — matters because a chunk constructs its `Element`
    // eagerly inside that future: an `Element::External` extension that
    // allocates signals in its constructor (a whiteboard `CoreCanvas::new()`
    // building per-canvas state), or a component that calls `signal()`
    // at construction time, does so during the future's polls, before
    // `build_inner` ever runs. Without the loader being scoped those
    // creations are unowned and leak until the thread exits (dev builds
    // warn: "signal created outside any reactive scope").
    //
    // The bug only bites lazy/wasm-split because the non-lazy walker
    // path is always already inside a host scope (app root or
    // `when`/`switch` branch) for the whole construct+build sequence;
    // spawn_async's body is run as a fresh JS task with no active scope.
    let chunk_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(Some(Box::new(reactive::Scope::new()))));

    // Set when the surrounding scope tears down (see `LazyCancelGuard`).
    // The async continuation reads it after its await to abandon a load
    // that resolved after the parent unmounted.
    let cancelled: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // Capture the ambient navigator context ONCE, synchronously, while the
    // screen's `AmbientNavGuard`/`ScreenStateGuard`/`ScreenRouteGuard` are
    // still on the stack. The chunk's `Element` is constructed AND built
    // inside an async task after the chunk fetch — long after the screen
    // build returned and its guards dropped — so without re-establishing
    // this context every `link` inside the chunk captures `None` and
    // silently no-ops on activation (the href still renders; clicks do
    // nothing). Same pattern as `when_switch`/`each`/`dynamic`. Weak nav
    // ref inside — see `AmbientNavContext`.
    let nav_ctx = crate::primitives::navigator::shared::capture_ambient_nav_context();

    // `retry_holder` owns the `retry` closure for the element's lifetime (the
    // `_cleanup_effect` below adopts it). The error UI is handed a `retry`
    // handle that reaches this slot through a *weak* ref, so the cycle
    // `run_load → retry → run_load` can't leak: on teardown the effect drops
    // the holder, the strong chain unwinds, and any live weak retry no-ops.
    let retry_holder: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));

    // Drive the loader inside an async task, painting the Ready state on
    // success and the Error state on failure. Re-callable: `retry` invokes it
    // again after resetting to the loading UI.
    //
    // Requires the `async-driver` feature on runtime-core. Without it the chunk
    // never loads (loading UI stays visible) — Lazy is async-by-nature, so we
    // don't pretend to support a non-async build cleanly. The wrapper template
    // enables async-driver unconditionally; this gate is purely so the
    // framework itself still compiles in minimal configurations.
    //
    // SSR (headless) keeps the loading UI: a one-shot server render can't paint
    // lazy content (GPU canvas, etc.), and resolving the loader synchronously
    // would make the server HTML diverge from the client's placeholder, which
    // hydration must then remount. `renders_lazy_chunks()` is `false` only on
    // SSR, so skipping the spawn leaves `show_loading()`'s output as the
    // server's HTML; the live client hydrates it and then loads the real chunk.
    #[cfg(feature = "async-driver")]
    if backend.borrow().renders_lazy_chunks() {
        // A retry handle that reaches `retry_holder` weakly — no strong
        // back-edge into `run_load`, so nothing leaks. No-ops if the element
        // has torn down (holder gone) — mirrors the `cancelled` guard.
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

        // Spawns one load attempt. Does NOT reset to loading — the initial call
        // runs right after `show_loading()` above, and `retry` resets first.
        let run_load: Rc<dyn Fn()> = {
            let backend = backend.clone();
            let container = n.clone();
            let chunk_slot = chunk_node.clone();
            let chunk_scope = chunk_scope.clone();
            let cancelled = cancelled.clone();
            let on_state = on_state.clone();
            let error = error.clone();
            let loader = loader.clone();
            let retry_weak = retry_weak.clone();
            let nav_ctx = nav_ctx.clone();
            Rc::new(move || {
                if cancelled.get() {
                    return;
                }
                let backend = backend.clone();
                let container = container.clone();
                let chunk_slot = chunk_slot.clone();
                let chunk_scope = chunk_scope.clone();
                let cancelled = cancelled.clone();
                let on_state = on_state.clone();
                let error = error.clone();
                let retry_weak = retry_weak.clone();
                let nav_ctx = nav_ctx.clone();
                let fut = (loader)();
                crate::driver::spawn_async(async move {
                    // Poll the loader inside the chunk scope so state the chunk
                    // constructs eagerly (External-extension signals, a
                    // component's `signal()` at construction) is owned by the
                    // chunk, not leaked. `None` = torn down mid-load; abandon.
                    let result = match (ScopedLoad {
                        scope: chunk_scope.clone(),
                        nav_ctx: nav_ctx.clone(),
                        inner: fut,
                    })
                    .await
                    {
                        Some(r) => r,
                        None => return,
                    };
                    // The surrounding scope may also have torn down between the
                    // last poll and here (web: a real async fetch). Bail before
                    // touching the detached container.
                    if cancelled.get() {
                        return;
                    }
                    match result {
                        Ok(chunk_primitive) => {
                            // Build the chunk's content under the chunk scope so
                            // its reactive state (and cleanups) are owned there,
                            // with the ambient nav context restored so links
                            // built here (and captured by nested `when`/`switch`
                            // snapshots) keep the screen's navigator.
                            let child_node = {
                                let mut sb = chunk_scope.borrow_mut();
                                let Some(scope) = sb.as_mut() else { return };
                                let _nav_restore = nav_ctx.enter();
                                reactive::with_scope(scope.as_mut(), || {
                                    super::build_inner(&backend, chunk_primitive)
                                })
                            };
                            swap_child(&backend, &container, &chunk_slot, child_node);
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
                                    // The error UI is simple, author-owned chrome
                                    // (text + a retry button); build it OUTSIDE
                                    // the chunk scope like the placeholder, so
                                    // repeated fail→retry cycles don't accumulate
                                    // state in the chunk scope. `retry` reaches
                                    // back here weakly.
                                    let err = crate::primitives::lazy::LazyError::__new(
                                        message,
                                        retry_weak.clone(),
                                    );
                                    let err_node = super::build_inner(&backend, build_err(&err));
                                    swap_child(&backend, &container, &chunk_slot, err_node);
                                }
                                None => {
                                    crate::logging::log(
                                        crate::logging::LogLevel::Error,
                                        &format!(
                                            "[idealyst] lazy chunk failed to load: {message} \
                                             (no .on_error handler — the loading UI stays visible)"
                                        ),
                                    );
                                }
                            }
                        }
                    }
                });
            })
        };

        // `retry` = reset to loading, then load again. Stored in the holder so
        // the weak handle handed to the error UI resolves to it.
        let retry: Rc<dyn Fn()> = {
            let show_loading = show_loading.clone();
            let run_load = run_load.clone();
            Rc::new(move || {
                show_loading(true);
                run_load();
            })
        };
        *retry_holder.borrow_mut() = Some(retry);

        // Kick off the initial load (loading UI is already painted above).
        run_load();
    }
    #[cfg(not(feature = "async-driver"))]
    {
        // Suppress unused warnings; the loader is dropped (chunk
        // never loads) and Rendered is never fired.
        let _ = (&loader, &chunk_node, &chunk_scope, &cancelled, &error, &retry_holder, &nav_ctx);
    }

    // Hold the chunk_node slot, the cancel guard, and the retry holder for
    // cleanup-on-surrounding-scope-drop. When the surrounding scope drops, this
    // Effect's slot is freed, its closure drops, and with it:
    //   - the `LazyCancelGuard` — cancels any in-flight load and tears the
    //     chunk's `Scope` down (running every cleanup the chunk registered,
    //     e.g. `release_graphics`);
    //   - `chunk_node` — releases the chunk's backend node through the standard
    //     path;
    //   - `retry_holder` — drops the strong `run_load` chain, so a lingering
    //     weak retry handed to the error UI upgrades to nothing and no-ops.
    let cancel_guard = LazyCancelGuard { cancelled, chunk_scope };
    let _cleanup_effect = crate::reactive::Effect::new(move || {
        let _ = &chunk_node;
        let _ = &cancel_guard;
        let _ = &retry_holder;
    });

    n
}

/// Swap the container's single child to `child`: clear whatever state UI is
/// currently mounted (loading / previous), insert the new node, and record it
/// as the chunk's mounted node for release on teardown.
#[cfg(feature = "async-driver")]
fn swap_child<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    container: &B::Node,
    chunk_slot: &Rc<RefCell<Option<B::Node>>>,
    child: B::Node,
) {
    {
        let mut be = backend.borrow_mut();
        be.clear_children(container);
    }
    {
        let mut be = backend.borrow_mut();
        be.insert(&mut container.clone(), child.clone());
    }
    *chunk_slot.borrow_mut() = Some(child);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: when the surrounding scope tears down, the cancel guard
    /// (dropped with the cleanup effect's closure) must both flip the
    /// cancellation flag and drop the chunk scope. The async continuation
    /// reads that flag after its await to abandon a late-resolving load
    /// instead of building into the orphaned scope / detached container.
    ///
    /// A full end-to-end async-teardown test would need a backend, an
    /// installed async executor, and a manually-resolved future to
    /// deterministically interleave teardown with resolution — none of
    /// which are reachable at this layer. This exercises the exact drop
    /// mechanism the fix relies on.
    /// Regression: a chunk that allocates reactive state eagerly during
    /// construction (the whiteboard `CoreCanvas::new()` shape — signals made
    /// in an `Element::External` constructor) must have that state owned by
    /// the chunk scope, not leaked. The signal is created inside the loader
    /// future, so scoping only `build_inner` (the pre-fix behavior) left it
    /// unowned: dev builds warned and the slot leaked until thread exit.
    ///
    /// We poll `ScopedLoad` — the adapter that runs each loader poll under the
    /// chunk scope — over a loader that creates a signal holding a drop-flag,
    /// then drop the scope and assert the flag fired. An unowned signal's slot
    /// would never be freed, so its value would never drop.
    #[test]
    fn loader_signals_are_owned_by_chunk_scope() {
        use crate::builder::IntoElement;
        use std::cell::Cell;
        use std::future::Future;
        use std::task::{Context, Poll};

        struct DropFlag(Rc<Cell<bool>>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let dropped = Rc::new(Cell::new(false));
        let dropped_for_loader = dropped.clone();

        let scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
            Rc::new(RefCell::new(Some(Box::new(reactive::Scope::new()))));

        // A loader that eagerly allocates a signal as it constructs its
        // `Element` — the same moment `CoreCanvas::new()` runs in a real chunk.
        let inner: crate::primitives::lazy::LazyFuture = Box::pin(async move {
            let _sig = crate::reactive::Signal::new(Rc::new(DropFlag(dropped_for_loader)));
            Ok(crate::view(Vec::new()).into_element())
        });

        let mut fut = ScopedLoad { scope: scope.clone(), nav_ctx: Default::default(), inner };
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        match std::pin::Pin::new(&mut fut).poll(&mut cx) {
            Poll::Ready(Some(Ok(_))) => {}
            _ => panic!("test loader resolves to an Element on first poll"),
        }

        assert!(!dropped.get(), "signal value stays alive while the chunk scope is live");
        // Tear the chunk scope down (what surrounding-scope drop does).
        scope.borrow_mut().take();
        assert!(
            dropped.get(),
            "chunk scope must own+free signals the loader created during construction"
        );
    }

    /// Regression: a `link` constructed inside a lazy chunk must capture the
    /// screen's navigator. The chunk fn runs inside the loader future, in an
    /// async task that resolves AFTER the screen build returned (its
    /// `AmbientNavGuard` long dropped) — so `ScopedLoad` must re-establish
    /// the nav context captured at mount around every poll. Against the buggy
    /// code the chunk saw `ambient_navigator() == None` and every link it
    /// built silently no-op'd on activation (while still rendering its href).
    #[cfg(feature = "prim-navigator")]
    #[test]
    fn scoped_load_restores_ambient_nav_for_chunk_construction() {
        use crate::builder::IntoElement;
        use crate::primitives::navigator::shared::{
            ambient_navigator, capture_ambient_nav_context, AmbientNavGuard,
        };
        use crate::primitives::navigator::NavigatorControl;
        use std::cell::Cell;
        use std::future::Future;
        use std::task::{Context, Poll};

        // Capture with a navigator on the stack — what `build()` does at
        // mount, while the screen's guards are still live.
        let control = Rc::new(NavigatorControl::new());
        let guard = AmbientNavGuard::push(control.clone());
        let nav_ctx = capture_ambient_nav_context();
        drop(guard);
        assert!(
            ambient_navigator().is_none(),
            "precondition: the load resolves with no guard on the stack",
        );

        // The chunk fn observes the ambient navigator as it constructs its
        // element — exactly where `link()` reads it.
        let saw_nav = Rc::new(Cell::new(false));
        let saw_nav_in_chunk = saw_nav.clone();
        let inner: crate::primitives::lazy::LazyFuture = Box::pin(async move {
            saw_nav_in_chunk.set(ambient_navigator().is_some());
            Ok(crate::view(Vec::new()).into_element())
        });

        let scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
            Rc::new(RefCell::new(Some(Box::new(reactive::Scope::new()))));
        let mut fut = ScopedLoad { scope, nav_ctx, inner };
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        match std::pin::Pin::new(&mut fut).poll(&mut cx) {
            Poll::Ready(Some(Ok(_))) => {}
            _ => panic!("test loader resolves to an Element on first poll"),
        }

        assert!(
            saw_nav.get(),
            "chunk construction must see the mount-time ambient navigator",
        );
        // And the guard must have popped when the poll returned.
        assert!(
            ambient_navigator().is_none(),
            "the restored context must not leak past the poll",
        );
    }

    #[test]
    fn cancel_guard_cancels_and_drops_scope_on_teardown() {
        let cancelled: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let chunk_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
            Rc::new(RefCell::new(Some(Box::new(reactive::Scope::new()))));

        {
            let _guard = LazyCancelGuard {
                cancelled: cancelled.clone(),
                chunk_scope: chunk_scope.clone(),
            };
            assert!(!cancelled.get(), "not cancelled while the guard is live");
            assert!(chunk_scope.borrow().is_some(), "chunk scope live while the guard is live");
        }

        assert!(cancelled.get(), "teardown must cancel any in-flight load");
        assert!(
            chunk_scope.borrow().is_none(),
            "teardown must drop the chunk scope so its cleanups run at the right moment"
        );
    }
}
