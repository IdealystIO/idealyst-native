//! The batched-Repeat fast path (one FFI round-trip for N static rows).

use runtime_core::BackendBatch;
use runtime_scene::Host;

/// Batched materialization of static View+Text repeat rows. Serves the
/// `Element::Repeat` expansion in `walker/view.rs`. `: Host` supplies
/// `insert_many` for the frozen `execute_batch_with_attach` default.
pub trait BatchOps: Host {
    /// Opt-in flag for the batched-Repeat path.
    fn supports_batched_repeat(&self) -> bool {
        false
    }

    /// Execute a queued batch in one round-trip; returned `Vec` is
    /// indexed by `local_id` and sized `batch.node_count`.
    #[allow(unused_variables)]
    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
        unimplemented!(
            "execute_batch is only called when supports_batched_repeat() returns true; \
             this backend opted in without implementing it"
        )
    }

    /// Execute a batch AND parent the `attach_locals` row tops to
    /// `parent` in one call. Default is the literal
    /// [`execute_batch`](Self::execute_batch) + `Host::insert_many`
    /// sequence; backends fold the attach into the same round-trip.
    fn execute_batch_with_attach(
        &mut self,
        batch: BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node> {
        let nodes = self.execute_batch(batch);
        if !attach_locals.is_empty() {
            let rows: Vec<Self::Node> = attach_locals
                .iter()
                .map(|&id| nodes[id as usize].clone())
                .collect();
            self.insert_many(parent, rows);
        }
        nodes
    }
}
