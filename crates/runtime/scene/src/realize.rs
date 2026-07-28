//! Realization: walk an [`Element`] tree once, delegate every `Item` to
//! its [`Registry`] handler, spawn one driver effect per structural hole,
//! and collect everything created into a [`Realized`] whose drop is the
//! entire unmount story.
//!
//! # Structural drivers
//!
//! The Dyn and Keyed drivers are direct ports of the old walker
//! (`runtime-core/src/walker/{when_switch,dynamic,each}.rs`), invariant
//! for invariant. Each invariant names the `scene-parity` golden that pins
//! it — those goldens are the P1 exit gate, and any change to the
//! orderings below must survive that suite.
//!
//! **The two OPPOSITE dispose orderings** (both deliberate, both pinned):
//!
//! - **Anchored** swap (`dispose_order_when.anchored.golden`): the old
//!   branch's scope drops FIRST (cleanups fire), THEN the anchor is
//!   cleared, then the new branch builds. The anchor keeps the region's
//!   place in the tree, so there is no window where a sibling's index
//!   shifts; dropping the scope first frees the old subtree's reactive
//!   slots atomically before any node is touched.
//! - **Spliced** swap (`dispose_order_when.spliced.golden`,
//!   `dispose_order_each.spliced.golden`): the old node(s) are
//!   `remove_child`'d FIRST, THEN the scope drops. Order matters — the
//!   native view leaves the tree before the scope frees so a reactive
//!   style/text effect in the old subtree can never fire against a
//!   half-detached node.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_world::{collect_owned, effect, untrack, Owned};

use crate::element::{DynKind, DynSpec, Element, Key, RetireHook};
use crate::host::Host;
use crate::registry::Registry;

// ============================================================================
// Live tree
// ============================================================================

/// A realized tree: the live structure plus the [`Owned`] scope holding
/// every signal/effect created during realization (driver effects, prop
/// bindings, absorbed component scopes). **Dropping this IS unmount** —
/// cleanups fire, subscriptions unlink, slots free. Structural detachment
/// of the root nodes is the caller's business (the enclosing region or
/// host does it), matching the old walker's split between scope teardown
/// and node removal.
pub struct Realized<N> {
    /// The live structure. Public so hosts (navigators, tests) can walk it.
    pub root: LiveNode<N>,
    owned: Owned,
}

impl<N: Clone> Realized<N> {
    /// The top-level backend nodes this tree contributes to a parent —
    /// see [`LiveNode::collect_nodes`].
    pub fn collect_nodes(&self) -> Vec<N> {
        self.root.collect_nodes()
    }

    /// The reactive scope this tree owns (diagnostics: slot counts).
    pub fn owned(&self) -> &Owned {
        &self.owned
    }
}

/// The live counterpart of an [`Element`]: real nodes where items were,
/// structure elsewhere.
pub enum LiveNode<N> {
    /// A mounted primitive and the live children its handler realized.
    Item { node: N, children: Vec<LiveNode<N>> },
    /// Flat siblings, no node of their own.
    Fragment(Vec<LiveNode<N>>),
    /// A live structural hole (see [`DynLive`]).
    Dyn(DynLive<N>),
    /// A live keyed list (see [`KeyedLive`]).
    Keyed(KeyedLive<N>),
}

impl<N: Clone> LiveNode<N> {
    /// The top-level backend nodes this subtree contributes to its parent,
    /// in order: an item is one node, a fragment is its children's nodes
    /// concatenated, an anchored region is its anchor, and a spliced
    /// region is its *current* spliced node set. This is the multi-node
    /// accounting unit the keyed reconciler works in
    /// (`each_multi_node_rows.spliced.golden`).
    pub fn collect_nodes(&self) -> Vec<N> {
        let mut out = Vec::new();
        self.collect_nodes_into(&mut out);
        out
    }

    fn collect_nodes_into(&self, out: &mut Vec<N>) {
        match self {
            LiveNode::Item { node, .. } => out.push(node.clone()),
            LiveNode::Fragment(children) => {
                for child in children {
                    child.collect_nodes_into(out);
                }
            }
            LiveNode::Dyn(dyn_live) => match &dyn_live.anchor {
                Some(anchor) => out.push(anchor.clone()),
                None => out.extend(dyn_live.slot.borrow().nodes.iter().cloned()),
            },
            LiveNode::Keyed(keyed) => match keyed {
                KeyedLive::Anchored { anchor, .. } => out.push(anchor.clone()),
                KeyedLive::Spliced { state } => {
                    for entry in &state.borrow().rows {
                        out.extend(entry.nodes.iter().cloned());
                    }
                }
            },
        }
    }
}

/// A live structural hole. `anchor` is `Some` on the anchored path (the
/// hole contributes exactly the anchor node) and `None` on the spliced
/// path (the hole contributes its current spliced nodes).
pub struct DynLive<N> {
    pub(crate) anchor: Option<N>,
    pub(crate) slot: Rc<RefCell<DynSlot<N>>>,
}

impl<N> DynLive<N> {
    /// Borrow the hole's current subtree (`None` before the first build —
    /// only observable if the driver's first fire panicked).
    pub fn with_current<R>(&self, f: impl FnOnce(Option<&Realized<N>>) -> R) -> R {
        f(self.slot.borrow().realized.as_ref())
    }
}

/// The swapped state behind a Dyn hole: the current subtree and (spliced
/// path) the node set it spliced into the parent.
pub(crate) struct DynSlot<N> {
    pub(crate) realized: Option<Realized<N>>,
    pub(crate) nodes: Vec<N>,
}

/// A live keyed list.
pub enum KeyedLive<N> {
    /// No-splice fallback: the whole list lives under an anchor and fully
    /// rebuilds on every change (`each_append.anchored.golden`).
    Anchored {
        anchor: N,
        slot: Rc<RefCell<Option<Realized<N>>>>,
    },
    /// Keyed reconciliation directly in the real parent.
    Spliced { state: Rc<RefCell<KeyedState<N>>> },
}

/// The mounted rows of a spliced keyed list, in list order.
pub struct KeyedState<N> {
    pub(crate) rows: Vec<KeyedEntry<N>>,
}

impl<N> KeyedState<N> {
    /// Number of mounted rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Borrow one row's realized subtree by key.
    pub fn with_row<R>(&self, key: &Key, f: impl FnOnce(&Realized<N>) -> R) -> Option<R> {
        self.rows
            .iter()
            .find(|e| e.key == *key)
            .map(|e| f(&e.realized))
    }
}

/// One mounted row: its identity, its realized subtree (owns the row's
/// signals/effects/cleanups), and the top-level nodes the row spliced into
/// the parent (a row body can be several flat siblings).
pub(crate) struct KeyedEntry<N> {
    pub(crate) key: Key,
    pub(crate) realized: Realized<N>,
    pub(crate) nodes: Vec<N>,
}

// ============================================================================
// MountCx + realization walk
// ============================================================================

/// The context handed to [`Registry`](crate::Registry) handlers: full
/// backend access plus the scene's child-realization entry points. Also
/// the internal walk state (live-node frames, absorbed component scopes).
pub struct MountCx<'a, H: Host> {
    backend: &'a Rc<RefCell<H>>,
    registry: &'a Rc<Registry<H>>,
    /// One frame per in-flight handler invocation: the live children that
    /// handler's `realize_children_into` calls have produced so far.
    frames: Vec<Vec<LiveNode<H::Node>>>,
    /// Component scopes (`Element::Owned`) encountered during this walk,
    /// folded into the resulting `Realized`'s `Owned` at the end.
    absorbed: Vec<Owned>,
}

impl<'a, H: Host> MountCx<'a, H> {
    /// The platform backend, for handlers that need `create_*`/prop calls.
    pub fn backend(&self) -> &Rc<RefCell<H>> {
        self.backend
    }

    /// The handler registry (rarely needed by handlers directly).
    pub fn registry(&self) -> &Rc<Registry<H>> {
        self.registry
    }

    /// Realize `children` into `parent`: the child-splice loop, port of
    /// the old walker's `view.rs::splice_children`. Static items append
    /// with plain `insert`; fragments splice flat; reactive regions take
    /// the spliced path (when [`Host::supports_splice`]) or nest under an
    /// anchor. A single threaded `inserted` counter runs THROUGH fragments
    /// so a reactive region after a fragment captures its correct ABSOLUTE
    /// base index in `parent` (`fragment_base_index.spliced.golden`).
    ///
    /// Call once per parent: the counter starts at 0, so a second batch of
    /// children for the same parent would mis-base later reactive regions.
    pub fn realize_children_into(&mut self, parent: &mut H::Node, children: Vec<Element>) {
        let mut inserted = 0usize;
        let nodes = self.splice_children(parent, children, &mut inserted);
        self.frames
            .last_mut()
            .expect("realize_children_into called outside a handler invocation")
            .extend(nodes);
    }

    /// Realize a detached single-root tree (navigator screens, portal
    /// content, virtualizer rows): returns the root node (not attached to
    /// anything) and the `Realized` that owns the subtree. Panics if the
    /// tree has zero or multiple top-level nodes — wrap fragment roots in
    /// an item.
    pub fn realize_detached(&mut self, element: Element) -> (H::Node, Realized<H::Node>) {
        let realized = build_realized(
            self.backend,
            self.registry,
            move || element,
            false,
            |cx, element| cx.realize_element_detached(element),
        );
        let mut nodes = realized.collect_nodes();
        match nodes.len() {
            1 => (nodes.pop().expect("len checked"), realized),
            n => panic!(
                "realize_detached requires a single-root element (got {n} top-level nodes) — \
                 wrap fragment roots in an item"
            ),
        }
    }

    // --- internal walk ---

    /// Peel `Element::Owned` wrappers, banking their scopes for the merge.
    fn unwrap_owned(&mut self, mut element: Element) -> Element {
        loop {
            match element {
                Element::Owned { element: inner, owned } => {
                    self.absorbed.push(owned);
                    element = *inner;
                }
                other => return other,
            }
        }
    }

    /// The core child-splice loop. `inserted` is threaded (never reset) so
    /// fragments — and any reactive region following one — see the
    /// absolute child index within `parent`.
    fn splice_children(
        &mut self,
        parent: &mut H::Node,
        children: Vec<Element>,
        inserted: &mut usize,
    ) -> Vec<LiveNode<H::Node>> {
        let mut out = Vec::with_capacity(children.len());
        for child in children {
            let child = self.unwrap_owned(child);
            match child {
                Element::Fragment(sub) => {
                    // Layout-transparent: splice the fragment's children
                    // directly into `parent`, sharing the SAME counter.
                    let nested = self.splice_children(parent, sub, inserted);
                    out.push(LiveNode::Fragment(nested));
                }
                other => out.push(self.realize_placed(parent, other, inserted)),
            }
        }
        out
    }

    /// Realize one non-fragment child into `parent`, advancing `inserted`
    /// by the number of top-level nodes the child contributed.
    fn realize_placed(
        &mut self,
        parent: &mut H::Node,
        element: Element,
        inserted: &mut usize,
    ) -> LiveNode<H::Node> {
        match element {
            Element::Item { .. } => {
                let live = self.mount_item(element);
                let LiveNode::Item { node, .. } = &live else {
                    unreachable!("mount_item returns LiveNode::Item")
                };
                self.backend.borrow_mut().insert(parent, node.clone());
                *inserted += 1;
                live
            }
            Element::Dyn(spec) => {
                if self.backend.borrow().supports_splice() {
                    // Anchorless: the hole splices its subtree directly
                    // into the real parent at the captured base index.
                    let (slot, count) = drive_dyn_spliced(
                        self.backend,
                        self.registry,
                        parent.clone(),
                        *inserted,
                        spec,
                    );
                    *inserted += count;
                    LiveNode::Dyn(DynLive { anchor: None, slot })
                } else {
                    let (anchor, slot) = drive_dyn_anchored(self.backend, self.registry, spec);
                    self.backend.borrow_mut().insert(parent, anchor.clone());
                    *inserted += 1;
                    LiveNode::Dyn(DynLive {
                        anchor: Some(anchor),
                        slot,
                    })
                }
            }
            Element::Keyed { items, render } => {
                if self.backend.borrow().supports_splice() {
                    let (state, count) = drive_keyed_spliced(
                        self.backend,
                        self.registry,
                        parent.clone(),
                        *inserted,
                        items,
                        render,
                    );
                    *inserted += count;
                    LiveNode::Keyed(KeyedLive::Spliced { state })
                } else {
                    let (anchor, slot) =
                        drive_keyed_anchored(self.backend, self.registry, items, render);
                    self.backend.borrow_mut().insert(parent, anchor.clone());
                    *inserted += 1;
                    LiveNode::Keyed(KeyedLive::Anchored { anchor, slot })
                }
            }
            Element::Fragment(_) => unreachable!("fragments are handled by splice_children"),
            Element::Owned { .. } => unreachable!("owned wrappers are peeled by unwrap_owned"),
        }
    }

    /// Realize one element with NO parent to splice into (subtree roots:
    /// the app root, keyed rows, detached trees). Items and fragments
    /// realize with their top nodes dangling (the caller attaches them);
    /// Dyn/Keyed take the ANCHORED path even on splice-capable hosts —
    /// a hole needs a stable parent to swap under, and with no parent the
    /// anchor provides it (matches the old walker, where a `when` at a
    /// subtree root always anchors — only children-list positions splice).
    fn realize_element_detached(&mut self, element: Element) -> LiveNode<H::Node> {
        let element = self.unwrap_owned(element);
        match element {
            Element::Item { .. } => self.mount_item(element),
            Element::Fragment(children) => LiveNode::Fragment(
                children
                    .into_iter()
                    .map(|child| self.realize_element_detached(child))
                    .collect(),
            ),
            Element::Dyn(spec) => {
                let (anchor, slot) = drive_dyn_anchored(self.backend, self.registry, spec);
                LiveNode::Dyn(DynLive {
                    anchor: Some(anchor),
                    slot,
                })
            }
            Element::Keyed { items, render } => {
                let (anchor, slot) =
                    drive_keyed_anchored(self.backend, self.registry, items, render);
                LiveNode::Keyed(KeyedLive::Anchored { anchor, slot })
            }
            Element::Owned { .. } => unreachable!("owned wrappers are peeled by unwrap_owned"),
        }
    }

    /// Mount one `Item` through its registered handler. The handler's
    /// `realize_children_into` calls land in a fresh frame, which becomes
    /// the item's live children.
    fn mount_item(&mut self, element: Element) -> LiveNode<H::Node> {
        let Element::Item { data, children } = element else {
            unreachable!("mount_item called on a non-Item")
        };
        let type_id = (*data).type_id();
        let handler = self.registry.get(type_id).unwrap_or_else(|| {
            panic!(
                "runtime-scene: no handler registered for item payload ({type_id:?}) — \
                 register one on this backend's Registry before realizing"
            )
        });
        let payload: Rc<dyn Any> = Rc::from(data);
        self.frames.push(Vec::new());
        let node = handler(self, payload, children);
        let kids = self.frames.pop().expect("handler frame imbalance");
        LiveNode::Item {
            node,
            children: kids,
        }
    }
}

/// Realize an element tree against a backend + registry. Must run with the
/// owning [`World`](runtime_world::World) ambient (inside `enter`, or
/// inside one of its effects). The walk runs UNTRACKED — a mount is a
/// snapshot; reactive regions subscribe through their own driver effects
/// (whose bodies re-enable tracking, per the kernel's nested-effect rule).
pub fn realize<H: Host>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    element: Element,
) -> Realized<H::Node> {
    build_realized(
        backend,
        registry,
        move || element,
        false,
        |cx, element| cx.realize_element_detached(element),
    )
}

/// Shared (re-)realization wrapper: runs the element PRODUCER and then the
/// mount walk inside one `collect_owned` scope, then folds absorbed
/// component scopes into the collector's `Owned`.
///
/// Two invariants live here:
///
/// - EVERY realization — mount, driver rebuild, keyed row — goes through
///   this collector: driver effects re-run from `World::flush` with no
///   active collector, and signals/effects created there would otherwise
///   be world-root-owned and leak until world drop (P0 handoff invariant).
/// - The PRODUCER (a branch builder, a row `render`) runs INSIDE the
///   collector too — element construction belongs to the subtree's scope,
///   exactly like the old walker's `with_scope(|| ...builder()...)`.
///   An effect a builder creates (probe cleanup, component effect) must
///   die with the subtree, not with the world.
///
/// `produce_tracked` selects the old walker's tracking split: a Plain Dyn
/// closure runs tracked (its reads ARE the deps — `dynamic.rs`), while
/// guarded builds and keyed row renders run untracked (only the
/// select/enumeration reads are deps). The mount walk itself is always
/// untracked; nested reactive constructs subscribe through their own
/// driver effects (the kernel's nested-effect tracking rule).
fn build_realized<'a, H: Host>(
    backend: &'a Rc<RefCell<H>>,
    registry: &'a Rc<Registry<H>>,
    produce: impl FnOnce() -> Element,
    produce_tracked: bool,
    mount: impl FnOnce(&mut MountCx<'a, H>, Element) -> LiveNode<H::Node>,
) -> Realized<H::Node> {
    let ((root, absorbed), mut owned) = collect_owned(|| {
        let element = if produce_tracked {
            produce()
        } else {
            untrack(produce)
        };
        untrack(|| {
            let mut cx = MountCx {
                backend,
                registry,
                frames: Vec::new(),
                absorbed: Vec::new(),
            };
            let root = mount(&mut cx, element);
            (root, cx.absorbed)
        })
    });
    for scope in absorbed {
        owned.merge(scope);
    }
    Realized { root, owned }
}

/// Realize a subtree detached, returning its top-level nodes alongside.
/// `produce` runs inside the subtree's collector (untracked).
fn build_detached<H: Host>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    produce: impl FnOnce() -> Element,
) -> (Vec<H::Node>, Realized<H::Node>) {
    let realized = build_realized(backend, registry, produce, false, |cx, element| {
        cx.realize_element_detached(element)
    });
    let nodes = realized.collect_nodes();
    (nodes, realized)
}

/// Realize a subtree INTO an anchor (append via the child-splice loop, so
/// plain `insert` for items and correct nesting for reactive content).
fn build_into_anchor<H: Host>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    anchor: &H::Node,
    produce: impl FnOnce() -> Element,
    produce_tracked: bool,
) -> Realized<H::Node> {
    build_realized(backend, registry, produce, produce_tracked, |cx, element| {
        let mut anchor_mut = anchor.clone();
        let mut inserted = 0usize;
        let mut nodes = cx.splice_children(&mut anchor_mut, vec![element], &mut inserted);
        if nodes.len() == 1 {
            nodes.pop().expect("len checked")
        } else {
            LiveNode::Fragment(nodes)
        }
    })
}

// ============================================================================
// Dyn drivers
// ============================================================================

/// Hand the swapped-out subtree to the retire hook, or drop it (the
/// default teardown). Called at the exact point the old scope would drop,
/// so hook-vs-drop changes teardown TIMING only, never op order.
fn retire_or_drop<N: 'static>(retire: &mut Option<RetireHook>, old: Realized<N>) {
    match retire {
        Some(hook) => hook(Box::new(old)),
        None => drop(old),
    }
}

/// The anchored Dyn driver — port of `when_switch.rs::build_when_closure`
/// / `dynamic.rs::build_dynamic`. Creates the anchor, then an effect that
/// on every (non-deduped) fire: drops the old subtree's scope FIRST, then
/// `clear_children(anchor)`, then builds + inserts the new subtree under
/// the anchor.
///
/// A VIRGIN anchor is not cleared: the old walker unconditionally cleared
/// the freshly created, still-empty anchor on the first fire (a harmless
/// no-op it never special-cased). Skipping it is sanctioned divergence #2
/// in scene-parity's README; the parity harness normalizes exactly this
/// pattern out of the anchored goldens for the new-core comparison.
fn drive_dyn_anchored<H: Host>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    spec: DynSpec,
) -> (H::Node, Rc<RefCell<DynSlot<H::Node>>>) {
    let anchor = backend.borrow_mut().create_anchor();
    let slot: Rc<RefCell<DynSlot<H::Node>>> = Rc::new(RefCell::new(DynSlot {
        realized: None,
        nodes: Vec::new(),
    }));

    let backend = backend.clone();
    let registry = registry.clone();
    let anchor_e = anchor.clone();
    let slot_e = slot.clone();
    let DynSpec { kind, mut retire } = spec;

    // The driver effect registers into the ambient collector (the
    // enclosing Realized's Owned) — dropping that Realized retires the
    // hole itself. It fires once synchronously here, mounting the initial
    // subtree into the anchor BEFORE the caller inserts the anchor.
    let _driver = effect(move || {
        // Guarded holes decide BEFORE any teardown: an unchanged guard
        // value means the mounted subtree is already correct — zero
        // structural ops (when_dedup_extra_signal). The decision read is
        // tracked; it is this effect's dependency set.
        if let DynKind::Guarded { changed, .. } = &kind {
            if !changed() {
                return;
            }
        }
        // ANCHORED DISPOSE ORDER (dispose_order_when.anchored.golden):
        // scope drop FIRST — cleanup markers land BEFORE clear_children.
        // The Realized is taken OUT of the slot before dropping so the old
        // subtree's cleanups can never observe a held slot borrow (P0
        // handoff: assignment-into-borrowed-slot hazard).
        let old = slot_e.borrow_mut().realized.take();
        if let Some(old) = old {
            retire_or_drop(&mut retire, old);
            backend.borrow_mut().clear_children(&anchor_e);
        }
        let realized = match &kind {
            // Plain: the closure runs TRACKED inside the fresh scope —
            // its reads are the rebuild deps (dynamic.rs).
            DynKind::Plain(f) => build_into_anchor(&backend, &registry, &anchor_e, || f(), true),
            // Guarded: the build runs UNTRACKED inside the fresh scope —
            // only the select's reads are deps (when_switch.rs).
            DynKind::Guarded { build, .. } => {
                build_into_anchor(&backend, &registry, &anchor_e, || build(), false)
            }
        };
        slot_e.borrow_mut().realized = Some(realized);
    });

    (anchor, slot)
}

/// The spliced Dyn driver — port of `when_switch.rs::build_when_spliced` /
/// `build_switch_spliced`. No anchor: the subtree's top nodes splice
/// directly into the real parent at the stable `base_index` captured from
/// the threaded child counter. Returns the region's initial node count so
/// the caller advances its running index for trailing siblings.
fn drive_dyn_spliced<H: Host>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    parent: H::Node,
    base_index: usize,
    spec: DynSpec,
) -> (Rc<RefCell<DynSlot<H::Node>>>, usize) {
    let slot: Rc<RefCell<DynSlot<H::Node>>> = Rc::new(RefCell::new(DynSlot {
        realized: None,
        nodes: Vec::new(),
    }));

    let backend = backend.clone();
    let registry = registry.clone();
    let slot_e = slot.clone();
    let DynSpec { kind, mut retire } = spec;

    let _driver = effect(move || {
        if let DynKind::Guarded { changed, .. } = &kind {
            if !changed() {
                return; // guard unchanged: zero structural ops
            }
        }
        // SPLICED DISPOSE ORDER (dispose_order_when.spliced.golden):
        // remove the old node(s) from the parent FIRST, THEN drop the old
        // scope — cleanup markers land AFTER remove_child. A reactive
        // effect in the old subtree must never fire against a
        // half-detached node.
        let (old_nodes, old_realized) = {
            let mut s = slot_e.borrow_mut();
            (std::mem::take(&mut s.nodes), s.realized.take())
            // borrow released here, before any teardown runs
        };
        if let Some(old) = old_realized {
            {
                let mut b = backend.borrow_mut();
                for node in &old_nodes {
                    b.remove_child(&parent, node);
                }
            }
            retire_or_drop(&mut retire, old);
        }
        // Build detached, then splice every top node at the stable base.
        // (base_index is stable as long as content BEFORE the region is
        // static — the same documented constraint the old walker had.)
        let (nodes, realized) = match &kind {
            DynKind::Plain(f) => {
                // Tracked producer — see build_realized's tracking split.
                let realized =
                    build_realized(&backend, &registry, || f(), true, |cx, element| {
                        cx.realize_element_detached(element)
                    });
                let nodes = realized.collect_nodes();
                (nodes, realized)
            }
            DynKind::Guarded { build, .. } => build_detached(&backend, &registry, || build()),
        };
        {
            let mut b = backend.borrow_mut();
            let mut p = parent.clone();
            for (i, node) in nodes.iter().enumerate() {
                b.insert_at(&mut p, node.clone(), base_index + i);
            }
        }
        let mut s = slot_e.borrow_mut();
        s.nodes = nodes;
        s.realized = Some(realized);
    });

    // The effect ran once synchronously above, so the slot holds the
    // initial node set — this region's contribution to the parent index.
    let count = slot.borrow().nodes.len();
    (slot, count)
}

// ============================================================================
// Keyed drivers
// ============================================================================

type KeyedItems = Box<dyn Fn() -> Vec<(Key, Box<dyn Any>)>>;
type KeyedRender = Box<dyn Fn(Box<dyn Any>) -> Element>;

/// The spliced Keyed driver — port of `each.rs::build_spliced` +
/// `reconcile`, the 4-pass keyed reconciler.
fn drive_keyed_spliced<H: Host>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    parent: H::Node,
    base_index: usize,
    items: KeyedItems,
    render: KeyedRender,
) -> (Rc<RefCell<KeyedState<H::Node>>>, usize) {
    let state = Rc::new(RefCell::new(KeyedState { rows: Vec::new() }));

    let backend = backend.clone();
    let registry = registry.clone();
    let state_e = state.clone();

    let _driver = effect(move || {
        // TRACKED: enumerating the items is the rebuild dependency set.
        let new_items = items();
        reconcile(
            &backend, &registry, &parent, base_index, new_items, &render, &state_e,
        );
    });

    let count = state.borrow().rows.iter().map(|e| e.nodes.len()).sum();
    (state, count)
}

/// One keyed reconcile pass — `each.rs::reconcile`, pass for pass.
fn reconcile<H: Host>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    parent: &H::Node,
    base_index: usize,
    items: Vec<(Key, Box<dyn Any>)>,
    render: &KeyedRender,
    state: &Rc<RefCell<KeyedState<H::Node>>>,
) {
    // Take the mounted rows OUT of the shared state so no borrow is held
    // across user code (row renders re-enter the reactive system, and row
    // teardown runs arbitrary cleanups).
    let old_rows = std::mem::take(&mut state.borrow_mut().rows);
    let mut old_slots: Vec<Option<KeyedEntry<H::Node>>> = old_rows.into_iter().map(Some).collect();

    // PASS A — walk the new order. A kept key carries its entry over
    // untouched (scope + nodes; `render` is NOT called again, so row-local
    // state survives). A new key realizes fresh, detached, in its own
    // collect_owned. `reorder` tracks the monotonic-survivor optimization:
    // if survivors' old positions stay strictly increasing across the new
    // order, NO survivor needs to move — a mid-list insert emits exactly
    // one insert_at and existing rows are never touched
    // (each_insert_middle_survivors.spliced.golden — focus/IME
    // preservation depends on this).
    let mut new_rows: Vec<(KeyedEntry<H::Node>, bool)> = Vec::with_capacity(items.len());
    let mut last_old_pos: isize = -1;
    let mut reorder = false;
    for (key, data) in items {
        let found = old_slots
            .iter()
            .position(|slot| matches!(slot, Some(entry) if entry.key == key));
        if let Some(i) = found {
            let entry = old_slots[i].take().expect("position() found it");
            drop(data); // kept row: the fresh payload is never rendered
            if (i as isize) < last_old_pos {
                reorder = true;
            }
            last_old_pos = i as isize;
            new_rows.push((entry, false));
        } else {
            // The row renders inside its OWN collector (untracked), so
            // row-local signals/effects die with the row, independently
            // of siblings.
            let (nodes, realized) = build_detached(backend, registry, || render(data));
            new_rows.push((
                KeyedEntry {
                    key,
                    realized,
                    nodes,
                },
                true,
            ));
        }
    }

    // PASS B — leftover old slots are removed keys: unmount them. Nodes
    // out FIRST, then the scope drops (dispose_order_each.spliced.golden —
    // a reactive effect in the row can't fire against a half-detached
    // node). Surviving rows are untouched.
    for slot in old_slots.iter_mut() {
        if let Some(entry) = slot.take() {
            {
                let mut b = backend.borrow_mut();
                for node in &entry.nodes {
                    b.remove_child(parent, node);
                }
            }
            drop(entry.realized); // row cleanups fire here
        }
    }

    // PASS C — place rows into the new order. `insert_at` inserts a new
    // node and MOVES an already-mounted one (DOM insertBefore semantics).
    // Survivors in order → only NEW rows insert; a real reorder
    // repositions every node of every row in target order
    // (each_reverse.spliced.golden, each_multi_node_rows.spliced.golden).
    {
        let mut b = backend.borrow_mut();
        let mut p = parent.clone();
        let mut pos = base_index;
        for (entry, is_new) in &new_rows {
            for node in &entry.nodes {
                if reorder || *is_new {
                    b.insert_at(&mut p, node.clone(), pos);
                }
                pos += 1;
            }
        }
    }

    // PASS D — publish the new ordering, flagging duplicate keys (a usage
    // error that makes row identity ambiguous).
    let mut st = state.borrow_mut();
    st.rows = Vec::with_capacity(new_rows.len());
    for (entry, _) in new_rows {
        if st.rows.iter().any(|e| e.key == entry.key) {
            panic!(
                "keyed list: duplicate key {:?} — every item needs a unique key",
                entry.key
            );
        }
        st.rows.push(entry);
    }
}

/// The anchored Keyed fallback — port of `each.rs::build`'s no-splice
/// branch: a FULL rebuild on every items change. Drop the entire previous
/// list scope first, clear the anchor, rebuild every row fresh (per-row
/// state is lost — pinned as the fallback's contract by
/// `each_append.anchored.golden`; preserving it requires splice support).
/// Same virgin-anchor rule as the anchored Dyn driver.
fn drive_keyed_anchored<H: Host>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    items: KeyedItems,
    render: KeyedRender,
) -> (H::Node, Rc<RefCell<Option<Realized<H::Node>>>>) {
    let anchor = backend.borrow_mut().create_anchor();
    let slot: Rc<RefCell<Option<Realized<H::Node>>>> = Rc::new(RefCell::new(None));

    let backend = backend.clone();
    let registry = registry.clone();
    let anchor_e = anchor.clone();
    let slot_e = slot.clone();

    let _driver = effect(move || {
        let new_items = items(); // TRACKED
        // Anchored dispose order, list-wide: all row scopes drop first,
        // then the anchor clears (mirrors the anchored Dyn swap).
        let old = slot_e.borrow_mut().take();
        if let Some(old) = old {
            drop(old);
            backend.borrow_mut().clear_children(&anchor_e);
        }
        // Rebuild every row into the anchor through the child-splice loop
        // (plain inserts, fragments flattened, nested regions recurse) —
        // ONE scope for the whole list, like the old fallback's single
        // rebuilt Scope. Renders run untracked inside that scope; the
        // fragment wrapper splices flat, so op order matches the old
        // "build all elements, then insert_children" sequence.
        let realized = build_into_anchor(
            &backend,
            &registry,
            &anchor_e,
            || {
                Element::Fragment(
                    new_items
                        .into_iter()
                        .map(|(_key, data)| render(data))
                        .collect(),
                )
            },
            false,
        );
        *slot_e.borrow_mut() = Some(realized);
    });

    (anchor, slot)
}
