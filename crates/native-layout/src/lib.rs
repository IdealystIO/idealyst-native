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

use taffy::prelude::*;
use taffy::TaffyTree;

use framework_core::{
    AlignContent as FwAlignContent, AlignItems as FwAlignItems, AlignSelf as FwAlignSelf,
    FlexDirection as FwFlexDirection, FlexWrap as FwFlexWrap, JustifyContent as FwJustifyContent,
    Length as FwLength, Position as FwPosition, StyleRules,
};

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
}

impl LayoutTree {
    /// Construct an empty tree.
    pub fn new() -> Self {
        Self { tree: TaffyTree::new() }
    }

    /// Create a new leaf node (no children yet). Returns the handle
    /// the backend should associate with its native view.
    pub fn new_node(&mut self) -> LayoutNode {
        let id = self
            .tree
            .new_leaf(Style::default())
            .expect("taffy new_leaf");
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
    }

    /// Replace a node's style. Translates the framework's resolved
    /// `StyleRules` into Taffy's `Style`. Preserves any
    /// intrinsic-size axes that were previously set via
    /// [`Self::set_intrinsic_size`] — author styles that explicitly
    /// set width/height override; styles that leave them `Auto` keep
    /// the intrinsic.
    pub fn set_style(&mut self, node: LayoutNode, rules: &StyleRules) {
        let mut new_style = translate_style(rules);
        let existing = self
            .tree
            .style(node.0)
            .cloned()
            .unwrap_or(Style::default());
        // If author didn't set explicit width/height, fall back to
        // whatever we previously set (typically intrinsicContentSize).
        if matches!(new_style.size.width, Dimension::Auto) {
            new_style.size.width = existing.size.width;
        }
        if matches!(new_style.size.height, Dimension::Auto) {
            new_style.size.height = existing.size.height;
        }
        self.tree
            .set_style(node.0, new_style)
            .expect("taffy set_style");
    }

    /// Set a node's intrinsic content size. Used by native backends
    /// to seed Text / Button / Image / etc. with the size their
    /// native widget would prefer (UIView.intrinsicContentSize,
    /// View.measure(...)). The intrinsic is treated as both
    /// `size` (concrete) and `min_size` (so flex_shrink can't
    /// collapse below it). Author styles that explicitly set
    /// width/height override.
    pub fn set_intrinsic_size(&mut self, node: LayoutNode, width: f32, height: f32) {
        let mut style = self
            .tree
            .style(node.0)
            .cloned()
            .unwrap_or(Style::default());
        if matches!(style.size.width, Dimension::Auto) && width >= 0.0 {
            style.size.width = Dimension::Length(width);
        }
        if matches!(style.size.height, Dimension::Auto) && height >= 0.0 {
            style.size.height = Dimension::Length(height);
        }
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
        // Force the root's size to the viewport (preserve any
        // author-set min/max so they still constrain).
        let mut style = self
            .tree
            .style(root.0)
            .cloned()
            .unwrap_or(Style::default());
        style.size.width = Dimension::Length(width);
        style.size.height = Dimension::Length(height);
        self.tree.set_style(root.0, style).expect("taffy set_style");

        let space = Size {
            width: AvailableSpace::Definite(width),
            height: AvailableSpace::Definite(height),
        };
        self.tree
            .compute_layout(root.0, space)
            .expect("taffy compute_layout");
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
}

impl Default for LayoutTree {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Style translation: framework StyleRules → Taffy Style
// =============================================================================

fn translate_style(r: &StyleRules) -> Style {
    let mut s = Style::default();

    s.display = Display::Flex;
    s.position = match r.position {
        Some(FwPosition::Absolute) => Position::Absolute,
        _ => Position::Relative,
    };

    // Default to Column to match framework_core's `FlexDirection::default()`
    // (React Native convention). Taffy itself defaults to Row, which would
    // collapse vertically-stacked content into a horizontal row.
    s.flex_direction = match r.flex_direction.unwrap_or(FwFlexDirection::Column) {
        FwFlexDirection::Row => FlexDirection::Row,
        FwFlexDirection::Column => FlexDirection::Column,
        FwFlexDirection::RowReverse => FlexDirection::RowReverse,
        FwFlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
    };
    if let Some(w) = r.flex_wrap {
        s.flex_wrap = match w {
            FwFlexWrap::NoWrap => FlexWrap::NoWrap,
            FwFlexWrap::Wrap => FlexWrap::Wrap,
            FwFlexWrap::WrapReverse => FlexWrap::WrapReverse,
        };
    }
    if let Some(jc) = r.justify_content {
        s.justify_content = Some(match jc {
            FwJustifyContent::FlexStart => JustifyContent::FlexStart,
            FwJustifyContent::FlexEnd => JustifyContent::FlexEnd,
            FwJustifyContent::Center => JustifyContent::Center,
            FwJustifyContent::SpaceBetween => JustifyContent::SpaceBetween,
            FwJustifyContent::SpaceAround => JustifyContent::SpaceAround,
            FwJustifyContent::SpaceEvenly => JustifyContent::SpaceEvenly,
        });
    }
    if let Some(ai) = r.align_items {
        s.align_items = Some(match ai {
            FwAlignItems::FlexStart => AlignItems::FlexStart,
            FwAlignItems::FlexEnd => AlignItems::FlexEnd,
            FwAlignItems::Center => AlignItems::Center,
            FwAlignItems::Stretch => AlignItems::Stretch,
            FwAlignItems::Baseline => AlignItems::Baseline,
        });
    }
    if let Some(ac) = r.align_content {
        s.align_content = Some(match ac {
            FwAlignContent::FlexStart => AlignContent::FlexStart,
            FwAlignContent::FlexEnd => AlignContent::FlexEnd,
            FwAlignContent::Center => AlignContent::Center,
            FwAlignContent::Stretch => AlignContent::Stretch,
            FwAlignContent::SpaceBetween => AlignContent::SpaceBetween,
            FwAlignContent::SpaceAround => AlignContent::SpaceAround,
        });
    }
    if let Some(gap) = r.gap.as_ref().map(|t| t.value()) {
        let lp = length_to_lp(*gap);
        s.gap = Size { width: lp, height: lp };
    }
    if let Some(g) = r.row_gap.as_ref().map(|t| t.value()) {
        s.gap.height = length_to_lp(*g);
    }
    if let Some(g) = r.column_gap.as_ref().map(|t| t.value()) {
        s.gap.width = length_to_lp(*g);
    }

    if let Some(grow) = r.flex_grow.as_ref().map(|t| *t.value()) {
        s.flex_grow = grow;
    }
    if let Some(shrink) = r.flex_shrink.as_ref().map(|t| *t.value()) {
        s.flex_shrink = shrink;
    }
    if let Some(basis) = r.flex_basis.as_ref().map(|t| *t.value()) {
        s.flex_basis = length_to_dim(basis);
    }
    if let Some(asf) = r.align_self {
        s.align_self = match asf {
            FwAlignSelf::Auto => None,
            FwAlignSelf::FlexStart => Some(AlignSelf::FlexStart),
            FwAlignSelf::FlexEnd => Some(AlignSelf::FlexEnd),
            FwAlignSelf::Center => Some(AlignSelf::Center),
            FwAlignSelf::Stretch => Some(AlignSelf::Stretch),
            FwAlignSelf::Baseline => Some(AlignSelf::Baseline),
        };
    }

    if let Some(w) = r.width.as_ref().map(|t| *t.value()) {
        s.size.width = length_to_dim(w);
    }
    if let Some(h) = r.height.as_ref().map(|t| *t.value()) {
        s.size.height = length_to_dim(h);
    }
    if let Some(w) = r.min_width.as_ref().map(|t| *t.value()) {
        s.min_size.width = length_to_dim(w);
    }
    if let Some(h) = r.min_height.as_ref().map(|t| *t.value()) {
        s.min_size.height = length_to_dim(h);
    }
    if let Some(w) = r.max_width.as_ref().map(|t| *t.value()) {
        s.max_size.width = length_to_dim(w);
    }
    if let Some(h) = r.max_height.as_ref().map(|t| *t.value()) {
        s.max_size.height = length_to_dim(h);
    }

    s.padding = Rect {
        top: length_to_lp(r.padding_top.as_ref().map(|t| *t.value()).unwrap_or(FwLength::Px(0.0))),
        right: length_to_lp(r.padding_right.as_ref().map(|t| *t.value()).unwrap_or(FwLength::Px(0.0))),
        bottom: length_to_lp(r.padding_bottom.as_ref().map(|t| *t.value()).unwrap_or(FwLength::Px(0.0))),
        left: length_to_lp(r.padding_left.as_ref().map(|t| *t.value()).unwrap_or(FwLength::Px(0.0))),
    };
    s.margin = Rect {
        top: length_to_lpa(r.margin_top.as_ref().map(|t| *t.value())),
        right: length_to_lpa(r.margin_right.as_ref().map(|t| *t.value())),
        bottom: length_to_lpa(r.margin_bottom.as_ref().map(|t| *t.value())),
        left: length_to_lpa(r.margin_left.as_ref().map(|t| *t.value())),
    };
    s.inset = Rect {
        top: length_to_lpa(r.top.as_ref().map(|t| *t.value())),
        right: length_to_lpa(r.right.as_ref().map(|t| *t.value())),
        bottom: length_to_lpa(r.bottom.as_ref().map(|t| *t.value())),
        left: length_to_lpa(r.left.as_ref().map(|t| *t.value())),
    };

    s
}

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
