//! Ready-made [`Merge`](crate::Merge) policies — the "sensible defaults" so
//! the common conflict-resolution strategies are one-liners instead of
//! hand-written `merge` bodies. Pick one per entity, or write your own.
//!
//! ```ignore
//! impl Merge for Task {
//!     fn merge(ctx: MergeCtx<Self>) -> Resolution<Self> {
//!         // Last edit (by the app's own timestamp) wins:
//!         sync::policy::last_write_wins(ctx, |t| t.updated_at)
//!         // ...or one of:
//!         // sync::policy::server_wins(ctx)   // server is authoritative
//!         // sync::policy::client_wins(ctx)   // local always re-pushed
//!         // sync::policy::manual(ctx)        // block → app calls resolve()
//!     }
//! }
//! ```
//!
//! Each policy handles the delete cases (any of base / local / incoming may
//! be `None`) — see the per-function docs.

use crate::merge::{MergeCtx, Resolution};

/// **Server-authoritative**: always accept the server's state, discarding
/// the local edit on a conflict (including accepting a server-side delete).
/// No silent data *creation* — the local edit is dropped, not merged.
pub fn server_wins<T>(_ctx: MergeCtx<'_, T>) -> Resolution<T> {
    Resolution::TakeIncoming
}

/// **Client-authoritative**: always keep the local edit and re-push it over
/// the server's value (including re-pushing a local delete). The last client
/// to flush wins; use with care — it overwrites concurrent server changes.
pub fn client_wins<T>(_ctx: MergeCtx<'_, T>) -> Resolution<T> {
    Resolution::TakeLocal
}

/// **Manual**: never resolve automatically — mark the record
/// [`Conflicted`](crate::SyncState::Conflicted) and surface it to the app,
/// which presents both sides to the user and calls
/// [`Partition::resolve`](crate::Partition::resolve) with their choice. The
/// partition's outbox stays blocked until then, so nothing is lost or
/// guessed.
pub fn manual<T>(_ctx: MergeCtx<'_, T>) -> Resolution<T> {
    Resolution::Unresolved
}

/// **Last-write-wins** by an app-supplied ordering key (typically a
/// timestamp field on the entity). When both sides hold a value, the one
/// with the **strictly greater** key is kept; an equal key resolves to the
/// **server** so all clients converge on the same value deterministically.
///
/// When either side is a delete (no value to read a key from), this resolves
/// to the server — a delete-aware LWW needs a tombstone timestamp, which is
/// a custom `merge` rather than this generic default.
///
/// Caveat: LWW by a client wall-clock is only as trustworthy as the clients'
/// clocks. For robustness across devices, stamp edits from a server time or
/// a hybrid logical clock; this helper just compares whatever key you point
/// it at.
pub fn last_write_wins<T, K, F>(ctx: MergeCtx<'_, T>, key: F) -> Resolution<T>
where
    K: Ord,
    F: Fn(&T) -> K,
{
    match (ctx.local, ctx.incoming) {
        (Some(local), Some(incoming)) => {
            if key(local) > key(incoming) {
                Resolution::TakeLocal
            } else {
                Resolution::TakeIncoming
            }
        }
        // A delete on either side has no value to key on → defer to the
        // server for deterministic convergence.
        _ => Resolution::TakeIncoming,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct V {
        ts: u64,
    }

    fn v(ts: u64) -> V {
        V { ts }
    }

    fn ctx<'a>(local: Option<&'a V>, incoming: Option<&'a V>) -> MergeCtx<'a, V> {
        MergeCtx {
            base: None,
            local,
            incoming,
        }
    }

    #[test]
    fn server_wins_always_takes_incoming() {
        assert!(matches!(
            server_wins(ctx(Some(&v(9)), Some(&v(1)))),
            Resolution::TakeIncoming
        ));
        // Server deleted (incoming None) — still accept the server.
        assert!(matches!(
            server_wins(ctx(Some(&v(9)), None)),
            Resolution::TakeIncoming
        ));
    }

    #[test]
    fn client_wins_always_takes_local() {
        assert!(matches!(
            client_wins(ctx(Some(&v(1)), Some(&v(9)))),
            Resolution::TakeLocal
        ));
        // Local delete (local None) — re-push it.
        assert!(matches!(
            client_wins(ctx(None, Some(&v(9)))),
            Resolution::TakeLocal
        ));
    }

    #[test]
    fn manual_is_always_unresolved() {
        assert!(matches!(
            manual(ctx(Some(&v(1)), Some(&v(2)))),
            Resolution::Unresolved
        ));
    }

    #[test]
    fn lww_newer_local_wins() {
        assert!(matches!(
            last_write_wins(ctx(Some(&v(5)), Some(&v(3))), |x| x.ts),
            Resolution::TakeLocal
        ));
    }

    #[test]
    fn lww_newer_incoming_wins() {
        assert!(matches!(
            last_write_wins(ctx(Some(&v(3)), Some(&v(5))), |x| x.ts),
            Resolution::TakeIncoming
        ));
    }

    #[test]
    fn lww_tie_resolves_to_server_for_convergence() {
        assert!(matches!(
            last_write_wins(ctx(Some(&v(7)), Some(&v(7))), |x| x.ts),
            Resolution::TakeIncoming
        ));
    }

    #[test]
    fn lww_delete_on_either_side_defers_to_server() {
        assert!(matches!(
            last_write_wins(ctx(None, Some(&v(9))), |x| x.ts),
            Resolution::TakeIncoming
        ));
        assert!(matches!(
            last_write_wins(ctx(Some(&v(9)), None), |x| x.ts),
            Resolution::TakeIncoming
        ));
    }
}
