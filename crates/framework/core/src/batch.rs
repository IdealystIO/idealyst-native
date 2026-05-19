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
