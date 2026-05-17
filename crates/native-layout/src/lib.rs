//! Flex layout utility for native backends.
//!
//! Wraps [`taffy`] (a pure-Rust flex engine matching CSS semantics)
//! and translates `framework_core::StyleRules` into Taffy styles.
//! Backends that don't have a native layout system (iOS, Android)
//! build a parallel layout tree as they create native nodes, run
//! Taffy when the tree is complete, and apply the resulting frames
//! to their native views.
//!
//! ## Usage shape (typical backend)
//!
//! ```ignore
//! use native_layout::{LayoutTree, LayoutNode};
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

use framework_core::{
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
}

impl LayoutTree {
    /// Construct an empty tree.
    pub fn new() -> Self {
        Self {
            tree: TaffyTree::new(),
            measure_fns: HashMap::new(),
            auto_width: HashSet::new(),
            auto_height: HashSet::new(),
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

    /// Add `child` to `parent`'s child list. Order matches insertion;
    /// the backend should call this in the same order it would
    /// `addSubview` / `addView`.
    pub fn add_child(&mut self, parent: LayoutNode, child: LayoutNode) {
        self.tree
            .add_child(parent.0, child.0)
            .expect("taffy add_child");
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

        // --- Padding (per-side, all optional) ---

        if let Some(v) = rules.padding_top.as_ref().map(|t| *t.value()) {
            style.padding.top = length_to_lp(v);
        }
        if let Some(v) = rules.padding_right.as_ref().map(|t| *t.value()) {
            style.padding.right = length_to_lp(v);
        }
        if let Some(v) = rules.padding_bottom.as_ref().map(|t| *t.value()) {
            style.padding.bottom = length_to_lp(v);
        }
        if let Some(v) = rules.padding_left.as_ref().map(|t| *t.value()) {
            style.padding.left = length_to_lp(v);
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
