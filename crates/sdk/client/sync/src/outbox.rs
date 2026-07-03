//! The durable, per-partition mutation outbox.
//!
//! Offline writes are appended here as [`OutboxOp`]s and replayed to the
//! server (via `push`) on reconnect. The outbox is the **source of truth
//! for pending intent**: a mutation is "saved" only once it is durably in
//! the outbox (one [`storage`](storage) `set`), and a record's local edit
//! is never considered pushed until its op is removed *after* the server
//! ack is durable.
//!
//! This module is deliberately `T`-agnostic: an op carries its value as
//! already-serialized JSON (`value_json`), so the queue is a plain
//! `Vec<OutboxOp>` that persists and coalesces without knowing the entity
//! type. The engine serializes `T` when enqueuing and deserializes when
//! building a [`PushRequest`](crate::protocol::PushRequest).
//!
//! ## Coalescing
//!
//! While still offline, successive ops on the same record collapse before
//! they ever reach the wire (see [`coalesce`]). The highest-value rule is
//! create-then-delete **annihilation**: a record created and deleted
//! offline produces no wire traffic at all.
//!
//! Coalescing only ever touches **unsealed** ops. The instant an op is
//! handed to the transport it is [`sealed`](OutboxOp::sealed) and its
//! idempotency key is "burned"; a later edit to the same record must queue
//! *behind* the sealed op, never merge into it — otherwise acking the
//! in-flight op could resurrect state the user already changed.

use serde::{Deserialize, Serialize};

use crate::model::{Id, Rev};
use crate::protocol::OpKind;

/// One queued mutation, persisted in the outbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboxOp {
    /// Local monotonic sequence — assigns FIFO order and (with the
    /// engine's `client_id` + partition) derives the idempotency key.
    pub seq: u64,
    /// Stable-across-retries idempotency key sent to the server.
    pub idem_key: String,
    /// Target record.
    pub id: Id,
    /// The operation.
    pub kind: OpKind,
    /// Ancestor revision for `Update`/`Delete`; `None` for `Create`.
    pub base_rev: Option<Rev>,
    /// The op's value as serialized JSON (`None` for `Delete`).
    pub value_json: Option<String>,
    /// True once handed to the transport. Sealed ops are immutable to
    /// coalescing — a new edit queues behind them.
    pub sealed: bool,
}

impl OutboxOp {
    /// Construct an unsealed op.
    pub fn new(
        seq: u64,
        idem_key: String,
        id: Id,
        kind: OpKind,
        base_rev: Option<Rev>,
        value_json: Option<String>,
    ) -> Self {
        OutboxOp {
            seq,
            idem_key,
            id,
            kind,
            base_rev,
            value_json,
            sealed: false,
        }
    }
}

/// Fold `new_op` into `queue`, coalescing with the **last op for the same
/// id** when that op is still unsealed.
///
/// The combination table (rows = existing intent, cols = incoming intent):
///
/// | existing → incoming | result |
/// |---|---|
/// | Create → Create/Update | Create, newest value |
/// | Create → Delete | *annihilate* — emit nothing |
/// | Update → Update/Create | Update, newest value, earliest ancestor |
/// | Update → Delete | Delete, carrying the earliest ancestor |
/// | Delete → Create/Update | Update (resurrect), newest value |
/// | Delete → Delete | Delete (idempotent) |
///
/// The earliest ancestor (`base_rev`) is preserved through coalescing so a
/// later 3-way merge still diffs against the state the *first* offline
/// edit was made on. If the last op for the id is sealed (in flight), the
/// new op is appended unchanged so it replays behind the sealed one.
pub fn coalesce(queue: &mut Vec<OutboxOp>, new_op: OutboxOp) {
    let Some(pos) = queue.iter().rposition(|o| o.id == new_op.id) else {
        queue.push(new_op);
        return;
    };

    if queue[pos].sealed {
        // The most recent op for this record is already on the wire.
        // Queue behind it — never merge into a sealed op.
        queue.push(new_op);
        return;
    }

    let prev_kind = queue[pos].kind;
    match (prev_kind, new_op.kind) {
        // A record created and then deleted while offline never existed
        // server-side — drop both. The single highest-value rule.
        (OpKind::Create, OpKind::Delete) => {
            queue.remove(pos);
        }
        // A pending create absorbs later edits: still one create with the
        // newest value, `base_rev` stays `None`.
        (OpKind::Create, OpKind::Create) | (OpKind::Create, OpKind::Update) => {
            queue[pos].value_json = new_op.value_json;
        }
        // Update-then-update collapses to one update; keep the *earliest*
        // ancestor (already in `queue[pos].base_rev`) so merge has the
        // right base.
        (OpKind::Update, OpKind::Update) | (OpKind::Update, OpKind::Create) => {
            queue[pos].kind = OpKind::Update;
            queue[pos].value_json = new_op.value_json;
        }
        // Update-then-delete collapses to one delete carrying the ancestor.
        (OpKind::Update, OpKind::Delete) => {
            queue[pos].kind = OpKind::Delete;
            queue[pos].value_json = None;
        }
        // A delete followed by a re-create/edit resurrects the record:
        // becomes an update with the newest value, ancestor preserved.
        (OpKind::Delete, OpKind::Create) | (OpKind::Delete, OpKind::Update) => {
            queue[pos].kind = OpKind::Update;
            queue[pos].value_json = new_op.value_json;
        }
        // Idempotent double-delete.
        (OpKind::Delete, OpKind::Delete) => {}
    }
}

/// Seal every op in the queue so coalescing can no longer touch them.
///
/// v1 policy: seal and send the **whole queue** as one push batch (the
/// server applies them in FIFO order and returns positional results),
/// which keeps per-partition ordering trivially correct. Already-sealed
/// ops at the head (a prior in-flight attempt) are re-sent on a retry — the
/// idempotency key makes that safe. The engine calls this via
/// [`PartitionInner::seal_for_push`](crate::engine); it's exposed here too
/// for the coalescing tests below.
#[cfg(test)]
pub fn seal_all(queue: &mut [OutboxOp]) {
    for op in queue.iter_mut() {
        op.sealed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(seq: u64, id: &str, kind: OpKind, base_rev: Option<u64>, value: Option<&str>) -> OutboxOp {
        OutboxOp::new(
            seq,
            format!("k{seq}"),
            Id::from(id),
            kind,
            base_rev.map(Rev),
            value.map(|v| format!("\"{v}\"")),
        )
    }

    fn push(queue: &mut Vec<OutboxOp>, o: OutboxOp) {
        coalesce(queue, o);
    }

    #[test]
    fn distinct_ids_do_not_coalesce() {
        let mut q = Vec::new();
        push(&mut q, op(1, "a", OpKind::Create, None, Some("a1")));
        push(&mut q, op(2, "b", OpKind::Create, None, Some("b1")));
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn create_then_update_stays_create_with_newest_value() {
        let mut q = Vec::new();
        push(&mut q, op(1, "a", OpKind::Create, None, Some("v1")));
        push(&mut q, op(2, "a", OpKind::Update, Some(5), Some("v2")));
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].kind, OpKind::Create);
        assert_eq!(q[0].base_rev, None, "create carries no ancestor");
        assert_eq!(q[0].value_json.as_deref(), Some("\"v2\""));
    }

    #[test]
    fn create_then_delete_annihilates() {
        let mut q = Vec::new();
        push(&mut q, op(1, "a", OpKind::Create, None, Some("v1")));
        push(&mut q, op(2, "a", OpKind::Delete, None, None));
        assert!(q.is_empty(), "create+delete offline must emit nothing");
    }

    #[test]
    fn update_then_update_keeps_earliest_ancestor() {
        let mut q = Vec::new();
        push(&mut q, op(1, "a", OpKind::Update, Some(5), Some("v1")));
        push(&mut q, op(2, "a", OpKind::Update, Some(99), Some("v2")));
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].kind, OpKind::Update);
        assert_eq!(q[0].base_rev, Some(Rev(5)), "ancestor is the first edit's base");
        assert_eq!(q[0].value_json.as_deref(), Some("\"v2\""));
    }

    #[test]
    fn update_then_delete_is_delete_with_ancestor() {
        let mut q = Vec::new();
        push(&mut q, op(1, "a", OpKind::Update, Some(5), Some("v1")));
        push(&mut q, op(2, "a", OpKind::Delete, None, None));
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].kind, OpKind::Delete);
        assert_eq!(q[0].base_rev, Some(Rev(5)));
        assert_eq!(q[0].value_json, None);
    }

    #[test]
    fn delete_then_recreate_resurrects_as_update() {
        let mut q = Vec::new();
        push(&mut q, op(1, "a", OpKind::Delete, Some(5), None));
        push(&mut q, op(2, "a", OpKind::Create, None, Some("v2")));
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].kind, OpKind::Update);
        assert_eq!(q[0].base_rev, Some(Rev(5)));
        assert_eq!(q[0].value_json.as_deref(), Some("\"v2\""));
    }

    #[test]
    fn double_delete_is_idempotent() {
        let mut q = Vec::new();
        push(&mut q, op(1, "a", OpKind::Delete, Some(5), None));
        push(&mut q, op(2, "a", OpKind::Delete, Some(5), None));
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].kind, OpKind::Delete);
    }

    #[test]
    fn sealed_op_is_not_coalesced_new_op_queues_behind() {
        let mut q = Vec::new();
        push(&mut q, op(1, "a", OpKind::Update, Some(5), Some("v1")));
        seal_all(&mut q); // op 1 now in flight
        push(&mut q, op(2, "a", OpKind::Update, Some(5), Some("v2")));
        assert_eq!(q.len(), 2, "edit during in-flight push queues behind the sealed op");
        assert!(q[0].sealed);
        assert!(!q[1].sealed);
        assert_eq!(q[1].value_json.as_deref(), Some("\"v2\""));
    }

    #[test]
    fn coalesces_into_unsealed_tail_past_a_sealed_head() {
        // A sealed op for `a` is in flight; a later unsealed op for `a`
        // was already queued behind it. A third edit coalesces into the
        // unsealed tail, not the sealed head.
        let mut q = Vec::new();
        push(&mut q, op(1, "a", OpKind::Update, Some(5), Some("v1")));
        seal_all(&mut q);
        push(&mut q, op(2, "a", OpKind::Update, Some(5), Some("v2")));
        push(&mut q, op(3, "a", OpKind::Update, Some(5), Some("v3")));
        assert_eq!(q.len(), 2);
        assert!(q[0].sealed);
        assert_eq!(q[1].value_json.as_deref(), Some("\"v3\""), "coalesced into unsealed tail");
    }

    #[test]
    fn outbox_op_round_trips_through_json() {
        let o = op(1, "a", OpKind::Update, Some(5), Some("v1"));
        let json = serde_json::to_string(&o).unwrap();
        let back: OutboxOp = serde_json::from_str(&json).unwrap();
        assert_eq!(o, back);
    }
}
