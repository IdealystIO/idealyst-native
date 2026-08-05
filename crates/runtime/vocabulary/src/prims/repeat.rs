//! The static-range repeat payload — the new-core carrier of the old
//! `Element::Repeat` (`for i in 0..n` with a single-node body).

use runtime_scene::Element;

/// N structurally-identical sibling rows built from one template
/// closure. Carried by [`Element::Many`](runtime_scene::Element) (a
/// multi-node primitive — one payload, N sibling nodes), so the mount
/// handler gets PARENT access and can take the old core's batched-Repeat
/// fast path: enqueue the whole expansion as `BatchOp`s and submit ONE
/// `execute_batch_with_attach` FFI call instead of 4N per-node calls.
///
/// Rows that don't match the batchable shape (see
/// `handlers::repeat::mount_repeat`) fall back to per-row registry
/// mounts + one `insert_many`, exactly the old walker's fallback.
pub struct RepeatPrim {
    pub count: usize,
    pub row_builder: Box<dyn Fn(usize) -> Element>,
}
