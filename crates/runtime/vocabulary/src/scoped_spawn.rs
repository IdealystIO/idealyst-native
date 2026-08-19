//! `spawn_then` — run IO detached, apply its result inside a turn.
//!
//! # The shape this replaces
//!
//! ```ignore
//! spawn_async(async move {
//!     let saved = save_report(id).await;   // ← scope can die here
//!     nav.push(Route::Report(saved.id));
//!     busy.set(false);                     // ← writes a freed slot: abort
//! });
//! ```
//!
//! `spawn_async` is fully detached, and every `.await` is a flush
//! boundary — the host's post-dispatch hook flushes after each future
//! poll, so the world commits, structural drivers run, and scopes are torn
//! down *between two adjacent lines of one async block*. A `Signal<T>` is
//! `Copy` and carries no ownership, so it rides into an `async move` with
//! nothing in the types objecting, and the resumed continuation writes a
//! slot its scope already freed.
//!
//! [`spawn_then`] splits the two halves so the mistake has no place to
//! live:
//!
//! ```ignore
//! spawn_then(
//!     async move { save_report(id).await },   // IO only — captures no signals
//!     move |saved| {                          // sync — runs in a turn, or not at all
//!         nav.push(Route::Report(saved.id));
//!         busy.set(false);
//!     },
//! );
//! ```
//!
//! # The guarantee
//!
//! **The callback runs inside a turn, or not at all.**
//!
//! `then` is `FnOnce`, not a future, so it cannot suspend: it runs to
//! completion inside a single poll segment, and the liveness check sits
//! immediately before it with no await in between. Nothing can tear the
//! scope down partway through. That makes the state update **atomic** —
//! every write lands or none does — which is the one property a per-write
//! guard cannot provide. A guarded continuation writing a global toast and
//! a component-scoped `busy` lands the toast and silently drops the
//! `busy`; this cannot.
//!
//! Reads are covered by the same guarantee, and that matters more than it
//! sounds: a stale *read* can never be made benign (there is no `T` to
//! synthesize), so `let snapshot = data.get();` after an await is fatal
//! under every write policy. Inside `then` it is valid by construction.
//!
//! # The IO still completes
//!
//! Liveness is checked **after** the future resolves, so the request has
//! already gone out and come back — skipping the callback discards a
//! result, never an in-flight write. This is deliberate and not
//! negotiable: cancelling the future instead would abort saves mid-flight
//! and lose user data (`sdk/client/storage`'s write-through and the sync
//! SDK's uploads are the live examples). If work genuinely must be
//! abandoned on teardown, that is a cancellation token inside the future,
//! not this seam.
//!
//! # Anchoring
//!
//! The liveness token comes from [`ScopeAlive::current`] — the same
//! mechanism that guards every author callback crossing the backend seam,
//! so there is one teardown flag in the framework rather than two. Which
//! scope it binds to depends on where the spawn happens, and BOTH cases
//! matter:
//!
//! - **Spawned during a build** (component body, mount walk, any
//!   `collect_owned` region): anchors to the subtree being built, via
//!   `on_owned_drop`. Deliberately not `on_scope_drop`, which degrades to
//!   `on_cleanup` inside a running effect and would flip on that effect's
//!   next re-run, killing tasks belonging to a still-mounted subtree (the
//!   keyed-list case `callbacks_survive_a_keyed_reconcile` pins).
//! - **Spawned from an event handler** (`on_press`, `on_change`, …):
//!   inherits the *handler's own* token. This is the load-bearing case and
//!   the one that is easy to get wrong. A handler runs long after the
//!   build, invoked by the backend as a bare `Rc<dyn Fn()>` with no
//!   ownership collector and outside `World::enter` — so asking for a
//!   fresh anchor there yields a token bound to nothing, permanently
//!   `true`, guarding nothing at all. `ScopeAlive`'s wrappers publish
//!   their token for the duration of the call precisely so a spawn reached
//!   from inside one binds to the node that mounted the handler.
//!   Regression: `handler_spawned_task_dies_with_its_node`.
//!
//! The build case is checked FIRST, so a handler that realizes a subtree
//! (a navigator push) gives that subtree its own lifetime rather than the
//! button's.
//!
//! Outside any world the token is permanently live, so a task spawned from
//! a test or a boot path still applies its result.

use std::future::Future;

use runtime_shared::driver::spawn_async;

use crate::callback_guard::ScopeAlive;

/// Run `task` detached, then apply `then` to its output **inside a turn**
/// — unless the spawning scope was torn down while the task was in
/// flight, in which case `then` never runs.
///
/// Put every signal read and write in `then`; keep `task` to IO. See the
/// module docs for the guarantee and why the future is not cancelled.
///
/// ```ignore
/// spawn_then(
///     async move { save_report(id).await },
///     move |saved| {
///         nav.push(Route::Report(saved.id));
///         busy.set(false);
///     },
/// );
/// ```
pub fn spawn_then<T, F, A>(task: F, then: A)
where
    T: 'static,
    F: Future<Output = T> + 'static,
    A: FnOnce(T) + 'static,
{
    // Taken HERE, in the caller's scope — not inside the async block,
    // where there is no ambient scope to anchor to.
    let alive = ScopeAlive::current();
    spawn_async(async move {
        let value = task.await;
        if alive.get() {
            // Publish the token for the callback's duration, so a task
            // chained from inside `then` inherits this same lifetime
            // rather than anchoring to nothing (nothing is being built
            // here, and no guarded callback is on the stack).
            alive.clone().run_within(|| then(value));
        }
        // Dead scope: `then` drops unrun, releasing its captures.
    });
}
