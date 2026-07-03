//! Local record model — the unit the cache stores and the sync engine
//! reconciles.
//!
//! A record's relationship to the server is two **orthogonal** axes, not
//! one flat enum — conflating them is the classic offline-sync bug:
//!
//! - [`SyncState`] — where the local copy sits relative to the server.
//! - [`Intent`] — what the outbox still owes the server for this record.
//!
//! plus a [`Presence`] bit (live vs. tombstone). A [`Record`] carries both
//! axes, the current value, and — crucially — the **frozen ancestor**
//! (`base_rev` + `base_value`) the local edit was made on top of. That
//! ancestor is what makes a 3-way merge possible; without it you can only
//! do last-write-wins. See the `merge` module.

use serde::{Deserialize, Serialize};

/// Stable, **client-minted** identity of a record within a partition.
///
/// The app supplies ids when it creates entities (it's authoring them
/// anyway), so the SDK never generates ids and the entire "server mints
/// the id, client must remap a temp id" class of bugs disappears. A UUID,
/// a ULID, or any app-unique string works.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Id(pub String);

impl Id {
    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Id {
    fn from(s: &str) -> Self {
        Id(s.to_string())
    }
}

impl From<String> for Id {
    fn from(s: String) -> Self {
        Id(s)
    }
}

/// Server-assigned revision of a single record. Monotonic per record.
///
/// The SDK only ever compares revisions for ordering — applying an
/// incoming change is guarded by `incoming.rev > stored.rev` so that a
/// crash-replayed or out-of-order page can never overwrite newer data with
/// older (see the engine's apply path). The server maps whatever internal
/// versioning it has (a per-row version column, a logical clock) onto this
/// monotonic `u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Rev(pub u64);

/// Opaque, server-issued partition sync cursor.
///
/// The SDK treats it as a black box: it persists the last cursor the
/// server returned and hands it back on the next `pull`. The server alone
/// interprets it (it typically encodes "highest partition revision the
/// client has seen") and decides whether it can answer with a delta or
/// must fall back to a snapshot. Keeping it opaque is what lets the server
/// evolve its change-tracking without a client release.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cursor(pub String);

/// Where a local record's copy sits relative to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncState {
    /// Matches the server at `base_rev`; nothing owed.
    Synced,
    /// Edited locally on top of `base_rev`; an outbox op is (or will be)
    /// queued to push it.
    Dirty,
    /// The server advanced under a local dirty edit; needs the app's
    /// [`merge`](crate::merge) policy before it can sync again. Until
    /// resolved its partition's outbox is blocked.
    Conflicted,
}

/// What the outbox still owes the server for a record.
///
/// Orthogonal to [`SyncState`]: a record can be `Dirty` with intent
/// `Update`, `Dirty` with intent `Delete`, etc. `None` means the outbox
/// has nothing queued for this record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Intent {
    /// Nothing queued.
    None,
    /// Exists only locally; never acknowledged by the server.
    Create,
    /// Edited on top of a known `base_rev`.
    Update,
    /// Deletion queued.
    Delete,
}

/// Liveness of a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presence {
    /// The record exists.
    Live,
    /// The record is deleted — kept as a tombstone so a pending delete can
    /// replay and so a delete/update conflict can be surfaced.
    Tombstone,
}

/// A cached record: its value plus the bookkeeping the sync engine needs.
///
/// `base_rev` + `base_value` are the **frozen common ancestor** — the
/// server state the current local edit derives from. They are captured the
/// moment a `Synced` record is first edited and held unchanged across
/// further offline edits, so a later 3-way merge has a real ancestor to
/// diff against. On a successful push ack they advance to the server's new
/// revision/value and the record returns to `Synced`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record<T> {
    /// Stable identity within the partition.
    pub id: Id,
    /// Current local value. `None` exactly when [`Presence::Tombstone`].
    pub value: Option<T>,
    /// Server revision this local state derives from. `None` for a record
    /// created locally that the server has never acknowledged.
    pub base_rev: Option<Rev>,
    /// Frozen ancestor value for 3-way merge — the server value the
    /// current local edit was made on top of. `None` when there is no
    /// ancestor (a never-synced local create) or once `Synced`.
    pub base_value: Option<T>,
    /// Relationship to the server.
    pub sync: SyncState,
    /// What the outbox owes for this record.
    pub intent: Intent,
    /// Liveness.
    pub presence: Presence,
}

impl<T> Record<T> {
    /// A freshly-synced record straight from the server (clean, nothing
    /// owed). Used when applying a pull to a record with no local edits.
    pub fn synced(id: Id, value: T, rev: Rev) -> Self {
        Record {
            id,
            value: Some(value),
            base_rev: Some(rev),
            base_value: None,
            sync: SyncState::Synced,
            intent: Intent::None,
            presence: Presence::Live,
        }
    }

    /// A server tombstone (the server told us this id is deleted). Clean —
    /// the deletion is already reflected server-side.
    pub fn server_tombstone(id: Id, rev: Rev) -> Self {
        Record {
            id,
            value: None,
            base_rev: Some(rev),
            base_value: None,
            sync: SyncState::Synced,
            intent: Intent::None,
            presence: Presence::Tombstone,
        }
    }

    /// True if the record has unsynced local work (dirty or conflicted).
    /// The apply path consults this before overwriting from a pull: a
    /// clean record fast-paths to overwrite, a dirty one routes to merge.
    pub fn is_dirty(&self) -> bool {
        matches!(self.sync, SyncState::Dirty | SyncState::Conflicted)
    }
}

/// The sync status of one entry, surfaced to the UI — a coarsened view of
/// [`SyncState`] suitable for a per-item indicator (a badge/dot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EntryStatus {
    /// In sync with the server.
    #[default]
    Synced,
    /// Has a local edit queued to push (or in flight).
    Pending,
    /// The server diverged and the app must resolve it.
    Conflicted,
}

/// A live entry as the UI sees it: the value plus its [`EntryStatus`]. The
/// reactive view returned by [`Partition::entries`](crate::Partition::entries)
/// is a `Vec` of these, so a list can render each item with a sync
/// indicator without the app tracking state itself.
///
/// `Serialize`/`Deserialize` so the web multi-tab coordinator can broadcast
/// the owner tab's entries to follower tabs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry<T> {
    /// The entry's stable id.
    pub id: Id,
    /// The current value.
    pub value: T,
    /// Where this entry sits relative to the server.
    pub status: EntryStatus,
}
