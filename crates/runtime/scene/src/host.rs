//! The [`Host`] trait — the entire structural seam between the scene and a
//! platform.
//!
//! Extracted from the old `Backend` mega-trait's structural subset with
//! signatures unchanged (design §4), so backends implement it by
//! delegating to methods they already have. Everything else a platform
//! does (creating primitives, props, styling, events) goes through
//! [`Registry`](crate::Registry) handlers, which receive the full backend
//! via [`MountCx`](crate::MountCx) — the scene itself only ever needs
//! these seven operations.

/// The structural operations the scene's drivers emit. One impl per
/// backend; the parity recorder in `scene-parity` is the reference for the
/// exact op semantics the goldens pin.
pub trait Host: 'static {
    /// The platform's real node handle. `Clone` is required because
    /// structural regions retain node handles across effect fires (a
    /// spliced region must `remove_child` the exact nodes it inserted).
    type Node: Clone + 'static;

    /// Append `child` as `parent`'s last child.
    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node);

    /// Append many children at once (batched insertion — a
    /// `DocumentFragment` on web). Default: per-child [`insert`](Self::insert).
    /// Unused by the P1 drivers (it serves the static `Repeat` fast path,
    /// which lands with the vocabulary in P2) but part of the frozen seam.
    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        for child in children {
            self.insert(parent, child);
        }
    }

    /// Insert `child` at `index` among `parent`'s children. For an
    /// ALREADY-mounted child this is a move (DOM `insertBefore`
    /// semantics) — the keyed reconciler relies on that for reorders.
    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize);

    /// Detach `child` from `parent`, leaving siblings untouched.
    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node);

    /// Detach ALL of `node`'s children (the anchored swap primitive).
    fn clear_children(&mut self, node: &Self::Node);

    /// The subtree rooted at `node` is being DISCARDED — not merely
    /// detached — and the host may free whatever it keeps for it.
    ///
    /// [`clear_children`](Self::clear_children) cannot carry this
    /// meaning, because the navigator uses it for both: a
    /// `LazyDisposing` eviction really is gone, while a
    /// `LazyPersistent` switch-away is detached and the SAME node is
    /// re-inserted on return without the route builder re-running. A
    /// host that freed its per-node state on `clear_children` would
    /// leave a retained screen with nothing to come back to.
    ///
    /// Called by the navigator handler at the points where it drops a
    /// screen's `Realized` — the eviction, stack pop / replace / reset,
    /// and navigator teardown — while the subtree is still assembled,
    /// so a host that walks its own children can still find them.
    ///
    /// Default: no-op. Hosts that keep no per-node state outside the
    /// node itself (web, SSR — the DOM node dies with its last
    /// reference) need nothing here. It matters for a host holding a
    /// side registry keyed by node, which is exactly the iOS backend:
    /// its `view_to_layout` owns a strong `Retained<UIView>` and a
    /// Taffy node per view, and without this seam a disposed screen's
    /// entries lived forever — every later layout pass walking them,
    /// and each unparented node counted as another root to compute.
    fn release_subtree(&mut self, _node: &Self::Node) {}

    /// Create a reactive anchor: a layout-transparent container the
    /// anchored drivers swap subtrees under (`display: contents` on web; a
    /// plain view elsewhere).
    fn create_anchor(&mut self) -> Self::Node;

    /// Can this host splice children directly (`remove_child` +
    /// `insert_at`) into a real parent? `true` → style-less reactive
    /// regions go anchorless (web CSR, splice-capable native); `false` →
    /// every reactive region nests under a [`create_anchor`](Self::create_anchor)
    /// (SSR, web hydration, legacy native).
    fn supports_splice(&self) -> bool;
}
