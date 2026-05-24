//! Theme cohort: the shared subscription that re-applies every
//! static-styled node when the active theme changes.
//!
//! The cohort exists so the walker can avoid allocating a dedicated
//! per-node Effect for static styles. Instead, each `attach_style_static`
//! registers a closure here, and one framework-installed driver Effect
//! iterates the slab on every theme/token change. That collapses
//! 10k arena slots + 10k closure boxes (for a 10k-row scoreboard) into
//! a single Effect + one slab.
//!
//! Items defined here:
//! - [`CohortId`] / [`theme_cohort_register`] / [`theme_cohort_unregister`]
//!   — registry surface used by both the per-node and bulk static paths.
//! - [`install_theme_cohort_driver`] — installs the one driver Effect
//!   lazily on the first register call.
//! - [`set_backend_cascade_tokens`] — used by `mount` to stash the
//!   active backend's "tokens propagate via CSS cascade" capability,
//!   read by the driver to decide whether the per-node fan-out is even
//!   needed on a token-only update.

use crate::backend::Backend;
use crate::reactive::Effect;
use std::cell::RefCell;
use std::rc::Rc;

/// Opaque id for a cohort entry. Returned by
/// [`theme_cohort_register`] and consumed by
/// [`theme_cohort_unregister`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct CohortId(u32);

/// One entry in the theme cohort. The framework doesn't know how to
/// re-apply on its own — backends are type-erased. So each entry
/// carries the typed re-apply closure inside, and the cohort just
/// iterates and calls them.
///
/// The closure captures everything it needs (backend, node, app),
/// so dropping the entry tears down those captures. A 10 000-row
/// cohort holds 10 000 closures — but each is small (Rc clones +
/// one Node clone + one `StyleApplication` clone) and we never
/// allocate `Effect` slots / arena entries for them.
struct CohortEntry {
    reapply: Box<dyn Fn()>,
}

thread_local! {
    /// Theme cohort: every static-style-attached node lives in this
    /// dense slab. A single framework-installed Effect subscribes
    /// to the active theme and iterates the slab on every fire,
    /// calling each entry's `reapply` closure. So we pay one Effect
    /// for the whole app instead of one per styled node.
    ///
    /// Layout: `Vec<Option<CohortEntry>>` indexed by the `CohortId`'s
    /// inner `u32`. Freed slots become `None` and their ids go on
    /// the freelist. Same shape as the reactive arena's signal /
    /// effect storage — and chosen for the same reason: a HashMap
    /// keyed by the same `u32` paid a ~30 ms hashing cost during a
    /// 10k-row mount that the slab avoids entirely.
    static THEME_COHORT: RefCell<Vec<Option<CohortEntry>>> = const { RefCell::new(Vec::new()) };

    /// Recycled slot ids. Popped on register, pushed on unregister.
    /// Without this, monotonic ids would grow per rebuild and the
    /// `Vec<Option<_>>` would balloon with None slots over time —
    /// same issue we fixed in the reactive arena.
    static THEME_COHORT_FREE: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };

    /// Has the cohort driver effect been installed? Set on first
    /// register; never cleared. The effect lives in the root
    /// `Owner`'s scope and is dropped when that scope drops.
    static THEME_COHORT_DRIVER_INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };

    /// Mirror of `Backend::token_updates_propagate_via_cascade()`,
    /// stashed here so the cohort driver Effect can read it without
    /// holding a backend reference. Set by `mount(...)` based on
    /// the active backend's capability. Read in the driver to
    /// decide whether the per-cohort-entry fan-out is even needed
    /// on token signal change.
    ///
    /// Cleared when the driver's RAII guard fires (so a subsequent
    /// `render(...)` with a different backend doesn't see stale
    /// state).
    static BACKEND_CASCADE_TOKENS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Stash the active backend's "tokens propagate via CSS cascade"
/// capability. Read by the cohort driver to short-circuit the
/// per-node fan-out on token-only updates.
pub(super) fn set_backend_cascade_tokens(value: bool) {
    BACKEND_CASCADE_TOKENS.with(|c| c.set(value));
}

pub(super) fn theme_cohort_register(reapply: Box<dyn Fn()>) -> CohortId {
    let entry = CohortEntry { reapply };
    let id = THEME_COHORT.with(|slab| {
        let mut slab = slab.borrow_mut();
        if let Some(idx) = THEME_COHORT_FREE.with(|f| f.borrow_mut().pop()) {
            slab[idx as usize] = Some(entry);
            idx
        } else {
            let idx = slab.len() as u32;
            slab.push(Some(entry));
            idx
        }
    });
    CohortId(id)
}

pub(super) fn theme_cohort_unregister(id: CohortId) {
    THEME_COHORT.with(|slab| {
        if let Some(slot) = slab.borrow_mut().get_mut(id.0 as usize) {
            if slot.take().is_some() {
                THEME_COHORT_FREE.with(|f| f.borrow_mut().push(id.0));
            }
        }
    });
}

/// Install (idempotently) the cohort driver effect: subscribes to
/// the active theme signal and re-applies every cohort entry when
/// the theme changes. Created lazily on the first
/// `theme_cohort_register` call so we only pay for it when the
/// static-style path is actually used.
///
/// The driver registers with the currently-active `Scope` (the
/// root `Owner`'s scope at first call). When that scope drops, the
/// driver effect drops and we clear the flag so a subsequent
/// render reinstalls. The cohort map itself is also cleared on
/// driver drop — its entries' `reapply` closures captured Rcs to
/// the old backend, which is gone.
pub(super) fn install_theme_cohort_driver<B: Backend + 'static>(backend: &Rc<RefCell<B>>) {
    if THEME_COHORT_DRIVER_INSTALLED.with(|c| c.get()) {
        return;
    }
    THEME_COHORT_DRIVER_INSTALLED.with(|c| c.set(true));

    // RAII guard captured by the driver closure. On drop (scope
    // teardown), clears the installed flag and drops every cohort
    // entry. Putting the cleanup on a captured guard rather than a
    // separate cleanup effect avoids ordering hazards.
    struct DriverGuard;
    impl Drop for DriverGuard {
        fn drop(&mut self) {
            THEME_COHORT_DRIVER_INSTALLED.with(|c| c.set(false));
            THEME_COHORT.with(|m| m.borrow_mut().clear());
            THEME_COHORT_FREE.with(|f| f.borrow_mut().clear());
            // Reset the cascade flag so a follow-up `render(...)`
            // with a different backend doesn't inherit stale state.
            BACKEND_CASCADE_TOKENS.with(|c| c.set(false));
        }
    }
    let _guard = DriverGuard;

    // Capture the backend Rc so the driver can push token updates
    // even when the per-cohort-entry fan-out is short-circuited.
    let backend_for_tokens = backend.clone();

    let _e = Effect::new(move || {
        // Anchor the guard inside the effect closure so it lives
        // exactly as long as the effect.
        let _ = &_guard;
        // Subscribe to every currently-registered token signal so
        // a later `update_tokens` call (theme swap) re-fires this
        // driver Effect even if the slab below is still empty at
        // first-run time. Without this the first iteration touches
        // no signals — the cohort is empty before any entry has
        // registered — and the Effect goes idle forever.
        //
        // Subsequent re-runs ALSO touch each entry's tokens via the
        // resolve calls in `apply_one`, which keeps per-token
        // subscriptions fresh as the cohort grows.
        crate::style::subscribe_to_all_token_signals();

        // Flush pending token updates to the backend BEFORE deciding
        // whether to fan out. The normal flush path lives inside
        // `ensure_registered_with` (called by `apply_one`) — but if
        // the cohort fan-out short-circuits, no `apply_one` runs and
        // the backend would never see the new `:root` variable
        // values. Flushing here covers both code paths: cohort-skip
        // backends get the variables in, fan-out backends find an
        // empty queue when `apply_one` runs (idempotent).
        let pending: Vec<Vec<crate::TokenEntry>> = crate::style::take_pending_token_updates();
        if !pending.is_empty() {
            let mut b = backend_for_tokens.borrow_mut();
            for upd in &pending {
                b.update_tokens(upd);
            }
        }

        // Fast-path: backends that propagate token-value updates
        // via cascade (web's `var(--token)` references on `:root`)
        // don't need a per-node fan-out — the browser handles the
        // visible change. Skipping saves O(N) work per theme swap.
        // For 10k rows that's the difference between ~100 ms and
        // ~1 ms of theme-apply cost.
        //
        // Note: this stays correct only for stylesheets whose
        // resolved CSS is theme-stable (every `Tokenized<T>`
        // emits as `var()` on web). `Derived<T>` closures that
        // produce concrete values from token VALUES would need
        // the rule body to re-emit and aren't covered. The
        // backend declares the capability; the framework just
        // honors it.
        if BACKEND_CASCADE_TOKENS.with(|c| c.get()) {
            return;
        }

        // Trade-off: the static cohort path is intentionally
        // coarser than per-node Effects — all entries reapply when
        // any of their union-of-tokens changes. The reactive style
        // path (each node owns its Effect) gets true per-node
        // isolation. The cohort exists for the 10k-rows scoreboard
        // memory profile; if you need finer-grained reactivity,
        // use a reactive style source instead.
        //
        // Iterate the slab under a single immutable borrow. Skip
        // empty slots. The `reapply` closure does DOM/backend work
        // only — never touches the cohort slab — so the long
        // borrow is safe.
        THEME_COHORT.with(|slab| {
            for entry in slab.borrow().iter().flatten() {
                (entry.reapply)();
            }
        });
    });
    let _ = _e;
}
