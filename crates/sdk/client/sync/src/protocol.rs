//! The wire protocol — two messages, generic over the app's entity `T`,
//! schema-agnostic.
//!
//! Everything the SDK does is client-side bookkeeping over exactly two
//! round-trips:
//!
//! - [`pull`](PullRequest) — "give me what changed since this cursor"
//!   (degrading to a full snapshot when the cursor is absent or the server
//!   has pruned the change-log past it).
//! - [`push`](PushRequest) — "apply these queued mutations", each carrying
//!   an idempotency key (safe retries) and a base revision (concurrency
//!   detection).
//!
//! The app authors the server bodies for these (typically via the
//! `sync_entity!` macro, which emits ordinary `#[server]` fns) and wires
//! them to the engine through the [`Transport`] trait. Because the
//! messages are generic over `T`, the framework's existing `x-srv-schema`
//! drift check (HTTP 426) catches incompatible-`T` deploys for free.

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

use crate::error::SyncError;
use crate::model::{Cursor, Id, Rev};

// ---------------------------------------------------------------------------
// pull
// ---------------------------------------------------------------------------

/// Client → server: fetch changes for one partition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequest {
    /// The partition to sync (e.g. `"project:123"`).
    pub partition: String,
    /// The last cursor the server issued, or `None` for the initial pull
    /// (which is inherently a snapshot — there is nothing to delta from).
    pub cursor: Option<Cursor>,
    /// Optional page-size hint. The server may page large responses; the
    /// client applies every page (following [`PullResponse::has_more`])
    /// before committing the new cursor.
    pub limit: Option<u32>,
}

/// Whether a [`PullResponse`] is an incremental delta or a full snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PullMode {
    /// `changes` are the records that changed since the request cursor.
    Delta,
    /// `changes` are the full live set as of a single consistent revision.
    /// The client applies this as **replace-with-reconciliation** (diff
    /// against the local cache, preserving/merging dirty records) — never
    /// a blind clear, which would nuke unsynced offline edits.
    Snapshot,
}

/// One change in a pull response. Deletes are **explicit** tombstones — a
/// record merely *absent* from a delta means "unchanged", never "deleted".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Change<T> {
    /// The record exists with this value at this revision.
    Upsert {
        /// Record identity.
        id: Id,
        /// Server revision of this value.
        rev: Rev,
        /// The value.
        value: T,
    },
    /// The record was deleted at this revision.
    Tombstone {
        /// Record identity.
        id: Id,
        /// Server revision at which it was deleted.
        rev: Rev,
    },
}

impl<T> Change<T> {
    /// The id this change targets.
    pub fn id(&self) -> &Id {
        match self {
            Change::Upsert { id, .. } | Change::Tombstone { id, .. } => id,
        }
    }

    /// The revision this change carries.
    pub fn rev(&self) -> Rev {
        match self {
            Change::Upsert { rev, .. } | Change::Tombstone { rev, .. } => *rev,
        }
    }
}

/// Server → client: the result of a [`PullRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullResponse<T> {
    /// Delta vs. snapshot — selects the client's apply path.
    pub mode: PullMode,
    /// The changes, ordered by ascending revision.
    pub changes: Vec<Change<T>>,
    /// The cursor to send on the next pull. The client persists this
    /// **only after** durably applying every change in every page.
    pub next_cursor: Cursor,
    /// True if more pages remain for this cursor; the client loops until
    /// `false` before committing `next_cursor`.
    pub has_more: bool,
}

// ---------------------------------------------------------------------------
// push
// ---------------------------------------------------------------------------

/// What a queued mutation does to a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpKind {
    /// Create a record the server has never seen (`base_rev` is `None`).
    Create,
    /// Update an existing record (`base_rev` = the ancestor it was edited
    /// on top of, for concurrency detection).
    Update,
    /// Delete an existing record (`base_rev` = ancestor, so the server can
    /// detect a concurrent change before honoring the delete).
    Delete,
}

/// One queued mutation in a [`PushRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Op<T> {
    /// Stable-across-retries idempotency key. The server dedups on this so
    /// a lost-ack retry is a no-op that replays the original result. It
    /// guards **transport duplication** — distinct from `base_rev`, which
    /// guards concurrency. Both are required.
    pub idem_key: String,
    /// Target record identity (client-minted).
    pub id: Id,
    /// The operation.
    pub kind: OpKind,
    /// Ancestor revision for `Update`/`Delete`; `None` for `Create`.
    pub base_rev: Option<Rev>,
    /// The value for `Create`/`Update`; `None` for `Delete`.
    pub value: Option<T>,
}

/// Client → server: apply a batch of mutations to one partition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushRequest<T> {
    /// The partition these ops target.
    pub partition: String,
    /// The ops, in FIFO order. The server applies them in order and
    /// returns one positional [`OpResult`] per op.
    pub ops: Vec<Op<T>>,
}

/// The outcome of one [`Op`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OpResult<T> {
    /// Applied successfully; advance the record to `new_rev`, clear intent.
    Applied {
        /// Target record.
        id: Id,
        /// The record's new server revision.
        new_rev: Rev,
    },
    /// The `idem_key` was already seen — the server replayed the original
    /// result. Treated identically to [`OpResult::Applied`] by the client.
    Duplicate {
        /// Target record.
        id: Id,
        /// The record's server revision after the original apply.
        new_rev: Rev,
    },
    /// The op's `base_rev` was stale — the server moved independently. The
    /// server returns its current value so the client can run the merge
    /// trait without a second round-trip.
    Conflict {
        /// Target record.
        id: Id,
        /// The server's current revision.
        server_rev: Rev,
        /// The server's current value.
        server_value: T,
    },
    /// The target record no longer exists server-side. For an `Update`
    /// intent this is a delete/update conflict (route to merge); for a
    /// `Delete` intent it is success (the delete already happened).
    Gone {
        /// Target record.
        id: Id,
    },
    /// A domain/validation rejection (rides the server fn's `Err` path).
    /// The op is dropped and the reason surfaced to the app.
    Rejected {
        /// Target record.
        id: Id,
        /// Why the server rejected it.
        reason: String,
    },
}

impl<T> OpResult<T> {
    /// The id this result targets.
    pub fn id(&self) -> &Id {
        match self {
            OpResult::Applied { id, .. }
            | OpResult::Duplicate { id, .. }
            | OpResult::Conflict { id, .. }
            | OpResult::Gone { id }
            | OpResult::Rejected { id, .. } => id,
        }
    }
}

/// Server → client: positional results for a [`PushRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushResponse<T> {
    /// One result per submitted op, in the same order.
    pub results: Vec<OpResult<T>>,
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// A boxed, non-`Send` future returned by a [`Transport`] call.
///
/// Non-`Send` to match the framework's single-threaded async model
/// (`spawn_async` → `spawn_local` on web, where futures are inherently
/// `!Send`). Mirrors `storage::StorageFuture`'s boxing for the same
/// object-safety reason, minus the `Send` bound.
pub type TransportFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, SyncError>> + 'a>>;

/// The app's bridge from the engine to its server functions.
///
/// The engine is generic over this trait and never knows how the messages
/// reach the server — the app implements `pull`/`push` by calling its own
/// `#[server]` fns (the `sync_entity!` macro generates this impl). This is
/// why the `sync` crate has no dependency on `server`: the protocol
/// *types* are the only shared contract.
pub trait Transport<T> {
    /// Fetch changes for a partition.
    fn pull(&self, req: PullRequest) -> TransportFuture<'_, PullResponse<T>>;
    /// Apply a batch of mutations to a partition.
    fn push(&self, req: PushRequest<T>) -> TransportFuture<'_, PushResponse<T>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative app entity.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Project {
        name: String,
        archived: bool,
    }

    #[test]
    fn pull_response_round_trips() {
        let resp = PullResponse {
            mode: PullMode::Delta,
            changes: vec![
                Change::Upsert {
                    id: Id::from("a"),
                    rev: Rev(7),
                    value: Project {
                        name: "Apollo".into(),
                        archived: false,
                    },
                },
                Change::Tombstone {
                    id: Id::from("b"),
                    rev: Rev(8),
                },
            ],
            next_cursor: Cursor("rev:8".into()),
            has_more: false,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: PullResponse<Project> = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, back);
    }

    #[test]
    fn push_request_and_results_round_trip() {
        let req = PushRequest {
            partition: "project:1".into(),
            ops: vec![Op {
                idem_key: "client9:project:1:42".into(),
                id: Id::from("a"),
                kind: OpKind::Update,
                base_rev: Some(Rev(7)),
                value: Some(Project {
                    name: "Apollo 2".into(),
                    archived: false,
                }),
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: PushRequest<Project> = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);

        let resp = PushResponse {
            results: vec![
                OpResult::Applied {
                    id: Id::from("a"),
                    new_rev: Rev(8),
                },
                OpResult::Conflict {
                    id: Id::from("c"),
                    server_rev: Rev(9),
                    server_value: Project {
                        name: "Server".into(),
                        archived: true,
                    },
                },
                OpResult::Gone { id: Id::from("d") },
            ],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: PushResponse<Project> = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, back);
    }

    #[test]
    fn change_accessors() {
        let up: Change<Project> = Change::Upsert {
            id: Id::from("x"),
            rev: Rev(3),
            value: Project {
                name: "n".into(),
                archived: false,
            },
        };
        assert_eq!(up.id(), &Id::from("x"));
        assert_eq!(up.rev(), Rev(3));
    }
}
