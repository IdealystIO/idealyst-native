//! `IdealystView` — the custom GTK widget behind the framework's
//! `view` / `pressable` container primitives.
//!
//! ## Why a hand-written `GtkWidget` (not a `GtkFixed` subclass)
//!
//! The framework does its own absolute layout (Taffy) and needs each
//! container to report **its own** computed size and paint a custom
//! background / gradient / border. A `GtkFixed` subclass looked
//! tempting (it has child transforms), but GTK routes a `GtkFixed`'s
//! measurement through its `GtkFixedLayout` layout manager, so a
//! `WidgetImpl::measure` override there is silently ignored — every
//! container then collapses to the union of its children (a leaf
//! gradient view with no children measures 0×0 and never paints; a
//! container with an off-frame child inflates and drags the window
//! with it).
//!
//! So `IdealystView` subclasses `GtkWidget` directly and implements the
//! three vfuncs itself:
//! - [`measure`](imp::IdealystView::measure) → the node's own
//!   Taffy-computed size (set via [`set_layout_size`]), independent of
//!   children. This is what keeps the window from growing to fit an
//!   off-frame child and lets a childless gradient view have a real
//!   size.
//! - [`size_allocate`](imp::IdealystView::size_allocate) → allocate each
//!   child its own measured size at its stored `GskTransform` (Taffy
//!   position ∘ author/animated transform; see [`crate::transform`]).
//! - [`snapshot`](imp::IdealystView::snapshot) → paint background /
//!   gradient / border / rounded-clip via GSK, then snapshot the
//!   children on top.
//!
//! Per-frame animation stays cheap: opacity via `Widget::set_opacity`
//! (composites the subtree), transforms by updating a child's stored
//! transform + `queue_allocate`, colors by mutating the [`PaintModel`]
//! + `queue_draw`. No CSS reparse anywhere.

use std::cell::RefCell;

use gtk4::glib;
use gtk4::graphene;
use gtk4::gsk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::color;
use crate::gradient::{self, GradientPaint};

/// Per-side border, GSK order: top, right, bottom, left.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BorderPaint {
    pub widths: [f32; 4],
    pub colors: [[f32; 4]; 4],
}

impl BorderPaint {
    fn any(&self) -> bool {
        self.widths.iter().any(|w| *w > 0.0)
    }
}

/// Everything an [`IdealystView`] paints. Cheap to mutate per-frame.
///
/// `radius` is per-corner in GSK order `[top_left, top_right,
/// bottom_right, bottom_left]`, in px (a `999` "pill" value is clamped
/// against the box in [`clamp_radius`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaintModel {
    pub background: Option<[f32; 4]>,
    pub gradient: Option<GradientPaint>,
    pub radius: [f32; 4],
    pub border: Option<BorderPaint>,
    /// `overflow: hidden` (or any non-zero radius) — clip children to
    /// the rounded box.
    pub overflow_hidden: bool,
}

impl PaintModel {
    fn has_radius(&self) -> bool {
        self.radius.iter().any(|r| *r > 0.0)
    }
    fn clips(&self) -> bool {
        self.overflow_hidden || self.has_radius()
    }
}

/// Clamp each corner radius to half the shorter side so a CSS-style
/// "max radius" (`999px`, used by the sun-glare disc for a perfect
/// circle) doesn't blow past the box. GTK doesn't auto-clamp.
pub fn clamp_radius(radius: [f32; 4], w: f32, h: f32) -> [f32; 4] {
    let max = (w.min(h) / 2.0).max(0.0);
    [
        radius[0].clamp(0.0, max),
        radius[1].clamp(0.0, max),
        radius[2].clamp(0.0, max),
        radius[3].clamp(0.0, max),
    ]
}

fn rounded_rect(bounds: &graphene::Rect, r: [f32; 4]) -> gsk::RoundedRect {
    gsk::RoundedRect::new(
        *bounds,
        graphene::Size::new(r[0], r[0]),
        graphene::Size::new(r[1], r[1]),
        graphene::Size::new(r[2], r[2]),
        graphene::Size::new(r[3], r[3]),
    )
}

mod imp {
    use super::*;

    /// One child: the widget + its current `GskTransform` (position ∘
    /// author ∘ animated), applied at `size_allocate`.
    pub struct Child {
        pub widget: gtk4::Widget,
        pub transform: RefCell<Option<gsk::Transform>>,
        /// This child's Taffy frame size, pushed by the layout pass for
        /// **leaf** children (Label/Button/Picture/…). `None` until the
        /// layout pass frames it, or for a widget Taffy never frames.
        ///
        /// Kept here rather than as a `set_size_request` on the child
        /// because a size request is a *minimum*: it becomes a floor the
        /// widget can never measure below, so a text block that needs
        /// fewer lines after the window widens would stay stuck at its
        /// old taller height. `IdealystView` children don't need this —
        /// they carry their own [`IdealystView::layout_size`].
        pub layout_size: std::cell::Cell<Option<(i32, i32)>>,
    }

    #[derive(Default)]
    pub struct IdealystView {
        pub model: RefCell<PaintModel>,
        /// Own Taffy-computed `(w, h)`, reported from `measure`.
        pub layout_size: std::cell::Cell<(i32, i32)>,
        /// Children in paint order (also the z-order — reordered by the
        /// backend on animated `ZIndex`).
        pub children: RefCell<Vec<Child>>,
        /// Set only on the framework root: run the Taffy layout pass
        /// against the size GTK is allocating us, so every node's size +
        /// child transform is set before we allocate our children. This
        /// synchronizes the external layout engine with GTK's own
        /// allocation cycle (the reliable place to do it — GTK drives
        /// `size_allocate` on map + every resize).
        pub layout_cb: RefCell<Option<std::rc::Rc<dyn Fn(i32, i32)>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IdealystView {
        const NAME: &'static str = "IdealystView";
        type Type = super::IdealystView;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for IdealystView {
        fn dispose(&self) {
            // A GtkWidget must unparent its children before disposal or
            // GTK logs a critical.
            for child in self.children.borrow_mut().drain(..) {
                child.widget.unparent();
            }
        }
    }

    impl WidgetImpl for IdealystView {
        fn measure(&self, orientation: gtk4::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let (w, h) = self.layout_size.get();
            let size = match orientation {
                gtk4::Orientation::Horizontal => w,
                _ => h,
            }
            .max(0);
            // (min, natural, min_baseline, natural_baseline). Natural is
            // the node's own Taffy size (what a parent uses to allocate
            // this child) — never the union of children, which would
            // grow the window.
            //
            // Minimum depends on whether this is the framework ROOT (the
            // only node given a `layout_cb`, in `LinuxBackend::finish`):
            //
            // - **Root → 0.** Reporting the Taffy size as the minimum
            //   makes it the WINDOW's minimum, freezing the window at its
            //   current content size so it can't be resized smaller. The
            //   window fills the root to its content area regardless.
            // - **Everything else → the Taffy size.** `GtkFixedLayout`
            //   (the `GtkFixed` document inside a `scroll_view`, reached
            //   via `crate::scroll_document`) allocates each child at its
            //   MINIMUM, not its natural size. With a 0 minimum the whole
            //   scrollable page was allocated 0×0 and painted nothing —
            //   its `snapshot` skips children when its own box is empty.
            //   Parents we implement allocate from `layout_size` directly
            //   (see `size_allocate`), so a non-zero min costs them
            //   nothing; it only teaches GTK-managed parents the truth.
            let min = if self.layout_cb.borrow().is_some() {
                0
            } else {
                size
            };
            (min, size, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            // Root only: run the Taffy pass against GTK's allocation
            // first, so every node's `layout_size` + child transform is
            // current before we allocate. (Must not hold the children
            // borrow across this — the callback mutates child transforms.)
            let cb = self.layout_cb.borrow().clone();
            if let Some(cb) = cb {
                cb(width, height);
            }
            // Allocate each child its Taffy frame size at its transform.
            // For IdealystView children read `layout_size` DIRECTLY — not
            // via `measure()`, whose result GTK caches and only refreshes
            // on `queue_resize`; the layout pass sets sizes quietly (it
            // runs inside this very `size_allocate`), so a cached measure
            // would hand back the pre-resize size and freeze that subtree.
            // Leaf widgets (Label/Button/…) take the Taffy frame the
            // layout pass recorded in `Child::layout_size`, falling back
            // to their intrinsic measure only when unframed. Allocating a
            // leaf its *natural* measure instead is what broke the
            // website's text: a wrapping `GtkLabel`'s natural width is
            // the whole paragraph on ONE line, so a label allocated
            // ~3000px wide inside a 1024px parent pushed its text far
            // off-frame and only a fragment stayed visible. Taffy's frame
            // is authoritative — `measure()` is an input to the layout
            // pass (via the measure fn), never its output.
            for child in self.children.borrow().iter() {
                let (w, h) = if let Some(iv) = child.widget.downcast_ref::<super::IdealystView>() {
                    iv.layout_size()
                } else {
                    crate::leaf_alloc_size(child.layout_size.get(), || {
                        let w = child.widget.measure(gtk4::Orientation::Horizontal, -1).1;
                        let h = child.widget.measure(gtk4::Orientation::Vertical, w).1;
                        (w, h)
                    })
                };
                let transform = child.transform.borrow().clone();
                child.widget.allocate(w.max(0), h.max(0), -1, transform);
            }
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let obj = self.obj();
            let w = obj.width() as f32;
            let h = obj.height() as f32;
            if w > 0.0 && h > 0.0 {
                let model = self.model.borrow();
                let bounds = graphene::Rect::new(0.0, 0.0, w, h);
                let radius = clamp_radius(model.radius, w, h);
                let clips = model.clips();

                if clips {
                    snapshot.push_rounded_clip(&rounded_rect(&bounds, radius));
                }
                if let Some(bg) = model.background {
                    snapshot.append_color(&color::to_gdk(bg), &bounds);
                }
                if let Some(g) = &model.gradient {
                    gradient::append(snapshot, w, h, g);
                }
                // Children (in z-order) over the background/gradient.
                for child in self.children.borrow().iter() {
                    obj.snapshot_child(&child.widget, snapshot);
                }
                if let Some(b) = &model.border {
                    if b.any() {
                        let colors = [
                            color::to_gdk(b.colors[0]),
                            color::to_gdk(b.colors[1]),
                            color::to_gdk(b.colors[2]),
                            color::to_gdk(b.colors[3]),
                        ];
                        snapshot.append_border(&rounded_rect(&bounds, radius), &b.widths, &colors);
                    }
                }
                if clips {
                    snapshot.pop();
                }
            } else {
                // No own box yet — still paint children so nothing
                // disappears mid-allocation.
                for child in self.children.borrow().iter() {
                    obj.snapshot_child(&child.widget, snapshot);
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct IdealystView(ObjectSubclass<imp::IdealystView>)
        @extends gtk4::Widget;
}

impl Default for IdealystView {
    fn default() -> Self {
        Self::new()
    }
}

impl IdealystView {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Append a child (framework `insert`). Parents the widget and adds
    /// it to the end of the paint/z order.
    pub fn add_child(&self, child: &impl IsA<gtk4::Widget>) {
        let child = child.clone().upcast::<gtk4::Widget>();
        child.set_parent(self);
        self.imp().children.borrow_mut().push(imp::Child {
            widget: child,
            transform: RefCell::new(None),
            layout_size: std::cell::Cell::new(None),
        });
    }

    /// Record a **leaf** child's Taffy frame size, read back by
    /// [`size_allocate`](imp::IdealystView::size_allocate). Returns
    /// `false` if `child` isn't ours (caller then falls back to
    /// `set_size_request`).
    ///
    /// Quiet by design: the layout pass runs inside the root's
    /// `size_allocate`, which allocates children immediately afterward,
    /// so there's nothing to queue.
    pub fn set_child_layout_size(&self, child: &gtk4::Widget, w: i32, h: i32) -> bool {
        for c in self.imp().children.borrow().iter() {
            if c.widget == *child {
                c.layout_size.set(Some((w, h)));
                return true;
            }
        }
        false
    }

    /// Remove + unparent every child (framework `clear_children`).
    pub fn remove_all_children(&self) {
        for child in self.imp().children.borrow_mut().drain(..) {
            child.widget.unparent();
        }
    }

    /// Set a child's transform (position ∘ author ∘ animated) and
    /// re-allocate so it takes effect. Use during animation (outside an
    /// allocation cycle).
    pub fn set_child_transform(&self, child: &gtk4::Widget, transform: Option<gsk::Transform>) {
        if self.store_child_transform(child, transform) {
            self.queue_allocate();
        }
    }

    /// Store a child's transform WITHOUT queueing a re-allocation.
    /// Used by the root's layout callback, which runs *inside*
    /// `size_allocate` — queueing there is illegal — and whose result
    /// is consumed by the very allocation loop that follows. Returns
    /// whether the child was found.
    pub fn set_child_transform_quiet(
        &self,
        child: &gtk4::Widget,
        transform: Option<gsk::Transform>,
    ) -> bool {
        self.store_child_transform(child, transform)
    }

    fn store_child_transform(&self, child: &gtk4::Widget, transform: Option<gsk::Transform>) -> bool {
        let children = self.imp().children.borrow();
        let Some(entry) = children.iter().find(|c| &c.widget == child) else {
            return false;
        };
        *entry.transform.borrow_mut() = transform;
        true
    }

    /// Install the root layout callback (see `imp::IdealystView::layout_cb`).
    pub fn set_layout_callback(&self, f: std::rc::Rc<dyn Fn(i32, i32)>) {
        *self.imp().layout_cb.borrow_mut() = Some(f);
    }

    /// Reorder children to match `order` (by widget identity) — the
    /// animated z-index restack. Widgets not in `order` keep their
    /// relative position at the end.
    pub fn reorder_children(&self, order: &[gtk4::Widget]) {
        let mut children = self.imp().children.borrow_mut();
        children.sort_by_key(|c| {
            order
                .iter()
                .position(|w| w == &c.widget)
                .unwrap_or(usize::MAX)
        });
        drop(children);
        self.queue_allocate();
        // Snapshot order follows the Vec, so a repaint reflects the
        // new stacking too.
        self.queue_draw();
    }

    /// Replace the whole paint model (from `apply_style`) and repaint.
    pub fn set_model(&self, model: PaintModel) {
        *self.imp().model.borrow_mut() = model;
        self.queue_draw();
    }

    /// Mutate the paint model in place (per-frame animation:
    /// background color, one gradient stop) and repaint.
    pub fn update_model(&self, f: impl FnOnce(&mut PaintModel)) {
        f(&mut self.imp().model.borrow_mut());
        self.queue_draw();
    }

    /// Set this node's own layout size (from the Taffy pass).
    ///
    /// Invalidates GTK's cached `measure()` **only when the size actually
    /// changes**. Parents we implement ([`size_allocate`](imp::
    /// IdealystView::size_allocate)) read [`layout_size`](Self::
    /// layout_size) directly and don't need this, but a GTK-managed
    /// parent does: the `GtkFixed` document inside a `scroll_view`, a
    /// `GtkViewport`, the `GtkOverlay` a portal mounts into. Those
    /// allocate from the cached measure, which nothing but `queue_resize`
    /// clears — so without this the whole scrollable page was allocated
    /// at its stale 0×0 and every descendant collapsed (labels measured
    /// at width 0, paddings producing negative content boxes).
    ///
    /// The change-gate is what makes this safe to call from inside the
    /// root's `size_allocate`: at steady state the sizes are identical,
    /// nothing is queued, and the 60 Hz pump doesn't turn into an endless
    /// relayout loop. Only a genuine size change costs one extra pass.
    pub fn set_layout_size(&self, w: i32, h: i32) {
        if self.imp().layout_size.get() == (w, h) {
            return;
        }
        self.imp().layout_size.set((w, h));
        self.queue_resize();
    }

    /// The resolved background colour this view paints, if any. Used by
    /// the `IDEALYST_GTK_DUMP_LAYOUT` dump to tell "the framework never
    /// set a background" apart from "it set one and we didn't paint it".
    pub fn paint_background(&self) -> Option<[f32; 4]> {
        self.imp().model.borrow().background
    }

    /// The full paint state this view hands GSK on every snapshot.
    ///
    /// Read by `introspect_native` for the parity capture. It is the resolved
    /// state (`apply_style` has already run tokens, breakpoints and state
    /// overlays through it), not raw author input — but it is still the state we
    /// intend to paint rather than an independent engine's read of what WAS
    /// painted, which GTK cannot answer for a custom widget. See
    /// `crate::introspect`.
    pub fn paint_model(&self) -> PaintModel {
        self.imp().model.borrow().clone()
    }

    /// This node's Taffy layout size — the authoritative size a parent
    /// allocates it to (see the note in `size_allocate`).
    pub fn layout_size(&self) -> (i32, i32) {
        self.imp().layout_size.get()
    }
}

