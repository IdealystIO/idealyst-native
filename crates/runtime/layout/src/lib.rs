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
    DisplayKind as FwDisplayKind, FlexDirection as FwFlexDirection, FlexWrap as FwFlexWrap,
    JustifyContent as FwJustifyContent, Length as FwLength, Position as FwPosition, StyleRules,
    TrackSize as FwTrackSize,
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
    /// Nodes whose `max_size.width` is the framework's *default*
    /// `100%` cross-axis clamp — NOT an author-set `max_width`. The
    /// clamp makes an un-stretched flex container wrap its text to the
    /// available width like CSS/web (a flex/block box never exceeds its
    /// parent's inline size), instead of sizing to single-line
    /// max-content and overflowing off-screen — the native-only,
    /// web-clean overflow bug. `set_style` re-derives the clamp on
    /// every apply and lifts it for nodes that legitimately exceed
    /// their parent (see `apply_default_max_width`). Cleared when the
    /// author sets an explicit `max_width`.
    auto_max_width: HashSet<NodeId>,
    /// Nodes with `width: <percent>` AND an author `max_width: <definite px>`.
    /// Taffy measures such a node's intrinsic HEIGHT at the percent-resolved
    /// width (ignoring the cap), then renders the WIDTH capped — so a wrapping
    /// child (a `text_area`, multi-line text) is measured at the wider pre-cap
    /// width (fewer lines → too short) while it renders at the narrower capped
    /// width (more lines → taller), and the ancestor clips the overflow.
    /// Browsers measure height at the used (capped) width; [`Self::compute`]
    /// restores that parity with a second pass over these nodes. Maintained by
    /// `set_style`; see the `capped_percent_width` handling in `compute`.
    capped_percent_width: HashSet<NodeId>,
    /// Nodes marked as horizontal scroll viewports (`overflow-x:
    /// scroll`). Their direct children hold content that is *meant* to
    /// be wider than the viewport (so it can scroll), so those children
    /// are exempted from the `100%` width clamp.
    hscroll_parents: HashSet<NodeId>,
    /// Direct children of a [`hscroll_parents`](Self::hscroll_parents)
    /// node — exempt from the default width clamp so horizontal-scroll
    /// content can exceed the viewport.
    hscroll_content: HashSet<NodeId>,
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
    /// Direct children of a `Display::Grid` container. A grid item's CSS
    /// default `min-width: auto` resolves to its content's min size,
    /// which Taffy treats as the column's floor — so a wide cell can't
    /// shrink and its column overflows instead of wrapping (the
    /// `table-layout: auto` behavior every backend needs). We default
    /// these items to `min-width: 0` (overridable by an explicit author
    /// `min_width`) so `auto`/`fr` column tracks can size the column and
    /// wrap the content. Tracked so a reactive restyle keeps the reset.
    grid_items: HashSet<NodeId>,
    /// Grid containers that want content-PROPORTIONAL columns
    /// (`table-layout: auto`): each column's share of the width is
    /// proportional to its content, so a text-heavy column is wider than
    /// a short one instead of every column splitting the width evenly.
    /// Keyed by node → column count. A grid whose `grid_template_columns`
    /// are all `Auto` opts in (the `table` SDK's recipe). `compute` does a
    /// measure pass to learn each column's max-content, then assigns `fr`
    /// weights proportional to it — uniform `auto`/`fr` tracks can only
    /// split evenly. See [`compute`](Self::compute).
    table_grids: HashMap<NodeId, usize>,
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
            auto_max_width: HashSet::new(),
            capped_percent_width: HashSet::new(),
            hscroll_parents: HashSet::new(),
            hscroll_content: HashSet::new(),
            author_padding: HashMap::new(),
            safe_area_extra: HashMap::new(),
            grid_items: HashSet::new(),
            table_grids: HashMap::new(),
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
        // Default cross-axis clamp: never grow wider than the parent's
        // inline size. This is the CSS/web default a block/flex box
        // gets for free (it fills, and content wraps to the available
        // width); Taffy instead sizes an un-stretched flex container to
        // its single-line max-content and overflows. `set_style`
        // re-derives this clamp on every apply and lifts it for nodes
        // that legitimately exceed their parent (explicit `width`,
        // `position: absolute`, `aspect_ratio`, horizontal-scroll
        // content). Tracked in `auto_max_width` so an author-set
        // `max_width` overrides it. A `Percent` of an *indefinite*
        // parent resolves to no constraint, so this is a no-op until
        // some ancestor has a definite width — exactly when web clamps.
        style.max_size.width = Dimension::Percent(1.0);
        let id = self
            .tree
            .new_leaf(style)
            .expect("taffy new_leaf");
        // Seed style leaves width and height as `Auto`; record that so
        // root nodes get filled to the viewport on each `compute()`.
        self.auto_width.insert(id);
        self.auto_height.insert(id);
        self.auto_max_width.insert(id);
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
        if self.hscroll_parents.contains(&parent.0) {
            self.exempt_as_hscroll_content(child.0);
        }
        self.mark_grid_item_if_grid_parent(parent.0, child.0);
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
        if self.hscroll_parents.contains(&parent.0) {
            self.exempt_as_hscroll_content(child.0);
        }
        self.mark_grid_item_if_grid_parent(parent.0, child.0);
    }

    /// If `parent` is a grid container, reset `child` as a grid item.
    /// Covers the case where a child is inserted into an ALREADY-grid
    /// parent (reactive splice). The other order — children inserted
    /// before the parent's `display: grid` style lands, which is how
    /// `view` builds (children then style) — is handled in `set_style`,
    /// which resets a grid's existing children when its display becomes
    /// Grid.
    fn mark_grid_item_if_grid_parent(&mut self, parent: NodeId, child: NodeId) {
        let parent_is_grid = self
            .tree
            .style(parent)
            .map(|s| s.display == Display::Grid)
            .unwrap_or(false);
        if parent_is_grid {
            self.reset_grid_item(child);
        }
    }

    /// Record `child` as a grid item and reset its `min-width` to 0 unless
    /// the author pinned one (the slot is still `Auto`) — never clobber an
    /// explicit author minimum. A grid item's CSS default `min-width:
    /// auto` floors the column at the cell's content size in Taffy, so a
    /// wide cell can't shrink and its column overflows instead of
    /// wrapping; `min-width: 0` restores `table-layout: auto` behavior.
    /// See [`grid_items`](Self::grid_items).
    fn reset_grid_item(&mut self, child: NodeId) {
        self.grid_items.insert(child);
        if let Ok(mut style) = self.tree.style(child).cloned() {
            if style.min_size.width == Dimension::Auto {
                style.min_size.width = Dimension::Length(0.0);
                let _ = self.tree.set_style(child, style);
            }
        }
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
    ///
    /// Tolerant of `child` not actually being a child of `parent`: Taffy's
    /// `remove_child` resolves the index with `children.position(..).unwrap()`
    /// and PANICS on a non-child — and that panic can't be caught by the
    /// `let _ =` below (it's an internal unwrap, not a returned `Err`). On a
    /// backend's non-unwinding FFI boundary (objc method / JNI callback) that
    /// panic becomes a whole-process abort. A portal / detached-window-root
    /// node is an orphan Taffy ROOT that was never wired as a child of
    /// `parent`, so the anchorless spliced-`when` unmount path
    /// (`Backend::remove_child(parent, portal)`) would hit exactly this.
    /// Guard by membership; removing a non-child is a no-op.
    ///
    /// Regression: tapping a `Modal`'s backdrop to dismiss it crashed here on
    /// iOS and Android — `if open { Modal }`'s unmount calls `remove_child`
    /// with the Modal's portal node (an orphan root).
    pub fn remove_child(&mut self, parent: LayoutNode, child: LayoutNode) {
        let is_child = self
            .tree
            .children(parent.0)
            .map(|kids| kids.contains(&child.0))
            .unwrap_or(false);
        if is_child {
            let _ = self.tree.remove_child(parent.0, child.0);
        }
    }

    /// Drop a node entirely (frees its slot in the tree).
    pub fn remove_node(&mut self, node: LayoutNode) {
        let _ = self.tree.remove(node.0);
        self.measure_fns.remove(&node.0);
        self.auto_width.remove(&node.0);
        self.auto_height.remove(&node.0);
        self.auto_max_width.remove(&node.0);
        self.capped_percent_width.remove(&node.0);
        self.hscroll_parents.remove(&node.0);
        self.hscroll_content.remove(&node.0);
        self.author_padding.remove(&node.0);
        self.safe_area_extra.remove(&node.0);
        self.grid_items.remove(&node.0);
        self.table_grids.remove(&node.0);
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
    ///
    /// Returns `true` iff this changed the node's **layout** geometry. The
    /// Taffy `Style` we build holds geometry only — flex / size / spacing /
    /// position / aspect-ratio. Paint properties (color, background,
    /// border-color, border-radius, box-shadow, opacity) never reach it. So a
    /// `false` return means a paint-only change (e.g. a `:hover` border-color
    /// swap): a backend can skip scheduling a layout pass for it. Text-measure
    /// inputs (font, line-height) also aren't in the Taffy `Style` — a node
    /// with a measure fn must gate on those separately (see the macOS
    /// backend's text-measure signature). The `tree.set_style` write itself is
    /// unconditional, so Taffy's dirty bookkeeping is identical for every
    /// backend; only the return value is advisory.
    pub fn set_style(&mut self, node: LayoutNode, rules: &StyleRules) -> bool {
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
        let existing = self.tree.style(node.0).cloned().unwrap_or_default();
        let mut style = existing.clone();

        // Display defaults to Flex (the framework's React-Native-like
        // convention); a style may opt a node into Grid, which is what
        // the `table` SDK uses for cross-row column alignment. We always
        // write the field so a node that loses its `display` on a
        // reactive re-apply reverts to Flex rather than keeping a stale
        // Grid.
        style.display = match rules.display {
            Some(FwDisplayKind::Grid) => Display::Grid,
            _ => Display::Flex,
        };
        // Grid column tracks. Only meaningful under `Display::Grid`;
        // harmless (ignored by the flex algorithm) otherwise, but we
        // clear it when absent so a reverted node doesn't keep stale
        // tracks. Rows stay implicit (`grid_auto_flow: Row` default) so
        // direct children fill the tracks left-to-right, wrapping every
        // `len()` cells — one grid row per table row.
        style.grid_template_columns = match rules.grid_template_columns.as_ref() {
            Some(cols) => cols.iter().map(track_sizing_function).collect(),
            None => Default::default(),
        };

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
        } else {
            // No explicit rule. A grid container's tracks should FILL the
            // container — `justify-content: stretch` grows `auto` tracks
            // to absorb leftover space (the framework's flex `FlexStart`
            // seed would instead leave them hugging content with a gap on
            // the inline end). `stretch` also still lets a track shrink to
            // its `min-content` when space is tight, so content sizes the
            // column AND text wraps only when it genuinely can't fit —
            // matching `table-layout: auto`. Flex keeps the `FlexStart`
            // default. Set deterministically so a grid→flex revert can't
            // leave a stale `Stretch`.
            style.justify_content = Some(if matches!(rules.display, Some(FwDisplayKind::Grid)) {
                JustifyContent::Stretch
            } else {
                JustifyContent::FlexStart
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
        } else if self.grid_items.contains(&node.0) {
            // Grid item with no author min-width: reset the CSS `auto`
            // default (which Taffy floors at content size) to 0 so the
            // column can shrink and wrap. Re-applied here on every
            // restyle so a reactive cell keeps the reset.
            style.min_size.width = Dimension::Length(0.0);
        }
        if let Some(h) = rules.min_height.as_ref().map(|t| *t.value()) {
            style.min_size.height = length_to_dim(h);
        }
        if let Some(w) = rules.max_width.as_ref().map(|t| *t.value()) {
            style.max_size.width = length_to_dim(w);
            // Author owns max-width now — stop applying the default clamp.
            self.auto_max_width.remove(&node.0);
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

        // Re-derive the default `max-width: 100%` cross-axis clamp from
        // the now-merged style. Runs last so it sees the final
        // `position` / `aspect_ratio` / explicit `width` this apply
        // produced (state overlays carry partial rules, so we must read
        // the merged result, not the incoming `rules`).
        self.apply_default_max_width(node.0, &mut style);

        // Track the `width: <percent>` + author `max_width: <definite px>`
        // pattern so `compute` can re-resolve its height at the capped width
        // (Taffy measures it at the pre-cap width — see the field docs). The
        // default `Percent(1.0)` clamp is NOT a definite cap, so it never
        // triggers this; only an author `Length` max-width does.
        if matches!(style.size.width, Dimension::Percent(_))
            && matches!(style.max_size.width, Dimension::Length(_))
        {
            self.capped_percent_width.insert(node.0);
        } else {
            self.capped_percent_width.remove(&node.0);
        }

        // Geometry-only diff: equal Taffy `Style` ⇒ a paint-only change.
        let changed = style != existing;
        self.tree
            .set_style(node.0, style)
            .expect("taffy set_style");

        // When this node IS (or just became) a grid, reset its existing
        // children as grid items. `view` builds children BEFORE attaching
        // the parent's style, so `add_child` ran while this node was still
        // the default Flex and missed them — catch them here. Idempotent.
        if matches!(rules.display, Some(FwDisplayKind::Grid)) {
            let children = self.tree.children(node.0).unwrap_or_default();
            for c in children {
                self.reset_grid_item(c);
            }
            // An all-`Auto` column grid is a content-proportional table:
            // record it so `compute` can weight the columns by content.
            // `compute` overwrites the actual Taffy tracks each pass, so
            // we don't pin them here.
            match rules.grid_template_columns.as_ref() {
                Some(cols) if !cols.is_empty() && cols.iter().all(|t| matches!(t, FwTrackSize::Auto)) => {
                    self.table_grids.insert(node.0, cols.len());
                }
                _ => {
                    self.table_grids.remove(&node.0);
                }
            }
        }
        changed
    }

    /// Re-derive the framework's default `max-width: 100%` cross-axis
    /// clamp for `node` against its merged Taffy `style`. No-op when the
    /// author set an explicit `max_width` (node not in `auto_max_width`).
    ///
    /// The clamp is LIFTED (back to `Auto`) for nodes that legitimately
    /// size wider than their parent's inline box — matching how CSS
    /// only fits-to-content/clamps normal-flow boxes:
    /// - **explicit `width`** (`!auto_width`): the author owns the size;
    ///   a `width: 600` carousel inside a 300 viewport must stay 600.
    /// - **`position: absolute`**: resolves against its containing block
    ///   and may exceed it (e.g. an `aspect_ratio` overlay sized from a
    ///   percentage height — the `aspect_ratio_resolves_width…` test).
    /// - **`aspect_ratio`**: width is derived from height and may exceed
    ///   the parent.
    /// - **horizontal-scroll content** (`hscroll_content`): content that
    ///   is meant to be wider than its scroll viewport so it can scroll.
    ///
    /// Otherwise the clamp is (re)asserted as `Percent(1.0)`.
    fn apply_default_max_width(&self, node: NodeId, style: &mut Style) {
        if !self.auto_max_width.contains(&node) {
            return;
        }
        let exempt = style.position == Position::Absolute
            || style.aspect_ratio.is_some()
            || !self.auto_width.contains(&node)
            || self.hscroll_content.contains(&node);
        style.max_size.width = if exempt {
            Dimension::Auto
        } else {
            Dimension::Percent(1.0)
        };
    }

    /// Mark `node` as horizontal-scroll content: exempt it from the
    /// default width clamp (so it can exceed its scroll viewport) and
    /// clear any clamp already written to its Taffy style.
    fn exempt_as_hscroll_content(&mut self, node: NodeId) {
        self.hscroll_content.insert(node);
        if self.auto_max_width.contains(&node) {
            if let Ok(mut style) = self.tree.style(node).cloned() {
                if style.max_size.width != Dimension::Auto {
                    style.max_size.width = Dimension::Auto;
                    let _ = self.tree.set_style(node, style);
                }
            }
        }
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
    /// behavior.
    ///
    /// Native scroll-view backends that parent the scroll content as a
    /// Taffy *child* of the scroll node — **iOS** (direct subviews of the
    /// UIScrollView), **Android** (inner FrameLayout), **macOS**
    /// (documentView) — MUST call this. Parenting the content under the
    /// scroll node makes the content's size contribute to the scroll node's
    /// automatic minimum size (a flex item's auto-min is its min-content),
    /// so without `overflow:scroll` a `flex_grow` scroll node grows to its
    /// content height instead of being bounded by its parent — it ends up as
    /// tall as its content and has nothing to scroll. The native scroll view
    /// still does its own pixel clipping + content-offset; this call only
    /// fixes the Taffy *sizing* of the viewport node.
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
        // A horizontal scroller's content is meant to exceed the
        // viewport (that's what scrolls), so its children must opt out
        // of the default `max-width: 100%` clamp. Record the parent and
        // exempt any children already attached; `add_child` exempts ones
        // added later. Called at scroll-view create time, so the child
        // list is usually empty here — the `add_child` hook does most of
        // the work.
        if horizontal {
            self.hscroll_parents.insert(node.0);
            if let Ok(children) = self.tree.children(node.0) {
                for child in children {
                    self.exempt_as_hscroll_content(child);
                }
            }
        }
    }

    /// Set (or clear) an animated `max-height` cap on a node, bypassing
    /// the full `set_style` translation. Native backends call this to
    /// realize [`AnimProp::MaxHeight`](runtime_core::animation::AnimProp::MaxHeight)
    /// per animation frame — the reference consumer is idea-ui's
    /// `Collapsible(transition = Measured)`, which tweens the body's
    /// max-height between `0` and its measured content height to
    /// collapse / expand. `Some(h)` clamps the node to `h` DIPs; `None`
    /// clears the cap back to Taffy `Auto` (matching the web backend
    /// clearing `style.maxHeight`).
    ///
    /// Why a dedicated setter rather than routing through `set_style`:
    /// the animated value updates ~60×/sec and carries no other style
    /// info, so building a `StyleRules` per frame would be wasteful.
    /// More importantly, the Collapsible's chrome (padding / opacity /
    /// border) animates via a *concurrent* variant swap that goes
    /// through `set_style` and does NOT declare `max_height`. Because
    /// `set_style` preserves Taffy fields its `StyleRules` doesn't
    /// mention, the cap written here survives that restyle — this setter
    /// is the single writer of `max_size.height` for an animated node.
    /// Taffy's own `set_style` marks the node dirty, so the next
    /// `compute()` reflows it (and its siblings).
    pub fn set_animated_max_height(&mut self, node: LayoutNode, value: Option<f32>) {
        let mut style = self
            .tree
            .style(node.0)
            .cloned()
            .unwrap_or(Style::default());
        style.max_size.height = match value {
            Some(h) => Dimension::Length(h.max(0.0)),
            None => Dimension::Auto,
        };
        let _ = self.tree.set_style(node.0, style);
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

        // --- Table grids: `table-layout: auto` column sizing -----------
        // Reproduce a browser's auto table. The pass above gives each grid
        // its width `W` (a bounded stretch child reports its container's
        // width regardless of column content). For each all-`Auto` grid we
        // measure every column's max-content (per cell, IN ISOLATION:
        // `compute_layout` on the cell with MaxContent available — during
        // normal layout Taffy measures a grid item at the GRID's width and
        // clamps to it) and size columns:
        //   - fits (Σmax ≤ W): column = max-content + a share of the
        //     leftover proportional to max-content.
        //   - over-full: WATER-FILL. Short columns keep their content; the
        //     largest columns shrink to a common cap C with Σ min(max,C)=W.
        //     For the common one-wide-column table this is the browser
        //     result — the text column gives back the overflow and wraps,
        //     short columns stay hugged to their content.
        // We deliberately do NOT use the exact CSS min/max formula: the GPU
        // backend's text engine reports min-content ≈ 0 for long/unbreakable
        // strings (its `measure_min_content` returns ~0), which would let
        // short columns collapse below their content. Water-fill needs only
        // the reliable max-content. The widths are pinned as fixed px tracks
        // and the tree is laid out again.
        if !self.table_grids.is_empty() {
            let grids: Vec<(NodeId, usize)> =
                self.table_grids.iter().map(|(k, v)| (*k, *v)).collect();
            let mut repinned = false;
            for (grid, n) in &grids {
                let n = *n;
                let w = self.tree.layout(*grid).map(|l| l.size.width).unwrap_or(0.0);
                let children = self.tree.children(*grid).unwrap_or_default();
                if n == 0 || children.is_empty() || w <= 0.0 {
                    continue;
                }
                // Measure ONLY each column's max-content (the unwrapped
                // single-line width). It's measured per cell IN ISOLATION
                // (`compute_layout` on the cell with MaxContent available),
                // because during normal layout Taffy measures a grid item
                // at the GRID's width and clamps to it. We do NOT use
                // min-content: this backend's text engine reports ~0
                // min-content for long/unbreakable strings, so the CSS
                // min/max formula collapses short columns. max-content alone
                // is reliable and drives the water-fill below.
                let mut max_cw = vec![0.0_f32; n];
                for (i, c) in children.iter().enumerate() {
                    let _ = self.tree.compute_layout_with_measure(
                        *c,
                        Size {
                            width: AvailableSpace::MaxContent,
                            height: AvailableSpace::MaxContent,
                        },
                        |kd, av, id, _c, _s| match measure_fns.get(&id) {
                            Some(f) => f(kd, av),
                            None => Size::ZERO,
                        },
                    );
                    max_cw[i % n] =
                        max_cw[i % n].max(self.tree.layout(*c).map(|l| l.size.width).unwrap_or(0.0));
                }
                let sum_max: f32 = max_cw.iter().sum();
                let widths: Vec<f32> = if sum_max <= w {
                    // Fits: every column gets its content, the leftover is
                    // shared in proportion to content (a browser's auto
                    // table gives wider columns more of the extra).
                    let extra = w - sum_max;
                    max_cw
                        .iter()
                        .map(|m| {
                            m + if sum_max > 0.0 { extra * m / sum_max } else { extra / n as f32 }
                        })
                        .collect()
                } else {
                    // Over-full: water-fill. Short columns keep their
                    // content; the largest columns shrink to a common cap C
                    // chosen so Σ min(max_i, C) = W. This matches a browser's
                    // table-layout: auto for the common one-wide-column table
                    // (the text column gives back the overflow, the rest stay
                    // hugged to their content) and needs no min-content.
                    let mut order: Vec<usize> = (0..n).collect();
                    order.sort_by(|a, b| {
                        max_cw[*a]
                            .partial_cmp(&max_cw[*b])
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    let mut widths = vec![0.0_f32; n];
                    let mut remaining = w;
                    let mut left = n;
                    for (rank, &i) in order.iter().enumerate() {
                        let fair = remaining / left as f32;
                        if max_cw[i] <= fair {
                            widths[i] = max_cw[i];
                            remaining -= max_cw[i];
                            left -= 1;
                        } else {
                            let cap = remaining / left as f32;
                            for &j in &order[rank..] {
                                widths[j] = cap;
                            }
                            break;
                        }
                    }
                    widths
                };
                if let Ok(mut s) = self.tree.style(*grid).cloned() {
                    s.grid_template_columns = widths
                        .iter()
                        .map(|px| {
                            TrackSizingFunction::Single(NonRepeatedTrackSizingFunction {
                                min: MinTrackSizingFunction::Fixed(LengthPercentage::Length(*px)),
                                max: MaxTrackSizingFunction::Fixed(LengthPercentage::Length(*px)),
                            })
                        })
                        .collect();
                    let _ = self.tree.set_style(*grid, s);
                    repinned = true;
                }
            }
            if repinned {
                // The per-cell isolation computes above scrambled the
                // tree's layout; lay it out once more from the root with
                // the pinned px columns.
                self.tree
                    .compute_layout_with_measure(root.0, space, |kd, av, id, _c, _s| {
                        match measure_fns.get(&id) {
                            Some(f) => f(kd, av),
                            None => Size::ZERO,
                        }
                    })
                    .expect("taffy compute_layout (table final pass)");
            }
        }
        self.measure_fns = measure_fns;

        // --- CSS-parity second pass for `width: %` + `max_width: <px>` ---
        //
        // Taffy sized these nodes' WIDTH correctly (it caps to the max-width),
        // but it measured their content HEIGHT at the *pre-cap* percent width.
        // For a wrapping child (a `text_area`, multi-line `text`) the height is
        // therefore measured at a WIDER width than the box renders at → fewer
        // lines → an ancestor shorter than its content → the content below
        // clips. Browsers measure height at the used (capped) width.
        //
        // The width Taffy resolved IS the used width, so pin each node whose
        // cap actually BOUND to that definite width and recompute: now the
        // height is measured at the same width the box renders at. We only act
        // when a cap bound (resolved width == the px max), so a viewport
        // narrower than the cap — where the percent already governs and the
        // height is correct — pays nothing. Restoring the percent afterwards
        // keeps the node responsive on the next pass (a new viewport width).
        if !self.capped_percent_width.is_empty() {
            let mut pins: Vec<(NodeId, Dimension)> = Vec::new();
            for &n in &self.capped_percent_width {
                let (Ok(layout), Ok(style)) = (self.tree.layout(n), self.tree.style(n)) else {
                    continue;
                };
                let resolved_w = layout.size.width;
                // Cap bound iff the resolved width reached the definite max.
                let bound = matches!(style.max_size.width, Dimension::Length(m)
                    if resolved_w + 0.5 >= m && resolved_w.is_finite());
                if bound {
                    pins.push((n, style.size.width));
                    let mut pinned = style.clone();
                    pinned.size.width = Dimension::Length(resolved_w);
                    let _ = self.tree.set_style(n, pinned);
                }
            }
            if !pins.is_empty() {
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
                    .expect("taffy compute_layout (capped-width pass)");
                self.measure_fns = measure_fns;
                // Restore the percent widths so the next `compute()` (a resize
                // to a new viewport) re-resolves the cap fresh. This marks the
                // nodes dirty but leaves the just-computed frames intact for
                // `frame_of` until that next pass.
                for (n, original_w) in pins {
                    if let Ok(mut style) = self.tree.style(n).cloned() {
                        style.size.width = original_w;
                        let _ = self.tree.set_style(n, style);
                    }
                }
            }
        }
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

    /// Whether `node`'s current Taffy style is `position: absolute` —
    /// i.e. it's taken out of normal flow and contributes no in-flow
    /// size to its parent. Read by backends that must adapt a wrapper's
    /// geometry to whether its (already-styled) child is out of flow:
    /// the macOS presence placeholder fills its parent only for an
    /// absolute child (a floating FAB / popover) and otherwise stays
    /// in-flow so a stack of presences (toasts) lays out like web.
    pub fn is_absolute(&self, node: LayoutNode) -> bool {
        self.tree
            .style(node.0)
            .map(|s| s.position == Position::Absolute)
            .unwrap_or(false)
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

    /// Compute the scrollable content extent of a scroll node — the
    /// `(width, height)` a backend should set as its scroll view's
    /// `contentSize` — by walking the node's descendants in its content
    /// coordinate space, **respecting clipping**.
    ///
    /// A naive "bounding box of every descendant frame on both axes" walk
    /// over-reports: it counts content that is clipped (never visible /
    /// reachable), inflating `contentSize` so the scroll view scrolls to
    /// dead space. Two clipping rules fix that:
    ///
    /// 1. **The scroll view clips its CROSS axis to its own frame.** A
    ///    vertical scroller can't scroll sideways, so a child wider than its
    ///    bounds is clipped — it must not extend `contentSize.width`. Only
    ///    the SCROLL axis is unbounded (that's what scrolls). Which axis is
    ///    which is read from the node's own Taffy `overflow` (set by
    ///    [`set_overflow_scroll`](Self::set_overflow_scroll)).
    /// 2. **A descendant that clips its overflow bounds its subtree to its
    ///    own frame** — e.g. a nested scroll view scrolls its own content;
    ///    that content must not leak into the outer scroller's extent.
    ///
    /// A node with `overflow: Visible` does NOT clip — its overflowing
    /// children legitimately extend the extent (e.g. a sidebar whose
    /// `min_height: 100%` container is clamped to the viewport but whose
    /// Spacer-pushed footer sits below must still drive `contentSize.height`;
    /// only the SCROLL axis benefits — the cross axis is clipped by rule 1).
    ///
    /// Note: `StyleRules { overflow: Hidden }` is a backend-level
    /// `clipsToBounds` not reflected in Taffy `overflow`, so rule 2 here only
    /// fires for nodes Taffy knows clip (scroll views). Rule 1 still bounds
    /// the cross axis regardless — which is what stops an over-wide child (a
    /// non-wrapping button row, a wide table) from turning a vertical
    /// scroller into a horizontal one.
    pub fn scroll_content_extent(&self, scroll: LayoutNode) -> (f32, f32) {
        let style = self.tree.style(scroll.0).cloned().unwrap_or(Style::default());
        let sf = self.frame_of(scroll);
        // The SCROLL axis is unbounded; the CROSS axis clips to the frame.
        let scrolls_x = style.overflow.x == taffy::Overflow::Scroll;
        let scrolls_y = style.overflow.y == taffy::Overflow::Scroll;
        let clip_right = if scrolls_x { f32::INFINITY } else { sf.width };
        let clip_bottom = if scrolls_y { f32::INFINITY } else { sf.height };

        let mut max_x = 0.0_f32;
        let mut max_y = 0.0_f32;
        // (node, accumulated origin x/y, inherited clip right/bottom).
        let mut stack: Vec<(LayoutNode, f32, f32, f32, f32)> = self
            .children_of(scroll)
            .into_iter()
            .map(|c| (c, 0.0, 0.0, clip_right, clip_bottom))
            .collect();
        while let Some((node, ox, oy, cr, cb)) = stack.pop() {
            let f = self.frame_of(node);
            let nx = ox + f.x;
            let ny = oy + f.y;
            // This node's extent, clamped to the clip rect it lives in.
            max_x = max_x.max((nx + f.width).min(cr));
            max_y = max_y.max((ny + f.height).min(cb));
            // If this node clips an axis, tighten that axis's clip bound for
            // its subtree. (`Visible` → no clip → children inherit cr/cb.)
            let s = self.tree.style(node.0).cloned().unwrap_or(Style::default());
            let child_cr = if s.overflow.x != taffy::Overflow::Visible {
                cr.min(nx + f.width)
            } else {
                cr
            };
            let child_cb = if s.overflow.y != taffy::Overflow::Visible {
                cb.min(ny + f.height)
            } else {
                cb
            };
            for child in self.children_of(node) {
                stack.push((child, nx, ny, child_cr, child_cb));
            }
        }
        (max_x, max_y)
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

// --- Grid track translation -------------------------------------------------
//
// A framework `TrackSize` becomes one Taffy `TrackSizingFunction::Single`
// = `minmax(min, max)`. We split min/max so the CSS single-value
// semantics come out right:
//   - `Fr(v)`  → `minmax(auto, <v>fr)`  (a bare `<flex>` track floors at
//                auto so the column never shrinks below its content)
//   - keyword  → `minmax(keyword, keyword)`
//   - `Px(v)`  → fixed both sides
//   - `Minmax(lo, hi)` takes lo's min and hi's max (the one nested form).

fn track_min(t: &FwTrackSize) -> MinTrackSizingFunction {
    match t {
        FwTrackSize::Auto => MinTrackSizingFunction::Auto,
        FwTrackSize::MinContent => MinTrackSizingFunction::MinContent,
        FwTrackSize::MaxContent => MinTrackSizingFunction::MaxContent,
        // `fr` has no min form in CSS; a bare flex track floors at auto.
        FwTrackSize::Fr(_) => MinTrackSizingFunction::Auto,
        FwTrackSize::Px(v) => MinTrackSizingFunction::Fixed(LengthPercentage::Length(*v)),
        FwTrackSize::Minmax(lo, _) => track_min(lo),
    }
}

fn track_max(t: &FwTrackSize) -> MaxTrackSizingFunction {
    match t {
        FwTrackSize::Auto => MaxTrackSizingFunction::Auto,
        FwTrackSize::MinContent => MaxTrackSizingFunction::MinContent,
        FwTrackSize::MaxContent => MaxTrackSizingFunction::MaxContent,
        FwTrackSize::Fr(v) => MaxTrackSizingFunction::Fraction(*v),
        FwTrackSize::Px(v) => MaxTrackSizingFunction::Fixed(LengthPercentage::Length(*v)),
        FwTrackSize::Minmax(_, hi) => track_max(hi),
    }
}

fn track_sizing_function(t: &FwTrackSize) -> TrackSizingFunction {
    TrackSizingFunction::Single(NonRepeatedTrackSizingFunction {
        min: track_min(t),
        max: track_max(t),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::Tokenized;

    /// `auto` grid columns with cells that measure like real text
    /// reproduce `table-layout: auto`: a content-heavy column takes the
    /// space it needs and the others stay tight — and the table fills its
    /// width without overflowing — because the engine resets a grid
    /// item's `min-width: auto` (which Taffy floors at content size) to 0
    /// so wide columns can shrink and wrap. This is the exact behavior
    /// the SDK's native table relies on; it regressed when columns were
    /// split evenly (`1fr`) or overflowed (`auto` without the reset).
    #[test]
    fn grid_text_columns_match_table_layout_auto() {
        // single-line (max-content) width and longest-word (min-content)
        // width per cell. Returns extra height when forced narrower than
        // its single-line width (i.e. it wraps).
        fn text_measure(longest_word: f32, single_line: f32) -> MeasureFn {
            Rc::new(move |known: Size<Option<f32>>, avail: Size<AvailableSpace>| {
                let w = known.width.unwrap_or(match avail.width {
                    AvailableSpace::MinContent => longest_word,
                    AvailableSpace::MaxContent => single_line,
                    AvailableSpace::Definite(aw) => single_line.min(aw).max(longest_word),
                });
                let lines = (single_line / w.max(1.0)).ceil().max(1.0);
                Size { width: w, height: lines * 16.0 }
            })
        }
        use runtime_core::TrackSize as TS;

        // Over-full: a long Description (single-line 900 ≫ remaining
        // width) must shrink to fill the remainder and wrap; the short
        // columns stay near their content.
        {
            let mut t = LayoutTree::new();
            let grid = t.new_node();
            let mut gr = StyleRules::default();
            gr.display = Some(runtime_core::DisplayKind::Grid);
            gr.grid_template_columns = Some(vec![TS::Auto; 3]);
            t.set_style(grid, &gr);
            // (longest_word, single_line): Prop, Type, Description.
            let specs = [(60.0_f32, 60.0_f32), (200.0, 200.0), (90.0, 900.0)];
            let mut cells = Vec::new();
            for (mn, mx) in specs {
                let c = t.new_node(); // no explicit min-width → engine resets to 0
                t.set_measure_fn(c, text_measure(mn, mx));
                t.add_child(grid, c);
                cells.push(c);
            }
            t.compute(grid, 1000.0, 400.0);
            let w: Vec<f32> = cells.iter().map(|c| t.frame_of(*c).width).collect();
            // Over-full water-fill: max-content=[60,200,900], W=1000.
            // Prop (60) and Type (200) are below their fair share so they
            // keep their content; Description is capped at the remainder
            // 1000−60−200 = 740 and wraps. (Same result a browser gives a
            // one-wide-column table.) Short columns never collapse.
            assert!((w[0] - 60.0).abs() < 1.5, "Prop = its content (60): {}", w[0]);
            assert!((w[1] - 200.0).abs() < 1.5, "Type = its content (200): {}", w[1]);
            assert!((w[2] - 740.0).abs() < 1.5, "Description gets the remainder (740): {}", w[2]);
            assert!(t.frame_of(cells[2]).height > 16.5, "Description wrapped to >1 line");
        }

        // Real build order: `view` inserts children BEFORE attaching the
        // parent's style, so cells are add_child'd while the grid is still
        // the default Flex. The grid-item min-width reset must still apply
        // (via set_style catching existing children) or wide columns
        // overflow. This is the exact order the bug reproduced at.
        {
            let mut t = LayoutTree::new();
            let grid = t.new_node();
            let specs = [(60.0_f32, 60.0_f32), (200.0, 200.0), (90.0, 900.0)];
            let mut cells = Vec::new();
            for (mn, mx) in specs {
                let c = t.new_node();
                t.set_measure_fn(c, text_measure(mn, mx));
                t.add_child(grid, c); // grid still Flex here — like `view`
                cells.push(c);
            }
            // Style the grid AFTER its children are inserted.
            let mut gr = StyleRules::default();
            gr.display = Some(runtime_core::DisplayKind::Grid);
            gr.grid_template_columns = Some(vec![TS::Auto; 3]);
            t.set_style(grid, &gr);
            t.compute(grid, 1000.0, 400.0);
            let w: Vec<f32> = cells.iter().map(|c| t.frame_of(*c).width).collect();
            let total: f32 = w.iter().sum();
            assert!(
                (total - 1000.0).abs() < 1.0,
                "children-before-style order must still fill+wrap, not overflow: {w:?} total {total}",
            );
            assert!(w[2] > w[0] + 300.0, "Description still takes the most room: {w:?}");
        }

        // NESTED like the real SDK: outer passthrough view → inner grid →
        // cells. The flex parent must not let the grid size its tracks at
        // max-content and overflow; the inner grid still fills the outer
        // width and wraps the wide column.
        {
            let mut t = LayoutTree::new();
            let outer = t.new_node(); // passthrough flex view (the SDK's outer)
            let grid = t.new_node();
            let specs = [(60.0_f32, 60.0_f32), (200.0, 200.0), (90.0, 900.0)];
            let mut cells = Vec::new();
            for (mn, mx) in specs {
                let c = t.new_node();
                t.set_measure_fn(c, text_measure(mn, mx));
                t.add_child(grid, c);
                cells.push(c);
            }
            let mut gr = StyleRules::default();
            gr.display = Some(runtime_core::DisplayKind::Grid);
            gr.grid_template_columns = Some(vec![TS::Auto; 3]);
            t.set_style(grid, &gr);
            t.add_child(outer, grid);
            t.compute(outer, 1000.0, 400.0);
            let gw = t.frame_of(grid).width;
            let w: Vec<f32> = cells.iter().map(|c| t.frame_of(*c).width).collect();
            let total: f32 = w.iter().sum();
            assert!(
                (total - 1000.0).abs() < 1.5,
                "nested grid must fill+wrap, not overflow: grid={gw} cols={w:?} total {total}",
            );
            assert!(w[2] > w[0] + 300.0, "nested: Description takes the most room: {w:?}");
        }

        // REAL idea-ui nesting: grid → cell VIEW (flex column + horizontal
        // padding, min-width reset by the engine) → text grandchild (the
        // node that actually measures/wraps). The cell must propagate the
        // text's small min-content so the column shrinks and the text
        // wraps — not clip.
        {
            let mut t = LayoutTree::new();
            let grid = t.new_node();
            let mut gr = StyleRules::default();
            gr.display = Some(runtime_core::DisplayKind::Grid);
            gr.grid_template_columns = Some(vec![TS::Auto; 3]);
            // build cells (view + padding) with a text grandchild each
            let specs = [(60.0_f32, 60.0_f32), (200.0, 200.0), (90.0, 900.0)];
            let mut texts = Vec::new();
            let mut cells = Vec::new();
            for (mn, mx) in specs {
                let cell = t.new_node();
                let mut cr = StyleRules::default();
                cr.padding_left = Some(px(16.0));
                cr.padding_right = Some(px(16.0));
                t.set_style(cell, &cr);
                let text = t.new_node();
                t.set_measure_fn(text, text_measure(mn, mx));
                t.add_child(cell, text);
                t.add_child(grid, cell);
                texts.push(text);
                cells.push(cell);
            }
            t.set_style(grid, &gr); // display lands AFTER children (real order)
            t.compute(grid, 1000.0, 400.0);
            let cw: Vec<f32> = cells.iter().map(|c| t.frame_of(*c).width).collect();
            let total: f32 = cw.iter().sum();
            assert!(
                (total - 1000.0).abs() < 1.5,
                "cell-view nesting must fill+wrap, not overflow: cells={cw:?} total {total}",
            );
            assert!(cw[2] > cw[0] + 300.0, "Description cell takes the most room: {cw:?}");
            assert!(
                t.frame_of(texts[2]).height > 16.5,
                "Description text wrapped to >1 line (height {})",
                t.frame_of(texts[2]).height,
            );
        }

        // Water-fill with TWO wide columns: the short column keeps its
        // content; the two wide columns share the remaining width equally
        // (neither collapses below the other). max-content=[60,400,500],
        // W=800 → Prop keeps 60; the 400/500 columns can't fit their fair
        // share so both cap at (800−60)/2 = 370.
        {
            let mut t = LayoutTree::new();
            let grid = t.new_node();
            let mut gr = StyleRules::default();
            gr.display = Some(runtime_core::DisplayKind::Grid);
            gr.grid_template_columns = Some(vec![TS::Auto; 3]);
            t.set_style(grid, &gr);
            let specs = [(60.0_f32, 60.0_f32), (120.0, 400.0), (140.0, 500.0)];
            let mut cells = Vec::new();
            for (mn, mx) in specs {
                let c = t.new_node();
                t.set_measure_fn(c, text_measure(mn, mx));
                t.add_child(grid, c);
                cells.push(c);
            }
            t.compute(grid, 800.0, 400.0);
            let w: Vec<f32> = cells.iter().map(|c| t.frame_of(*c).width).collect();
            assert!((w[0] - 60.0).abs() < 1.5, "short column keeps content (60): {}", w[0]);
            assert!((w[1] - 370.0).abs() < 1.5, "wide column 1 capped at 370: {}", w[1]);
            assert!((w[2] - 370.0).abs() < 1.5, "wide column 2 capped at 370: {}", w[2]);
        }

        // PROOF for the two-pass fix: per-column `fr` weights proportional
        // to each column's max-content give content-PROPORTIONAL columns
        // (Description dominant), unlike equal `auto`/`1fr`. min-width:0
        // lets the wide column wrap.
        {
            let mut t = LayoutTree::new();
            let grid = t.new_node();
            let specs = [(60.0_f32, 60.0_f32), (200.0, 200.0), (90.0, 900.0)];
            let mut cells = Vec::new();
            for (mn, mx) in specs {
                let c = t.new_node();
                t.set_measure_fn(c, text_measure(mn, mx));
                t.add_child(grid, c);
                cells.push(c);
            }
            let mut gr = StyleRules::default();
            gr.display = Some(runtime_core::DisplayKind::Grid);
            // fr weights proportional to max-content: 60 / 200 / 900.
            gr.grid_template_columns = Some(vec![TS::Fr(60.0), TS::Fr(200.0), TS::Fr(900.0)]);
            t.set_style(grid, &gr);
            t.compute(grid, 1000.0, 400.0);
            let w: Vec<f32> = cells.iter().map(|c| t.frame_of(*c).width).collect();
            println!("[proportional-fr] columns={w:?} total {}", w.iter().sum::<f32>());
            assert!((w.iter().sum::<f32>() - 1000.0).abs() < 1.0, "fills: {w:?}");
            assert!(w[2] > w[1] * 2.0, "Description ≫ Type via proportional fr: {w:?}");
        }

        // Under-full: all columns short (total content < width). They fill
        // the table without any single column ballooning past the others
        // absurdly — the content-heavy one is still widest.
        {
            let mut t = LayoutTree::new();
            let grid = t.new_node();
            let mut gr = StyleRules::default();
            gr.display = Some(runtime_core::DisplayKind::Grid);
            gr.grid_template_columns = Some(vec![TS::Auto; 3]);
            t.set_style(grid, &gr);
            let specs = [(60.0_f32, 60.0_f32), (120.0, 120.0), (140.0, 140.0)];
            let mut cells = Vec::new();
            for (mn, mx) in specs {
                let c = t.new_node();
                t.set_measure_fn(c, text_measure(mn, mx));
                t.add_child(grid, c);
                cells.push(c);
            }
            t.compute(grid, 1000.0, 400.0);
            let w: Vec<f32> = cells.iter().map(|c| t.frame_of(*c).width).collect();
            // EXACT CSS `table-layout: auto` (fits case): Min=Max=[60,120,140]
            // (no wrap), Σmax=320 ≤ W=1000, so the leftover (680) is split in
            // proportion to max-content: width = max + 680·max/320 →
            // [187.5, 375, 437.5]. Columns rank by content; never split evenly.
            assert!((w[0] - 187.5).abs() < 1.5, "Prop = 60 + share: {}", w[0]);
            assert!((w[1] - 375.0).abs() < 1.5, "Type = 120 + share: {}", w[1]);
            assert!((w[2] - 437.5).abs() < 1.5, "Description = 140 + share: {}", w[2]);
        }
    }

    /// Regression for the macOS toast bug: a column of `presence`-wrapped
    /// cards must STACK, which requires each presence placeholder to be
    /// IN-FLOW (size to its child), not `position: absolute; inset: 0`.
    ///
    /// This proves the mechanism the macOS `create_presence_placeholder` fix
    /// relies on — and `is_absolute`, the getter `insert` reads to decide
    /// whether to upgrade a placeholder to fill. With in-flow wrappers the two
    /// cards lay out one below the other and the stack's height is their sum;
    /// with absolute inset:0 wrappers (the old, buggy placeholder) both
    /// collapse onto y=0 and overlap — exactly the "toasts don't stack, no
    /// bottom gap" symptom.
    #[test]
    fn presence_wrappers_stack_in_flow_but_overlap_when_absolute() {
        use runtime_core::FlexDirection;

        // (column container, two wrappers, two 50×30 leaf cards), built with
        // `wrapper_style` applied to each wrapper.
        fn build(wrapper_style: StyleRules) -> (LayoutTree, LayoutNode, [LayoutNode; 2]) {
            let mut t = LayoutTree::new();
            let stack = t.new_node();
            let mut sr = StyleRules::default();
            sr.flex_direction = Some(FlexDirection::Column);
            t.set_style(stack, &sr);

            let mut wrappers = Vec::new();
            for _ in 0..2 {
                let w = t.new_node();
                t.set_style(w, &wrapper_style);
                let card = t.new_node();
                let mut cr = StyleRules::default();
                cr.width = Some(px(50.0));
                cr.height = Some(px(30.0));
                t.set_style(card, &cr);
                t.add_child(w, card);
                t.add_child(stack, w);
                wrappers.push(w);
            }
            (t, stack, [wrappers[0], wrappers[1]])
        }

        // In-flow wrappers (the fixed placeholder): each sizes to its card and
        // the second stacks below the first.
        {
            let (mut t, stack, [w0, w1]) = build(StyleRules::default());
            assert!(!t.is_absolute(w0), "default wrapper is in-flow");
            t.compute(stack, 400.0, 400.0);
            let f0 = t.frame_of(w0);
            let f1 = t.frame_of(w1);
            assert_eq!(f0.height, 30.0, "wrapper sizes to its card");
            assert!((f0.y - 0.0).abs() < 0.5, "first card at the top: y0={}", f0.y);
            assert!(
                (f1.y - 30.0).abs() < 0.5,
                "second card stacks one card-height below the first: y1={} (expected ~30)",
                f1.y
            );
        }

        // Absolute inset:0 wrappers (the OLD, buggy placeholder): both fill the
        // stack and pin to the same origin, so the cards overlap — they don't
        // stack (the reported macOS toast symptom).
        {
            let mut abs = StyleRules::default();
            abs.position = Some(runtime_core::Position::Absolute);
            abs.top = Some(px(0.0));
            abs.left = Some(px(0.0));
            abs.right = Some(px(0.0));
            abs.bottom = Some(px(0.0));
            let (mut t, stack, [w0, w1]) = build(abs);
            assert!(t.is_absolute(w0), "absolute wrapper reports out-of-flow");
            t.compute(stack, 400.0, 400.0);
            let f0 = t.frame_of(w0);
            let f1 = t.frame_of(w1);
            assert_eq!(f0.y, f1.y, "absolute wrappers pin to the same y");
            assert_eq!(f0.y, 0.0, "both at inset:0 origin — they overlap, no stacking");
        }
    }

    fn px(v: f32) -> Tokenized<FwLength> {
        Tokenized::Literal(FwLength::Px(v))
    }
    fn autol() -> Tokenized<FwLength> {
        Tokenized::Literal(FwLength::Auto)
    }
    fn f32t(v: f32) -> Tokenized<f32> {
        Tokenized::Literal(v)
    }

    /// `set_style` returns whether the change moved LAYOUT geometry. Paint-only
    /// restyles (color/background/border) must return `false` so a backend can
    /// skip a needless layout pass (the macOS scroll-jitter fix); a real
    /// geometry change (width) must return `true`.
    #[test]
    fn set_style_reports_only_geometry_changes() {
        use runtime_core::Color;
        let mut t = LayoutTree::new();
        let node = t.new_node();

        let mut base = StyleRules::default();
        base.width = Some(px(100.0));
        base.background = Some(Tokenized::Literal(Color("#ffffff".into())));
        // First application establishes the style — geometry went from the
        // default to a definite 100px width, so it counts as a change.
        assert!(t.set_style(node, &base), "first apply sets geometry");

        // Re-applying the identical style is a no-op.
        assert!(!t.set_style(node, &base), "identical restyle is not a layout change");

        // A paint-only change (background + border color) must NOT report a
        // layout change — this is exactly the `:hover` scroll-jitter case.
        let mut painted = base.clone();
        painted.background = Some(Tokenized::Literal(Color("#000000".into())));
        painted.border_top_color = Some(Tokenized::Literal(Color("#cccccc".into())));
        assert!(
            !t.set_style(node, &painted),
            "color/background/border are not in the Taffy style → no layout change",
        );

        // A width change IS a layout change.
        let mut wider = painted.clone();
        wider.width = Some(px(200.0));
        assert!(t.set_style(node, &wider), "width change is a layout change");
    }

    /// Regression: a `Display::Grid` container aligns its columns across
    /// every row — column `i` has one x and one width no matter how the
    /// cells in that column differ row to row.
    ///
    /// This is the layout-engine guarantee the `table` SDK relies on for
    /// cross-platform parity. The old native table laid each row out as
    /// an independent flex container, so a wide cell in row 0 widened
    /// only row 0's copy of that column and columns drifted between rows.
    /// The first half of this test reproduces that drift with two flex
    /// rows; the second half shows the grid fixes it with the SAME cell
    /// content. Cell widths are seeded as intrinsic sizes so the tracks
    /// have something to size to (a real cell's text measures similarly).
    #[test]
    fn grid_aligns_columns_across_rows_unlike_independent_flex_rows() {
        // Per-row, per-column intrinsic cell widths. Row 0's column 1 is
        // wide enough (220) to exceed its equal share of a 300px row
        // (100), so under independent flex rows it steals width from its
        // siblings and that column's x lands differently than in row 1
        // (where every cell fits the equal share) — the drift bug.
        let widths = [[20.0_f32, 220.0, 20.0], [80.0, 10.0, 50.0]];

        // --- Old behavior: two independent flex rows ---------------------
        // Each row is its own flex container; a cell claims equal share
        // (grow:1 / basis:0). The two rows therefore disagree on every
        // column's x — this is the bug.
        let flex_col1_x = {
            let mut t = LayoutTree::new();
            let root = t.new_node();
            let mut col1_cells = Vec::new();
            for row_widths in widths.iter() {
                let row = t.new_node();
                let mut rr = StyleRules::default();
                rr.flex_direction = Some(FwFlexDirection::Row);
                t.set_style(row, &rr);
                t.add_child(root, row);
                for (c, w) in row_widths.iter().enumerate() {
                    let cell = t.new_node();
                    let mut cr = StyleRules::default();
                    cr.flex_grow = Some(f32t(1.0));
                    cr.flex_basis = Some(px(0.0));
                    t.set_style(cell, &cr);
                    t.set_intrinsic_size(cell, *w, 10.0);
                    t.add_child(row, cell);
                    if c == 1 {
                        col1_cells.push(cell);
                    }
                }
            }
            t.compute(root, 300.0, 0.0);
            [t.frame_of(col1_cells[0]).x, t.frame_of(col1_cells[1]).x]
        };
        assert!(
            (flex_col1_x[0] - flex_col1_x[1]).abs() > 1.0,
            "independent flex rows MUST drift: row0 col1 x={} vs row1 col1 x={}",
            flex_col1_x[0],
            flex_col1_x[1],
        );

        // --- New behavior: one grid, columns shared across rows ----------
        let mut t = LayoutTree::new();
        let root = t.new_node();
        let grid = t.new_node();
        let mut gr = StyleRules::default();
        gr.display = Some(runtime_core::DisplayKind::Grid);
        // Predictable max-content tracks for the assertion: each column
        // sizes to its widest cell across both rows.
        gr.grid_template_columns = Some(vec![
            runtime_core::TrackSize::MaxContent,
            runtime_core::TrackSize::MaxContent,
            runtime_core::TrackSize::MaxContent,
        ]);
        t.set_style(grid, &gr);
        t.add_child(root, grid);

        // Six cells, row-major auto-flow → row0 = cells 0..3, row1 = 3..6.
        let mut cells = Vec::new();
        for row_widths in widths.iter() {
            for w in row_widths.iter() {
                let cell = t.new_node();
                t.set_intrinsic_size(cell, *w, 10.0);
                t.add_child(grid, cell);
                cells.push(cell);
            }
        }
        t.compute(root, 300.0, 0.0);

        for col in 0..3 {
            let top = t.frame_of(cells[col]); // row 0
            let bot = t.frame_of(cells[col + 3]); // row 1
            assert!(
                (top.x - bot.x).abs() < 0.5,
                "grid column {col} must share one x across rows: {} vs {}",
                top.x,
                bot.x,
            );
            assert!(
                (top.width - bot.width).abs() < 0.5,
                "grid column {col} must share one width across rows: {} vs {}",
                top.width,
                bot.width,
            );
        }
        // And each column sized to its widest cell (max-content).
        let col0 = t.frame_of(cells[0]).width;
        let col1 = t.frame_of(cells[1]).width;
        assert!(col0 >= 80.0 - 0.5, "column 0 sizes to its widest cell (80): {col0}");
        assert!(col1 >= 220.0 - 0.5, "column 1 sizes to its widest cell (220): {col1}");
    }

    /// `auto` grid columns (the `table` SDK's recipe) fill the container
    /// width because a grid defaults to `justify-content: stretch`: a
    /// `width: 100%` HTML table fills, and the native grid must match.
    /// With equally narrow content the columns share the container evenly
    /// rather than hugging content and leaving a gap. Guards the
    /// grid-stretch default — without it, `auto` tracks hug content under
    /// the framework's `flex-start` seed and this fails.
    #[test]
    fn grid_auto_tracks_fill_container_width() {
        let mut t = LayoutTree::new();
        let root = t.new_node();
        let grid = t.new_node();
        let mut gr = StyleRules::default();
        gr.display = Some(runtime_core::DisplayKind::Grid);
        gr.grid_template_columns = Some(vec![runtime_core::TrackSize::Auto; 3]);
        t.set_style(grid, &gr);
        t.add_child(root, grid);
        // Three equally narrow cells (10px each) — total content 30 ≪ 300.
        let mut cells = Vec::new();
        for _ in 0..3 {
            let cell = t.new_node();
            t.set_intrinsic_size(cell, 10.0, 10.0);
            t.add_child(grid, cell);
            cells.push(cell);
        }
        t.compute(root, 300.0, 0.0);
        let total: f32 = cells.iter().map(|c| t.frame_of(*c).width).sum();
        assert!(
            total >= 300.0 - 1.0,
            "auto columns must stretch to fill the 300px container, got {total}",
        );
        // Equal content → equal share (~100 each).
        for c in &cells {
            let w = t.frame_of(*c).width;
            assert!(
                (w - 100.0).abs() < 1.0,
                "equal-content auto columns share the width evenly (~100), got {w}",
            );
        }
    }

    /// `auto` grid columns are CONTENT-sized, like `table-layout: auto`:
    /// a column with wider content is wider than its siblings instead of
    /// every column being forced equal (the user-reported regression of
    /// the earlier `1fr` recipe, which split width evenly and wrapped
    /// long text). Wide content "pushes the cell wider"; the leftover is
    /// still shared so the table fills its width.
    #[test]
    fn grid_auto_columns_are_content_sized_not_forced_equal() {
        let mut t = LayoutTree::new();
        let root = t.new_node();
        let grid = t.new_node();
        let mut gr = StyleRules::default();
        gr.display = Some(runtime_core::DisplayKind::Grid);
        gr.grid_template_columns = Some(vec![runtime_core::TrackSize::Auto; 3]);
        t.set_style(grid, &gr);
        t.add_child(root, grid);
        // Column 0 has wide content (180); columns 1 and 2 are narrow (20).
        // Single row so each cell maps 1:1 to its column.
        let widths = [180.0_f32, 20.0, 20.0];
        let mut cells = Vec::new();
        for w in widths {
            let cell = t.new_node();
            t.set_intrinsic_size(cell, w, 10.0);
            t.add_child(grid, cell);
            cells.push(cell);
        }
        t.compute(root, 400.0, 0.0);
        let col0 = t.frame_of(cells[0]).width;
        let col1 = t.frame_of(cells[1]).width;
        assert!(
            col0 > col1 + 50.0,
            "the wide-content column must be substantially wider than a \
             narrow one (content pushes width), got col0={col0} col1={col1}",
        );
        assert!(
            col0 >= 180.0 - 1.0,
            "the wide column is at least its content width (180), got {col0}",
        );
        let total = col0 + col1 + t.frame_of(cells[2]).width;
        assert!(
            total >= 400.0 - 1.0,
            "columns still fill the 400px container, got total {total}",
        );
    }

    /// Regression: the idea-ui `Modal` card must size to its content (up to a
    /// viewport cap) and scroll past the cap — not collapse to 0×0.
    ///
    /// The bug ("modal opens but nothing renders, only the backdrop"): the
    /// Modal's surface is a content-sized view (`max_height` cap, auto height)
    /// wrapping a `scroll_view`. `scroll_view` seeds its node with
    /// `flex_grow:1 / flex_basis:0` (the "fill a bounded parent" shape). With
    /// `overflow:scroll`, that node contributes 0 to its content-sized
    /// parent's intrinsic height, so the surface — and the whole card —
    /// collapsed to 0 height. Only the full-bleed backdrop stayed visible.
    ///
    /// The fix gives the scroller the "content-sized up to a cap" shape:
    /// `flex_grow:0 + flex_basis:auto + min_height:0 + max_height:cap` (the
    /// body keeps `flex_shrink:0` so tall content overflows the cap and
    /// scrolls). This test reproduces both: a short modal hugs its content; a
    /// tall one is capped (and its body keeps full height so the scroll view
    /// has something to scroll).
    fn modal_card_heights(content: f32, cap: f32) -> (f32, f32, f32) {
        let mut t = LayoutTree::new();
        let root = t.new_node();

        // Surface: content-sized, definite width, clips to rounded corners.
        let surface = t.new_node();
        let mut sr = StyleRules::default();
        sr.width = Some(px(300.0));
        sr.max_height = Some(px(cap));
        sr.overflow = Some(runtime_core::Overflow::Hidden);
        t.set_style(surface, &sr);

        // Scroller: the real path — `scroll_view` seed, then the modal's
        // `modal_scroll_sheet` override (the fix under test).
        let scroller = t.new_node();
        t.set_overflow_scroll(scroller, false); // seeds grow:1/basis:0 + overflow.y scroll
        let mut scr = StyleRules::default();
        scr.flex_direction = Some(FwFlexDirection::Column);
        scr.flex_grow = Some(f32t(0.0));
        scr.flex_basis = Some(autol());
        scr.min_height = Some(px(0.0));
        scr.max_height = Some(px(cap));
        t.set_style(scroller, &scr);

        // Body: column, and crucially `flex_shrink:0` so it keeps full height.
        let body = t.new_node();
        let mut br = StyleRules::default();
        br.flex_direction = Some(FwFlexDirection::Column);
        br.flex_shrink = Some(f32t(0.0));
        br.height = Some(px(content));
        br.width = Some(px(300.0));
        t.set_style(body, &br);

        t.add_child(root, surface);
        t.add_child(surface, scroller);
        t.add_child(scroller, body);
        t.compute(root, 393.0, 852.0);
        (
            t.frame_of(surface).height,
            t.frame_of(scroller).height,
            t.frame_of(body).height,
        )
    }

    #[test]
    fn regression_modal_scroller_content_sized_then_capped() {
        let cap = 700.0;

        // Short content: the card hugs its content (NOT 0 → the collapse bug;
        // NOT the full cap → a giant empty box).
        let (surface, scroller, body) = modal_card_heights(200.0, cap);
        assert!(
            (surface - 200.0).abs() < 1.0,
            "short modal surface must size to its 200px content, got {surface} \
             (0 == the collapse bug; {cap} == always-cap-tall)"
        );
        assert!((scroller - 200.0).abs() < 1.0, "scroller hugs content, got {scroller}");
        assert!((body - 200.0).abs() < 1.0, "body at natural height, got {body}");

        // Tall content: the card caps at the viewport, and the body keeps its
        // full height so the scroll view has overflow to scroll.
        let (surface, scroller, body) = modal_card_heights(2000.0, cap);
        assert!(
            (surface - cap).abs() < 1.0,
            "tall modal surface must cap at {cap}, got {surface}"
        );
        assert!((scroller - cap).abs() < 1.0, "tall scroller caps at {cap}, got {scroller}");
        assert!(
            (body - 2000.0).abs() < 1.0,
            "tall body must keep its full 2000px height (so the scroll view \
             scrolls), got {body}"
        );
    }

    /// Reproduction for the idea-ui adorned `Field` not filling its column on
    /// macOS. Structure: a definite-width surface > FieldGroup (default column,
    /// `align_items: stretch`) > shell (flex ROW, `width: 100%`) > [icon leaf,
    /// input leaf with a ~100pt intrinsic measure + `flex_grow: 1`]. The shell
    /// must stretch to the full surface width and the input must grow to fill
    /// the row; the symptom was the shell hugging the lone icon (~24pt).
    ///
    /// Control: a NON-adorned input (leaf directly in FieldGroup) fills via the
    /// seed's cross-axis stretch — so whatever's wrong is specific to the
    /// flex-row shell, not the column stretch in general.
    #[derive(Clone, Copy)]
    enum Fill {
        None,
        AlignSelfStretch,
    }

    fn field_widths(fill: Fill, scroll_wrapped: bool) -> (f32, f32, f32) {
        let intrinsic: MeasureFn = Rc::new(|known: Size<Option<f32>>, _avail| Size {
            width: known.width.unwrap_or(100.0),
            height: known.height.unwrap_or(17.0),
        });

        let mut t = LayoutTree::new();
        let root = t.new_node(); // window; compute() fills it to 600 wide

        // Optional vertical scroll wrapper (the docs page lives in one). A
        // vertical scroller seeds grow:1/basis:0 and overflow.y scroll; its
        // content width is the viewport width — the question is whether that
        // width stays *definite* for a percent-width grandchild.
        let host = if scroll_wrapped {
            let sc = t.new_node();
            t.set_overflow_scroll(sc, false);
            t.add_child(root, sc);
            sc
        } else {
            root
        };

        // The DemoSurface the docs page wraps every preview in: a column that
        // *centers* its children (`align_items: center`). This is the actual
        // reason the Field doesn't fill — a centering parent sizes a child to
        // its content and centers it, rather than stretching it. Modeling this
        // is what my first test omitted.
        let surface = t.new_node();
        let mut surf = StyleRules::default();
        surf.align_items = Some(runtime_core::AlignItems::Center);
        surf.justify_content = Some(runtime_core::JustifyContent::Center);
        t.set_style(surface, &surf);
        t.add_child(host, surface);

        let group = t.new_node(); // FieldGroup
        let mut gr = StyleRules::default();
        // The fix under test: a Field must fill its container width even when
        // the container centers. The shipped strategy on the FieldGroup is
        // `align_self: stretch` — it overrides the parent's `align_items:
        // center` for this child (the Divider precedent) and keeps width auto.
        match fill {
            Fill::None => {}
            Fill::AlignSelfStretch => {
                gr.align_self = Some(runtime_core::AlignSelf::Stretch)
            }
        }
        t.set_style(group, &gr);
        t.add_child(surface, group);

        // Shell: flex row with the shipped `width: 100%` (resolves against the
        // FieldGroup; only fills once the group itself fills).
        let shell = t.new_node();
        let mut sr = StyleRules::default();
        sr.flex_direction = Some(FwFlexDirection::Row);
        sr.align_items = Some(runtime_core::AlignItems::Center);
        sr.width = Some(Tokenized::Literal(FwLength::Percent(100.0)));
        t.set_style(shell, &sr);
        t.add_child(group, shell);

        // Icon leaf: fixed 24×24.
        let icon = t.new_node();
        let mut ir = StyleRules::default();
        ir.width = Some(px(24.0));
        ir.height = Some(px(24.0));
        t.set_style(icon, &ir);
        t.add_child(shell, icon);

        // Input leaf: flex_grow 1, intrinsic ~100 measure (mimics NSTextField).
        let input = t.new_node();
        let mut inr = StyleRules::default();
        inr.flex_grow = Some(f32t(1.0));
        t.set_style(input, &inr);
        t.set_measure_fn(input, intrinsic.clone());
        t.add_child(shell, input);

        t.compute(root, 600.0, 800.0);
        (
            t.frame_of(group).width,
            t.frame_of(shell).width,
            t.frame_of(input).width,
        )
    }

    #[test]
    fn adorned_field_shell_fills_column() {
        // Bug repro: under a CENTERING parent (DemoSurface), a Field with no
        // cross-axis fill hugs its content and gets centered — the icon-only
        // adorned field collapsed this way on macOS.
        let (group_n, _shell_n, _input_n) = field_widths(Fill::None, true);
        assert!(
            group_n < 200.0,
            "WITHOUT the fix a centering parent collapses the field to its \
             content width, got {group_n} (this is the bug)"
        );

        // The fix we ship — `align_self: stretch` on FieldGroup — must fill in
        // BOTH the plain and the scroll-wrapped tree (the docs page scrolls).
        for scroll in [false, true] {
            let (group_s, shell_s, input_s) = field_widths(Fill::AlignSelfStretch, scroll);
            assert!(
                (group_s - 600.0).abs() < 1.0,
                "align_self:stretch field must fill the 600px surface (scroll={scroll}), got {group_s}"
            );
            assert!(
                (shell_s - 600.0).abs() < 1.0,
                "shell fills once the group fills (scroll={scroll}), got {shell_s}"
            );
            assert!(input_s > 500.0, "input grows to fill the row (scroll={scroll}), got {input_s}");
        }
    }

    /// Build the docs-`forms` Bio tree and measure ancestor heights as the
    /// autosizing `text_area` leaf grows. Mirrors the LIVE structure that
    /// clipped the counter beneath a grown Bio:
    ///   scroll page → DemoSurface card (`align_items: center`)
    ///     → DemoSurfaceContent (`align_items: center`, `width: 100%`,
    ///        `max_width: 480`)
    ///       → Stack → [ FieldGroup (`align_self: stretch`)
    ///                     → label(16) + textarea-leaf(measure) + helper(16),
    ///                   counter(16) ]
    ///
    /// The text-area leaf measures like real prose: `lines = ceil(run / width)`
    /// (a WIDER offered width → FEWER lines → a SHORTER box), floored at 4 rows.
    /// `glyph_run` is the content's total advance width; each entry is one
    /// "edit" — set the run, `mark_dirty(leaf)`, recompute. `viewport_w` lets
    /// the test exercise BOTH a wide window (the cap binds at 480) and a narrow
    /// one (the percent governs). Returns `(surface_height, stack_height)` per
    /// edit so the test can assert the ancestor always CONTAINS its child.
    fn bio_surface_vs_stack(edits: &[f32], viewport_w: f32) -> Vec<(f32, f32)> {
        let glyph_run = Rc::new(std::cell::Cell::new(0.0f32));
        let gr = glyph_run.clone();
        const LINE_H: f32 = 14.25;
        const FLOOR_ROWS: f32 = 4.0;
        let measure: MeasureFn = Rc::new(move |known: Size<Option<f32>>, avail| {
            let definite_w = match avail.width {
                AvailableSpace::Definite(w) => Some(w),
                AvailableSpace::MaxContent => None,      // ∞ → single line
                AvailableSpace::MinContent => Some(1.0), // 0 → max wrapping
            };
            let w = known.width.unwrap_or(definite_w.unwrap_or(f32::INFINITY));
            let lines = match definite_w {
                Some(dw) if dw >= 1.0 => (gr.get() / dw).ceil().max(1.0),
                _ => 1.0,
            };
            let rows = lines.max(FLOOR_ROWS);
            Size { width: w, height: known.height.unwrap_or(rows * LINE_H) }
        });

        let mut t = LayoutTree::new();
        let root = t.new_node(); // window

        // The forms page lives inside a vertical scroll view.
        let host = t.new_node();
        t.set_overflow_scroll(host, false);
        t.add_child(root, host);

        // DemoSurface CARD: centers its content (the parent that clipped).
        let card = t.new_node();
        let mut card_s = StyleRules::default();
        card_s.align_items = Some(runtime_core::AlignItems::Center);
        card_s.justify_content = Some(runtime_core::JustifyContent::Center);
        t.set_style(card, &card_s);
        t.add_child(host, card);

        // DemoSurfaceContent: the EXACT shipped pattern — fill up to a 480 cap,
        // centered. This `width: 100%` + `max_width` combo is what Taffy
        // under-measured the height of (the bug this test guards).
        let surface = t.new_node();
        let mut surf = StyleRules::default();
        surf.align_items = Some(runtime_core::AlignItems::Center);
        surf.width = Some(Tokenized::Literal(FwLength::Percent(100.0)));
        surf.max_width = Some(px(480.0));
        t.set_style(surface, &surf);
        t.add_child(card, surface);

        // Stack: the Textarea field + the character counter.
        let stack = t.new_node();
        t.add_child(surface, stack);

        // FieldGroup: align_self stretch overrides the centering parent.
        let group = t.new_node();
        let mut gr_s = StyleRules::default();
        gr_s.align_self = Some(runtime_core::AlignSelf::Stretch);
        t.set_style(group, &gr_s);
        t.add_child(stack, group);

        let label = t.new_node();
        let mut lr = StyleRules::default();
        lr.height = Some(px(16.0));
        t.set_style(label, &lr);
        t.add_child(group, label);

        // The autosizing text_area chrome container (the Taffy leaf).
        let textarea = t.new_node();
        t.set_measure_fn(textarea, measure.clone());
        t.add_child(group, textarea);

        let helper = t.new_node();
        let mut hr = StyleRules::default();
        hr.height = Some(px(16.0));
        t.set_style(helper, &hr);
        t.add_child(group, helper);

        // The character counter — the element the live bug clipped.
        let counter = t.new_node();
        let mut cr = StyleRules::default();
        cr.height = Some(px(16.0));
        t.set_style(counter, &cr);
        t.add_child(stack, counter);

        let mut out = Vec::new();
        for &run in edits {
            glyph_run.set(run);
            t.mark_dirty(textarea);
            t.compute(root, viewport_w, 5000.0);
            out.push((t.frame_of(surface).height, t.frame_of(stack).height));
        }
        out
    }

    /// Regression for the macOS Bio `text_area` clipping its counter, root-caused
    /// to a Taffy/CSS divergence (NOT macOS-specific): a `width: 100%` +
    /// `max_width` container measures a wrapping child's intrinsic HEIGHT at the
    /// pre-cap percent width (fewer lines → too short), then renders the WIDTH
    /// capped (more lines → taller) — so the ancestor is shorter than its child
    /// and clips the content below. Confirmed live via the robot: the Bio
    /// textarea grew 72→170→184 while DemoSurfaceContent stayed sized to a
    /// 142-px-textarea height, clipping the "N characters" counter.
    ///
    /// `LayoutTree::compute`'s capped-percent-width second pass restores CSS
    /// parity (measure height at the used/capped width). The ancestor must
    /// CONTAIN its child at every growth step, in BOTH a wide window (the cap
    /// binds at 480) and a narrow one (the percent governs) — the latter guards
    /// against a fix that merely MOVES the bug to small viewports.
    #[test]
    fn regression_capped_percent_width_measures_height_at_used_width() {
        // glyph runs: empty, then two growth steps (≈880, then ≈980 chars × 7pt).
        let edits = [0.0, 6160.0, 6860.0];
        for viewport_w in [676.0_f32, 360.0] {
            let r = bio_surface_vs_stack(&edits, viewport_w);
            for (i, (surface, stack)) in r.iter().enumerate() {
                assert!(
                    surface + 0.5 >= *stack,
                    "viewport={viewport_w}, edit #{i}: ancestor surface ({surface}) \
                     must contain its child stack ({stack}) — a shorter parent \
                     clips the content below the text area",
                );
            }
            // It must also TRACK the growth, not freeze at the first measure.
            assert!(
                r.last().unwrap().0 > r.first().unwrap().0 + 1.0,
                "viewport={viewport_w}: surface must grow with the text area: {r:?}",
            );
        }
    }

    /// Regression: a VERTICAL scroll view's content extent must clip its
    /// CROSS axis to the scroll view's frame — an over-wide child (a
    /// non-wrapping button row, a wide table) must NOT turn it into a
    /// horizontal scroller.
    ///
    /// The bug: the iOS `contentSize` sync took the bounding box of every
    /// descendant frame on BOTH axes, so a child wider than the viewport
    /// drove `contentSize.width` past the bounds → a phantom horizontal
    /// scroll with dead space. [`scroll_content_extent`](LayoutTree::scroll_content_extent)
    /// clips the cross axis (rule 1): the width tracks the 393px frame, not
    /// the 800px child. (Verified load-bearing: a naive max-x/max-y walk
    /// reports 800.)
    #[test]
    fn scroll_content_extent_clips_cross_axis() {
        let mut t = LayoutTree::new();
        let root = t.new_node();
        let mut rr = StyleRules::default();
        rr.flex_direction = Some(FwFlexDirection::Column);
        t.set_style(root, &rr);

        let scroller = t.new_node();
        t.set_overflow_scroll(scroller, false);
        let mut scr = StyleRules::default();
        scr.flex_direction = Some(FwFlexDirection::Column);
        scr.width = Some(pct(100.0));
        t.set_style(scroller, &scr);

        let wide = t.new_node();
        let mut wr = StyleRules::default();
        wr.width = Some(px(800.0));
        wr.height = Some(px(100.0));
        t.set_style(wide, &wr);

        t.add_child(root, scroller);
        t.add_child(scroller, wide);
        t.compute(root, 393.0, 852.0);

        let (w, h) = t.scroll_content_extent(scroller);
        assert!(
            (w - 393.0).abs() < 1.0,
            "vertical scroll content width must clip to the 393px viewport, \
             got {w} (≈800 == the cross-axis bleed)"
        );
        assert!((h - 100.0).abs() < 1.0, "scroll content height tracks the child, got {h}");
    }

    fn pct(v: f32) -> Tokenized<FwLength> {
        Tokenized::Literal(FwLength::Percent(v))
    }

    /// Regression: the idea-ui `Modal`'s safe-area handling relies on two
    /// Taffy behaviors that must not silently change under an engine upgrade.
    ///
    /// The Modal pads its fullscreen centering container by the platform
    /// safe-area insets so the card centers within the SAFE rect (not the
    /// full window) — required because the insets are asymmetric (top notch ≠
    /// bottom home-indicator), so centering in the full window would leave the
    /// card under the larger inset. The dimming backdrop is a sibling with
    /// `position:absolute; inset:0`, and it must STILL fill the whole window
    /// (an absolute child resolves against its parent's padding box, which
    /// includes the padding region) — otherwise the notch/home-indicator
    /// strips wouldn't be dimmed.
    ///
    /// This pins both: backdrop == full window, and the card sits entirely
    /// inside the safe rect under realistic asymmetric insets.
    #[test]
    fn regression_modal_safe_area_backdrop_fullbleed_card_centered_in_safe_rect() {
        // iPhone-class viewport with a Dynamic Island (top) and home
        // indicator (bottom).
        let (vw, vh) = (393.0_f32, 852.0_f32);
        let (top, bottom) = (59.0_f32, 34.0_f32);
        let (card_w, card_h) = (300.0_f32, 200.0_f32);

        let mut t = LayoutTree::new();
        let root = t.new_node();

        // Centering container: fills the window, padded by the insets,
        // centers its in-flow child.
        let container = t.new_node();
        let mut cr = StyleRules::default();
        cr.width = Some(pct(100.0));
        cr.height = Some(pct(100.0));
        cr.padding_top = Some(px(top));
        cr.padding_bottom = Some(px(bottom));
        cr.align_items = Some(FwAlignItems::Center);
        cr.justify_content = Some(FwJustifyContent::Center);
        t.set_style(container, &cr);

        // Backdrop: absolute, inset 0 (sibling of the card, painted behind).
        let backdrop = t.new_node();
        let mut br = StyleRules::default();
        br.position = Some(FwPosition::Absolute);
        br.top = Some(px(0.0));
        br.left = Some(px(0.0));
        br.right = Some(px(0.0));
        br.bottom = Some(px(0.0));
        t.set_style(backdrop, &br);

        // Card: a fixed size standing in for the content-sized surface.
        let card = t.new_node();
        let mut kr = StyleRules::default();
        kr.width = Some(px(card_w));
        kr.height = Some(px(card_h));
        t.set_style(card, &kr);

        t.add_child(root, container);
        t.add_child(container, backdrop);
        t.add_child(container, card);
        t.compute(root, vw, vh);

        // Backdrop fills the WHOLE window despite the container padding.
        let bd = t.frame_of(backdrop);
        assert!(
            bd.x.abs() < 0.5
                && bd.y.abs() < 0.5
                && (bd.width - vw).abs() < 0.5
                && (bd.height - vh).abs() < 0.5,
            "backdrop must stay full-bleed (the scrim dims the whole window, \
             notch + home-indicator included), got {bd:?}"
        );

        // Card sits entirely within the safe rect [top, vh - bottom].
        let c = t.frame_of(card);
        assert!(
            c.y >= top - 0.5,
            "card top {} must clear the top inset {top}",
            c.y
        );
        assert!(
            c.y + c.height <= vh - bottom + 0.5,
            "card bottom {} must clear the bottom inset (safe edge {})",
            c.y + c.height,
            vh - bottom
        );
    }

    /// Regression: `remove_child` must NOT panic when `child` isn't
    /// actually a child of `parent`.
    ///
    /// The Modal-dismiss crash (iOS + Android): a `Modal` lowers to an
    /// `Element::Portal`, which the backends register as an orphan Taffy
    /// ROOT (`insert`/`insert_at` deliberately skip wiring a portal as a
    /// child of its surrounding parent). When `if open { Modal }` flips
    /// false, the anchorless spliced-`when` unmount calls
    /// `Backend::remove_child(parent, portal)` → `LayoutTree::remove_child`.
    /// Taffy's `remove_child` does `children.position(..).unwrap()` and
    /// panics because the portal isn't in `parent`'s child list; on the
    /// backend's non-unwinding FFI boundary that panic aborts the process
    /// ("panic in a function that cannot unwind" → SIGABRT). The fix makes a
    /// non-child removal a no-op. This test panics before the fix and passes
    /// after.
    #[test]
    fn regression_remove_child_ignores_non_child_no_panic() {
        let mut t = LayoutTree::new();
        let parent = t.new_node();
        let orphan = t.new_node(); // never added as a child of `parent`

        // Before the fix this aborts inside taffy's `remove_child`
        // (`called Option::unwrap() on a None value`).
        t.remove_child(parent, orphan);
        assert_eq!(
            t.children_of(parent).len(),
            0,
            "removing a non-child leaves the parent's child set untouched"
        );

        // A genuine child is still removed (the fix doesn't break the
        // normal path).
        let real_child = t.new_node();
        t.add_child(parent, real_child);
        assert_eq!(t.children_of(parent).len(), 1);
        t.remove_child(parent, real_child);
        assert_eq!(
            t.children_of(parent).len(),
            0,
            "a real child is detached as before"
        );
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

    /// Regression: a native scroll view whose content is parented as a
    /// Taffy CHILD of the scroll node (the Android ScrollView shape: outer
    /// node → inner FrameLayout child → content) must mark the scroll node
    /// `overflow: scroll`, or the scroll node grows to its content height
    /// and there is nothing to scroll.
    ///
    /// The bug ("I can't scroll the sidebar" on Android): the outer scroll
    /// node is a `flex_grow:1 / flex_basis:0` child of a bounded panel, so
    /// it *should* fill the panel and let its tall content overflow. But a
    /// flex item's automatic minimum size is its `min-content` — and the
    /// content (the inner + its children) is 800px tall — so flexbox cannot
    /// shrink the outer below 800. The outer grows to 800, ends up exactly
    /// as tall as its content, and the native ScrollView has zero scrollable
    /// overflow. Marking the outer `overflow:scroll` suppresses the
    /// automatic-minimum-size floor (CSS rule), letting the parent bound the
    /// outer to the panel height while the content overflows — exactly what
    /// makes a ScrollView scroll. This is why macOS/terminal call
    /// `set_overflow_scroll`; Android (which also parents content under the
    /// scroll node) needs it for the same reason.
    #[test]
    fn regression_scroll_node_bounded_by_overflow_scroll_not_content() {
        let panel_h = 300.0_f32;
        let child_h = 200.0_f32;
        let n_children = 4; // 4 * 200 = 800 content, well past the 300 panel

        // Build panel(fixed 300, Column) → outer scroll node
        // (flex_grow:1, flex_basis:0) → inner(Column) → N fixed children.
        // When `overflow_scroll` is true, mark the outer as a scroll node.
        // Returns the outer scroll node's computed height.
        let outer_height = |overflow_scroll: bool| -> f32 {
            let mut t = LayoutTree::new();

            let panel = t.new_node();
            let mut panel_rules = StyleRules::default();
            panel_rules.width = Some(px(260.0));
            panel_rules.height = Some(px(panel_h)); // bounded viewport
            panel_rules.flex_direction = Some(FwFlexDirection::Column);
            t.set_style(panel, &panel_rules);

            let outer = t.new_node();
            let mut outer_rules = StyleRules::default();
            outer_rules.flex_grow = Some(Tokenized::Literal(1.0));
            outer_rules.flex_basis = Some(px(0.0));
            outer_rules.flex_direction = Some(FwFlexDirection::Column);
            t.set_style(outer, &outer_rules);
            if overflow_scroll {
                t.set_overflow_scroll(outer, false);
            }
            t.add_child(panel, outer);

            let inner = t.new_node();
            let mut inner_rules = StyleRules::default();
            inner_rules.flex_direction = Some(FwFlexDirection::Column);
            inner_rules.align_items = Some(FwAlignItems::Stretch);
            t.set_style(inner, &inner_rules);
            t.add_child(outer, inner);

            for _ in 0..n_children {
                let child = t.new_node();
                let mut child_rules = StyleRules::default();
                child_rules.height = Some(px(child_h));
                child_rules.flex_shrink = Some(Tokenized::Literal(0.0));
                t.set_style(child, &child_rules);
                t.add_child(inner, child);
            }

            t.compute(panel, 260.0, panel_h);
            t.frame_of(outer).height
        };

        // The fix: overflow:scroll bounds the outer to the panel so its
        // content overflows and the ScrollView can scroll.
        let bounded = outer_height(true);
        assert!(
            (bounded - panel_h).abs() < 1.0,
            "outer scroll node with overflow:scroll should stay bounded to \
             the {panel_h} panel (content overflows → ScrollView scrolls), \
             got {bounded}"
        );

        // The bug: without overflow:scroll the outer's auto-min-size = its
        // 800px content, so it grows to content and there's nothing to
        // scroll. Documents what the fix prevents.
        let grown = outer_height(false);
        let content_h = child_h * n_children as f32; // 800
        assert!(
            (grown - content_h).abs() < 1.0,
            "outer scroll node WITHOUT overflow:scroll grows to its {content_h} \
             content (the bug: scroll node == content height, no overflow to \
             scroll), got {grown}"
        );
    }

    // -----------------------------------------------------------------
    // Default `max-width: 100%` cross-axis clamp (web-parity wrap) +
    // its exemptions.
    // -----------------------------------------------------------------

    /// Simulates a UILabel/TextView measure_fn: at a finite available
    /// width the text wraps (width = avail, height grows by line); at
    /// `MaxContent`/unbounded it reports its single-line width;
    /// `MinContent` reports the longest word.
    fn text_measure(single_line: f32, longest_word: f32, line_h: f32) -> MeasureFn {
        Rc::new(move |known, avail| {
            if let Some(w) = known.width {
                let lines = if w >= single_line { 1.0 } else { (single_line / w).ceil() };
                return Size { width: w, height: known.height.unwrap_or(lines * line_h) };
            }
            let avail_w = match avail.width {
                AvailableSpace::Definite(w) => w,
                AvailableSpace::MaxContent => f32::INFINITY,
                AvailableSpace::MinContent => longest_word,
            };
            if !avail_w.is_finite() || avail_w >= single_line {
                return Size { width: single_line, height: line_h };
            }
            let w = avail_w.max(longest_word);
            let lines = (single_line / w).ceil();
            Size { width: w, height: lines * line_h }
        })
    }

    /// Regression for the native-only, web-clean overflow bug: an
    /// un-stretched flex ROW (circle + long text) inside a bounded card.
    /// Web bounds the row to the card and wraps the text; native (Taffy)
    /// used to size the row to single-line max-content (376px) and run
    /// the text off the right edge of the 300px card.
    ///
    /// The default `max-width: 100%` clamp makes the row stay within the
    /// card and the text wrap — without the author writing `width: 100%`
    /// + `flex: 1` + `min-width: 0` by hand.
    #[test]
    fn regression_unstretched_flex_row_wraps_text_to_parent_width() {
        let mut t = LayoutTree::new();
        let card = t.new_node(); // 300-wide viewport

        // Pill: column whose `align-items: flex-start` removes the
        // cross-axis stretch — the exact shape that triggered the bug.
        let pill = t.new_node();
        let mut pr = StyleRules::default();
        pr.flex_direction = Some(FwFlexDirection::Column);
        pr.align_items = Some(runtime_core::AlignItems::FlexStart);
        t.set_style(pill, &pr);

        let row = t.new_node();
        let mut rr = StyleRules::default();
        rr.flex_direction = Some(FwFlexDirection::Row);
        rr.align_items = Some(runtime_core::AlignItems::Center);
        t.set_style(row, &rr);

        let circle = t.new_node();
        let mut cr = StyleRules::default();
        cr.width = Some(px(26.0));
        cr.height = Some(px(26.0));
        t.set_style(circle, &cr);

        let label = t.new_node();
        t.set_measure_fn(label, text_measure(350.0, 80.0, 20.0));

        t.add_child(card, pill);
        t.add_child(pill, row);
        t.add_child(row, circle);
        t.add_child(row, label);
        t.compute(card, 300.0, 800.0);

        let row_f = t.frame_of(row);
        let label_f = t.frame_of(label);
        assert!(
            row_f.width <= 300.5,
            "row must stay within the 300px card (was 376 single-line overflow), got {}",
            row_f.width
        );
        assert!(
            label_f.x + label_f.width <= 300.5,
            "label must not run past the card's right edge, ends at {}",
            label_f.x + label_f.width
        );
        assert!(
            label_f.height >= 39.0,
            "label must have wrapped to 2 lines (~40px), got height {}",
            label_f.height
        );
    }

    /// Exemption: a node with an explicit `width` larger than its parent
    /// keeps that width (the clamp would otherwise shrink a 600px
    /// carousel track to its 300px parent). The author owns the size.
    #[test]
    fn max_width_default_exempts_explicit_wide_width() {
        let mut t = LayoutTree::new();
        let root = t.new_node();
        let wide = t.new_node();
        let mut wr = StyleRules::default();
        wr.width = Some(px(600.0));
        wr.height = Some(px(40.0));
        t.set_style(wide, &wr);
        t.add_child(root, wide);
        t.compute(root, 300.0, 800.0);
        assert!(
            (t.frame_of(wide).width - 600.0).abs() < 0.5,
            "explicit width:600 must NOT be clamped to the 300px parent, got {}",
            t.frame_of(wide).width
        );
    }

    /// Exemption: horizontal-scroll content may exceed its viewport so
    /// it can scroll. A `set_overflow_scroll(.., horizontal: true)`
    /// node's children opt out of the clamp.
    #[test]
    fn max_width_default_exempts_horizontal_scroll_content() {
        let mut t = LayoutTree::new();
        let root = t.new_node();

        let scroller = t.new_node();
        t.set_overflow_scroll(scroller, true); // horizontal viewport
        // Don't stretch the content to the viewport, so its own
        // content-based width is what's under test.
        let mut sr = StyleRules::default();
        sr.align_items = Some(runtime_core::AlignItems::FlexStart);
        t.set_style(scroller, &sr);

        // Content track: a row of two 200px items → wants 400px, wider
        // than the 300px viewport.
        let track = t.new_node();
        let mut tr = StyleRules::default();
        tr.flex_direction = Some(FwFlexDirection::Row);
        t.set_style(track, &tr);
        for _ in 0..2 {
            let item = t.new_node();
            let mut ir = StyleRules::default();
            ir.width = Some(px(200.0));
            ir.height = Some(px(50.0));
            t.set_style(item, &ir);
            t.add_child(track, item);
        }

        t.add_child(root, scroller);
        t.add_child(scroller, track); // exempts `track` from the clamp
        t.compute(root, 300.0, 800.0);

        assert!(
            (t.frame_of(track).width - 400.0).abs() < 0.5,
            "horizontal-scroll content must keep its 400px content width \
             (not clamp to the 300px viewport), got {}",
            t.frame_of(track).width
        );
    }

    /// The author-set `max_width` still wins over the default clamp:
    /// setting `max_width: 500` on a node inside a 300px parent must
    /// produce a 500px cap, not the 300px default — proving the default
    /// is released (not merely widened) when the author opts in.
    #[test]
    fn author_max_width_overrides_default_clamp() {
        let mut t = LayoutTree::new();
        let root = t.new_node();
        let child = t.new_node();
        let mut cr = StyleRules::default();
        cr.max_width = Some(px(500.0));
        cr.width = Some(px(800.0)); // wants 800, capped by max_width 500
        t.set_style(child, &cr);
        t.add_child(root, child);
        t.compute(root, 300.0, 800.0);
        assert!(
            (t.frame_of(child).width - 500.0).abs() < 0.5,
            "author max_width:500 must win over the 100% default clamp, got {}",
            t.frame_of(child).width
        );
    }

    /// Regression: a single-axis scroller (e.g. the codeblock external) must
    /// report ~0 for its MIN-content size — it scrolls, so it can shrink to
    /// nothing. Reporting its full CONTENT width for the MinContent query floors
    /// every flex ancestor at that width (Taffy's flex `min-width:auto`), so a
    /// navigator outlet couldn't shrink to its allotment and the page overflowed
    /// the window — the macOS "screen renders too wide until you resize" bug
    /// (only pages WITH a wide codeblock hit it; resize re-queried with a
    /// Definite width and it fit). Pins the layout principle behind the backends'
    /// `install_external_content_measure` fix (the ObjC `cellSizeForBounds`
    /// measure itself needs a live cell, so can't be unit-tested directly).
    #[test]
    fn scroller_min_content_zero_lets_flex_outlet_shrink() {
        use std::rc::Rc;
        // Content wider than the outlet's flex allotment (1024 - 252 = 772).
        const CONTENT_W: f32 = 900.0;

        // Build Row[ sidebar(fixed 252, no shrink), outlet(grow 1, basis 0) ]
        // → scroller(measure fn). `min_content_w` is the value under test.
        let outlet_width = |min_content_w: f32| -> f32 {
            let mut t = LayoutTree::new();
            let root = t.new_node();
            let mut rr = StyleRules::default();
            rr.flex_direction = Some(FwFlexDirection::Row);
            t.set_style(root, &rr);

            let sidebar = t.new_node();
            let mut sr = StyleRules::default();
            sr.width = Some(px(252.0));
            sr.flex_shrink = Some(Tokenized::Literal(0.0));
            t.set_style(sidebar, &sr);

            let outlet = t.new_node();
            let mut or = StyleRules::default();
            or.flex_grow = Some(Tokenized::Literal(1.0));
            or.flex_basis = Some(px(0.0));
            or.flex_direction = Some(FwFlexDirection::Column);
            t.set_style(outlet, &or);

            let scroller = t.new_node();
            t.set_measure_fn(
                scroller,
                Rc::new(move |known: Size<Option<f32>>, avail: Size<AvailableSpace>| {
                    let w = known.width.unwrap_or(match avail.width {
                        AvailableSpace::Definite(w) => w,
                        AvailableSpace::MinContent => min_content_w,
                        AvailableSpace::MaxContent => CONTENT_W,
                    });
                    Size { width: w, height: known.height.unwrap_or(40.0) }
                }),
            );

            t.add_child(root, sidebar);
            t.add_child(root, outlet);
            t.add_child(outlet, scroller);
            t.compute(root, 1024.0, 768.0);
            t.frame_of(outlet).width
        };

        // BUG: min-content = content width → outlet floored at 900, overflows
        // its 772 allotment.
        let buggy = outlet_width(CONTENT_W);
        assert!(
            buggy > 772.5,
            "with content-width min-content the outlet can't shrink (overflows), got {buggy}"
        );
        // FIX: min-content 0 → outlet shrinks to its 772 flex allotment.
        let fixed = outlet_width(0.0);
        assert!(
            (fixed - 772.0).abs() < 0.5,
            "with min-content 0 the outlet fits its 772 allotment, got {fixed}"
        );
    }

    /// Regression: `set_animated_max_height` must clamp a node's computed
    /// height (the AnimProp::MaxHeight path that drives idea-ui's Measured
    /// Collapsible), AND that cap must survive a concurrent `set_style`
    /// restyle that doesn't mention `max_height` (the Collapsible's
    /// padding/opacity/border variant swap). Before the macOS fix the
    /// animated max-height never reached Taffy, so a collapsed body kept
    /// its full natural height — "doesn't collapse all the way".
    #[test]
    fn regression_animated_max_height_clamps_and_survives_restyle() {
        // root(Column) → body(overflow:hidden, Column) → content(fixed 200
        // tall). As a flex child the body hugs its 200px content; an
        // animated max-height of 0 must collapse it to 0, and a later
        // restyle (no max_height) must not resurrect the 200px.
        let mut t = LayoutTree::new();

        let root = t.new_node();
        let mut root_rules = StyleRules::default();
        root_rules.flex_direction = Some(FwFlexDirection::Column);
        t.set_style(root, &root_rules);

        let body = t.new_node();
        let mut body_rules = StyleRules::default();
        body_rules.flex_direction = Some(FwFlexDirection::Column);
        body_rules.flex_shrink = Some(Tokenized::Literal(0.0));
        body_rules.overflow = Some(runtime_core::Overflow::Hidden);
        t.set_style(body, &body_rules);
        t.add_child(root, body);

        let content = t.new_node();
        let mut content_rules = StyleRules::default();
        content_rules.height = Some(px(200.0));
        content_rules.flex_shrink = Some(Tokenized::Literal(0.0));
        t.set_style(content, &content_rules);
        t.add_child(body, content);

        // Baseline: body hugs its content (~200) with no cap.
        t.compute(root, 300.0, 800.0);
        let natural = t.frame_of(body).height;
        assert!(
            (natural - 200.0).abs() < 0.5,
            "uncapped body should hug its 200px content, got {natural}"
        );

        // Animate the cap to 0 (closed) — the body must collapse.
        t.set_animated_max_height(body, Some(0.0));
        t.compute(root, 300.0, 800.0);
        let collapsed = t.frame_of(body).height;
        assert!(
            collapsed < 0.5,
            "body capped to max-height 0 must collapse to ~0, got {collapsed}"
        );

        // A concurrent chrome restyle (the variant swap) re-runs `set_style`
        // with NO `max_height` — the animated cap must persist, NOT revert
        // the body to its 200px content height.
        let mut restyle = StyleRules::default();
        restyle.flex_direction = Some(FwFlexDirection::Column);
        restyle.overflow = Some(runtime_core::Overflow::Hidden);
        restyle.padding_top = Some(px(0.0));
        t.set_style(body, &restyle);
        t.compute(root, 300.0, 800.0);
        let after_restyle = t.frame_of(body).height;
        assert!(
            after_restyle < 0.5,
            "animated max-height cap must survive a max_height-less restyle, \
             got {after_restyle}"
        );

        // Animate the cap back open (None clears it) — the body re-expands
        // to its natural content height.
        t.set_animated_max_height(body, None);
        t.compute(root, 300.0, 800.0);
        let reopened = t.frame_of(body).height;
        assert!(
            (reopened - 200.0).abs() < 0.5,
            "clearing the cap must re-expand the body to its 200px content, got {reopened}"
        );
    }
}




