//! Every mounted node a `ui!` site built, reachable by tag.
//!
//! The overlay's live path needs to find an instance from a `(site,
//! node)` pair. Walking down from one `Realized` cannot do it: a handler
//! is free to realize a subtree into its OWN storage rather than into
//! the live tree it was called from, and the navigator handlers do
//! exactly that — a screen's `Realized` is held by the navigator, not
//! hung under it. In a navigator-based app that puts essentially the
//! whole UI out of reach.
//!
//! So registration happens at the one place every mounted node passes
//! through: `mount_item`. Any handler that realizes through
//! [`realize`](crate::realize) is covered, including ones that do not
//! exist yet, which is the point — the alternative was a seam per
//! handler, and the next handler would have forgotten it.
//!
//! # Unregistration is ownership, not a hook
//!
//! The registry holds `Weak`; the `Rc` lives in the [`LiveNode`] the
//! node belongs to. Dropping a `Realized` IS unmount, so a popped
//! navigator screen drops its live tree, drops the `Rc`, and its entries
//! go dead with no cleanup callback to forget to fire. A patch that
//! arrives afterwards finds nothing and applies nothing — which is the
//! required behaviour, and here it is the only possible behaviour.
//!
//! (`on_cleanup` would have been the obvious hook and does not work:
//! it panics outside an effect, and the realize walk is not inside one.)
//!
//! # Thread-local, which here means per-session
//!
//! A scene is single-threaded. In the dev sidecar each session runs on
//! its own thread with its own `World`, and a patch is applied on that
//! session's thread — so a thread-local registry is exactly that
//! session's set of mounted nodes, and one session's patch cannot reach
//! another's tree.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::element::NodeTag;

/// What the registry knows about one mounted node.
///
/// Held by `Rc` from the [`LiveNode`](crate::LiveNode) that owns the
/// node, and by `Weak` from the registry.
pub struct LiveOrigin<N> {
    pub tag: NodeTag,
    /// The payload's type, so an applier knows whether this node takes
    /// `update_text` or `update_button_label`. The live side has a node
    /// handle and no payload.
    pub type_id: TypeId,
    pub node: N,
    /// The top-level backend nodes of this node's children, in order.
    ///
    /// Recorded so a structural edit can detach and re-attach them
    /// without the live TREE — the registry is handle-based, and this is
    /// the part of the tree a patch actually needs. Updated in place
    /// when a patch replaces the child list, so a second patch sees the
    /// current set.
    pub children: RefCell<Vec<N>>,
    /// Set by a rebuilder that replaced this instance's subtree, so the
    /// stale entry stops matching. See `runtime_vocabulary::overlay`.
    pub retired: std::cell::Cell<bool>,
}

struct Entry {
    tag: NodeTag,
    origin: Weak<dyn Any>,
}

thread_local! {
    static LIVE: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

/// Record a mounted node. Called from `mount_item`; nothing else should.
pub(crate) fn register<N: 'static>(origin: &Rc<LiveOrigin<N>>) {
    let erased: Rc<dyn Any> = origin.clone();
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        // Amortized prune: dead entries accumulate as subtrees unmount,
        // and nothing else ever visits them. Cheap because a dead weak
        // is one atomic-free load.
        if live.len() % 64 == 0 {
            live.retain(|e| e.origin.strong_count() > 0);
        }
        live.push(Entry { tag: origin.tag, origin: Rc::downgrade(&erased) });
    });
}

/// Every live instance of `(site, node)`, in mount order.
///
/// One pair can match SEVERAL instances — every row of a `for` builds
/// the same node of the same site — so this returns all of them.
/// Entries whose subtree has unmounted are skipped and pruned.
pub fn instances<N: 'static>(site: u64, node: u32) -> Vec<Rc<LiveOrigin<N>>> {
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        live.retain(|e| e.origin.strong_count() > 0);
        live.iter()
            .filter(|e| e.tag.site == site && e.tag.node == node)
            .filter_map(|e| e.origin.upgrade())
            .filter_map(|rc| rc.downcast::<LiveOrigin<N>>().ok())
            .filter(|o| !o.retired.get())
            .collect()
    })
}

/// How many live instances are registered. Diagnostics and tests.
pub fn live_count() -> usize {
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        live.retain(|e| e.origin.strong_count() > 0);
        live.len()
    })
}

/// Forget every registration. Test support: the registry is
/// thread-local and per-process, so a suite that mounts in one case must
/// not leak into the next.
pub fn reset() {
    LIVE.with(|live| live.borrow_mut().clear());
}
