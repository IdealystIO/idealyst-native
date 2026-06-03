//! Flex layout utility for native backends.
//!
//! Wraps [`taffy`] (a pure-Rust flex engine matching CSS semantics)
//! and translates `runtime_core::StyleRules` into Taffy styles.
//! Backends that don't have a native layout system (iOS, Android)
//! build a parallel layout tree as they create native nodes, run
//! Taffy when the tree is complete, and apply the resulting frames
//! to their native views.
//!
//! ## Usage shape (typical backend)
//!
//! ```ignore
//! use runtime_layout::{LayoutTree, LayoutNode};
//!
//! struct MyBackend {
//!     layout: LayoutTree,
//!     // (LayoutNode → native view) association is the backend's
//!     // choice — a Vec, a HashMap keyed by view pointer, or stored
//!     // alongside the native view in an enum variant.
//! }
//!
//! impl Backend for MyBackend {
//!     fn create_view(&mut self) -> Self::Node {
//!         let layout = self.layout.new_node();
//!         let native = make_native_view();
//!         MyNode::View { view: native, layout }
//!     }
//!     fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
//!         self.layout.add_child(parent.layout(), child.layout());
//!         attach_native(parent.view(), child.view());
//!     }
//!     fn apply_style(&mut self, node: &Self::Node, rules: &Rc<StyleRules>) {
//!         self.layout.set_style(node.layout(), rules);
//!         paint_native(node.view(), rules);
//!     }
//!     fn finish(&mut self, root: Self::Node) {
//!         let (w, h) = self.viewport_size();
//!         self.layout.compute(root.layout(), w, h);
//!         // walk and apply frames via my own (LayoutNode → view) map
//!     }
//! }
//! ```
//!
//! Web backends ignore this crate entirely — CSS does layout.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use taffy::prelude::*;
use taffy::TaffyTree;

// Re-export Taffy types backends need to write measure functions.
// Keeps `taffy` as a non-public dep of consumers.
pub use taffy::{AvailableSpace, Size};

use runtime_core::{
    AlignContent as FwAlignContent, AlignItems as FwAlignItems, AlignSelf as FwAlignSelf,
    FlexDirection as FwFlexDirection, FlexWrap as FwFlexWrap, JustifyContent as FwJustifyContent,
    Length as FwLength, Position as FwPosition, StyleRules,
};

/// Measure function signature. Taffy calls this for nodes that have
/// no explicit size, passing the cross-axis constraint (e.g. "you can
/// be at most this wide") and asking for the intrinsic size in the
/// remaining axis. Used by Text nodes that wrap — given an available
/// width, the backend asks the underlying widget (UILabel, TextView)
/// how tall it needs to be.
///
/// Arguments:
/// - `known_dimensions`: dimensions already pinned by ancestor layout.
///   `Some(w)` for width means "you must be exactly this wide".
/// - `available_space`: the space the parent is offering. Definite,
///   MinContent, or MaxContent.
///
/// Returns the size the node would like to be.
pub type MeasureFn =
    Rc<dyn Fn(Size<Option<f32>>, Size<AvailableSpace>) -> Size<f32>>;

// =============================================================================
// Public types
// =============================================================================

/// Opaque handle for a node in the layout tree. Mirrors Taffy's
/// `NodeId` but kept opaque so the engine can be swapped without
/// churning every backend.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LayoutNode(NodeId);

/// Final computed frame of a node after layout, in points / CSS
/// pixels. Origin is top-left of the parent's content box.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

// =============================================================================
// LayoutTree
// =============================================================================

/// A flex layout tree. Owned by the backend; one instance per
/// `Backend` typically suffices (or one per render root for
/// multi-window backends).
pub struct LayoutTree {
    tree: TaffyTree<()>,
    /// Set of `NodeId`s that have been freed via [`Self::remove_node`].
    /// Used purely as an assertion source: if anything calls
    /// [`Self::set_style`] / [`Self::set_safe_area_extra`] on a freed
    /// id, we panic with a clear message pointing at the bad caller
    /// instead of letting Taffy's SlotMap panic obscure the source.
    /// Temporary — remove once the wgpu backend lifecycle ordering is
    /// proven correct.
    dropped: HashSet<NodeId>,
    /// Per-node measure functions for intrinsically-sized leaves
    /// (Text, etc.). Looked up during `compute` and forwarded to
    /// Taffy's `compute_layout_with_measure`.
    measure_fns: HashMap<NodeId, MeasureFn>,
    /// Nodes whose width is author-intended `Auto`. `compute()` fills
    /// these with the viewport width on every call (so a viewport
    /// resize — orientation flip, iPad split-view — re-applies). Set
    /// on `new_node`; cleared by `set_style` when the author writes
    /// an explicit `width`.
    auto_width: HashSet<NodeId>,
    /// Same as `auto_width` for the height axis.
    auto_height: HashSet<NodeId>,
    /// Author-set padding in px, per-node, per-side. Tracked
    /// separately from the Taffy node's effective padding so we can
    /// re-combine with `safe_area_extra` whenever either changes
    /// (style updates, orientation flips, dynamic-island changes).
    /// Only px is supported in the combine path; if the author
    /// declares `padding: 5%`, the safe-area extra is silently
    /// ignored for that side. Acceptable — percent paddings are
    /// rare in practice and the alternative (a Taffy
    /// `LengthPercentage` expression language) costs more than it
    /// buys.
    author_padding: HashMap<NodeId, [f32; 4]>, // top, right, bottom, left
    /// Safe-area-driven extra padding in px, per-node, per-side.
    /// Combined with `author_padding` to produce Taffy's effective
    /// `style.padding`.
    safe_area_extra: HashMap<NodeId, [f32; 4]>, // top, right, bottom, left
}

impl LayoutTree {
    /// Construct an empty tree.
    pub fn new() -> Self {
        Self {
            tree: TaffyTree::new(),
            dropped: HashSet::new(),
            measure_fns: HashMap::new(),
            auto_width: HashSet::new(),
            auto_height: HashSet::new(),
            author_padding: HashMap::new(),
            safe_area_extra: HashMap::new(),
        }
    }

    /// Create a new leaf node (no children yet). Returns the handle
    /// the backend should associate with its native view.
    ///
    /// The seed style matches the framework's React-Native-like
    /// conventions: flex container, column flow, **stretch** in the
    /// cross axis. Taffy's `Style::default()` is `display: Block,
    /// flex_direction: Row, align_items: None` — that would arrange
    /// children of unstyled containers horizontally and size each to
    /// its intrinsic content width, leaving lots of empty space.
    /// `set_style` merges into the existing style rather than
    /// replacing it, so these seed values survive unless the author
    /// explicitly overrides them.
    pub fn new_node(&mut self) -> LayoutNode {
        let mut style = Style::default();
        style.display = Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.align_items = Some(AlignItems::Stretch);
        style.justify_content = Some(JustifyContent::FlexStart);
        let id = self
            .tree
            .new_leaf(style)
            .expect("taffy new_leaf");
        // Seed style leaves width and height as `Auto`; record that so
        // root nodes get filled to the viewport on each `compute()`.
        self.auto_width.insert(id);
        self.auto_height.insert(id);
        LayoutNode(id)
    }

    /// Opt a root node out of the viewport auto-fill on the named axes,
    /// so `compute()` leaves those `Auto` axes alone and the root wraps to
    /// its content instead of filling the viewport.
    ///
    /// Used by content-sized portal roots that the platform sizes and
    /// positions itself — e.g. a centered modal-card Dialog on Android
    /// (`WRAP_CONTENT` + `gravity=CENTER`): if Taffy force-fills the
    /// holder to the viewport, the card lands top-left inside a fullscreen
    /// holder instead of being centered. Wrapping the holder lets the
    /// window gravity do the centering. Per-axis because edge sheets fill
    /// one axis and wrap the other (a Top sheet is full-width, wrap-height).
    pub fn set_root_axes_wrap(&mut self, node: LayoutNode, wrap_width: bool, wrap_height: bool) {
        if wrap_width {
            self.auto_width.remove(&node.0);
        }
        if wrap_height {
            self.auto_height.remove(&node.0);
        }
    }

    /// Add `child` to `parent`'s child list. Order matches insertion;
    /// the backend should call this in the same order it would
    /// `addSubview` / `addView`.
    pub fn add_child(&mut self, parent: LayoutNode, child: LayoutNode) {
        // A node that was previously laid out as a ROOT has had its
        // `Auto` size axes overwritten with `Length(viewport)` by
        // `compute()` (so the root gets a definite size for Taffy). When
        // such a node is now reparented as a child, that baked-in
        // viewport width/height must revert to `Auto` — otherwise the
        // child carries a hardcoded full-viewport dimension into its new
        // parent's flex layout, overriding the cross-axis stretch.
        //
        // This bug surfaced on the iOS runtime-server drawer: dev-client
        // creates a sidebar "holder" view (Taffy root, auto_width), the
        // first layout pass computes it at the viewport width (393pt) and
        // bakes `Length(393)` into its style, then the chrome handler
        // adopts the holder as a child of the 280pt sidebar wrapper. The
        // baked 393 stuck → the holder (and its `width:100%` content)
        // rendered full-bleed past the 280pt panel. `auto_width` is the
        // signal that the node never had an author-set width, so the
        // baked viewport value is purely a root artifact to undo here.
        self.revert_root_baked_size(child);
        self.tree
            .add_child(parent.0, child.0)
            .expect("taffy add_child");
    }

    /// Insert `child` into `parent` at a specific `child_index` (clamped
    /// by the caller). Companion to [`add_child`](Self::add_child) for
    /// anchorless reactive regions that splice their rows at a stable
    /// base index instead of always appending. Applies the same
    /// root-baked-size revert.
    pub fn add_child_at_index(
        &mut self,
        parent: LayoutNode,
        child: LayoutNode,
        index: usize,
    ) {
        self.revert_root_baked_size(child);
        let count = self.tree.children(parent.0).map(|c| c.len()).unwrap_or(0);
        let idx = index.min(count);
        self.tree
            .insert_child_at_index(parent.0, idx, child.0)
            .expect("taffy insert_child_at_index");
    }

    /// A node previously laid out as a ROOT has had its `Auto` size axes
    /// overwritten with `Length(viewport)` by `compute()`. When it's
    /// reparented as a child, revert those baked dimensions to `Auto`
    /// (only on axes the author never set), so the child doesn't carry a
    /// hardcoded full-viewport size into its new parent's flex layout.
    /// See [`add_child`](Self::add_child)'s history for the iOS drawer
    /// bug this guards against.
    fn revert_root_baked_size(&mut self, child: LayoutNode) {
        if self.auto_width.contains(&child.0) || self.auto_height.contains(&child.0) {
            if let Ok(mut style) = self.tree.style(child.0).cloned() {
                if self.auto_width.contains(&child.0) {
                    style.size.width = Dimension::Auto;
                }
                if self.auto_height.contains(&child.0) {
                    style.size.height = Dimension::Auto;
                }
                let _ = self.tree.set_style(child.0, style);
            }
        }
    }

    /// Remove `child` from `parent` (for dynamic mounts / unmounts).
    pub fn remove_child(&mut self, parent: LayoutNode, child: LayoutNode) {
        let _ = self.tree.remove_child(parent.0, child.0);
    }

    /// Drop a node entirely (frees its slot in the tree).
    pub fn remove_node(&mut self, node: LayoutNode) {
        let _ = self.tree.remove(node.0);
        self.measure_fns.remove(&node.0);
        self.auto_width.remove(&node.0);
        self.auto_height.remove(&node.0);
        self.author_padding.remove(&node.0);
        self.safe_area_extra.remove(&node.0);
        self.dropped.insert(node.0);
    }

    /// Set per-side safe-area extra padding for a node. The Taffy
    /// node's effective padding becomes `author_padding +
    /// safe_area_extra` on each side. Called by the backend
    /// reactively whenever the platform reports a safe-area change
    /// (orientation, dynamic island, sheet adaptation) on nodes the
    /// author opted in via `.safe_area(...)`.
    ///
    /// Pass zeros for sides the node doesn't opt into — the backend
    /// is the one that masks per-side based on `SafeAreaSides`.
    pub fn set_safe_area_extra(
        &mut self,
        node: LayoutNode,
        top: f32,
        right: f32,
        bottom: f32,
        left: f32,
    ) {
        assert!(
            !self.dropped.contains(&node.0),
            "LayoutTree::set_safe_area_extra called on already-removed node {:?} — \
             a safe-area reactive effect outlived its `drop_subtree`. \
             Check the framework `Scope` ordering at the call site.",
            node.0
        );
        let extra = [top, right, bottom, left];
        // Skip the Taffy write if nothing changed — common during a
        // layout pass on every frame.
        if self.safe_area_extra.get(&node.0).copied() == Some(extra) {
            return;
        }
        self.safe_area_extra.insert(node.0, extra);

        let author = self.author_padding.get(&node.0).copied().unwrap_or([0.0; 4]);
        let mut style = self
            .tree
            .style(node.0)
            .cloned()
            .unwrap_or(Style::default());
        style.padding.top = LengthPercentage::Length(author[0] + top);
        style.padding.right = LengthPercentage::Length(author[1] + right);
        style.padding.bottom = LengthPercentage::Length(author[2] + bottom);
        style.padding.left = LengthPercentage::Length(author[3] + left);
        self.tree.set_style(node.0, style).expect("taffy set_style");
    }

    /// Apply the framework's resolved style rules to a node by
    /// *merging into* the existing Taffy style — only fields the
    /// author explicitly set in `StyleRules` get updated; everything
    /// else keeps its previous value.
    ///
    /// This shape is essential because `apply_style` runs more than
    /// once per node (base style + state overlays for hover/press
    /// etc.). A state overlay typically only sets a handful of
    /// properties (e.g. `background: hover_color`); if we naively
    /// replaced the whole Taffy style we'd lose every other property
    /// the base style established (gap, padding, flex_direction,
    /// width, …) — which is exactly the bug we saw with the dashboard
    /// page's gap getting wiped.
    pub fn set_style(&mut self, node: LayoutNode, rules: &StyleRules) {
        // Surface lifecycle bugs cleanly: if a caller hands us a node
        // that was already freed via `remove_node`, panic *here* with a
        // backtrace pointing at the bad caller instead of letting
        // Taffy's SlotMap panic obscure the chain. The actual fix
        // belongs at the call site — keep cohort entries / reactive
        // style effects from outliving their layout node.
        assert!(
            !self.dropped.contains(&node.0),
            "LayoutTree::set_style called on already-removed node {:?} — \
             a `StyleHandle` or reactive style effect outlived its `drop_subtree`. \
             Check that the framework `Scope` owning this node is released \
             BEFORE the backend frees the Taffy slot.",
            node.0
        );
        let mut style = self
            .tree
            .style(node.0)
            .cloned()
            .unwrap_or(Style::default());

        // Display is always Flex for framework views.
        style.display = Display::Flex;

        // Position — always set (Relative/Absolute is binary, no
        // "unset" form in our rules).
        style.position = match rules.position {
            Some(FwPosition::Absolute) => Position::Absolute,
            _ => Position::Relative,
        };

        // --- Flex container properties ---

        if let Some(d) = rules.flex_direction {
            style.flex_direction = match d {
                FwFlexDirection::Row => FlexDirection::Row,
                FwFlexDirection::Column => FlexDirection::Column,
                FwFlexDirection::RowReverse => FlexDirection::RowReverse,
                FwFlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
            };
        }
        if let Some(w) = rules.flex_wrap {
            style.flex_wrap = match w {
                FwFlexWrap::NoWrap => FlexWrap::NoWrap,
                FwFlexWrap::Wrap => FlexWrap::Wrap,
                FwFlexWrap::WrapReverse => FlexWrap::WrapReverse,
            };
        }
        if let Some(jc) = rules.justify_content {
            style.justify_content = Some(match jc {
                FwJustifyContent::FlexStart => JustifyContent::FlexStart,
                FwJustifyContent::FlexEnd => JustifyContent::FlexEnd,
                FwJustifyContent::Center => JustifyContent::Center,
                FwJustifyContent::SpaceBetween => JustifyContent::SpaceBetween,
                FwJustifyContent::SpaceAround => JustifyContent::SpaceAround,
                FwJustifyContent::SpaceEvenly => JustifyContent::SpaceEvenly,
            });
        }
        if let Some(ai) = rules.align_items {
            style.align_items = Some(match ai {
                FwAlignItems::FlexStart => AlignItems::FlexStart,
                FwAlignItems::FlexEnd => AlignItems::FlexEnd,
                FwAlignItems::Center => AlignItems::Center,
                FwAlignItems::Stretch => AlignItems::Stretch,
                FwAlignItems::Baseline => AlignItems::Baseline,
            });
        }
        if let Some(ac) = rules.align_content {
            style.align_content = Some(match ac {
                FwAlignContent::FlexStart => AlignContent::FlexStart,
                FwAlignContent::FlexEnd => AlignContent::FlexEnd,
                FwAlignContent::Center => AlignContent::Center,
                FwAlignContent::Stretch => AlignContent::Stretch,
                FwAlignContent::SpaceBetween => AlignContent::SpaceBetween,
                FwAlignContent::SpaceAround => AlignContent::SpaceAround,
            });
        }
        if let Some(gap) = rules.gap.as_ref().map(|t| t.value()) {
            let lp = length_to_lp(*gap);
            style.gap = Size { width: lp, height: lp };
        }
        if let Some(g) = rules.row_gap.as_ref().map(|t| t.value()) {
            style.gap.height = length_to_lp(*g);
        }
        if let Some(g) = rules.column_gap.as_ref().map(|t| t.value()) {
            style.gap.width = length_to_lp(*g);
        }

        // --- Flex item properties ---

        if let Some(grow) = rules.flex_grow.as_ref().map(|t| *t.value()) {
            style.flex_grow = grow;
        }
        if let Some(shrink) = rules.flex_shrink.as_ref().map(|t| *t.value()) {
            style.flex_shrink = shrink;
        }
        if let Some(basis) = rules.flex_basis.as_ref().map(|t| *t.value()) {
            style.flex_basis = length_to_dim(basis);
        }
        if let Some(asf) = rules.align_self {
            style.align_self = match asf {
                FwAlignSelf::Auto => None,
                FwAlignSelf::FlexStart => Some(AlignSelf::FlexStart),
                FwAlignSelf::FlexEnd => Some(AlignSelf::FlexEnd),
                FwAlignSelf::Center => Some(AlignSelf::Center),
                FwAlignSelf::Stretch => Some(AlignSelf::Stretch),
                FwAlignSelf::Baseline => Some(AlignSelf::Baseline),
            };
        }

        // --- Sizing ---

        if let Some(w) = rules.width.as_ref().map(|t| *t.value()) {
            style.size.width = length_to_dim(w);
            self.auto_width.remove(&node.0);
        }
        if let Some(h) = rules.height.as_ref().map(|t| *t.value()) {
            style.size.height = length_to_dim(h);
            self.auto_height.remove(&node.0);
        }
        if let Some(w) = rules.min_width.as_ref().map(|t| *t.value()) {
            style.min_size.width = length_to_dim(w);
        }
        if let Some(h) = rules.min_height.as_ref().map(|t| *t.value()) {
            style.min_size.height = length_to_dim(h);
        }
        if let Some(w) = rules.max_width.as_ref().map(|t| *t.value()) {
            style.max_size.width = length_to_dim(w);
        }
        if let Some(h) = rules.max_height.as_ref().map(|t| *t.value()) {
            style.max_size.height = length_to_dim(h);
        }
        if let Some(ar) = rules.aspect_ratio {
            style.aspect_ratio = Some(ar);
        }

        // --- Padding (per-side, all optional) ---
        //
        // Author padding is tracked separately so safe-area extras
        // can be re-combined on every change. For each side: snapshot
        // the author value into `author_padding`, then write
        // `author + safe_area_extra` to Taffy. Only px is combined
        // (the common case); a percent author value falls back to a
        // pure author write — safe-area on the same side is silently
        // skipped in that mode.
        if let Some(v) = rules.padding_top.as_ref().map(|t| *t.value()) {
            if let FwLength::Px(px) = v {
                self.author_padding.entry(node.0).or_insert([0.0; 4])[0] = px;
                let extra = self.safe_area_extra.get(&node.0).map(|e| e[0]).unwrap_or(0.0);
                style.padding.top = LengthPercentage::Length(px + extra);
            } else {
                style.padding.top = length_to_lp(v);
            }
        }
        if let Some(v) = rules.padding_right.as_ref().map(|t| *t.value()) {
            if let FwLength::Px(px) = v {
                self.author_padding.entry(node.0).or_insert([0.0; 4])[1] = px;
                let extra = self.safe_area_extra.get(&node.0).map(|e| e[1]).unwrap_or(0.0);
                style.padding.right = LengthPercentage::Length(px + extra);
            } else {
                style.padding.right = length_to_lp(v);
            }
        }
        if let Some(v) = rules.padding_bottom.as_ref().map(|t| *t.value()) {
            if let FwLength::Px(px) = v {
                self.author_padding.entry(node.0).or_insert([0.0; 4])[2] = px;
                let extra = self.safe_area_extra.get(&node.0).map(|e| e[2]).unwrap_or(0.0);
                style.padding.bottom = LengthPercentage::Length(px + extra);
            } else {
                style.padding.bottom = length_to_lp(v);
            }
        }
        if let Some(v) = rules.padding_left.as_ref().map(|t| *t.value()) {
            if let FwLength::Px(px) = v {
                self.author_padding.entry(node.0).or_insert([0.0; 4])[3] = px;
                let extra = self.safe_area_extra.get(&node.0).map(|e| e[3]).unwrap_or(0.0);
                style.padding.left = LengthPercentage::Length(px + extra);
            } else {
                style.padding.left = length_to_lp(v);
            }
        }

        // --- Margin (per-side, all optional) ---

        if let Some(v) = rules.margin_top.as_ref().map(|t| *t.value()) {
            style.margin.top = length_to_lpa(Some(v));
        }
        if let Some(v) = rules.margin_right.as_ref().map(|t| *t.value()) {
            style.margin.right = length_to_lpa(Some(v));
        }
        if let Some(v) = rules.margin_bottom.as_ref().map(|t| *t.value()) {
            style.margin.bottom = length_to_lpa(Some(v));
        }
        if let Some(v) = rules.margin_left.as_ref().map(|t| *t.value()) {
            style.margin.left = length_to_lpa(Some(v));
        }

        // --- Inset (top/right/bottom/left for position: absolute) ---

        if let Some(v) = rules.top.as_ref().map(|t| *t.value()) {
            style.inset.top = length_to_lpa(Some(v));
        }
        if let Some(v) = rules.right.as_ref().map(|t| *t.value()) {
            style.inset.right = length_to_lpa(Some(v));
        }
        if let Some(v) = rules.bottom.as_ref().map(|t| *t.value()) {
            style.inset.bottom = length_to_lpa(Some(v));
        }
        if let Some(v) = rules.left.as_ref().map(|t| *t.value()) {
            style.inset.left = length_to_lpa(Some(v));
        }

        self.tree
            .set_style(node.0, style)
            .expect("taffy set_style");
    }

    /// Install a measure function for a node. Taffy calls it during
    /// layout when the node has no explicit size, passing the
    /// available cross-axis size and expecting the intrinsic main-axis
    /// size in return. Use this for content that wraps (Text) so the
    /// engine asks the platform widget for its wrapped height given
    /// an available width.
    pub fn set_measure_fn(&mut self, node: LayoutNode, f: MeasureFn) {
        self.measure_fns.insert(node.0, f);
        // Tell Taffy this leaf has a measure func so it doesn't
        // collapse to its `size`.
        let _ = self.tree.mark_dirty(node.0);
    }

    /// Mark a node as needing re-measure on the next layout pass.
    /// Used by native backends when the underlying widget's intrinsic
    /// content changed (e.g. `UILabel.text` swapped) but neither the
    /// layout style nor the children topology changed — Taffy would
    /// otherwise use its cached size.
    pub fn mark_dirty(&mut self, node: LayoutNode) {
        let _ = self.tree.mark_dirty(node.0);
    }

    /// True iff Taffy has marked this node (or any of its descendants
    /// via `mark_dirty` propagation) as needing recomputation. Backends
    /// use this to skip `compute()` on hidden persistent roots whose
    /// subtree hasn't been touched since the last layout pass — a
    /// stack navigator that keeps N screens mounted otherwise pays
    /// N × per-root layout cost on every refresh, even though only
    /// one root is active.
    pub fn is_dirty(&self, node: LayoutNode) -> bool {
        self.tree.dirty(node.0).unwrap_or(true)
    }

    /// Mark a node as a scroll container on the given axis. Maps to
    /// Taffy's `overflow.x` / `overflow.y` = `Overflow::Scroll`, which
    /// (a) gives the node a definite main-axis size from its parent's
    /// constraint rather than from its children's content (so the
    /// scroll viewport doesn't grow to fit its content) and (b)
    /// disables children's contribution to the parent's intrinsic
    /// size on the scroll axis.
    ///
    /// Backends that render their own scroll machinery (terminal's
    /// cell-grid clip + offset; future hosts) call this at create
    /// time so the Taffy layout matches the rendered scroll-viewport
    /// behavior. Native backends with platform scroll views
    /// (UIScrollView, etc.) don't need this — the scroll view's
    /// native frame is set by its parent and its content has its own
    /// coordinate space.
    pub fn set_overflow_scroll(&mut self, node: LayoutNode, horizontal: bool) {
        let _ = self.tree.set_style(node.0, {
            let mut style = self
                .tree
                .style(node.0)
                .cloned()
                .unwrap_or(Style::default());
            if horizontal {
                style.overflow.x = taffy::Overflow::Scroll;
            } else {
                style.overflow.y = taffy::Overflow::Scroll;
            }
            // `flex_basis: 0` + `flex_grow: 1` tells Taffy "fill the
            // available main-axis space from the parent" rather than
            // "be as big as your content". Without this the ScrollView
            // would size itself to its content (and so have zero
            // scrollable area). Author styles can still override these
            // — `set_style` preserves Taffy state and only writes the
            // fields their `StyleRules` explicitly set.
            style.flex_basis = taffy::Dimension::from_length(0.0);
            style.flex_grow = 1.0;
            style
        });
    }

    /// Set a node's intrinsic content size. Used by native backends
    /// to seed Text / Button / Image / etc. with the size their
    /// native widget would prefer (UIView.intrinsicContentSize,
    /// View.measure(...)).
    ///
    /// We write the intrinsic into `min_size` (a floor) AND
    /// `flex_basis` (the main-axis content size). We **deliberately
    /// leave `size` as `Auto`** so the CSS Flex `align-items: stretch`
    /// behavior applies — when the parent stretches its children in
    /// the cross axis, the child can only grow if its cross size is
    /// `auto`. If we wrote the intrinsic into `size.width`, a button
    /// inside a vertical column would be stuck at its content width
    /// instead of stretching to fill the column.
    ///
    /// Author styles that explicitly set `width`/`height` still
    /// override — `set_style` writes those into `size` directly.
    pub fn set_intrinsic_size(&mut self, node: LayoutNode, width: f32, height: f32) {
        let mut style = self
            .tree
            .style(node.0)
            .cloned()
            .unwrap_or(Style::default());
        if width >= 0.0 {
            style.min_size.width = Dimension::Length(width);
        }
        if height >= 0.0 {
            style.min_size.height = Dimension::Length(height);
        }
        // flex_basis seeds the main-axis content size for flex items.
        // Without a measure_func, Taffy needs *some* hint of what the
        // content's natural main-axis extent is; using the larger of
        // width/height as a heuristic isn't right in general but works
        // for the common case (Text/Button placed in row or column).
        // Better: a real measure_func that reports per-axis content
        // size — already plumbed via `set_measure_fn`.
        self.tree
            .set_style(node.0, style)
            .expect("taffy set_style");
    }

    /// Run flex layout against the given viewport size. Frames are
    /// then readable via [`Self::frame_of`].
    ///
    /// Roots get their size forced to the viewport — without this,
    /// a root whose author style is `width: auto / height: auto`
    /// would collapse to its children's intrinsic size (often zero
    /// for empty subtrees), making the whole subtree invisible.
    /// Authors are presumed to want the root to fill its host area
    /// unless they explicitly override.
    pub fn compute(&mut self, root: LayoutNode, width: f32, height: f32) {
        // Fill viewport on axes the author left as `Auto`, but
        // preserve explicit `width` / `height`. Without this fallback,
        // a root with `width: auto / height: auto` would collapse to
        // its children's intrinsic size (often zero for empty
        // subtrees), making the whole subtree invisible. With it, a
        // root that does set an explicit width (e.g. a 320pt drawer
        // sidebar) keeps that width and won't get expanded to the
        // full viewport.
        //
        // We consult `auto_width` / `auto_height` rather than reading
        // the current style — the previous `compute()` call already
        // overwrote the style's `Auto` axes with `Length(viewport)`,
        // so checking the style itself would falsely conclude that
        // the author had set them explicitly and skip refilling on
        // the next viewport change (orientation flip, etc.).
        let mut style = self
            .tree
            .style(root.0)
            .cloned()
            .unwrap_or(Style::default());
        if self.auto_width.contains(&root.0) {
            style.size.width = Dimension::Length(width);
        }
        if self.auto_height.contains(&root.0) {
            style.size.height = Dimension::Length(height);
        }
        self.tree.set_style(root.0, style).expect("taffy set_style");

        let space = Size {
            width: AvailableSpace::Definite(width),
            height: AvailableSpace::Definite(height),
        };
        // Take the measure_fns out so the closure passed to
        // `compute_layout_with_measure` doesn't have to borrow `self`
        // (the closure runs *inside* `self.tree.compute_layout_with_measure`
        // which holds a mutable borrow on the tree).
        let measure_fns = std::mem::take(&mut self.measure_fns);
        self.tree
            .compute_layout_with_measure(
                root.0,
                space,
                |known_dimensions, available_space, node_id, _ctx, _style| {
                    match measure_fns.get(&node_id) {
                        Some(f) => f(known_dimensions, available_space),
                        None => Size::ZERO,
                    }
                },
            )
            .expect("taffy compute_layout");
        self.measure_fns = measure_fns;
    }

    /// Read the most recently computed frame for `node`. Returns the
    /// zero frame if [`Self::compute`] hasn't run, or if `node` was
    /// never registered.
    pub fn frame_of(&self, node: LayoutNode) -> Frame {
        let layout = self.tree.layout(node.0).copied().unwrap_or_default();
        Frame {
            x: layout.location.x,
            y: layout.location.y,
            width: layout.size.width,
            height: layout.size.height,
        }
    }

    /// Returns `true` if `node` has no parent in the layout tree —
    /// i.e. it's a root that the caller should run [`Self::compute`]
    /// against. Backends with multiple disconnected subtrees (iOS's
    /// per-screen mounts via `mount_screen_in_vc`) use this to find
    /// all roots after build.
    pub fn is_root(&self, node: LayoutNode) -> bool {
        self.tree.parent(node.0).is_none()
    }

    /// Debug: format a node's resolved Taffy style as a string, for
    /// log diagnostics. Includes the flex-container props that
    /// matter for "why isn't this view positioned where I expect"
    /// debugging.
    pub fn debug_style(&self, node: LayoutNode) -> String {
        let s = self
            .tree
            .style(node.0)
            .cloned()
            .unwrap_or(Style::default());
        format!(
            "flex_dir={:?} justify={:?} align_items={:?} gap=({:?},{:?}) padding={:?},{:?},{:?},{:?} size=({:?},{:?}) min=({:?},{:?})",
            s.flex_direction,
            s.justify_content,
            s.align_items,
            s.gap.width, s.gap.height,
            s.padding.top, s.padding.right, s.padding.bottom, s.padding.left,
            s.size.width, s.size.height,
            s.min_size.width, s.min_size.height,
        )
    }

    /// Return the direct children of `node`. Used by backends that
    /// need to walk a subtree's resolved frames (e.g. iOS's
    /// `scrollView.contentSize` sync, which sums child extents to
    /// determine the scrollable area).
    pub fn children_of(&self, node: LayoutNode) -> Vec<LayoutNode> {
        self.tree
            .children(node.0)
            .map(|cs| cs.into_iter().map(LayoutNode).collect())
            .unwrap_or_default()
    }

    /// Return `node`'s parent in the layout tree, or `None` if it's a
    /// root. Used by backends that need to walk a subtree's resolved
    /// frames upward — e.g. iOS's `Position::Sticky` impl sums Taffy
    /// frame Y values from a sticky child to its enclosing scroll
    /// view to derive the child's natural y in the scroll view's
    /// content coordinate space (unaffected by UIKit transforms).
    pub fn parent_of(&self, node: LayoutNode) -> Option<LayoutNode> {
        self.tree.parent(node.0).map(LayoutNode)
    }
}

impl Default for LayoutTree {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Length conversion helpers (used by set_style)
// =============================================================================

fn length_to_lp(l: FwLength) -> LengthPercentage {
    match l {
        FwLength::Px(v) => LengthPercentage::Length(v),
        FwLength::Percent(v) => LengthPercentage::Percent(v / 100.0),
        FwLength::Auto => LengthPercentage::Length(0.0),
    }
}

fn length_to_lpa(l: Option<FwLength>) -> LengthPercentageAuto {
    match l {
        Some(FwLength::Px(v)) => LengthPercentageAuto::Length(v),
        Some(FwLength::Percent(v)) => LengthPercentageAuto::Percent(v / 100.0),
        Some(FwLength::Auto) | None => LengthPercentageAuto::Auto,
    }
}

fn length_to_dim(l: FwLength) -> Dimension {
    match l {
        FwLength::Px(v) => Dimension::Length(v),
        FwLength::Percent(v) => Dimension::Percent(v / 100.0),
        FwLength::Auto => Dimension::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::Tokenized;

    fn px(v: f32) -> Tokenized<FwLength> {
        Tokenized::Literal(FwLength::Px(v))
    }
    fn pct(v: f32) -> Tokenized<FwLength> {
        Tokenized::Literal(FwLength::Percent(v))
    }

    /// Reproduces the welcome example's sun-glare wrapper layout
    /// to verify Taffy resolves width from height + aspect_ratio
    /// for an absolutely-positioned child.
    ///
    /// Setup: page (100% × 100%, relative) → wrapper (abs, top:0,
    /// right:0, height: 60%, aspect_ratio: 1.0). With viewport
    /// 390 × 844, the wrapper's height should be 506.4 and width
    /// should also be 506.4 (from aspect_ratio).
    #[test]
    fn aspect_ratio_resolves_width_from_height_pct_on_abs_child() {
        let mut t = LayoutTree::new();

        let page = t.new_node();
        let mut page_rules = StyleRules::default();
        page_rules.position = Some(FwPosition::Relative);
        page_rules.width = Some(pct(100.0));
        page_rules.height = Some(pct(100.0));
        t.set_style(page, &page_rules);

        let wrapper = t.new_node();
        let mut wrapper_rules = StyleRules::default();
        wrapper_rules.position = Some(FwPosition::Absolute);
        wrapper_rules.top = Some(px(0.0));
        wrapper_rules.right = Some(px(0.0));
        wrapper_rules.height = Some(pct(60.0));
        wrapper_rules.aspect_ratio = Some(1.0);
        t.set_style(wrapper, &wrapper_rules);
        t.add_child(page, wrapper);

        t.compute(page, 390.0, 844.0);

        let pf = t.frame_of(page);
        let wf = t.frame_of(wrapper);
        eprintln!("page  : {:?}", pf);
        eprintln!("wrap  : {:?}", wf);

        let expected = 844.0 * 0.6;
        assert!(
            (wf.height - expected).abs() < 0.5,
            "wrapper height {} should be ~{}",
            wf.height,
            expected
        );
        assert!(
            (wf.width - expected).abs() < 0.5,
            "wrapper width {} should be ~{} (resolved from height via aspect_ratio)",
            wf.width,
            expected
        );
    }

    /// Regression: a node that was laid out as a ROOT (viewport-filled)
    /// and is later reparented as a child must NOT keep the baked-in
    /// viewport width — it should revert to flex/stretch sizing under
    /// its new parent.
    ///
    /// Mirrors the iOS runtime-server drawer bug: dev-client builds the
    /// sidebar "holder" as a standalone Taffy root (auto_width); the
    /// first layout pass computes it at viewport width and bakes
    /// `Length(viewport)` into its style; then the drawer handler adopts
    /// the holder as a child of a 280pt sidebar wrapper. Before the fix
    /// the holder kept its 393pt root width and rendered full-bleed past
    /// the panel; after the fix `add_child` reverts the baked viewport
    /// width to `Auto` so the holder stretches to the 280pt wrapper.
    #[test]
    fn regression_reparented_root_drops_baked_viewport_width() {
        let mut t = LayoutTree::new();

        // Holder: a fresh node (no author width → auto_width) laid out
        // ONCE as a root against the full viewport. This bakes
        // Length(393) into its style — exactly what dev-client's
        // standalone `create_view` + first layout pass does.
        let holder = t.new_node();
        t.compute(holder, 393.0, 852.0);
        assert!(
            (t.frame_of(holder).width - 393.0).abs() < 0.5,
            "holder as a root should fill the viewport width (393), got {}",
            t.frame_of(holder).width
        );

        // Wrapper: explicit 280pt width, column flow, stretch children
        // (the iOS drawer's `sized_sidebar`). Adopt the holder as its
        // child — this is the reparent that must drop the baked 393.
        let wrapper = t.new_node();
        let mut wrapper_rules = StyleRules::default();
        wrapper_rules.width = Some(px(280.0));
        wrapper_rules.height = Some(pct(100.0));
        wrapper_rules.flex_direction = Some(FwFlexDirection::Column);
        t.set_style(wrapper, &wrapper_rules);
        t.add_child(wrapper, holder);

        t.compute(wrapper, 393.0, 852.0);

        let wf = t.frame_of(wrapper);
        let hf = t.frame_of(holder);
        assert!(
            (wf.width - 280.0).abs() < 0.5,
            "wrapper should be its explicit 280pt, got {}",
            wf.width
        );
        assert!(
            (hf.width - 280.0).abs() < 0.5,
            "reparented holder should stretch to the 280pt wrapper, not \
             keep its baked 393pt root width — got {}",
            hf.width
        );
    }

    /// Regression for the Android portal view-overlay rewrite
    /// ([[project_android_portal_is_dialog_smell]]): a viewport portal is
    /// now a full-bleed overlay registered as a fresh Taffy ROOT (both
    /// axes `Auto` → viewport-filled by `compute`), and the idea-ui
    /// `Modal` centers its card with a `width:100% height:100%`
    /// flex-center wrapper (`justify/align center`) holding the card.
    ///
    /// The old Dialog path needed `set_root_axes_wrap(overlay, true,
    /// true)` so a `WRAP_CONTENT` Dialog window's gravity could center
    /// the card; with a full-bleed overlay there is NO gravity and NO
    /// wrap — centering must be pure flex inside the viewport-filled
    /// overlay. This test asserts that without any `set_root_axes_wrap`
    /// the overlay fills the viewport AND a fixed-size card centers
    /// inside it. If a future change reintroduces root-wrap for portals,
    /// the overlay would collapse to the card's size and the card would
    /// land top-left — this test catches that.
    #[test]
    fn regression_android_portal_overlay_centers_card_without_root_wrap() {
        let mut t = LayoutTree::new();
        let (vw, vh) = (393.0_f32, 852.0_f32);

        // Overlay: a fresh root (auto on both axes, NO set_root_axes_wrap).
        let overlay = t.new_node();

        // Flex-center wrapper: 100% × 100%, center on both axes. This is
        // the idea-ui Modal's content wrapper.
        let wrapper = t.new_node();
        let mut wrapper_rules = StyleRules::default();
        wrapper_rules.width = Some(pct(100.0));
        wrapper_rules.height = Some(pct(100.0));
        wrapper_rules.justify_content = Some(FwJustifyContent::Center);
        wrapper_rules.align_items = Some(FwAlignItems::Center);
        t.set_style(wrapper, &wrapper_rules);
        t.add_child(overlay, wrapper);

        // Card: a fixed 300 × 200 box (the modal surface).
        let card = t.new_node();
        let mut card_rules = StyleRules::default();
        card_rules.width = Some(px(300.0));
        card_rules.height = Some(px(200.0));
        t.set_style(card, &card_rules);
        t.add_child(wrapper, card);

        t.compute(overlay, vw, vh);

        let of = t.frame_of(overlay);
        let cf = t.frame_of(card);

        // 1. The overlay fills the viewport (root auto-fill still applies
        //    — no root-wrap collapsed it to the card).
        assert!(
            (of.width - vw).abs() < 0.5 && (of.height - vh).abs() < 0.5,
            "overlay root should fill the viewport {vw}x{vh}, got {}x{}",
            of.width,
            of.height
        );

        // 2. The card is centered by flex (NOT pinned top-left). Center
        //    x = (393-300)/2 = 46.5, center y = (852-200)/2 = 326.
        let expected_x = (vw - 300.0) / 2.0;
        let expected_y = (vh - 200.0) / 2.0;
        // Tolerance 1.0: Taffy rounds frames to the pixel grid, so the
        // 46.5 center lands at 47 — still centered, not top-left (0).
        assert!(
            (cf.x - expected_x).abs() < 1.0 && (cf.y - expected_y).abs() < 1.0,
            "card should center at ({expected_x}, {expected_y}) via flex, \
             got ({}, {}) — a top-left landing means the overlay collapsed \
             (root-wrap reintroduced) or the wrapper didn't fill",
            cf.x,
            cf.y
        );
    }
}
