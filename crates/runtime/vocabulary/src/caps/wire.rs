//! Declarative wire-binding hooks for generator backends (Roku /
//! remote renderers): binding metadata notes + lazy slot capture.
//!
//! Effect-driven backends leave every default no-op in place; the
//! walker still builds its Rust Effects regardless. Only backends that
//! ship bindings to a remote runtime consume these. Expected to retire
//! with the adopt-sentinel once the wire client mounts through the
//! scene Registry (design §8) — kept frozen until then.

use runtime_shared::__serde_json::Value;
use runtime_scene::Host;

/// Serves `walker/text.rs` (`note_text_binding`, `note_signal_initial`),
/// `walker/when_switch.rs` (`note_when_binding`, `note_switch_binding`,
/// slot capture), and `walker/virtualizer.rs` (`note_repeat_binding`,
/// `note_virtualizer_binding`, slot capture).
pub trait WireBindingOps: Host {
    /// Record a `TextSource::Bound`'s signal ids + transformer method.
    #[allow(unused_variables)]
    fn note_text_binding(&mut self, node: &Self::Node, signal_ids: &[u64], method: &'static str) {
        // default: no-op
    }

    /// Declare a signal's existence + initial value to the remote
    /// renderer (called per signal per binding; backends dedupe).
    #[allow(unused_variables)]
    fn note_signal_initial(&mut self, signal_id: u64, value: &Value) {
        // default: no-op
    }

    /// Record a declaratively-built `when`'s condition + branch nodes.
    #[allow(unused_variables)]
    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        // default: no-op
    }

    /// Record a declaratively-built `switch`'s arms + default.
    #[allow(unused_variables)]
    fn note_switch_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        arms: &[(Value, Self::Node)],
        default_node: &Self::Node,
    ) {
        // default: no-op
    }

    /// Record a repeat's count method + row template.
    #[allow(unused_variables)]
    fn note_repeat_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
    ) {
        // default: no-op
    }

    /// Native windowed-list opt-in; default delegates to
    /// [`note_repeat_binding`](Self::note_repeat_binding) (unwindowed
    /// but correct rows).
    #[allow(unused_variables)]
    fn note_virtualizer_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
        horizontal: bool,
    ) {
        self.note_repeat_binding(anchor, signal_ids, count_method, row_template, row_index_signal_id);
    }

    /// Opt-in for lazy slot materialization (inactive subtrees never
    /// materialize on-device).
    fn supports_lazy_slot_capture(&self) -> bool {
        false
    }

    /// Begin redirecting backend mutations into a capture buffer.
    fn begin_slot_capture(&mut self) {
        // default: no-op
    }

    /// End the most-recent capture region, associating it with
    /// `slot_root`.
    #[allow(unused_variables)]
    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        // default: no-op
    }
}
