//! Native GTK4 backend.
//!
//! Implements [`runtime_core::Backend`] over real GTK4 widgets so a
//! backend-agnostic idealyst app tree renders as native Linux UI. The
//! design goal is idealyst's core thesis: one author tree, native
//! output — not a GPU surface emulating widgets.
//!
//! ## Rendering model
//!
//! - **`view` / `pressable`** → [`view::IdealystView`], a hand-written
//!   `GtkWidget` subclass that paints background / gradient / border /
//!   rounded-clip via GSK render nodes in its `snapshot()`, reports its
//!   own Taffy size from `measure`, and allocates its children at their
//!   computed transforms in `size_allocate` (see [`view`] for why a
//!   `GtkFixed` subclass doesn't work here). Children are positioned
//!   absolutely from the Taffy layout pass (see [`transform`]). This is
//!   what lets the welcome scene's gradients, opacity, and per-frame
//!   scale/translate/color animations render natively and smoothly,
//!   without per-frame CSS reparsing.
//! - **`text`** → real [`gtk4::Label`] styled with Pango attributes
//!   (see [`text`]).
//! - **`button` / `text_input` / `toggle` / `slider` / …** → the
//!   corresponding real GTK widgets (`gtk::Button`, `gtk::Entry`,
//!   `gtk::Switch`, `gtk::Scale`), so they look and behave native.
//!
//! Layout is a Taffy pass ([`runtime_layout`]): `apply_style` pushes
//! flex/size/position into the tree, and [`LinuxBackend::run_layout`]
//! computes frames and writes each node's position + size into GTK. It
//! runs inside the root widget's `size_allocate` (installed as a
//! callback in [`LinuxBackend::finish`]) so the external layout engine
//! stays in step with GTK's own allocation cycle.
//!
//! ## Scope
//!
//! `view`, `text`, layout, styling, and the animation fast-path are
//! fully implemented — enough to run the animated welcome app. Image /
//! icon / virtualizer / navigator / portal / external remain
//! placeholders (they render a labeled placeholder rather than
//! panicking); fleshing those out into fully-styled native widgets is
//! follow-on work for a general-purpose GTK backend, not needed by the
//! welcome scene.
//!
//! ## Threading
//!
//! GTK4 is single-threaded — every method here runs on the GTK main
//! thread (the host mounts + drives the tree there). The scheduler in
//! the companion `host-gtk` crate dispatches all framework callbacks on
//! that same thread, so no cross-thread synchronization is needed.
//!
//! ## Build gating
//!
//! The lib body is gated on `cfg(target_os = "linux")`. On macOS /
//! Windows hosts the crate compiles to an empty rlib so workspace
//! builds don't pull `gtk4` into the dep graph.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::pango::prelude::*;
use gtk4::prelude::*;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::animation::AnimProp;
use runtime_core::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
};
use runtime_core::{Length, Overflow, Tokenized};
use runtime_core::{Action, Backend, Color, ColorScheme, Platform, StyleRules};
use runtime_layout::{LayoutNode, LayoutTree};

mod color;
mod fonts;
mod gradient;
mod handles;
mod text;
mod transform;
mod view;

use transform::NodeTransform;
pub use view::IdealystView;
use view::{BorderPaint, PaintModel};

// =========================================================================
// Node
// =========================================================================

/// Backend handle for a mounted GTK widget. Holds a strong ref to the
/// widget; cloning shares the underlying GObject reference, matching
/// framework `Clone` semantics.
#[derive(Clone)]
pub struct LinuxNode {
    pub(crate) id: u64,
    pub(crate) widget: gtk4::Widget,
}

impl std::fmt::Debug for LinuxNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxNode")
            .field("id", &self.id)
            .field("widget_type", &self.widget.type_().name())
            .finish()
    }
}

/// What framework primitive a node is — drives per-kind styling
/// (paint model for views, Pango attrs for text) + animation dispatch.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    View,
    Pressable,
    Text,
    Other,
}

/// Per-node backend state. Transform is split static/animated so a
/// restyle can't clobber an in-flight animation (see [`transform`]).
struct NodeState {
    widget: gtk4::Widget,
    layout: LayoutNode,
    kind: NodeKind,
    /// Taffy frame `(x, y, w, h)` in parent-local px, filled by `finish`.
    frame: (f32, f32, f32, f32),
    transform: NodeTransform,
    static_opacity: f32,
    anim_opacity: Option<f32>,
    /// Sibling stacking scalar (animated `ZIndex`). Higher paints later.
    z: f32,
    /// Sibling insertion order — tiebreak for equal `z`.
    order: u64,
    /// Resolved text style (text nodes only).
    text: text::TextPaint,
}

// =========================================================================
// Style → paint-model helpers (free fns; the unit-testable pieces live
// in the `view` / `gradient` / `color` / `transform` modules).
// =========================================================================

fn len_px(t: &Option<Tokenized<Length>>) -> f32 {
    match t.as_ref().map(|x| x.resolve()) {
        Some(Length::Px(v)) => v,
        _ => 0.0,
    }
}

fn f32_of(t: &Option<Tokenized<f32>>) -> f32 {
    t.as_ref().map(|x| x.resolve()).unwrap_or(0.0)
}

fn color_of(t: &Option<Tokenized<Color>>) -> [f32; 4] {
    t.as_ref()
        .map(|x| color::to_srgb(&x.resolve()))
        .unwrap_or([0.0, 0.0, 0.0, 0.0])
}

fn build_border(s: &StyleRules) -> Option<BorderPaint> {
    let widths = [
        f32_of(&s.border_top_width),
        f32_of(&s.border_right_width),
        f32_of(&s.border_bottom_width),
        f32_of(&s.border_left_width),
    ];
    if widths.iter().all(|w| *w <= 0.0) {
        return None;
    }
    Some(BorderPaint {
        widths,
        colors: [
            color_of(&s.border_top_color),
            color_of(&s.border_right_color),
            color_of(&s.border_bottom_color),
            color_of(&s.border_left_color),
        ],
    })
}

fn build_paint_model(s: &StyleRules) -> PaintModel {
    PaintModel {
        background: s.background.as_ref().map(|c| color::to_srgb(&c.resolve())),
        gradient: s.background_gradient.as_ref().map(gradient::resolve),
        // GSK corner order: top-left, top-right, bottom-right, bottom-left.
        radius: [
            len_px(&s.border_top_left_radius),
            len_px(&s.border_top_right_radius),
            len_px(&s.border_bottom_right_radius),
            len_px(&s.border_bottom_left_radius),
        ],
        border: build_border(s),
        overflow_hidden: matches!(s.overflow, Some(Overflow::Hidden)),
    }
}

// =========================================================================
// Backend
// =========================================================================

pub struct LinuxBackend {
    pub(crate) host_window: gtk4::Window,
    next_id: u64,
    next_order: u64,
    pub(crate) layout: LayoutTree,
    nodes: HashMap<u64, NodeState>,
    /// parent id → child ids in insertion order (for z-index reorder).
    children: HashMap<u64, Vec<u64>>,
    /// child id → parent id (for locating siblings on z change).
    parent_of: HashMap<u64, u64>,
    pub(crate) external_handlers: runtime_core::ExternalRegistry<LinuxBackend>,
    /// Temp font files kept alive for the process (Pango reads lazily).
    _font_files: Vec<PathBuf>,
    /// The framework root node id, captured on first `finish`. Lets the
    /// host re-run the layout pass ([`LinuxBackend::relayout`]) after the
    /// window is actually sized — `mount`'s single `finish` runs while
    /// the window allocation is still 0.
    root_id: Option<u64>,
    /// Weak self-reference, set by the host after wrapping the backend in
    /// `Rc<RefCell<..>>`. Node handles (see [`handles`]) carry a clone so
    /// per-frame animation writes can reach back into the backend.
    self_ref: std::rc::Weak<std::cell::RefCell<LinuxBackend>>,
}

impl LinuxBackend {
    /// Construct a backend rooted at `host_window`. The window must be
    /// realized by the host before widget operations happen.
    pub fn new(host_window: gtk4::Window) -> Self {
        // The framework root becomes the window's child in `finish`;
        // GtkWindow then stretches it to fill the content area (unlike a
        // GtkFixed, which would give it only its natural size).
        Self {
            host_window,
            next_id: 1,
            next_order: 0,
            layout: LayoutTree::new(),
            nodes: HashMap::new(),
            children: HashMap::new(),
            parent_of: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
            _font_files: Vec::new(),
            root_id: None,
            self_ref: std::rc::Weak::new(),
        }
    }

    pub fn host_window(&self) -> &gtk4::Window {
        &self.host_window
    }

    /// Install the backend's weak self-reference. The host calls this
    /// immediately after wrapping the backend in `Rc<RefCell<..>>`, so
    /// node handles built during mount can reach back into it.
    pub fn set_self_ref(&mut self, me: std::rc::Weak<std::cell::RefCell<LinuxBackend>>) {
        self.self_ref = me;
    }

    /// Clone of the weak self-reference, for [`handles`].
    pub(crate) fn self_ref(&self) -> std::rc::Weak<std::cell::RefCell<LinuxBackend>> {
        self.self_ref.clone()
    }

    /// Drive one animation frame: re-allocate + repaint the root. The
    /// host calls this ~60 Hz. Per-frame `set_animated_*` writes update
    /// node state (transform slots, opacity, paint colors) but GTK's
    /// frame clock goes idle on Wayland and `queue_draw` alone doesn't
    /// reliably wake it; explicitly queueing an allocate (re-applies the
    /// animated transforms via the root's `size_allocate` → `run_layout`)
    /// and a draw here keeps the scene advancing. No-op until mounted.
    pub fn pump(&self) {
        if let Some(root) = self.root_id.and_then(|id| self.nodes.get(&id)) {
            root.widget.queue_allocate();
            root.widget.queue_draw();
        }
    }

    /// A node's current Taffy frame `(x, y, w, h)`, for handle `frame()`
    /// reads (welcome's orbit math reads the page's viewport size).
    pub(crate) fn node_frame(&self, id: u64) -> Option<(f32, f32, f32, f32)> {
        self.nodes.get(&id).map(|s| s.frame)
    }

    /// Run the Taffy layout pass against `width` × `height` and write
    /// every node's size + child transform into GTK. Called from the
    /// root widget's `size_allocate` (see [`view::IdealystView`]'s
    /// layout callback, installed in `finish`) — i.e. inside GTK's own
    /// allocation cycle, on first map and every resize. Uses the "quiet"
    /// widget setters (no `queue_resize`/`queue_allocate`, illegal
    /// during allocation) since the results are consumed by the very
    /// allocation pass that invoked us.
    pub fn run_layout(&mut self, width: f32, height: f32) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let Some(root_layout) = self
            .root_id
            .and_then(|id| self.nodes.get(&id))
            .map(|s| s.layout)
        else {
            return;
        };
        self.layout.compute(root_layout, width, height);

        // Write each node's Taffy frame into GTK: pin the size, then
        // recompose the child transform (position ∘ author ∘ animated).
        let ids: Vec<u64> = self.nodes.keys().copied().collect();
        for id in &ids {
            let (frame, widget) = {
                let Some(st) = self.nodes.get(id) else {
                    continue;
                };
                (self.layout.frame_of(st.layout), st.widget.clone())
            };
            if let Some(st) = self.nodes.get_mut(id) {
                st.frame = (frame.x, frame.y, frame.width, frame.height);
            }
            let (w, h) = (frame.width.round() as i32, frame.height.round() as i32);
            // IdealystView containers report their size via `layout_size`
            // (read directly by the parent's `size_allocate`); leaf
            // widgets (Label/Button/…) size via the normal min-size
            // request, which can't propagate past the nearest IdealystView.
            if let Some(v) = widget.downcast_ref::<IdealystView>() {
                v.set_layout_size(w, h);
            } else {
                widget.set_size_request(w, h);
            }
        }
        for id in &ids {
            self.rebuild_transform(*id, true);
        }
    }

    pub fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&Rc<T>, &mut LinuxBackend) -> LinuxNode + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }

    pub fn has_external<T: 'static>(&self) -> bool {
        self.external_handlers.has::<T>()
    }

    pub fn register_external_view(&mut self, widget: gtk4::Widget) -> LinuxNode {
        self.wrap(widget, NodeKind::Other)
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn wrap(&mut self, widget: gtk4::Widget, kind: NodeKind) -> LinuxNode {
        let id = self.alloc_id();
        let layout = self.layout.new_node();
        self.nodes.insert(
            id,
            NodeState {
                widget: widget.clone(),
                layout,
                kind,
                frame: (0.0, 0.0, 0.0, 0.0),
                transform: NodeTransform::default(),
                static_opacity: 1.0,
                anim_opacity: None,
                z: 0.0,
                order: 0,
                text: text::TextPaint::default(),
            },
        );
        LinuxNode { id, widget }
    }

    fn placeholder(&mut self, message: &str) -> LinuxNode {
        let label = gtk4::Label::new(Some(message));
        label.add_css_class("idealyst-placeholder");
        self.wrap(label.upcast::<gtk4::Widget>(), NodeKind::Other)
    }

    /// Recompose a node's `GskTransform` (layout position ∘ static ∘
    /// animated) and write it onto the parent. `quiet` avoids queueing a
    /// re-allocation — set it when called from within the layout pass
    /// (already inside `size_allocate`); leave it off for animation
    /// writes, which must queue a fresh allocation.
    fn rebuild_transform(&self, id: u64, quiet: bool) {
        let Some(st) = self.nodes.get(&id) else {
            return;
        };
        let (x, y, w, h) = st.frame;
        let xf = transform::build_child_transform(&st.transform, (x, y), (w, h));
        let widget = st.widget.clone();
        // The root's parent is the window (no transform slot — it fills
        // the window); every other node's parent is an IdealystView, or
        // a ScrolledWindow's inner Fixed.
        if let Some(parent) = widget.parent() {
            if let Some(iv) = parent.downcast_ref::<IdealystView>() {
                if quiet {
                    iv.set_child_transform_quiet(&widget, Some(xf));
                } else {
                    iv.set_child_transform(&widget, Some(xf));
                }
            } else if let Some(fixed) = parent.downcast_ref::<gtk4::Fixed>() {
                fixed.set_child_transform(&widget, Some(&xf));
            }
        }
    }

    /// Re-link a parent's children in `(z, insertion-order)` order so
    /// GTK paints them in the animated z-index order (planet fly-over).
    fn reorder_siblings(&self, parent_id: u64) {
        let Some(ids) = self.children.get(&parent_id) else {
            return;
        };
        let mut ordered: Vec<u64> = ids.clone();
        ordered.sort_by(|a, b| {
            let (za, oa) = self.nodes.get(a).map(|s| (s.z, s.order)).unwrap_or((0.0, 0));
            let (zb, ob) = self.nodes.get(b).map(|s| (s.z, s.order)).unwrap_or((0.0, 0));
            za.partial_cmp(&zb)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(oa.cmp(&ob))
        });
        let Some(parent_w) = self.nodes.get(&parent_id).map(|s| s.widget.clone()) else {
            return;
        };
        if let Some(iv) = parent_w.downcast_ref::<IdealystView>() {
            let order: Vec<gtk4::Widget> = ordered
                .iter()
                .filter_map(|id| self.nodes.get(id).map(|s| s.widget.clone()))
                .collect();
            iv.reorder_children(&order);
        }
    }

    /// The Pango font map GTK resolves text through (for registering
    /// embedded `face!` fonts). Read from the host window's context.
    fn font_map(&self) -> Option<gtk4::pango::FontMap> {
        self.host_window.pango_context().font_map()
    }
}

// =========================================================================
// Backend trait
// =========================================================================

impl Backend for LinuxBackend {
    type Node = LinuxNode;

    fn color_scheme(&self) -> ColorScheme {
        ColorScheme::Auto
    }

    fn platform(&self) -> Platform {
        Platform::Custom("linux")
    }

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        let widget = IdealystView::new();
        self.wrap(widget.upcast::<gtk4::Widget>(), NodeKind::View)
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        let label = gtk4::Label::new(Some(content));
        label.set_wrap(true);
        label.set_xalign(0.0);
        let node = self.wrap(label.clone().upcast::<gtk4::Widget>(), NodeKind::Text);
        // Give Taffy the label's intrinsic size so flex layout can size
        // + center text. The closure reads the label live at compute
        // time, so it reflects the font `apply_style` sets afterward.
        if let Some(layout) = self.nodes.get(&node.id).map(|s| s.layout) {
            let label_for_measure = label.clone();
            self.layout.set_measure_fn(
                layout,
                Rc::new(move |known, available| {
                    text::measure(&label_for_measure, known, available)
                }),
            );
        }
        node
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&runtime_core::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_core::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let button = gtk4::Button::with_label(label);
        let fire = on_click.fire.clone();
        button.connect_clicked(move |_| (fire)());
        self.wrap(button.upcast::<gtk4::Widget>(), NodeKind::Other)
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // A styled container (IdealystView) with a click gesture — the
        // framework's Pressable is "a View that fires a callback".
        let widget = IdealystView::new();
        let gesture = gtk4::GestureClick::new();
        let fire = on_click.clone();
        gesture.connect_released(move |_, _, _, _| (fire)());
        widget.add_controller(gesture);
        self.wrap(widget.upcast::<gtk4::Widget>(), NodeKind::Pressable)
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let (Some(parent_layout), Some(child_layout)) = (
            self.nodes.get(&parent.id).map(|s| s.layout),
            self.nodes.get(&child.id).map(|s| s.layout),
        ) else {
            return;
        };
        self.layout.add_child(parent_layout, child_layout);

        // Track sibling order + parent linkage for z-index reordering.
        let order = self.next_order;
        self.next_order += 1;
        if let Some(cs) = self.nodes.get_mut(&child.id) {
            cs.order = order;
        }
        self.children.entry(parent.id).or_default().push(child.id);
        self.parent_of.insert(child.id, parent.id);

        // GTK attach. `finish`/`relayout` writes the real transform.
        // Most parents are IdealystView; ScrolledWindow routes to its
        // inner Fixed document.
        if let Some(iv) = parent.widget.downcast_ref::<IdealystView>() {
            iv.add_child(&child.widget);
        } else if let Some(scrolled) = parent.widget.downcast_ref::<gtk4::ScrolledWindow>() {
            if let Some(inner) = scrolled
                .child()
                .and_then(|c| c.downcast::<gtk4::Fixed>().ok())
            {
                inner.put(&child.widget, 0.0, 0.0);
            }
        }
    }

    fn clear_children(&mut self, node: &Self::Node) {
        if let Some(iv) = node.widget.downcast_ref::<IdealystView>() {
            iv.remove_all_children();
        } else if let Some(scrolled) = node.widget.downcast_ref::<gtk4::ScrolledWindow>() {
            if let Some(inner) = scrolled
                .child()
                .and_then(|c| c.downcast::<gtk4::Fixed>().ok())
            {
                let mut child = inner.first_child();
                while let Some(c) = child {
                    let next = c.next_sibling();
                    inner.remove(&c);
                    child = next;
                }
            }
        }
        // Drop tracked children so stale ids don't linger in reorder.
        if let Some(ids) = self.children.remove(&node.id) {
            for id in ids {
                self.parent_of.remove(&id);
            }
        }
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        if let Some(label) = node.widget.downcast_ref::<gtk4::Label>() {
            label.set_text(content);
        }
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        if let Some(btn) = node.widget.downcast_ref::<gtk4::Button>() {
            btn.set_label(label);
        }
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let id = node.id;
        let Some((layout, kind)) = self.nodes.get(&id).map(|s| (s.layout, s.kind)) else {
            return;
        };

        // 1. Layout — push flex/size/position/padding/margin into Taffy.
        self.layout.set_style(layout, style);

        // 2. Opacity (all kinds) + static transform (all kinds).
        if let Some(st) = self.nodes.get_mut(&id) {
            st.static_opacity = style.opacity.as_ref().map(|t| t.resolve()).unwrap_or(1.0);
            let eff = st.anim_opacity.unwrap_or(st.static_opacity);
            st.widget.set_opacity(eff as f64);
            st.transform.statik = style
                .transform
                .as_ref()
                .map(|v| transform::fold_static(v))
                .unwrap_or_default();
        }
        self.rebuild_transform(id, false);

        // 3. Per-kind visuals.
        match kind {
            NodeKind::View | NodeKind::Pressable => {
                let pm = build_paint_model(style);
                if let Some(st) = self.nodes.get(&id) {
                    if let Some(v) = st.widget.downcast_ref::<IdealystView>() {
                        v.set_model(pm);
                    }
                }
            }
            NodeKind::Text => {
                if let Some(st) = self.nodes.get_mut(&id) {
                    st.text = text::resolve(style, &st.text);
                    if let Some(label) = st.widget.downcast_ref::<gtk4::Label>() {
                        text::apply(label, &st.text);
                    }
                }
            }
            NodeKind::Other => {}
        }
    }

    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        let id = node.id;
        let mut reorder_parent: Option<u64> = None;
        if let Some(st) = self.nodes.get_mut(&id) {
            match prop {
                AnimProp::Opacity => {
                    st.anim_opacity = Some(value);
                    st.widget.set_opacity(value as f64);
                    return;
                }
                AnimProp::TranslateX => st.transform.animated.translate_x = value,
                AnimProp::TranslateY => st.transform.animated.translate_y = value,
                AnimProp::Scale => st.transform.animated.scale = value,
                AnimProp::ScaleX => st.transform.animated.scale_x = value,
                AnimProp::ScaleY => st.transform.animated.scale_y = value,
                AnimProp::RotateZ => st.transform.animated.rotate_deg = value,
                AnimProp::ZIndex => {
                    if st.z != value {
                        st.z = value;
                        reorder_parent = self.parent_of.get(&id).copied();
                    }
                }
                // MaxHeight would drive a Taffy reflow; welcome doesn't
                // use it, so it's a no-op here rather than a wrong write.
                _ => return,
            }
        }
        if let Some(pid) = reorder_parent {
            self.reorder_siblings(pid);
        } else {
            self.rebuild_transform(id, false);
        }
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        let id = node.id;
        let Some(st) = self.nodes.get_mut(&id) else {
            return;
        };
        match prop {
            AnimProp::BackgroundColor => {
                if let Some(v) = st.widget.downcast_ref::<IdealystView>() {
                    v.update_model(|m| m.background = Some(value));
                }
            }
            AnimProp::ForegroundColor => {
                st.text.color = value;
                if let Some(label) = st.widget.downcast_ref::<gtk4::Label>() {
                    text::apply(label, &st.text);
                }
            }
            AnimProp::GradientStopColor(idx) => {
                if let Some(v) = st.widget.downcast_ref::<IdealystView>() {
                    v.update_model(|m| {
                        if let Some(g) = &mut m.gradient {
                            if let Some(stop) = g.stops.get_mut(idx as usize) {
                                stop.1 = value;
                            }
                        }
                    });
                }
            }
            _ => {}
        }
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        handles::make_view_handle(self, node)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        handles::make_text_handle(self, node)
    }

    fn finish(&mut self, root: Self::Node) {
        self.root_id = Some(root.id);

        // First mount: make the framework root the window's child so
        // GtkWindow stretches it to fill the content area, and install
        // the layout callback so the Taffy pass runs inside the root's
        // `size_allocate` (GTK's own layout cycle — fires on first map +
        // every resize). This is what keeps the external layout engine in
        // step with GTK allocation without fighting the frame clock.
        if root.widget.parent().is_none() {
            self.host_window.set_child(Some(&root.widget));
            if let Some(iv) = root.widget.downcast_ref::<IdealystView>() {
                let me = self.self_ref();
                iv.set_layout_callback(Rc::new(move |w, h| {
                    if let Some(b) = me.upgrade() {
                        if let Ok(mut b) = b.try_borrow_mut() {
                            b.run_layout(w as f32, h as f32);
                        }
                    }
                }));
            }
        }
    }

    // ---------------------------------------------------------------------
    // Fonts / assets.
    // ---------------------------------------------------------------------

    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        if kind != AssetTag::Font {
            return;
        }
        let Some((bytes, ext)) = fonts::embedded_bytes(source) else {
            return;
        };
        let Some(font_map) = self.font_map() else {
            eprintln!("[backend-linux] no Pango font map; font {id:?} not registered");
            return;
        };
        match fonts::add_font(&font_map, id.0, bytes, ext) {
            Ok(path) => self._font_files.push(path),
            Err(e) => eprintln!("[backend-linux] failed to register font {id:?}: {e}"),
        }
    }

    fn register_typeface(
        &mut self,
        _id: TypefaceId,
        _family_name: &str,
        _faces: &[TypefaceFace],
        _fallback: SystemFallback,
    ) {
        // No-op: the faces are already added to the Pango font map by
        // `register_asset`, and Pango resolves by the family name baked
        // into each TTF (e.g. "Inter"), which matches the author's
        // `font_family`. Nothing more to record here.
    }

    // ---------------------------------------------------------------------
    // Placeholders / native leaf widgets not yet fully styled. See the
    // crate-level "Scope" note — these render rather than panic.
    // ---------------------------------------------------------------------

    fn create_image(
        &mut self,
        _src: &str,
        _alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Image not yet implemented on Linux backend")
    }

    fn create_icon(
        &mut self,
        _data: &runtime_core::primitives::icon::IconData,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Icon not yet implemented on Linux backend")
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _on_blur: Option<runtime_core::primitives::text_input::BlurHandler>,
        secure: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let entry = gtk4::Entry::new();
        entry.set_text(initial_value);
        if secure {
            entry.set_visibility(false);
        }
        self.wrap(entry.upcast::<gtk4::Widget>(), NodeKind::Other)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        if let Some(entry) = node.widget.downcast_ref::<gtk4::Entry>() {
            entry.set_visibility(!secure);
        }
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _wrap: bool,
        _min_rows: Option<u32>,
        _max_rows: Option<u32>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let view = gtk4::TextView::new();
        view.buffer().set_text(initial_value);
        let scrolled = gtk4::ScrolledWindow::new();
        scrolled.set_child(Some(&view));
        self.wrap(scrolled.upcast::<gtk4::Widget>(), NodeKind::Other)
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let switch = gtk4::Switch::new();
        switch.set_active(initial_value);
        let fire = on_change.clone();
        switch.connect_state_notify(move |s| (fire)(s.is_active()));
        self.wrap(switch.upcast::<gtk4::Widget>(), NodeKind::Other)
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let scale =
            gtk4::Scale::with_range(gtk4::Orientation::Horizontal, min as f64, max as f64, 1.0);
        scale.set_value(initial_value as f64);
        let fire = on_change.clone();
        scale.connect_value_changed(move |s| (fire)(s.value() as f32));
        self.wrap(scale.upcast::<gtk4::Widget>(), NodeKind::Other)
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let scrolled = gtk4::ScrolledWindow::new();
        if horizontal {
            scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Never);
        } else {
            scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        }
        let inner = gtk4::Fixed::new();
        scrolled.set_child(Some(&inner));

        if let Some(cb) = on_scroll {
            let cb_for_h = cb.clone();
            let scrolled_for_h = scrolled.clone();
            scrolled.hadjustment().connect_value_changed(move |adj| {
                let x = adj.value() as f32;
                let y = scrolled_for_h.vadjustment().value() as f32;
                cb_for_h(x, y);
            });
            let scrolled_for_v = scrolled.clone();
            scrolled.vadjustment().connect_value_changed(move |adj| {
                let x = scrolled_for_v.hadjustment().value() as f32;
                let y = adj.value() as f32;
                cb(x, y);
            });
        }

        self.wrap(scrolled.upcast::<gtk4::Widget>(), NodeKind::Other)
    }

    fn create_activity_indicator(
        &mut self,
        _size: runtime_core::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let spinner = gtk4::Spinner::new();
        spinner.start();
        self.wrap(spinner.upcast::<gtk4::Widget>(), NodeKind::Other)
    }

    fn create_virtualizer(
        &mut self,
        _callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        _layout: runtime_core::VirtualLayout,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Virtualizer not yet implemented on Linux backend")
    }

    fn create_graphics(
        &mut self,
        _on_ready: runtime_core::primitives::graphics::OnReady,
        _on_resize: runtime_core::primitives::graphics::OnResize,
        _on_lost: runtime_core::primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Graphics not yet implemented on Linux backend")
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        if let Some(handler) = self.external_handlers.get(type_id) {
            return handler(payload, self);
        }
        self.placeholder(&format!(
            "External \"{type_name}\" not registered on Linux backend"
        ))
    }

    fn create_portal(
        &mut self,
        _target: runtime_core::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Portal not yet implemented on Linux backend")
    }

    fn create_navigator(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _presentation: Rc<dyn std::any::Any>,
        _host: runtime_core::primitives::navigator::NavigatorHost<Self::Node>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder(&format!(
            "Navigator \"{type_name}\" not yet implemented on Linux backend"
        ))
    }
}
