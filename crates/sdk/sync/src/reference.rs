//! A reference implementation of the protocol's **server half**.
//!
//! The app authors the bodies of its `pull`/`push` server fns; this module
//! is what those bodies can delegate to. [`Authority`] is a complete,
//! schema-agnostic, in-memory authoritative store with a monotonic
//! revision, an explicit change-log (so deltas can enumerate exactly what
//! changed, including deletes), idempotency-key dedup, and cursor-expiry →
//! snapshot fallback. It is:
//!
//! - **Directly usable** for small or in-memory server state.
//! - **A precise executable spec** for a database-backed server: the same
//!   four behaviors (monotonic rev, explicit tombstones in the log,
//!   idempotency dedup, expire-old-cursor-to-snapshot) are what a SQL
//!   implementation must reproduce.
//!
//! Gated behind the `reference-server` feature so client builds never
//! compile it. It pulls in no dependencies beyond what the crate already
//! uses.
//!
//! ## What a real DB server must mirror
//!
//! 1. A **monotonic per-partition revision** (a version column or logical
//!    clock), bumped on every write, recorded on each row.
//! 2. An **explicit tombstone** for deletes that survives in the
//!    change-log at least as long as the oldest cursor you'll honor —
//!    otherwise a long-offline client never learns about the delete and
//!    silently resurrects the row.
//! 3. **Idempotency-key dedup**: persist each applied op's key + result so
//!    a lost-ack retry replays the original outcome instead of double-
//!    applying.
//! 4. **Cursor-too-old → snapshot**: if a cursor predates your retained
//!    change-log, answer with a consistent full snapshot, not a partial
//!    delta.

use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::model::{Cursor, Id, Rev};
use crate::protocol::{
    Change, Op, OpKind, OpResult, PullMode, PullResponse, PushResponse,
};

/// One authoritative record: its revision and value (`None` = tombstone).
#[derive(Debug, Clone)]
struct Row<T> {
    rev: Rev,
    /// `Some` = live, `None` = deleted (tombstone).
    value: Option<T>,
}

/// An in-memory authoritative store implementing the server side of the
/// sync protocol for one partition. See the module docs.
pub struct Authority<T> {
    /// Monotonic revision; bumped on every write.
    rev: u64,
    /// Current state, keyed by id (live rows and tombstones alike).
    rows: BTreeMap<Id, Row<T>>,
    /// Change-log: every write in revision order, so a delta can replay
    /// exactly what changed (including tombstones). A real server prunes
    /// this; the horizon below models that.
    log: Vec<(Rev, Id)>,
    /// The oldest revision still represented in `log`. A cursor at or below
    /// this can't be served as a delta (we may have pruned changes it
    /// hasn't seen) → snapshot.
    log_horizon: u64,
    /// Idempotency: applied op key → the result to replay on retry.
    seen: HashMap<String, OpResult<T>>,
}

impl<T> Default for Authority<T> {
    fn default() -> Self {
        Authority {
            rev: 0,
            rows: BTreeMap::new(),
            log: Vec::new(),
            log_horizon: 0,
            seen: HashMap::new(),
        }
    }
}

impl<T: Clone> Authority<T> {
    /// A fresh, empty authority.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a record without going through `push` (test/fixture helper).
    pub fn seed(&mut self, id: impl Into<Id>, value: T) {
        let id = id.into();
        self.rev += 1;
        let rev = Rev(self.rev);
        self.rows.insert(id.clone(), Row { rev, value: Some(value) });
        self.log.push((rev, id));
    }

    /// Prune the change-log up to (and including) `rev`, raising the
    /// horizon. Models a server that retains only a bounded window; cursors
    /// older than the new horizon will be answered with a snapshot. The
    /// rows themselves (including tombstones) are kept.
    pub fn prune_log_through(&mut self, rev: u64) {
        self.log.retain(|(r, _)| r.0 > rev);
        self.log_horizon = self.log_horizon.max(rev);
    }

    fn cursor_now(&self) -> Cursor {
        Cursor(format!("rev:{}", self.rev))
    }

    fn parse_cursor(c: &Cursor) -> Option<u64> {
        c.0.strip_prefix("rev:").and_then(|n| n.parse().ok())
    }

    /// Serve a pull. With no cursor — or one older than the retained log —
    /// returns a consistent [`PullMode::Snapshot`] of all live rows at the
    /// current revision. Otherwise returns a [`PullMode::Delta`] of every
    /// change (upserts and tombstones) after the cursor.
    ///
    /// `limit` is honored for paging; `has_more` signals the client to keep
    /// pulling before committing the new cursor.
    pub fn pull(&self, cursor: Option<&Cursor>, limit: Option<u32>) -> PullResponse<T> {
        let cursor_rev = cursor.and_then(Self::parse_cursor);
        let can_delta = cursor_rev.map(|c| c >= self.log_horizon).unwrap_or(false);

        if !can_delta {
            return self.snapshot_page(limit);
        }
        self.delta_page(cursor_rev.unwrap(), limit)
    }

    fn snapshot_page(&self, limit: Option<u32>) -> PullResponse<T> {
        let live: Vec<(&Id, &Row<T>)> = self
            .rows
            .iter()
            .filter(|(_, r)| r.value.is_some())
            .collect();
        let (page, has_more) = take_page(&live, limit);
        let changes = page
            .iter()
            .map(|(id, r)| Change::Upsert {
                id: (*id).clone(),
                rev: r.rev,
                value: r.value.clone().expect("snapshot only includes live rows"),
            })
            .collect();
        // A snapshot is consistent as of the current revision; the next
        // delta flows from here.
        PullResponse {
            mode: PullMode::Snapshot,
            changes,
            next_cursor: self.cursor_now(),
            has_more,
        }
    }

    fn delta_page(&self, after: u64, limit: Option<u32>) -> PullResponse<T> {
        // Walk the log in revision order, emitting each changed row's
        // current state (an upsert or a tombstone).
        let changed: Vec<(&Id, &Row<T>)> = self
            .log
            .iter()
            .filter(|(rev, _)| rev.0 > after)
            .filter_map(|(_, id)| self.rows.get(id).map(|r| (id, r)))
            .collect();
        let (page, has_more) = take_page(&changed, limit);
        let last_rev = page.last().map(|(_, r)| r.rev).unwrap_or(Rev(after));
        let changes = page
            .iter()
            .map(|(id, r)| match &r.value {
                Some(v) => Change::Upsert {
                    id: (*id).clone(),
                    rev: r.rev,
                    value: v.clone(),
                },
                None => Change::Tombstone {
                    id: (*id).clone(),
                    rev: r.rev,
                },
            })
            .collect();
        PullResponse {
            mode: PullMode::Delta,
            changes,
            next_cursor: Cursor(format!("rev:{}", last_rev.0)),
            has_more,
        }
    }

    /// Apply a batch of ops, returning one positional result per op.
    /// Enforces idempotency (replays a seen key), concurrency (a stale
    /// `base_rev` → [`OpResult::Conflict`] carrying the server's value),
    /// and delete-of-missing → [`OpResult::Gone`].
    pub fn push(&mut self, ops: Vec<Op<T>>) -> PushResponse<T> {
        let mut results = Vec::with_capacity(ops.len());
        for op in ops {
            if let Some(prev) = self.seen.get(&op.idem_key) {
                results.push(as_duplicate(prev));
                continue;
            }
            let result = self.apply_one(&op);
            self.seen.insert(op.idem_key.clone(), result.clone());
            results.push(result);
        }
        PushResponse { results }
    }

    fn apply_one(&mut self, op: &Op<T>) -> OpResult<T> {
        let current = self.rows.get(&op.id).cloned();
        match op.kind {
            OpKind::Create | OpKind::Update => {
                // Concurrency check: the op's base must match what we hold.
                // A live row's rev must equal base_rev; a create expects no
                // live row.
                let live_rev = current.as_ref().and_then(|r| r.value.as_ref().map(|_| r.rev));
                let base_ok = match (op.kind, live_rev) {
                    (OpKind::Create, None) => op.base_rev.is_none(),
                    (OpKind::Update, Some(rev)) => op.base_rev == Some(rev),
                    // Update of a missing/tombstoned row.
                    (OpKind::Update, None) => return OpResult::Gone { id: op.id.clone() },
                    // Create over an existing live row → conflict.
                    (OpKind::Create, Some(_)) => false,
                    _ => false,
                };
                if !base_ok {
                    let r = current.unwrap();
                    return OpResult::Conflict {
                        id: op.id.clone(),
                        server_rev: r.rev,
                        server_value: r.value.expect("conflict only against a live row"),
                    };
                }
                let value = op
                    .value
                    .clone()
                    .expect("create/update op must carry a value");
                self.rev += 1;
                let rev = Rev(self.rev);
                self.rows.insert(op.id.clone(), Row { rev, value: Some(value) });
                self.log.push((rev, op.id.clone()));
                OpResult::Applied {
                    id: op.id.clone(),
                    new_rev: rev,
                }
            }
            OpKind::Delete => {
                let live = current.as_ref().and_then(|r| r.value.as_ref()).is_some();
                if !live {
                    return OpResult::Gone { id: op.id.clone() };
                }
                self.rev += 1;
                let rev = Rev(self.rev);
                self.rows.insert(op.id.clone(), Row { rev, value: None });
                self.log.push((rev, op.id.clone()));
                OpResult::Applied {
                    id: op.id.clone(),
                    new_rev: rev,
                }
            }
        }
    }

    /// The number of live (non-tombstone) rows — handy in tests.
    pub fn live_count(&self) -> usize {
        self.rows.values().filter(|r| r.value.is_some()).count()
    }
}

/// Take a `limit`-sized page off the front of `items`, reporting whether
/// more remain. `None`/`0` means "no limit".
fn take_page<'a, T>(
    items: &'a [(&'a Id, &'a Row<T>)],
    limit: Option<u32>,
) -> (&'a [(&'a Id, &'a Row<T>)], bool) {
    match limit {
        Some(n) if n > 0 && (n as usize) < items.len() => (&items[..n as usize], true),
        _ => (items, false),
    }
}

/// Re-present an applied result as a [`OpResult::Duplicate`] on retry;
/// non-applied outcomes replay verbatim.
fn as_duplicate<T: Clone>(prev: &OpResult<T>) -> OpResult<T> {
    match prev {
        OpResult::Applied { id, new_rev } => OpResult::Duplicate {
            id: id.clone(),
            new_rev: *new_rev,
        },
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Item {
        n: i32,
    }

    fn op(key: &str, id: &str, kind: OpKind, base: Option<u64>, val: Option<i32>) -> Op<Item> {
        Op {
            idem_key: key.into(),
            id: Id::from(id),
            kind,
            base_rev: base.map(Rev),
            value: val.map(|n| Item { n }),
        }
    }

    #[test]
    fn no_cursor_returns_snapshot() {
        let mut a = Authority::new();
        a.seed("x", Item { n: 1 });
        let resp = a.pull(None, None);
        assert_eq!(resp.mode, PullMode::Snapshot);
        assert_eq!(resp.changes.len(), 1);
    }

    #[test]
    fn cursor_returns_only_newer_changes_as_delta() {
        let mut a = Authority::<Item>::new();
        a.push(vec![op("k1", "a", OpKind::Create, None, Some(1))]);
        let after_first = a.pull(None, None).next_cursor;
        a.push(vec![op("k2", "b", OpKind::Create, None, Some(2))]);

        let resp = a.pull(Some(&after_first), None);
        assert_eq!(resp.mode, PullMode::Delta);
        // Only "b" changed after the first cursor.
        assert_eq!(resp.changes.len(), 1);
        assert_eq!(resp.changes[0].id(), &Id::from("b"));
    }

    #[test]
    fn delete_appears_as_tombstone_in_delta() {
        let mut a = Authority::<Item>::new();
        a.push(vec![op("k1", "a", OpKind::Create, None, Some(1))]);
        let c = a.pull(None, None).next_cursor;
        a.push(vec![op("k2", "a", OpKind::Delete, Some(1), None)]);
        let resp = a.pull(Some(&c), None);
        assert!(matches!(resp.changes[0], Change::Tombstone { .. }));
    }

    #[test]
    fn idempotent_key_replays_as_duplicate() {
        let mut a = Authority::<Item>::new();
        let r1 = a.push(vec![op("k1", "a", OpKind::Create, None, Some(1))]);
        let r2 = a.push(vec![op("k1", "a", OpKind::Create, None, Some(1))]);
        assert!(matches!(r1.results[0], OpResult::Applied { .. }));
        assert!(matches!(r2.results[0], OpResult::Duplicate { .. }));
        assert_eq!(a.live_count(), 1, "no double-create");
    }

    #[test]
    fn stale_base_rev_conflicts_with_server_value() {
        let mut a = Authority::<Item>::new();
        a.push(vec![op("k1", "a", OpKind::Create, None, Some(1))]);
        // Server advances to rev 2.
        a.push(vec![op("k2", "a", OpKind::Update, Some(1), Some(2))]);
        // A client still on base_rev 1 tries to update.
        let r = a.push(vec![op("k3", "a", OpKind::Update, Some(1), Some(9))]);
        match &r.results[0] {
            OpResult::Conflict { server_value, .. } => assert_eq!(server_value.n, 2),
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[test]
    fn expired_cursor_falls_back_to_snapshot() {
        let mut a = Authority::<Item>::new();
        a.push(vec![op("k1", "a", OpKind::Create, None, Some(1))]);
        let old = a.pull(None, None).next_cursor; // rev:1
        a.push(vec![op("k2", "b", OpKind::Create, None, Some(2))]);
        // Server prunes its change-log past rev 2 — so it can no longer
        // prove it can enumerate the rev-2 change for a client still at
        // rev 1. A delta would silently miss "b"; snapshot is the safe answer.
        a.prune_log_through(2);
        let resp = a.pull(Some(&old), None);
        assert_eq!(resp.mode, PullMode::Snapshot, "stale cursor → snapshot");
    }

    #[test]
    fn paging_reports_has_more() {
        let mut a = Authority::<Item>::new();
        for i in 0..5 {
            a.seed(format!("id{i}"), Item { n: i });
        }
        let resp = a.pull(None, Some(2));
        assert_eq!(resp.changes.len(), 2);
        assert!(resp.has_more);
    }
}
