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
    /// Whether this model would put anything on screen. A radius alone
    /// paints nothing — it only shapes a fill, a border or a clip — so it
    /// is deliberately NOT part of this test.
    pub fn paints(&self) -> bool {
        self.background.is_some()
            || self.gradient.is_some()
            || self.border.as_ref().is_some_and(|b| b.any())
    }

    fn has_radius(&self) -> bool {
        self.radius.iter().any(|r| *r > 0.0)
    }
    fn clips(&self) -> bool {
        self.overflow_hidden || self.has_radius()
    }
}

/// Clamp each corner radius to half the shorter side so an "as round as
/// possible" radius doesn't blow past the box. GTK doesn't auto-clamp.
///
/// Runs on every snapshot with the widget's CURRENT size, which is what lets
/// `Length::Full` arrive here as infinity (see `radius_px` in `lib.rs`) and
/// come out as a true pill at whatever size the box happens to be — including
/// across resizes, and including boxes larger than the old `999px` sentinel
/// could cover.
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

/// Paint a [`PaintModel`] into `bounds`, with `content` drawn between the
/// background and the border.
///
/// Shared by every widget that carries a box: [`IdealystView`] (children as
/// content) and [`IdealystLabel`] (its text as content). One implementation is
/// the point — a background painted one way for containers and another for text
/// is exactly the per-platform-looking divergence the framework exists to avoid.
pub(crate) fn paint_box(
    snapshot: &gtk4::Snapshot,
    model: &PaintModel,
    bounds: &graphene::Rect,
    content: impl FnOnce(&gtk4::Snapshot),
) {
    let (w, h) = (bounds.width(), bounds.height());
    let radius = clamp_radius(model.radius, w, h);
    let clips = model.clips();

    if clips {
        snapshot.push_rounded_clip(&rounded_rect(bounds, radius));
    }
    if let Some(bg) = model.background {
        snapshot.append_color(&color::to_gdk(bg), bounds);
    }
    if let Some(g) = &model.gradient {
        // `gradient::append` paints from the snapshot's current origin, so
        // shift to the box when it isn't at (0, 0) — a padded text leaf.
        if bounds.x() != 0.0 || bounds.y() != 0.0 {
            snapshot.save();
            snapshot.translate(&graphene::Point::new(bounds.x(), bounds.y()));
            gradient::append(snapshot, w, h, g);
            snapshot.restore();
        } else {
            gradient::append(snapshot, w, h, g);
        }
    }
    content(snapshot);
    if let Some(b) = &model.border {
        if b.any() {
            let colors = [
                color::to_gdk(b.colors[0]),
                color::to_gdk(b.colors[1]),
                color::to_gdk(b.colors[2]),
                color::to_gdk(b.colors[3]),
            ];
            snapshot.append_border(&rounded_rect(bounds, radius), &b.widths, &colors);
        }
    }
    if clips {
        snapshot.pop();
    }
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
        /// Box paint for a **leaf** child that carries one — a `text`
        /// primitive with a background / border / radius / gradient.
        ///
        /// `GtkLabel` is final in GTK4, so a text leaf cannot subclass its way
        /// to painting its own box, and it has no `IdealystView` of its own to
        /// do it. The parent already paints boxes and already knows this
        /// child's Taffy frame and transform, so it paints the child's box too.
        /// Before this, background / border / radius authored on a `text`
        /// primitive was silently dropped on this backend (idea-ui's Badge
        /// painted no pill at all, leaving white text on the page background).
        pub model: RefCell<PaintModel>,
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
            let paint_children = |s: &gtk4::Snapshot| {
                for child in self.children.borrow().iter() {
                    // A leaf child's own box (a `text` primitive's background /
                    // border / radius) — painted here because the child cannot:
                    // `GtkLabel` is final in GTK4 and a leaf has no
                    // `IdealystView` of its own. Drawn in the CHILD's space via
                    // its stored transform, which is the same transform
                    // `size_allocate` gave it, so the box lands exactly on the
                    // child's allocation.
                    let model = child.model.borrow();
                    if model.paints() {
                        if let Some((cw, ch)) = child.layout_size.get() {
                            if cw > 0 && ch > 0 {
                                s.save();
                                if let Some(t) = child.transform.borrow().as_ref() {
                                    s.transform(Some(t));
                                }
                                let b = graphene::Rect::new(0.0, 0.0, cw as f32, ch as f32);
                                // No content: the child's own text is painted by
                                // `snapshot_child` below, in the parent's space.
                                super::paint_box(s, &model, &b, |_| {});
                                s.restore();
                            }
                        }
                    }
                    drop(model);
                    obj.snapshot_child(&child.widget, s);
                }
            };
            if w > 0.0 && h > 0.0 {
                let bounds = graphene::Rect::new(0.0, 0.0, w, h);
                super::paint_box(snapshot, &self.model.borrow(), &bounds, paint_children);
            } else {
                // No own box yet — still paint children so nothing
                // disappears mid-allocation.
                paint_children(snapshot);
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
            model: RefCell::new(PaintModel::default()),
        });
    }

    /// Set (or clear) the box paint for a leaf child. Returns `false` if
    /// `child` isn't ours, so the caller can tell a missed paint from a
    /// no-op — a text leaf whose parent isn't an `IdealystView` would
    /// otherwise silently render unstyled.
    pub fn set_child_model(&self, child: &gtk4::Widget, model: PaintModel) -> bool {
        for c in self.imp().children.borrow().iter() {
            if c.widget == *child {
                if *c.model.borrow() != model {
                    *c.model.borrow_mut() = model;
                    self.queue_draw();
                }
                return true;
            }
        }
        false
    }

    /// A leaf child's recorded Taffy frame size — the box its paint covers.
    /// Introspection needs it to clamp a radius against the same box the
    /// painter does; the widget's own `width()`/`height()` are the CONTENT
    /// area (padding lives in its margins), which is smaller.
    pub fn child_layout_size(&self, child: &gtk4::Widget) -> Option<(i32, i32)> {
        self.imp()
            .children
            .borrow()
            .iter()
            .find(|c| c.widget == *child)
            .and_then(|c| c.layout_size.get())
    }

    /// The box paint recorded for a leaf child, if any. Test/introspection seam.
    pub fn child_model(&self, child: &gtk4::Widget) -> Option<PaintModel> {
        self.imp()
            .children
            .borrow()
            .iter()
            .find(|c| c.widget == *child)
            .map(|c| c.model.borrow().clone())
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



#[cfg(test)]
mod full_radius_tests {
    use super::clamp_radius;

    /// An unbounded radius (what `Length::Full` lowers to) resolves to a true
    /// pill at paint time, at any size.
    ///
    /// Regression: the pill reached this backend as `Px(999.0)`, so a box whose
    /// shorter side exceeded 1998px painted a literal 999px radius instead of a
    /// pill — the clamp stopped binding. `Full` lowers to infinity, so the
    /// clamp always binds and the corner tracks the box across resizes.
    #[test]
    fn regression_unbounded_radius_is_a_pill_at_any_box_size() {
        let full = [f32::INFINITY; 4];
        assert_eq!(clamp_radius(full, 80.0, 32.0), [16.0; 4]);
        // The size the sentinel could not cover.
        assert_eq!(clamp_radius(full, 3000.0, 4000.0), [1500.0; 4]);
        // A resize keeps it round rather than freezing the old value.
        assert_eq!(clamp_radius(full, 80.0, 200.0), [40.0; 4]);

        // What the sentinel did on that same large box: curved, not a pill.
        assert_eq!(clamp_radius([999.0; 4], 3000.0, 4000.0), [999.0; 4]);
    }

    /// Infinity must not leak into the render node as a non-finite size.
    #[test]
    fn an_unbounded_radius_always_resolves_finite() {
        for (w, h) in [(80.0, 32.0), (0.0, 0.0), (1.0, 4000.0)] {
            for r in clamp_radius([f32::INFINITY; 4], w, h) {
                assert!(r.is_finite(), "radius must be finite for {w}x{h}, got {r}");
            }
        }
    }
}

