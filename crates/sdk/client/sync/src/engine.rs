//! The per-partition sync state machine — the correctness core.
//!
//! [`PartitionInner`] holds the in-memory state (records, outbox, cursor)
//! and is a **pure, synchronous** state machine: every transition
//! (local edit, pull-apply, push-result handling, conflict resolution)
//! mutates in-memory state without any I/O. That is deliberate — the hard
//! parts (the record lifecycle, merge invocation, idempotent apply,
//! coalescing) are exactly the parts that must be exhaustively tested, and
//! keeping them synchronous makes them testable without a runtime, a
//! backend, or a server.
//!
//! The async layer ([`Partition`](crate::Partition)) is a thin shell that
//! sequences `storage` writes and `Transport` calls *around* these pure
//! transitions, upholding the crash-safety ordering. It never holds a
//! borrow across an `await`.

use std::collections::BTreeMap;
use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::merge::{Merge, MergeCtx, Resolution};
use crate::model::{Cursor, Entry, EntryStatus, Id, Intent, Presence, Record, Rev, SyncState};
use crate::outbox::{coalesce, OutboxOp};
use crate::protocol::{Change, Op, OpKind, OpResult, PullMode};

/// Decode the value an outbox op carries. A `Create`/`Update` op always
/// carries a value; a corrupt or value-less one is a programming error, so
/// this aborts loudly rather than substituting a default (see the repo's
/// crash-loud-on-FFI-panic rule applied to persisted state).
fn decode_op_value<T: DeserializeOwned>(op: &OutboxOp) -> T {
    let json = op
        .value_json
        .as_ref()
        .expect("sync: a create/update op must carry a value");
    serde_json::from_str(json).expect("sync: decoding an op value cannot fail")
}

/// In-memory state for one partition. See the module docs.
pub(crate) struct PartitionInner<T> {
    /// Records keyed by id, kept sorted so `live_values()` is stable.
    records: BTreeMap<Id, Record<T>>,
    /// The pending-mutation queue (FIFO).
    outbox: Vec<OutboxOp>,
    /// The last cursor the server issued, or `None` if never pulled.
    cursor: Option<Cursor>,
    /// Monotonic local op sequence — orders the outbox and derives
    /// idempotency keys.
    next_seq: u64,
    /// Stable per-install id; one half of the idempotency key.
    client_id: String,
    /// This partition's name; the other half of the idempotency key.
    partition: String,
    /// Last domain rejection reason, surfaced to the app.
    last_error: Option<String>,
}

impl<T> PartitionInner<T> {
    pub fn new(
        client_id: String,
        partition: String,
        records: Vec<Record<T>>,
        outbox: Vec<OutboxOp>,
        cursor: Option<Cursor>,
    ) -> Self {
        // Resume the sequence past the highest persisted op so restored
        // ops never collide with freshly-minted ones.
        let next_seq = outbox.iter().map(|o| o.seq + 1).max().unwrap_or(0);
        let records = records.into_iter().map(|r| (r.id.clone(), r)).collect();
        PartitionInner {
            records,
            outbox,
            cursor,
            next_seq,
            client_id,
            partition,
            last_error: None,
        }
    }

    /// The cursor to send on the next pull.
    pub fn cursor(&self) -> Option<Cursor> {
        self.cursor.clone()
    }

    /// Snapshot of the records for persistence.
    pub fn records_vec(&self) -> Vec<Record<T>>
    where
        T: Clone,
    {
        self.records.values().cloned().collect()
    }

    /// Snapshot of the outbox for persistence.
    pub fn outbox_vec(&self) -> Vec<OutboxOp> {
        self.outbox.clone()
    }

    /// The live values, in id order — what the UI binds to.
    pub fn live_values(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.records
            .values()
            .filter(|r| matches!(r.presence, Presence::Live))
            .filter_map(|r| r.value.clone())
            .collect()
    }

    /// The live entries with their per-item sync status, in id order — the
    /// status-aware view the UI binds to for indicators.
    pub fn entry_views(&self) -> Vec<Entry<T>>
    where
        T: Clone,
    {
        self.records
            .values()
            .filter(|r| matches!(r.presence, Presence::Live))
            .filter_map(|r| {
                r.value.clone().map(|value| Entry {
                    id: r.id.clone(),
                    value,
                    status: match r.sync {
                        SyncState::Synced => EntryStatus::Synced,
                        SyncState::Dirty => EntryStatus::Pending,
                        SyncState::Conflicted => EntryStatus::Conflicted,
                    },
                })
            })
            .collect()
    }

    /// True if any record is conflicted — the engine blocks this
    /// partition's outbox until the app resolves it.
    pub fn has_conflict(&self) -> bool {
        self.records
            .values()
            .any(|r| matches!(r.sync, SyncState::Conflicted))
    }

    /// Records currently awaiting the app's resolution.
    pub fn conflicts(&self) -> Vec<Id> {
        self.records
            .values()
            .filter(|r| matches!(r.sync, SyncState::Conflicted))
            .map(|r| r.id.clone())
            .collect()
    }

    /// The last domain-rejection reason, if any.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.clone()
    }

    /// True if the outbox has work to flush.
    pub fn has_pending(&self) -> bool {
        !self.outbox.is_empty()
    }

    fn idem_key(&self, seq: u64) -> String {
        format!("{}:{}:{}", self.client_id, self.partition, seq)
    }

    fn mint_op(
        &mut self,
        id: Id,
        kind: OpKind,
        base_rev: Option<Rev>,
        value: Option<T>,
    ) -> OutboxOp
    where
        T: Serialize,
    {
        let seq = self.next_seq;
        self.next_seq += 1;
        let value_json = value.map(|v| {
            serde_json::to_string(&v).expect("sync: serializing an outbox op value cannot fail")
        });
        OutboxOp::new(seq, self.idem_key(seq), id, kind, base_rev, value_json)
    }

    fn remove_ops_for(&mut self, id: &Id) {
        self.outbox.retain(|o| &o.id != id);
    }

    /// Drop every queued op for `id` and mint a fresh one matching the
    /// record's current `(intent, value, base_rev)`. Used after a merge or
    /// an app resolution turns a record back into pending work.
    fn rebuild_op_for(&mut self, id: &Id)
    where
        T: Serialize + Clone,
    {
        self.remove_ops_for(id);
        let Some(r) = self.records.get(id) else { return };
        let (kind, value) = match r.intent {
            Intent::Create => (OpKind::Create, r.value.clone()),
            Intent::Update => (OpKind::Update, r.value.clone()),
            Intent::Delete => (OpKind::Delete, None),
            Intent::None => return,
        };
        let base_rev = r.base_rev;
        let op = self.mint_op(id.clone(), kind, base_rev, value);
        coalesce(&mut self.outbox, op);
    }
}

impl<T: Clone + Serialize + DeserializeOwned + Merge> PartitionInner<T> {
    // -----------------------------------------------------------------
    // Local mutations
    // -----------------------------------------------------------------

    /// Create or update a record locally. Captures the frozen ancestor on
    /// the first edit of a synced record, queues a (coalesced) outbox op.
    pub fn enqueue_upsert(&mut self, id: Id, value: T) {
        let (base_rev, base_value, intent, op_kind) = match self.records.get(&id) {
            None => (None, None, Intent::Create, OpKind::Create),
            Some(r) => match (r.sync, r.intent) {
                // First edit of a clean record: freeze the ancestor.
                (SyncState::Synced, _) => (
                    r.base_rev,
                    r.value.clone(),
                    Intent::Update,
                    OpKind::Update,
                ),
                // Already a pending create: stays a create.
                (_, Intent::Create) => (None, None, Intent::Create, OpKind::Create),
                // Resurrecting a pending delete.
                (_, Intent::Delete) => {
                    if r.base_rev.is_some() {
                        (r.base_rev, r.base_value.clone(), Intent::Update, OpKind::Update)
                    } else {
                        (None, None, Intent::Create, OpKind::Create)
                    }
                }
                // Further edits of a dirty/conflicted update: keep the
                // frozen ancestor.
                _ => (
                    r.base_rev,
                    r.base_value.clone(),
                    Intent::Update,
                    OpKind::Update,
                ),
            },
        };

        self.records.insert(
            id.clone(),
            Record {
                id: id.clone(),
                value: Some(value.clone()),
                base_rev,
                base_value,
                sync: SyncState::Dirty,
                intent,
                presence: Presence::Live,
            },
        );
        let op = self.mint_op(id, op_kind, base_rev, Some(value));
        coalesce(&mut self.outbox, op);
    }

    /// Delete a record locally.
    pub fn enqueue_delete(&mut self, id: Id) {
        let Some(r) = self.records.get(&id) else {
            return; // deleting an unknown id is a no-op
        };

        // A purely-local create that's now deleted never existed
        // server-side: remove it and let coalescing annihilate the create.
        if matches!(r.intent, Intent::Create) {
            self.records.remove(&id);
            let op = self.mint_op(id, OpKind::Delete, None, None);
            coalesce(&mut self.outbox, op);
            return;
        }

        let base_rev = r.base_rev;
        let base_value = if matches!(r.sync, SyncState::Synced) {
            r.value.clone()
        } else {
            r.base_value.clone()
        };
        self.records.insert(
            id.clone(),
            Record {
                id: id.clone(),
                value: None,
                base_rev,
                base_value,
                sync: SyncState::Dirty,
                intent: Intent::Delete,
                presence: Presence::Tombstone,
            },
        );
        let op = self.mint_op(id, OpKind::Delete, base_rev, None);
        coalesce(&mut self.outbox, op);
    }

    // -----------------------------------------------------------------
    // Pull / apply
    // -----------------------------------------------------------------

    /// Apply a fully-paged pull response to the cache. `changes` is the
    /// concatenation of every page; `mode` selects delta vs. snapshot.
    pub fn apply_pull(&mut self, mode: PullMode, changes: Vec<Change<T>>) {
        let incoming_ids: Vec<Id> = changes.iter().map(|c| c.id().clone()).collect();
        for change in changes {
            self.apply_change(change);
        }
        if matches!(mode, PullMode::Snapshot) {
            self.reconcile_snapshot(&incoming_ids);
        }
    }

    /// Set the cursor after a pull's records are durable (the caller
    /// enforces that ordering).
    pub fn set_cursor(&mut self, cursor: Cursor) {
        self.cursor = Some(cursor);
    }

    fn apply_change(&mut self, change: Change<T>) {
        let id = change.id().clone();
        let rev = change.rev();
        let incoming: Option<T> = match &change {
            Change::Upsert { value, .. } => Some(value.clone()),
            Change::Tombstone { .. } => None,
        };

        let Some(existing) = self.records.get(&id) else {
            // Brand-new from the server.
            self.records.insert(
                id.clone(),
                match incoming {
                    Some(v) => Record::synced(id, v, rev),
                    None => Record::server_tombstone(id, rev),
                },
            );
            return;
        };

        // INV-4: never overwrite newer data with older. A change at or
        // below our ancestor revision is one we already incorporated.
        if let Some(brev) = existing.base_rev {
            if rev <= brev {
                return;
            }
        }

        if existing.is_dirty() {
            // The server moved under a local edit → genuine divergence.
            self.merge_record(id, rev, incoming);
        } else {
            // Clean record: fast-path overwrite.
            self.records.insert(
                id.clone(),
                match incoming {
                    Some(v) => Record::synced(id, v, rev),
                    None => Record::server_tombstone(id, rev),
                },
            );
        }
    }

    /// After every page of a snapshot is applied, remove clean records the
    /// server no longer has, and surface delete/update conflicts for dirty
    /// ones. Local-only creates (never synced) are preserved. This is the
    /// "replace-with-reconciliation" that makes a snapshot safe for
    /// offline edits — never a blind clear.
    fn reconcile_snapshot(&mut self, incoming_ids: &[Id]) {
        let present: std::collections::HashSet<&Id> = incoming_ids.iter().collect();
        let absent: Vec<Id> = self
            .records
            .keys()
            .filter(|id| !present.contains(id))
            .cloned()
            .collect();

        for id in absent {
            let Some(r) = self.records.get(&id) else { continue };
            match (r.base_rev, r.is_dirty()) {
                // Never acknowledged by the server (a pending local
                // create) — keep it; the snapshot simply predates it.
                (None, _) => {}
                // Was synced, no local edits, now gone server-side → the
                // server deleted it; drop it locally.
                (Some(_), false) => {
                    self.records.remove(&id);
                }
                // Was synced and edited locally, now gone server-side →
                // delete/update conflict; ask the app.
                (Some(rev), true) => {
                    self.merge_record(id, rev, None);
                }
            }
        }
    }

    /// Resolve a divergence at `server_rev` between the record's frozen
    /// ancestor + current local value and the server's `incoming` value
    /// (`None` = the server deleted it).
    fn merge_record(&mut self, id: Id, server_rev: Rev, incoming: Option<T>) {
        let resolution = {
            let r = self.records.get(&id).expect("merge_record: record exists");
            T::merge(MergeCtx {
                base: r.base_value.as_ref(),
                local: r.value.as_ref(),
                incoming: incoming.as_ref(),
            })
        };

        match resolution {
            Resolution::TakeLocal => {
                // Keep the local edit, rebase onto the server's new rev so
                // the next push targets current state.
                if let Some(r) = self.records.get_mut(&id) {
                    r.base_rev = Some(server_rev);
                    r.base_value = incoming;
                    r.sync = SyncState::Dirty;
                }
                self.rebuild_op_for(&id);
            }
            Resolution::TakeIncoming => {
                self.remove_ops_for(&id);
                self.records.insert(
                    id.clone(),
                    match incoming {
                        Some(v) => Record::synced(id, v, server_rev),
                        None => Record::server_tombstone(id, server_rev),
                    },
                );
            }
            Resolution::Merged(v) => {
                self.records.insert(
                    id.clone(),
                    Record {
                        id: id.clone(),
                        value: Some(v),
                        base_rev: Some(server_rev),
                        base_value: incoming,
                        sync: SyncState::Dirty,
                        intent: Intent::Update,
                        presence: Presence::Live,
                    },
                );
                self.rebuild_op_for(&id);
            }
            Resolution::Unresolved => {
                // Surface the conflict and block the partition. We anchor
                // the record to the server's state so a later resolve()
                // has a base to act on; the local value is preserved.
                if let Some(r) = self.records.get_mut(&id) {
                    r.sync = SyncState::Conflicted;
                    r.base_rev = Some(server_rev);
                    r.base_value = incoming;
                }
                self.remove_ops_for(&id);
            }
        }
    }

    // -----------------------------------------------------------------
    // Push / flush
    // -----------------------------------------------------------------

    /// Seal the whole queue and build the wire ops to send. Sealing is
    /// **persisted** by the caller before sending: a post-crash coalesce
    /// under an already-delivered idempotency key would otherwise drop the
    /// newer edit, so a sealed op is immutable to coalescing forever.
    pub fn seal_for_push(&mut self) -> Vec<Op<T>> {
        for op in &mut self.outbox {
            op.sealed = true;
        }
        self.outbox
            .iter()
            .map(|o| Op {
                idem_key: o.idem_key.clone(),
                id: o.id.clone(),
                kind: o.kind,
                base_rev: o.base_rev,
                value: o.value_json.as_ref().map(|j| {
                    serde_json::from_str(j).expect("sync: decoding a sealed op value cannot fail")
                }),
            })
            .collect()
    }

    /// Fold a push response (positional to `sent`) back into the cache and
    /// outbox: acked ops drop, conflicts route to merge, rejections block.
    pub fn process_push_results(&mut self, sent: &[Op<T>], results: Vec<OpResult<T>>) {
        let by_key: HashMap<String, OpResult<T>> = sent
            .iter()
            .zip(results)
            .map(|(op, res)| (op.idem_key.clone(), res))
            .collect();

        // Pull the sealed ops out; unsealed ops (queued during the flight)
        // stay untouched.
        let sealed: Vec<OutboxOp> = self.outbox.iter().filter(|o| o.sealed).cloned().collect();
        self.outbox.retain(|o| !o.sealed);

        for sop in sealed {
            match by_key.get(&sop.idem_key) {
                Some(res) => self.apply_result(&sop, res),
                None => {
                    // No result for a sent op (shouldn't happen) — requeue
                    // unsealed so it retries.
                    let mut o = sop;
                    o.sealed = false;
                    self.outbox.push(o);
                }
            }
        }
    }

    fn apply_result(&mut self, sop: &OutboxOp, res: &OpResult<T>) {
        match res {
            OpResult::Applied { new_rev, .. } | OpResult::Duplicate { new_rev, .. } => {
                // A newer local edit may have been queued (unsealed) behind
                // this op while it was in flight. If so, we must NOT clobber
                // it with the acked (older) value — keep it and rebase the
                // pending op onto the now-known server revision.
                let has_pending = self.outbox.iter().any(|o| o.id == sop.id);
                if !has_pending {
                    match sop.kind {
                        OpKind::Delete => {
                            self.records.remove(&sop.id);
                        }
                        OpKind::Create | OpKind::Update => {
                            let value: T = decode_op_value(sop);
                            self.records.insert(
                                sop.id.clone(),
                                Record::synced(sop.id.clone(), value, *new_rev),
                            );
                        }
                    }
                } else {
                    match sop.kind {
                        // The server is now empty for this id; the pending
                        // edit must re-create it from scratch.
                        OpKind::Delete => {
                            if let Some(r) = self.records.get_mut(&sop.id) {
                                r.base_rev = None;
                                r.base_value = None;
                                r.sync = SyncState::Dirty;
                                r.intent = Intent::Create;
                                r.presence = Presence::Live;
                            }
                        }
                        // The server now holds the acked value at `new_rev`;
                        // that's the ancestor the pending edit builds on.
                        OpKind::Create | OpKind::Update => {
                            let acked: T = decode_op_value(sop);
                            if let Some(r) = self.records.get_mut(&sop.id) {
                                r.base_rev = Some(*new_rev);
                                r.base_value = Some(acked);
                                r.sync = SyncState::Dirty;
                                if matches!(r.intent, Intent::Create) {
                                    r.intent = Intent::Update;
                                }
                            }
                        }
                    }
                    self.rebuild_op_for(&sop.id);
                }
            }
            OpResult::Conflict {
                server_rev,
                server_value,
                ..
            } => {
                // The record still holds the local edit (we removed only
                // the op). Merge against the server's returned value.
                self.merge_record(sop.id.clone(), *server_rev, Some(server_value.clone()));
            }
            OpResult::Gone { .. } => match sop.kind {
                // We wanted it gone and it's gone — success.
                OpKind::Delete => {
                    self.records.remove(&sop.id);
                }
                // We edited a record the server deleted → delete/update
                // conflict with no server value.
                OpKind::Create | OpKind::Update => self.resolve_gone(&sop.id),
            },
            OpResult::Rejected { reason, .. } => {
                // Domain validation refused this mutation. Block the
                // record so the app notices rather than silently looping.
                self.last_error = Some(reason.clone());
                if let Some(r) = self.records.get_mut(&sop.id) {
                    r.sync = SyncState::Conflicted;
                }
            }
        }
    }

    /// Handle a push `Gone` for an `Update`/`Create`: the server has no
    /// such record. Ask the app whether the local edit resurrects it.
    fn resolve_gone(&mut self, id: &Id) {
        let resolution = {
            let r = self.records.get(id).expect("resolve_gone: record exists");
            T::merge(MergeCtx {
                base: r.base_value.as_ref(),
                local: r.value.as_ref(),
                incoming: None,
            })
        };
        match resolution {
            // Resurrect: re-create from scratch (the server has nothing to
            // base an update on).
            Resolution::TakeLocal => {
                if let Some(r) = self.records.get_mut(id) {
                    r.base_rev = None;
                    r.base_value = None;
                    r.sync = SyncState::Dirty;
                    r.intent = Intent::Create;
                    r.presence = Presence::Live;
                }
                self.rebuild_op_for(id);
            }
            Resolution::Merged(v) => {
                self.records.insert(
                    id.clone(),
                    Record {
                        id: id.clone(),
                        value: Some(v),
                        base_rev: None,
                        base_value: None,
                        sync: SyncState::Dirty,
                        intent: Intent::Create,
                        presence: Presence::Live,
                    },
                );
                self.rebuild_op_for(id);
            }
            // Accept the deletion.
            Resolution::TakeIncoming => {
                self.records.remove(id);
            }
            Resolution::Unresolved => {
                if let Some(r) = self.records.get_mut(id) {
                    r.sync = SyncState::Conflicted;
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // App-driven conflict resolution
    // -----------------------------------------------------------------

    /// Resolve a record the engine left [`Conflicted`](SyncState::Conflicted).
    /// A no-op if the record isn't actually conflicted.
    pub fn resolve(&mut self, id: Id, resolution: Resolution<T>) {
        match self.records.get(&id) {
            Some(r) if matches!(r.sync, SyncState::Conflicted) => {}
            _ => return,
        }
        self.last_error = None;

        match resolution {
            Resolution::TakeLocal => {
                if let Some(r) = self.records.get_mut(&id) {
                    r.sync = SyncState::Dirty;
                    if matches!(r.intent, Intent::None) {
                        r.intent = Intent::Update;
                    }
                }
                self.rebuild_op_for(&id);
            }
            Resolution::TakeIncoming => {
                self.remove_ops_for(&id);
                let (value, rev) = {
                    let r = self.records.get(&id).unwrap();
                    (r.base_value.clone(), r.base_rev)
                };
                match (value, rev) {
                    (Some(v), Some(rev)) => {
                        self.records.insert(id.clone(), Record::synced(id, v, rev));
                    }
                    (None, Some(rev)) => {
                        self.records.insert(id.clone(), Record::server_tombstone(id, rev));
                    }
                    // No server anchor recorded — drop it.
                    _ => {
                        self.records.remove(&id);
                    }
                }
            }
            Resolution::Merged(v) => {
                if let Some(r) = self.records.get_mut(&id) {
                    r.value = Some(v);
                    r.sync = SyncState::Dirty;
                    r.intent = Intent::Update;
                    r.presence = Presence::Live;
                }
                self.rebuild_op_for(&id);
            }
            Resolution::Unresolved => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Doc {
        title: String,
        body: String,
    }

    // Test merge policy: a field-level 3-way merge. If only one side
    // changed a field vs. the ancestor, take that side; if both changed
    // the same field differently, it's unresolved.
    impl Merge for Doc {
        fn merge(ctx: MergeCtx<'_, Self>) -> Resolution<Self> {
            match (ctx.base, ctx.local, ctx.incoming) {
                // Both still exist: field-level merge.
                (Some(base), Some(local), Some(incoming)) => {
                    let title = pick(&base.title, &local.title, &incoming.title);
                    let body = pick(&base.body, &local.body, &incoming.body);
                    match (title, body) {
                        (Some(t), Some(b)) => Resolution::Merged(Doc { title: t, body: b }),
                        _ => Resolution::Unresolved,
                    }
                }
                // Create/create collision: no ancestor.
                (None, Some(_), Some(_)) => Resolution::Unresolved,
                // Delete/update conflicts: local edit wins (resurrect).
                (_, Some(_), None) => Resolution::TakeLocal,
                (_, None, Some(_)) => Resolution::TakeIncoming,
                _ => Resolution::TakeIncoming,
            }
        }
    }

    // Three-way field pick: returns the merged value, or None if both
    // sides changed it differently.
    fn pick(base: &str, local: &str, incoming: &str) -> Option<String> {
        if local == incoming {
            Some(local.to_string())
        } else if local == base {
            Some(incoming.to_string())
        } else if incoming == base {
            Some(local.to_string())
        } else {
            None
        }
    }

    fn doc(title: &str, body: &str) -> Doc {
        Doc {
            title: title.into(),
            body: body.into(),
        }
    }

    fn inner() -> PartitionInner<Doc> {
        PartitionInner::new("client".into(), "p".into(), Vec::new(), Vec::new(), None)
    }

    fn upsert_change(id: &str, rev: u64, title: &str, body: &str) -> Change<Doc> {
        Change::Upsert {
            id: Id::from(id),
            rev: Rev(rev),
            value: doc(title, body),
        }
    }

    // ---- local mutations + outbox ----

    #[test]
    fn local_create_then_update_coalesces_to_one_create() {
        let mut p = inner();
        p.enqueue_upsert(Id::from("a"), doc("t1", "b1"));
        p.enqueue_upsert(Id::from("a"), doc("t2", "b2"));
        let ops = p.outbox_vec();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, OpKind::Create);
        assert_eq!(p.live_values(), vec![doc("t2", "b2")]);
    }

    #[test]
    fn local_create_then_delete_emits_no_op_and_no_record() {
        let mut p = inner();
        p.enqueue_upsert(Id::from("a"), doc("t", "b"));
        p.enqueue_delete(Id::from("a"));
        assert!(p.outbox_vec().is_empty(), "create+delete must annihilate");
        assert!(p.live_values().is_empty());
    }

    #[test]
    fn editing_synced_record_freezes_ancestor() {
        let mut p = inner();
        // Seed a synced record via a delta pull.
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 5, "server", "body")]);
        p.enqueue_upsert(Id::from("a"), doc("local", "body"));
        let ops = p.outbox_vec();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, OpKind::Update);
        assert_eq!(ops[0].base_rev, Some(Rev(5)), "op targets the synced rev");
    }

    // ---- pull apply ----

    #[test]
    fn delta_overwrites_clean_record() {
        let mut p = inner();
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 1, "v1", "b")]);
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 2, "v2", "b")]);
        assert_eq!(p.live_values(), vec![doc("v2", "b")]);
    }

    #[test]
    fn stale_delta_is_ignored_idempotent_apply() {
        let mut p = inner();
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 5, "v5", "b")]);
        // A replayed older page must not clobber newer data (INV-4).
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 3, "v3", "b")]);
        assert_eq!(p.live_values(), vec![doc("v5", "b")]);
    }

    #[test]
    fn reapplying_same_delta_is_a_noop() {
        let mut p = inner();
        let page = vec![upsert_change("a", 5, "v5", "b")];
        p.apply_pull(PullMode::Delta, page.clone());
        p.apply_pull(PullMode::Delta, page);
        assert_eq!(p.live_values(), vec![doc("v5", "b")]);
    }

    #[test]
    fn delta_tombstone_removes_clean_record() {
        let mut p = inner();
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 1, "v", "b")]);
        p.apply_pull(
            PullMode::Delta,
            vec![Change::Tombstone {
                id: Id::from("a"),
                rev: Rev(2),
            }],
        );
        assert!(p.live_values().is_empty());
    }

    #[test]
    fn snapshot_preserves_local_create_but_drops_absent_clean() {
        let mut p = inner();
        // Two synced records from an initial snapshot.
        p.apply_pull(
            PullMode::Snapshot,
            vec![upsert_change("a", 1, "a", "b"), upsert_change("b", 1, "b", "b")],
        );
        // Local-only create.
        p.enqueue_upsert(Id::from("c"), doc("c", "local"));
        // New snapshot no longer contains "b" (server deleted it) and of
        // course not the local-only "c".
        p.apply_pull(PullMode::Snapshot, vec![upsert_change("a", 2, "a2", "b")]);

        let mut live = p.live_values();
        live.sort_by(|x, y| x.title.cmp(&y.title));
        assert_eq!(live, vec![doc("a2", "b"), doc("c", "local")]);
    }

    #[test]
    fn snapshot_does_not_clobber_dirty_edit() {
        let mut p = inner();
        p.apply_pull(PullMode::Snapshot, vec![upsert_change("a", 1, "title", "body")]);
        // Local edit to the body only.
        p.enqueue_upsert(Id::from("a"), doc("title", "my body"));
        // Server changed the title only (body unchanged from ancestor).
        p.apply_pull(PullMode::Snapshot, vec![upsert_change("a", 2, "server title", "body")]);
        // Field-level merge keeps both independent edits.
        assert_eq!(p.live_values(), vec![doc("server title", "my body")]);
        // The merged value is itself pending — re-queued as an update.
        let ops = p.outbox_vec();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, OpKind::Update);
        assert_eq!(ops[0].base_rev, Some(Rev(2)), "rebased onto the server rev");
    }

    #[test]
    fn conflicting_edits_become_unresolved_and_block() {
        let mut p = inner();
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 1, "title", "body")]);
        p.enqueue_upsert(Id::from("a"), doc("local title", "body"));
        // Server changed the SAME field differently → unresolved.
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 2, "server title", "body")]);
        assert!(p.has_conflict());
        assert_eq!(p.conflicts(), vec![Id::from("a")]);
    }

    #[test]
    fn resolve_take_local_requeues_against_server_rev() {
        let mut p = inner();
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 1, "t", "body")]);
        p.enqueue_upsert(Id::from("a"), doc("local", "body"));
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 2, "server", "body")]);
        assert!(p.has_conflict());

        p.resolve(Id::from("a"), Resolution::TakeLocal);
        assert!(!p.has_conflict());
        assert_eq!(p.live_values(), vec![doc("local", "body")]);
        let ops = p.outbox_vec();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].base_rev, Some(Rev(2)));
    }

    #[test]
    fn resolve_take_incoming_accepts_server_and_clears_op() {
        let mut p = inner();
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 1, "t", "body")]);
        p.enqueue_upsert(Id::from("a"), doc("local", "body"));
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 2, "server", "body")]);
        p.resolve(Id::from("a"), Resolution::TakeIncoming);
        assert!(!p.has_conflict());
        assert_eq!(p.live_values(), vec![doc("server", "body")]);
        assert!(p.outbox_vec().is_empty());
    }

    // ---- push results ----

    #[test]
    fn push_applied_advances_record_and_drops_op() {
        let mut p = inner();
        p.enqueue_upsert(Id::from("a"), doc("t", "b"));
        let sent = p.seal_for_push();
        assert_eq!(sent.len(), 1);
        p.process_push_results(
            &sent,
            vec![OpResult::Applied {
                id: Id::from("a"),
                new_rev: Rev(10),
            }],
        );
        assert!(p.outbox_vec().is_empty(), "acked op dropped");
        assert!(!p.has_pending());
        // Now synced — a subsequent edit targets rev 10.
        p.enqueue_upsert(Id::from("a"), doc("t2", "b"));
        assert_eq!(p.outbox_vec()[0].base_rev, Some(Rev(10)));
    }

    #[test]
    fn push_duplicate_is_treated_as_applied() {
        let mut p = inner();
        p.enqueue_upsert(Id::from("a"), doc("t", "b"));
        let sent = p.seal_for_push();
        // Lost-ack retry: server replays as Duplicate.
        p.process_push_results(
            &sent,
            vec![OpResult::Duplicate {
                id: Id::from("a"),
                new_rev: Rev(10),
            }],
        );
        assert!(p.outbox_vec().is_empty());
    }

    #[test]
    fn push_conflict_routes_to_merge() {
        let mut p = inner();
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 1, "title", "body")]);
        p.enqueue_upsert(Id::from("a"), doc("title", "local body"));
        let sent = p.seal_for_push();
        // Server rejects: it advanced to rev 7 with a different title.
        p.process_push_results(
            &sent,
            vec![OpResult::Conflict {
                id: Id::from("a"),
                server_rev: Rev(7),
                server_value: doc("server title", "body"),
            }],
        );
        // Field merge keeps both edits; new op rebased to rev 7.
        assert_eq!(p.live_values(), vec![doc("server title", "local body")]);
        assert_eq!(p.outbox_vec()[0].base_rev, Some(Rev(7)));
    }

    #[test]
    fn push_gone_on_delete_is_success() {
        let mut p = inner();
        p.apply_pull(PullMode::Delta, vec![upsert_change("a", 1, "t", "b")]);
        p.enqueue_delete(Id::from("a"));
        let sent = p.seal_for_push();
        p.process_push_results(&sent, vec![OpResult::Gone { id: Id::from("a") }]);
        assert!(p.live_values().is_empty());
        assert!(p.outbox_vec().is_empty());
    }

    #[test]
    fn push_rejected_blocks_the_record() {
        let mut p = inner();
        p.enqueue_upsert(Id::from("a"), doc("t", "b"));
        let sent = p.seal_for_push();
        p.process_push_results(
            &sent,
            vec![OpResult::Rejected {
                id: Id::from("a"),
                reason: "name taken".into(),
            }],
        );
        assert!(p.has_conflict());
        assert_eq!(p.last_error().as_deref(), Some("name taken"));
    }

    #[test]
    fn edit_during_inflight_push_queues_behind_sealed_op() {
        let mut p = inner();
        p.enqueue_upsert(Id::from("a"), doc("v1", "b"));
        let sent = p.seal_for_push(); // op for "a" now sealed/in-flight
        // User edits again before the ack.
        p.enqueue_upsert(Id::from("a"), doc("v2", "b"));
        assert_eq!(p.outbox_vec().len(), 2, "new edit queued behind sealed op");
        // Ack the first; the second survives.
        p.process_push_results(
            &sent,
            vec![OpResult::Applied {
                id: Id::from("a"),
                new_rev: Rev(3),
            }],
        );
        let ops = p.outbox_vec();
        assert_eq!(ops.len(), 1, "only the in-flight op was acked");
        assert_eq!(p.live_values(), vec![doc("v2", "b")]);
    }

    #[test]
    fn restored_inner_resumes_seq_past_persisted_ops() {
        let mut p = inner();
        p.enqueue_upsert(Id::from("a"), doc("t", "b"));
        let ops = p.outbox_vec();
        let records = p.records_vec();
        // Simulate a restart: rebuild from persisted state.
        let mut p2 = PartitionInner::<Doc>::new("client".into(), "p".into(), records, ops, None);
        p2.enqueue_upsert(Id::from("b"), doc("t2", "b2"));
        let seqs: Vec<u64> = p2.outbox_vec().iter().map(|o| o.seq).collect();
        // No seq collision between restored and fresh ops.
        let unique: std::collections::HashSet<_> = seqs.iter().collect();
        assert_eq!(unique.len(), seqs.len());
    }
}
