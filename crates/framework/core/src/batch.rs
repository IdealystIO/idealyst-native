//! Backend batching for `Primitive::Repeat`.
//!
//! The walker accumulates a small command queue when expanding a
//! Repeat whose rows match a "batchable" shape (View + Text + static
//! style). The whole queue is then submitted to the backend in a
//! single FFI call via [`Backend::execute_batch`]. The backend
//! materializes the structural ops, applies pre-minted classes, and
//! returns the resulting `Self::Node`s indexed by the `local_id` each
//! op referenced.
//!
//! ## Scope
//!
//! V1 is local-render only and Repeat-only. Other primitives and
//! reactive style closures fall back to the existing per-call path.
//! Class minting happens Rust-side before the batch is submitted, so
//! the JS side stays dumb (set `className` to a string).
//!
//! ## Why not the wire protocol's `Command` enum
//!
//! The AAS wire protocol owns its own `Command` enum (in the `wire`
//! crate) — that enum is shaped for sidecar/device transport and
//! serializability. Local-render batching has different needs:
//! `Rc<StyleRules>` references, `String` class names, no
//! serialization. So we mirror the *shape* (a single ops vec applied
//! in one round-trip) without taking a dep on the wire crate.

use std::rc::Rc;

use crate::style::StyleRules;

/// One structural operation queued for [`Backend::execute_batch`].
///
/// `local_id` / `node` / `parent` / `child` are dense `u32` indices
/// into the batch's local node table — the walker hands these out
/// monotonically as it queues ops, and the backend returns a parallel
/// `Vec<Self::Node>` indexed by them. They are NOT framework
/// `NodeId`s and are not stable beyond the lifetime of one
/// [`BackendBatch`].
pub enum BatchOp {
    /// Create a fresh container view. The materialized node lands at
    /// `local_id` in the batch's return vec.
    CreateView { local_id: u32 },
    /// Create a text node with the given static content.
    CreateText { local_id: u32, content: String },
    /// Apply a static style to a previously-created node. The walker
    /// supplies the pre-minted backend-side class identifier as
    /// `class_name` (e.g. the web backend's CSS class name). Backends
    /// that don't key styles by name can ignore this op — the
    /// walker also keeps the resolved `StyleRules` on hand for
    /// fallback paths, but the web backend reads only `class_name`
    /// inside the batch's tight loop.
    ApplyStyleStatic {
        node: u32,
        class_name: String,
        #[allow(dead_code)]
        rules: Rc<StyleRules>,
    },
    /// Append `child` to `parent`. Both are batch-local ids; the
    /// backend resolves them through its return-vec map.
    Insert { parent: u32, child: u32 },
}

/// A batch of structural commands to execute in one FFI round-trip.
///
/// Built by the walker as it expands a `Primitive::Repeat` whose
/// rows match the batchable shape (View + Text + static style, no
/// other primitives). Backends that opt in via
/// [`Backend::supports_batched_repeat`] receive this batch and
/// return one `Self::Node` per distinct `local_id` (size =
/// [`node_count`](Self::node_count)).
pub struct BackendBatch {
    pub ops: Vec<BatchOp>,
    /// Number of distinct `local_id` values referenced by `ops`. Used
    /// by backends to size their return Vec and pre-allocate the
    /// local→real-node map. The walker fills this as it builds the
    /// batch (one increment per `CreateView` / `CreateText`).
    pub node_count: u32,
}

impl BackendBatch {
    pub fn new() -> Self {
        Self {
            ops: Vec::new(),
            node_count: 0,
        }
    }

    pub fn with_capacity(ops: usize, nodes: u32) -> Self {
        Self {
            ops: Vec::with_capacity(ops),
            node_count: nodes,
        }
    }

    /// Reserve a fresh `local_id` and increment `node_count`. Returns
    /// the id the caller should use in the matching `CreateView` /
    /// `CreateText` op.
    pub fn next_id(&mut self) -> u32 {
        let id = self.node_count;
        self.node_count += 1;
        id
    }
}

impl Default for BackendBatch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the batch protocol — small, but a few invariants
    //! callers (the walker, backend impls) depend on:
    //!
    //! - `next_id` allocates strictly monotonically from 0 on a fresh
    //!   batch, matching the walker's "local_ids index into the
    //!   returned `Vec<Self::Node>`" contract.
    //! - `with_capacity(ops, nodes)` seeds `node_count` to `nodes`,
    //!   so subsequent `next_id`s start at `nodes` — letting backends
    //!   that pre-mint placeholder nodes interleave with the walker
    //!   without id collisions.
    //! - The `ops` Vec capacity is a hint, not a cap — pushes past
    //!   the initial capacity are allowed (just slower if they
    //!   trigger a realloc).

    use super::*;

    #[test]
    fn new_batch_has_zero_node_count_and_no_ops() {
        let batch = BackendBatch::new();
        assert_eq!(batch.node_count, 0);
        assert!(batch.ops.is_empty());
    }

    #[test]
    fn default_matches_new() {
        let a = BackendBatch::default();
        let b = BackendBatch::new();
        assert_eq!(a.node_count, b.node_count);
        assert_eq!(a.ops.len(), b.ops.len());
    }

    #[test]
    fn with_capacity_zero_nodes_starts_ids_at_zero() {
        let mut batch = BackendBatch::with_capacity(10, 0);
        assert_eq!(batch.next_id(), 0);
        assert_eq!(batch.next_id(), 1);
        assert_eq!(batch.next_id(), 2);
        assert_eq!(batch.node_count, 3);
    }

    #[test]
    fn with_capacity_nonzero_nodes_starts_ids_at_nodes() {
        // Caller passes 5 — `next_id` should resume from 5, not 0.
        // Lets backends that pre-mint placeholder local_ids leave
        // a contiguous gap for them at the front of the table.
        let mut batch = BackendBatch::with_capacity(10, 5);
        assert_eq!(batch.next_id(), 5);
        assert_eq!(batch.next_id(), 6);
        assert_eq!(batch.node_count, 7);
    }

    #[test]
    fn next_id_is_strictly_monotonic_across_many_calls() {
        let mut batch = BackendBatch::new();
        let ids: Vec<u32> = (0..1_000).map(|_| batch.next_id()).collect();
        for (idx, &id) in ids.iter().enumerate() {
            assert_eq!(id as usize, idx, "ids must equal their position");
        }
        assert_eq!(batch.node_count, 1_000);
    }

    #[test]
    fn ops_capacity_is_a_hint_not_a_cap() {
        // `with_capacity(3, …)` hints 3 ops; pushing 10 must still
        // work without panic. The walker over-allocates in
        // production paths but small-batch cases might under-shoot.
        let mut batch = BackendBatch::with_capacity(3, 0);
        for _ in 0..10 {
            batch.ops.push(BatchOp::CreateView {
                local_id: batch.node_count,
            });
            batch.node_count += 1;
        }
        assert_eq!(batch.ops.len(), 10);
        assert_eq!(batch.node_count, 10);
    }

    #[test]
    fn batch_op_variants_construct_with_expected_fields() {
        // Type-shape regression — if any variant gets renamed or
        // restructured this fails to compile. Cheap canary.
        let _create_view = BatchOp::CreateView { local_id: 0 };
        let _create_text = BatchOp::CreateText {
            local_id: 1,
            content: "hi".to_string(),
        };
        let _apply_style = BatchOp::ApplyStyleStatic {
            node: 0,
            class_name: "c".to_string(),
            rules: Rc::new(StyleRules::default()),
        };
        let _insert = BatchOp::Insert {
            parent: 0,
            child: 1,
        };
    }

    #[test]
    fn next_id_advances_node_count_in_lockstep() {
        // The walker uses `node_count` as both "how many slots the
        // backend should pre-allocate" and "the value of the next
        // local_id". `next_id` must keep these in sync.
        let mut batch = BackendBatch::new();
        assert_eq!(batch.node_count, 0);
        let _id0 = batch.next_id();
        assert_eq!(batch.node_count, 1);
        let _id1 = batch.next_id();
        assert_eq!(batch.node_count, 2);
        let _id2 = batch.next_id();
        assert_eq!(batch.node_count, 3);
    }
}
