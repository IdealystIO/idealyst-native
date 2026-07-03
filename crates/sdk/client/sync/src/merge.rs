//! Pluggable per-entity conflict resolution.
//!
//! When a pull (or a push `Conflict` result) brings a server value for a
//! record the client has edited locally, the engine asks the app's
//! [`Merge`] impl what to do. The protocol carries the three values a
//! correct 3-way merge needs — the third (the common ancestor) being the
//! one people forget:
//!
//! 1. `local` — the current local value.
//! 2. `incoming` — the server's value.
//! 3. `base` — the **ancestor**: the server value the local edit was made
//!    on top of (frozen at edit time in [`Record::base_value`]).
//!
//! Resolution is entirely the app's call. A `Merged(T)` result is itself a
//! *new local edit* and the engine re-queues it as an `Update` against the
//! server's new revision — it does not get silently marked synced.
//!
//! [`Record::base_value`]: crate::model::Record::base_value

/// The app's decision for one conflicting record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution<T> {
    /// Keep the local value; re-push it against the server's new revision.
    TakeLocal,
    /// Discard the local edit; accept the server's value as-is.
    TakeIncoming,
    /// Use this merged value; re-push it against the server's new revision.
    Merged(T),
    /// The app can't resolve it automatically. The record stays
    /// [`Conflicted`](crate::model::SyncState::Conflicted) and its
    /// partition's outbox stays blocked until the app resolves it (e.g.
    /// via UI), keeping the conflict visible rather than guessing.
    Unresolved,
}

/// The inputs to a single merge decision.
///
/// Any of the three may be `None`:
/// - `base = None` — there is no common ancestor (a create/create
///   collision: both sides created the same id independently).
/// - `local = None` — the local side is a delete.
/// - `incoming = None` — the server side is a delete (a tombstone).
///
/// The (`local = None`, `incoming = Some`) and (`local = Some`,
/// `incoming = None`) cases are the delete/update conflicts; the app
/// decides whether the edit resurrects the record or the delete wins.
pub struct MergeCtx<'a, T> {
    /// The frozen common ancestor, if any.
    pub base: Option<&'a T>,
    /// The current local value, if the local side isn't a delete.
    pub local: Option<&'a T>,
    /// The server's value, if the server side isn't a delete.
    pub incoming: Option<&'a T>,
}

/// Per-entity conflict resolution. Implement this for any type stored in a
/// [`Partition`](crate::Partition).
///
/// A reasonable default is "server wins" (`TakeIncoming`), but the whole
/// point of the trait is that the app can do better — field-level merges,
/// resurrect-on-delete-conflict, or surfacing `Unresolved` to a UI. The
/// SDK ships no default impl so the choice is always explicit.
pub trait Merge: Sized {
    /// Resolve one conflict. Called only on genuine divergence (a dirty
    /// local record whose incoming server revision differs from the local
    /// ancestor) — never on clean records, which fast-path overwrite.
    fn merge(ctx: MergeCtx<'_, Self>) -> Resolution<Self>;
}
