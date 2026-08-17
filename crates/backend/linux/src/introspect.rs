//! Platform-native render introspection for the GTK4 backend — the Linux half
//! of cross-platform render parity.
//!
//! `caps::IntrospectionOps` was an empty impl on `LinuxBackend`, so
//! `introspect_native` returned `None` for every element and a parity capture
//! against this backend came back empty: every element reported as "only on the
//! other platform", which is a flood, not a diff. Linux could not participate
//! in parity testing at all.
//!
//! # The cardinal rule, and exactly how it is honored here
//!
//! `runtime_shared::introspect` requires every value to be read from the LIVE
//! platform object, never echoed from the framework's own style structs —
//! otherwise the parity check is tautological ("the styles we asked for"
//! instead of "the styles the platform applied"). GTK makes that easy for some
//! properties and impossible for others, so this module is explicit about which
//! is which:
//!
//! - **Geometry** — `gtk_widget_compute_bounds(widget, toplevel)`: the rect GTK
//!   actually allocated, in window coordinates, after its own allocation cycle.
//!   This is emphatically NOT the Taffy frame. On this backend the two normally
//!   agree (Taffy drives the allocation), and a disagreement is itself a real
//!   bug worth catching — a widget whose parent overrode the frame, a stale
//!   cached measure, a `size_allocate` that never ran. On web the distinction is
//!   fundamental: the browser is the layout authority and Taffy's numbers are an
//!   input, so only the used geometry is comparable.
//! - **Text** — `GtkLabel::text()`, the string the widget will shape.
//! - **Font + text colour** — `pango::AttrIterator::font()` over the label's
//!   installed `AttrList`, i.e. `pango_attr_iterator_get_font`: PANGO's own
//!   resolution of the attribute stack, which is as close to "the resolved
//!   font" as this toolkit gets (the analogue of macOS reading back an
//!   `NSFont`). Where no attribute applies, the widget's `PangoContext`
//!   supplies the inherited value.
//! - **Opacity / hidden** — `gtk_widget_get_opacity` / `get_visible`: GTK's own
//!   verdict on the widget.
//! - **Background / border / corner radius** — read from
//!   [`IdealystView::paint_model`], the paint state the widget hands GSK on
//!   every snapshot. This is the one read that is not a query of an independent
//!   engine, because GTK has none to query: a custom widget's fill is not a
//!   style property GTK resolves, it is whatever the widget's `snapshot()`
//!   appends. It is still one step removed from author input — `apply_style`
//!   resolves tokens, breakpoints and state overlays before the model is
//!   written — so it catches a mis-resolved token, but it cannot catch a bug
//!   where the model is right and the painting is wrong. A screenshot diff is
//!   the tool for that; this is noted rather than papered over.
//!
//! # Native sub-hierarchy and its boundary
//!
//! `collect_native_tree` walks a primitive's own GTK sub-objects and stops at
//! any descendant that is itself a framework element. The walker announces
//! those through `note_introspection_root`, which tags the widget with the
//! [`ELEMENT_CSS_CLASS`] CSS class.
//!
//! A CSS class rather than a pointer set: the mark then lives and dies WITH the
//! widget. A `HashSet<*const Widget>` would need a removal hook on every
//! teardown path (`clear_children`, `remove_child`, portal release, virtualizer
//! recycling), and a missed one leaves a stale pointer that a freshly-allocated
//! widget can collide with — a boundary that silently appears in the wrong
//! place. The class costs one interned string per element and cannot go stale.

use gtk4::graphene;
use gtk4::pango;
use gtk4::prelude::*;
use runtime_shared::introspect::{
    collect_native_tree, keys, NativeNode, NativeRect, NativeValue,
};

use crate::{LinuxBackend, NodeKind};

/// CSS class marking a widget as a framework element root — the boundary the
/// native-subtree walk stops at. Set by `note_introspection_root`.
pub(crate) const ELEMENT_CSS_CLASS: &str = "idealyst-element";

impl LinuxBackend {
    /// Tag `widget` as a framework element root so the introspection walk
    /// treats it as a boundary. Backs `IntrospectionOps::note_introspection_root`.
    pub(crate) fn note_introspection_root_impl(&self, widget: &gtk4::Widget) {
        widget.add_css_class(ELEMENT_CSS_CLASS);
    }

    /// Read the native render tree for one element. Backs
    /// `IntrospectionOps::introspect_native`.
    ///
    /// `None` only when the id names no live node. A node that EXISTS is always
    /// reported, even at 0x0: dropping it would remove it from the parity
    /// capture, which the diff reads as "this element exists on the other
    /// platform only" — a structural finding, which is a worse lie than an
    /// honest zero rect. A widget GTK has not laid out reports 0x0 with
    /// `hidden` telling the caller which case it is.
    pub(crate) fn introspect_native_impl(&self, id: u64) -> Option<NativeNode> {
        let root = self.node_widget(id)?;
        let toplevel: gtk4::Widget = self.host_window.clone().upcast();
        let role = self.node_role(id);
        Some(collect_native_tree(
            &root,
            &|w: &gtk4::Widget| read_widget(w, &toplevel),
            &child_widgets,
            &|w: &gtk4::Widget| w.has_css_class(ELEMENT_CSS_CLASS),
        ))
        .map(|mut n| {
            // The role hint names which primitive this native object backs; it
            // is framework knowledge, not a platform read, so it is attached to
            // the element root only.
            if let Some(role) = role {
                n.role = Some(role.to_string());
            }
            n
        })
    }

    /// Whether native introspection is available. True once a window exists —
    /// which it always does for a mounted `LinuxBackend`.
    pub(crate) fn supports_native_introspection_impl(&self) -> bool {
        true
    }

    /// The widget backing a framework node id.
    fn node_widget(&self, id: u64) -> Option<gtk4::Widget> {
        self.nodes.get(&id).map(|s| s.widget.clone())
    }

    /// The framework primitive kind for a node, as the `role` hint.
    fn node_role(&self, id: u64) -> Option<&'static str> {
        Some(match self.nodes.get(&id)?.kind {
            NodeKind::View => "view",
            NodeKind::Text => "text",
            NodeKind::Pressable => "pressable",
            NodeKind::Other => "other",
        })
    }
}

/// Direct GTK child widgets, in sibling order.
fn child_widgets(w: &gtk4::Widget) -> Vec<gtk4::Widget> {
    let mut out = Vec::new();
    let mut cur = w.first_child();
    while let Some(c) = cur {
        cur = c.next_sibling();
        out.push(c);
    }
    out
}

/// Shallow read of one widget: GTK class name, allocated frame in window
/// coordinates, and the props the platform can answer for.
fn read_widget(w: &gtk4::Widget, toplevel: &gtk4::Widget) -> NativeNode {
    let frame = w
        .compute_bounds(toplevel)
        .map(|b| NativeRect {
            x: b.x(),
            y: b.y(),
            width: b.width(),
            height: b.height(),
        })
        // `compute_bounds` fails when the widget is not in the same tree as the
        // toplevel (a detached portal container mid-teardown). Fall back to the
        // allocation, which is still the real one, minus the window offset.
        .unwrap_or(NativeRect {
            x: 0.0,
            y: 0.0,
            width: w.width() as f32,
            height: w.height() as f32,
        });

    let mut node = NativeNode::leaf(w.type_().name(), frame);

    // GTK's own verdict on the widget, not our style intent.
    node.set(keys::HIDDEN, Some(NativeValue::Flag(!w.is_visible())));
    let opacity = w.opacity() as f32;
    node.set(keys::OPACITY, Some(NativeValue::Number(opacity)));

    // Painted state from the render tree — see `read_paint_from_gsk`. Applies to
    // any widget, not just our own view subclass: a GtkLabel's fill or a
    // GtkPicture's clip is read the same way.
    read_paint_from_gsk(w, &mut node);
    if let Some(label) = w.downcast_ref::<gtk4::Label>() {
        read_label(label, &mut node);
    }
    node
}

/// Background / border / corner radius read from the widget's **GSK render
/// node** — the tree GTK actually rasterizes.
///
/// This is the independent engine read the module doc's cardinal rule asks for.
/// It replaced a read of [`IdealystView::paint_model`], which was our own
/// *intent*: post-token and post-state-overlay, so it caught a mis-resolved
/// token, but blind to any bug between "what we told GSK" and "what GSK drew".
/// The render node is the other side of that line.
///
/// # How the widget's own paint is told apart from its children's
///
/// A widget's render node subtree includes everything its children paint, so a
/// naive walk would report a child's fill as the parent's background. Two
/// filters keep it honest:
///
/// - only nodes whose bounds match the widget's own size (within a pixel) count
///   — a child's fill is smaller, or offset, or both;
/// - the walk descends only through the wrapper kinds our own
///   [`IdealystView::snapshot`] emits (container / transform / clip / rounded
///   clip / opacity), and takes the FIRST match of each kind, which is the order
///   that snapshot appends them in (background, then gradient, then children,
///   then border).
///
/// Limits worth knowing: a child that exactly fills its parent and paints its
/// own background can be mistaken for the parent's, and a widget GTK has no
/// cached node for reports nothing rather than guessing (the framework root is
/// the one such case in practice — it paints no fill of its own anyway).
fn read_paint_from_gsk(w: &gtk4::Widget, node: &mut NativeNode) {
    let (wf, hf) = (w.width() as f32, w.height() as f32);
    if wf <= 0.0 || hf <= 0.0 {
        return;
    }
    // `snapshot_child` runs on the widget's PARENT by contract.
    let Some(parent) = w.parent() else { return };
    let snapshot = gtk4::Snapshot::new();
    parent.snapshot_child(w, &snapshot);
    let Some(root) = snapshot.to_node() else { return };

    let mut found = GskPaint::default();
    collect_gsk_paint(&root, wf, hf, 0, &mut found);

    node.set(keys::BACKGROUND_COLOR, found.background.map(NativeValue::Color));
    if let Some(r) = found.corner_radius.filter(|r| *r > 0.0) {
        node.set(keys::CORNER_RADIUS, Some(NativeValue::Length(r)));
    }
    if let Some((width, color)) = found.border {
        if width > 0.0 {
            node.set(keys::BORDER_WIDTH, Some(NativeValue::Length(width)));
            node.set(keys::BORDER_COLOR, Some(NativeValue::Color(color)));
        }
    }
}

/// What [`collect_gsk_paint`] harvests from a render-node subtree.
#[derive(Default)]
struct GskPaint {
    background: Option<[f32; 4]>,
    corner_radius: Option<f32>,
    /// Top-side width + colour. Per-side values exist on the node
    /// (`BorderNode::widths()` / `colors()` are `[_; 4]`) but the canonical
    /// schema carries a single `border_width`/`border_color`, so only the top is
    /// reported until the schema grows per-side keys on every backend.
    border: Option<(f32, [f32; 4])>,
}

/// Depth-first harvest of the paint nodes belonging to a widget of `w` x `h`.
fn collect_gsk_paint(node: &gtk4::gsk::RenderNode, w: f32, h: f32, depth: u32, out: &mut GskPaint) {
    use gtk4::gsk;
    // Our own snapshot nests a handful of wrappers deep; a bound keeps a
    // pathological tree from turning one introspect call into a long walk.
    if depth > 8 {
        return;
    }
    let covers = |b: graphene::Rect| {
        (b.width() - w).abs() <= 1.0 && (b.height() - h).abs() <= 1.0
    };

    if let Some(c) = node.downcast_ref::<gsk::ColorNode>() {
        if out.background.is_none() && covers(node.bounds()) {
            out.background = Some(rgba_to_srgb(&c.color()));
        }
        return;
    }
    if let Some(b) = node.downcast_ref::<gsk::BorderNode>() {
        if out.border.is_none() && covers(node.bounds()) {
            let widths = b.widths();
            let colors = b.colors();
            out.border = Some((widths[0], rgba_to_srgb(&colors[0])));
            if out.corner_radius.is_none() {
                out.corner_radius = Some(b.outline().corner()[0].width());
            }
        }
        return;
    }
    if let Some(rc) = node.downcast_ref::<gsk::RoundedClipNode>() {
        if out.corner_radius.is_none() && covers(node.bounds()) {
            out.corner_radius = Some(rc.clip().corner()[0].width());
        }
        collect_gsk_paint(&rc.child(), w, h, depth + 1, out);
        return;
    }
    if let Some(c) = node.downcast_ref::<gsk::ContainerNode>() {
        for i in 0..c.n_children() {
            collect_gsk_paint(&c.child(i), w, h, depth + 1, out);
        }
        return;
    }
    if let Some(c) = node.downcast_ref::<gsk::TransformNode>() {
        collect_gsk_paint(&c.child(), w, h, depth + 1, out);
        return;
    }
    if let Some(c) = node.downcast_ref::<gsk::ClipNode>() {
        collect_gsk_paint(&c.child(), w, h, depth + 1, out);
        return;
    }
    if let Some(c) = node.downcast_ref::<gsk::OpacityNode>() {
        collect_gsk_paint(&c.child(), w, h, depth + 1, out);
    }
}

/// `gdk::RGBA` -> the canonical straight-sRGB `[r, g, b, a]`, channels 0..=1.
fn rgba_to_srgb(c: &gtk4::gdk::RGBA) -> [f32; 4] {
    [c.red(), c.green(), c.blue(), c.alpha()]
}

/// Text plus the font Pango resolves for it.
fn read_label(label: &gtk4::Label, node: &mut NativeNode) {
    node.set(keys::TEXT, Some(NativeValue::Text(label.text().to_string())));

    // `pango_attr_iterator_get_font` resolves the attribute stack at the
    // iterator's position into one `FontDescription` — Pango's own answer for
    // "what font applies here", which is the closest this toolkit gets to a
    // resolved-font read. Position 0 is the run the label starts with; a label
    // with mixed runs (a code panel) reports its first, which is what a
    // single-value canonical key can carry.
    let attrs = match label.attributes() {
        Some(a) => a,
        // No attribute list: the widget inherits its font from the GTK style
        // context. Report that instead, so the key is never silently absent.
        None => {
            let ctx = label.pango_context();
            if let Some(desc) = ctx.font_description() {
                set_font(node, &desc);
            }
            return;
        }
    };
    let mut iter = attrs.iterator();
    let (desc, _lang, _extra) = iter.font();
    set_font(node, &desc);

    // Foreground colour, if the attribute stack sets one. Pango's 16-bit
    // channels → the canonical 0..=1 sRGB the schema uses.
    if let Some(attr) = iter.get(pango::AttrType::Foreground) {
        if let Some(c) = attr.downcast_ref::<pango::AttrColor>() {
            let c = c.color();
            node.set(
                keys::TEXT_COLOR,
                Some(NativeValue::Color([
                    c.red() as f32 / 65535.0,
                    c.green() as f32 / 65535.0,
                    c.blue() as f32 / 65535.0,
                    1.0,
                ])),
            );
        }
    }
}

/// Family / size / weight from a resolved `FontDescription`.
fn set_font(node: &mut NativeNode, desc: &pango::FontDescription) {
    if let Some(family) = desc.family() {
        node.set(
            keys::FONT_FAMILY,
            Some(NativeValue::Text(family.to_string())),
        );
    }
    // Absolute sizes are stored in device units (px × PANGO_SCALE); point sizes
    // in points × PANGO_SCALE. `create_text` sets absolute, so prefer that
    // reading and fall back to points for a font that came from GTK's CSS.
    let size = desc.size() as f32 / pango::SCALE as f32;
    if size > 0.0 {
        node.set(keys::FONT_SIZE, Some(NativeValue::Length(size)));
    }
    node.set(
        keys::FONT_WEIGHT,
        Some(NativeValue::Number(weight_to_css(desc.weight()))),
    );
}

/// Pango's named weights → the CSS numeric scale the canonical schema uses (the
/// same numbers web reports from `getComputedStyle`), so the two are directly
/// comparable. Pango's own values ARE these numbers, but the binding exposes
/// them as a non-exhaustive enum, so they are spelled out.
fn weight_to_css(w: pango::Weight) -> f32 {
    match w {
        pango::Weight::Thin => 100.0,
        pango::Weight::Ultralight => 200.0,
        pango::Weight::Light => 300.0,
        pango::Weight::Semilight => 350.0,
        pango::Weight::Book => 380.0,
        pango::Weight::Normal => 400.0,
        pango::Weight::Medium => 500.0,
        pango::Weight::Semibold => 600.0,
        pango::Weight::Bold => 700.0,
        pango::Weight::Ultrabold => 800.0,
        pango::Weight::Heavy => 900.0,
        pango::Weight::Ultraheavy => 1000.0,
        // Non-exhaustive enum: an unnamed weight is a raw numeric value Pango
        // passed through, and `Normal` is the honest fallback for reporting.
        _ => 400.0,
    }
}
