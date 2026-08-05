//! Native GTK4 backend.
//!
//! Implements [`runtime_shared::Backend`] over real GTK4 widgets so a
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
//! `view`, `text`, `icon`, layout, styling, and the animation fast-path
//! are fully implemented — enough to run the animated welcome app. `icon`
//! strokes/fills real Lucide glyph paths via GSK (see [`icon`]); its
//! stroke draw-on animation is the one gap (renders fully drawn).
//! `image` ([`image`]) renders a `gtk::Picture` with `object_fit` →
//! `content-fit` (local path / `data:` URI / `http(s)` via GIO).
//! `portal` ([`portal`]) mounts a full-viewport flex container into a
//! window-level `gtk::Overlay`; viewport placements are fully placed,
//! anchored placement is a documented gap (no per-frame scheduler on the
//! GTK host). `virtualizer` ([`virtualizer`]) is a windowed
//! `gtk::ScrolledWindow` list — only viewport+overscan cells are realized
//! and recycled; `ItemSize::Measured` degrades to the author estimate
//! (the framework can't measure a detached cell subtree). `navigator` /
//! `external` remain placeholders (they render a labeled placeholder
//! rather than panicking); fleshing those out is follow-on work for a
//! general-purpose GTK backend, not needed by the welcome scene.
//!
//! ## Debugging layout
//!
//! Set `IDEALYST_GTK_DUMP_LAYOUT=1` to have every
//! [`LinuxBackend::run_layout`] pass print one line per node: the Taffy
//! frame, GTK's *actual* allocation, whether the widget is mapped, and
//! its GTK parent. The Taffy frame and the GTK allocation are separate
//! facts, and nearly every rendering bug found so far has been a
//! disagreement between them — a node laid out at 1024x2886 but showing
//! `ALLOC=0x0 map=false par_gtk=None` was never parented; one showing a
//! correct frame with `ALLOC=0x0` was allocated from a stale cached
//! measure or a zero minimum. That single view is what turned "the page
//! is blank" into a specific line of code, twice.
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

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::animation::AnimProp;
use runtime_shared::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
};
use runtime_shared::{Length, Overflow, Tokenized};
use runtime_shared::{Action, Color, ColorScheme, Platform, PointerEvents, StyleRules};
use runtime_layout::{AvailableSpace, LayoutNode, LayoutTree, Size};

mod color;
mod file_drop;
mod fonts;
mod gl_loader;
mod graphics;
mod gradient;
mod handles;
mod image;
mod portal;
mod sticky;
mod touch;
mod virtualizer;
mod icon;
mod text;
mod transform;
mod view;

use transform::NodeTransform;
pub use view::IdealystView;
use view::{BorderPaint, PaintModel};

/// Re-exported so downstream crates (`host-gtk`, integration tests) can
/// name GTK types without pinning their own, necessarily-identical,
/// `gtk4` version — two `gtk4` majors in one binary would be two sets of
/// incompatible GObject wrappers over the same C library.
pub use gtk4;

/// Build the `graphics` primitive's `GtkGLArea` in isolation, for tests
/// that need a live `GlTarget` without standing up a whole render tree.
/// Exercises the real `on_ready` wiring — same function the backend's
/// `create_graphics` calls.
#[doc(hidden)]
pub fn build_gl_area_for_test(
    on_ready: runtime_shared::primitives::graphics::OnReady,
    on_resize: runtime_shared::primitives::graphics::OnResize,
    on_lost: runtime_shared::primitives::graphics::OnLost,
) -> gtk4::Widget {
    graphics::build_gl_area(on_ready, on_resize, on_lost)
}



// Post-dispatch flush-hook slot (new-core flush driver). Unconditional
// — the fire sites live in the out-of-repo host shell, which cannot
// see this crate's features; no-op default so the old core never pays.
pub mod dispatch_hook;

/// `runtime_scene::Host` + the 30 capability traits on [`LinuxBackend`],
/// plus the boot entry and flush driver.
pub mod newcore;

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

impl LinuxNode {
    /// Stable per-node id. SDK `Element::External` leaves that keep an
    /// imperative-ops table (video play/pause/seek, etc.) key it on this.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The wrapped GTK widget. SDK leaves reach their native widget
    /// (the `gtk::Video`, `gtk::Picture`, …) back through this to run
    /// imperative ops or read intrinsic size.
    pub fn widget(&self) -> &gtk4::Widget {
        &self.widget
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

/// Give the app window a deterministic canvas instead of letting the
/// user's desktop GTK theme show through.
///
/// A `GtkWindow` paints the *system theme's* background by default, so
/// on a dark desktop every region the app doesn't paint itself came out
/// dark — under a light-themed app that meant dark text on a dark band
/// (the website's mid-page sections). The same author tree looked
/// different on two Linux machines purely because their GTK themes
/// differed, which is exactly what the framework's one-tree/every-
/// backend thesis rules out.
///
/// White matches the canvas every other backend effectively provides for
/// an app that sets no root background: the browser's default `<body>`
/// on web and SSR. An app wanting a different base sets a background on
/// its own root view, exactly as it must on web — this is only the
/// fallback beneath it.
///
/// Deliberately NOT keyed to [`Backend::color_scheme`]: that reports the
/// *system* preference, apps are free to ignore it (the website hard-
/// codes its light theme), and keying the canvas to the desktop would
/// reintroduce the very "same app, different machine" divergence this
/// removes. Scoped to a CSS class on the window so the provider can't
/// leak into unrelated widgets.
fn install_canvas_background(window: &gtk4::Window) {
    const CANVAS_CLASS: &str = "idealyst-canvas";
    if window.has_css_class(CANVAS_CLASS) {
        return;
    }
    let provider = gtk4::CssProvider::new();
    provider.load_from_data(".idealyst-canvas { background-color: #ffffff; }");
    gtk4::style_context_add_provider_for_display(
        &gtk4::prelude::WidgetExt::display(window),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    window.add_css_class(CANVAS_CLASS);
}

/// The `GtkFixed` document inside a `scroll_view`'s `GtkScrolledWindow`
/// — where its framework children are parented.
///
/// **Not** simply `scrolled.child()`: `GtkScrolledWindow` auto-wraps a
/// child that doesn't implement `GtkScrollable` (a `GtkFixed` doesn't)
/// in a `GtkViewport`, so `child()` hands back the viewport and a direct
/// `downcast::<GtkFixed>()` fails. That failure used to be silent — the
/// `if let` simply didn't fire — so every child inserted into a
/// `scroll_view` was **dropped from the widget tree**: laid out
/// correctly by Taffy, never parented, never allocated (0×0), never
/// mapped. The website's entire page was invisible for exactly this
/// reason. Handles both shapes so it can't regress if GTK stops
/// wrapping (or we later use a scrollable document widget).
fn scroll_document(scrolled: &gtk4::ScrolledWindow) -> Option<gtk4::Fixed> {
    let child = scrolled.child()?;
    if let Ok(fixed) = child.clone().downcast::<gtk4::Fixed>() {
        return Some(fixed);
    }
    child
        .downcast::<gtk4::Viewport>()
        .ok()?
        .child()?
        .downcast::<gtk4::Fixed>()
        .ok()
}

/// Size to allocate a leaf child: the Taffy frame the layout pass
/// recorded for it, else the widget's own intrinsic measure.
///
/// Exists because a leaf's *natural* measure is the wrong answer once
/// Taffy has framed it. A wrapping `GtkLabel` reports its whole paragraph
/// on one line as its natural width, so allocating that ignores the width
/// Taffy assigned and defeats wrapping entirely. `None` means Taffy never
/// framed this widget (e.g. it hangs off a `ScrolledWindow` rather than
/// an [`IdealystView`]) — then its own measure is the best available
/// answer.
pub(crate) fn leaf_alloc_size(
    framed: Option<(i32, i32)>,
    intrinsic: impl FnOnce() -> (i32, i32),
) -> (i32, i32) {
    framed.unwrap_or_else(intrinsic)
}

/// Taffy measure function for an arbitrary wrapped GTK widget — the
/// intrinsic size an `Element::External` leaf reports to layout. Same
/// shape as [`text::measure`] but for any `gtk::Widget` (Picture, Video,
/// Label, …): natural width unconstrained, then height for that width.
fn widget_measure(
    widget: &gtk4::Widget,
    known: Size<Option<f32>>,
    available: Size<AvailableSpace>,
) -> Size<f32> {
    // Margins carry this leaf's padding and GTK reports them as part of
    // the measured size; Taffy wants content and re-adds padding itself.
    // See the note in `text::measure` — same double-count either way.
    let (mx, my) = widget_margins(widget);
    let (wmin, wnat, _, _) = widget.measure(gtk4::Orientation::Horizontal, -1);
    let (wmin, wnat) = ((wmin - mx).max(0), (wnat - mx).max(0));
    let width = known.width.unwrap_or_else(|| match available.width {
        AvailableSpace::Definite(aw) => (wnat as f32).min(aw),
        _ => wnat as f32,
    });
    // Height must be measured at a width the widget can actually take —
    // GTK warns ("Trying to measure GtkLabel for width of 0, but it
    // needs at least N") and returns junk when asked for less than its
    // minimum, and Taffy legitimately probes with a 0 width while
    // resolving flex minimums. Same clamp as `text::measure`.
    // `+ mx` puts the value back into GTK's margin-inclusive terms.
    let for_size = if width >= 1.0 {
        (width.round() as i32).max(wmin) + mx
    } else {
        -1
    };
    let (_hmin, hnat, _, _) = widget.measure(gtk4::Orientation::Vertical, for_size);
    Size {
        width,
        height: known.height.unwrap_or((hnat - my).max(0) as f32),
    }
}

/// Total horizontal and vertical margin on a widget, as `(mx, my)`.
///
/// A leaf's author padding lives in its GTK margins (`apply_style` step
/// 1a), and `gtk_widget_measure` folds margins into every size it
/// reports. Any code converting between GTK's margin-inclusive sizes and
/// Taffy's content-box sizes has to go through this.
pub(crate) fn widget_margins(widget: &gtk4::Widget) -> (i32, i32) {
    (
        widget.margin_start() + widget.margin_end(),
        widget.margin_top() + widget.margin_bottom(),
    )
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

/// A registered `position: sticky` node.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StickyEntry {
    /// The enclosing `scroll_view` it pins inside — `None` until
    /// resolved. Deliberately NOT resolved at registration time: the
    /// walker applies a node's style BEFORE inserting it into its
    /// parent, so at `apply_style` the node has no ancestry at all
    /// (observed: `node 247 is Sticky, enclosing_scroll=None
    /// parent=None`). Resolved on the first
    /// [`LinuxBackend::update_sticky`] after the tree is assembled.
    scroll: Option<u64>,
    /// `top` threshold in px — how far below the container's top edge it
    /// parks once engaged.
    top: f32,
}

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
    // The per-backend External table and navigator registry are gone in
    // runtime-v2: SDK payloads mount through `runtime_scene::Registry`
    // (typed by `TypeId`, installed at the app's boot seam) rather than
    // registering handlers on the backend itself. See CLAUDE.md §3.
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
    /// `position: sticky` nodes → the scroll node they pin inside and
    /// their `top` threshold. Populated by `apply_style`; consumed by
    /// [`LinuxBackend::update_sticky`] on scroll and after each layout.
    sticky_nodes: HashMap<u64, StickyEntry>,
    /// Last window size handed to `runtime_shared::set_viewport_size`, so
    /// [`run_layout`](Self::run_layout) only schedules a publish when the
    /// size actually changed. See there for why it's deferred.
    published_viewport: (f32, f32),
}

// The `LinuxExternalRegistrar` / `LinuxNavigatorRegistrar` inventory
// hooks are gone with the External table they fed. In runtime-v2 an SDK
// installs its handler on a `runtime_scene::Registry` at the app's boot
// seam (`start_in(..., my_sdk::register, app)`) instead of submitting a
// link-time ctor that mutates the backend. That removes the whole class
// of "SDK compiled in but silently never registered" bug these hooks
// existed to work around — an unregistered payload now panics at
// realize by design rather than falling through to a placeholder.

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
            _font_files: Vec::new(),
            root_id: None,
            self_ref: std::rc::Weak::new(),
            sticky_nodes: HashMap::new(),
            published_viewport: (0.0, 0.0),
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

    /// A node's frame in **window** coordinates: its parent-relative
    /// Taffy frame accumulated up to the root, with every intervening
    /// scroll container's current offset subtracted so the result is
    /// where the node actually sits on screen right now.
    ///
    /// This is what `ViewHandle::absolute_frame()` reports, and author
    /// code does real geometry with it — the website's table of contents
    /// compares each section's absolute frame against the scroll
    /// viewport's to decide which entry is active. Returning the
    /// parent-relative frame instead (as this used to) silently yields a
    /// section's offset *within its column*, so the scroll-spy compared
    /// unrelated numbers and never tracked.
    pub(crate) fn node_absolute_frame(&self, id: u64) -> Option<(f32, f32, f32, f32)> {
        let (_, _, w, h) = self.node_frame(id)?;
        let (mut x, mut y) = (0.0, 0.0);
        let mut cur = id;
        loop {
            let st = self.nodes.get(&cur)?;
            x += st.frame.0;
            y += st.frame.1;
            // A scroll container's children are displaced by however far
            // it's scrolled.
            if let Some(sw) = st.widget.downcast_ref::<gtk4::ScrolledWindow>() {
                x -= sw.hadjustment().value() as f32;
                y -= sw.vadjustment().value() as f32;
            }
            match self.parent_of.get(&cur) {
                Some(parent) => cur = *parent,
                None => return Some((x, y, w, h)),
            }
        }
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
        // Publish the window size to the framework's reactive viewport
        // signal so responsive author code (breakpoints, `viewport_size()`
        // derivations) sees the size it's laying out against. Without it
        // the signal stays at `ViewportSize::ZERO` and every breakpoint
        // resolves to its smallest bucket — the website rendered its
        // MOBILE layout in a 1024px window, sidebar collapsed away.
        //
        // Deferred to an idle, NOT called inline: signal writes run their
        // dependent effects SYNCHRONOUSLY, and those re-enter the walker
        // to restyle/rebuild the tree. `run_layout` is called from inside
        // the root's `size_allocate` with the backend already mutably
        // borrowed, so an inline publish panics with "RefCell already
        // borrowed" (`walker/style.rs`) — verified by crashing the app on
        // exactly this. At idle the allocation cycle is over and the
        // borrow is released. Gated on an actual change so a steady
        // window schedules nothing.
        if self.published_viewport != (width, height) {
            self.published_viewport = (width, height);
            gtk4::glib::source::idle_add_local_once(move || {
                runtime_shared::set_viewport_size(runtime_shared::ViewportSize { width, height });
            });
        }

        self.layout.compute(root_layout, width, height);

        if std::env::var_os("IDEALYST_GTK_DUMP_LAYOUT").is_some() {
            eprintln!(
                "[layout] === run_layout {width}x{height} root={:?} sticky={}",
                self.root_id,
                self.sticky_nodes.len(),
            );
            for (id, st) in self.nodes.iter() {
                if let Some(sw) = st.widget.downcast_ref::<gtk4::ScrolledWindow>() {
                    let v = sw.vadjustment();
                    eprintln!(
                        "[layout]   scroll id={id}: vadj value={:.0} page={:.0} upper={:.0} (scrollable={})",
                        v.value(),
                        v.page_size(),
                        v.upper(),
                        v.upper() > v.page_size(),
                    );
                }
            }
            let mut dump: Vec<u64> = self.nodes.keys().copied().collect();
            dump.sort();
            for id in dump {
                if let Some(st) = self.nodes.get(&id) {
                    let f = self.layout.frame_of(st.layout);
                    eprintln!(
                        "[layout] id={id:<4} {:<20} frame=({:>7.1},{:>7.1}) {:>7.1}x{:<7.1} parent={:?} ALLOC={}x{} map={} vis={} target={} bg={:?} par_gtk={:?}",
                        st.widget.type_().name(),
                        f.x,
                        f.y,
                        f.width,
                        f.height,
                        self.parent_of.get(&id),
                        st.widget.allocated_width(),
                        st.widget.allocated_height(),
                        st.widget.is_mapped(),
                        st.widget.is_visible(),
                        st.widget.can_target(),
                        st.widget
                            .downcast_ref::<IdealystView>()
                            .and_then(|v| v.paint_background())
                            .map(|c| [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (c[3] * 255.0) as u8]),
                        st.widget.parent().map(|p| p.type_().name().to_string()),
                    );
                    // Opacity is the usual reason a correctly-framed,
                    // mapped, visible node still paints nothing.
                    eprintln!(
                        "[layout]      opacity: static={:.2} anim={:?} gtk={:.2}  z={:.1}",
                        st.static_opacity,
                        st.anim_opacity,
                        st.widget.opacity(),
                        st.z,
                    );
                }
            }
        }

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
            // Clamp at 0: Taffy can hand back a negative box when padding
            // or borders exceed a definite size, and GTK rejects negative
            // sizes outright. A negative size has no meaning as a widget
            // allocation; zero is the honest floor. (Note: this is NOT
            // the source of GTK's startup "GtkGizmo (slider) reported min
            // width -2" notices — those persist with this clamp in place,
            // so they come from inside GTK's own scrollbar parts.)
            let (w, h) = (
                (frame.width.round() as i32).max(0),
                (frame.height.round() as i32).max(0),
            );
            // IdealystView containers report their size via `layout_size`
            // (read directly by the parent's `size_allocate`); leaf
            // widgets (Label/Button/…) have their frame recorded on the
            // parent's child record. `set_size_request` is the fallback
            // ONLY for a leaf whose parent isn't an IdealystView (e.g.
            // under a ScrolledWindow) — it's a *minimum*, so using it on
            // a wrapping label would floor the label's height and stop it
            // shrinking when the window widens and it needs fewer lines.
            if let Some(v) = widget.downcast_ref::<IdealystView>() {
                v.set_layout_size(w, h);
            } else {
                let recorded = widget
                    .parent()
                    .and_then(|p| p.downcast::<IdealystView>().ok())
                    .is_some_and(|p| p.set_child_layout_size(&widget, w, h));
                if !recorded {
                    widget.set_size_request(w, h);
                }
            }
        }
        for id in &ids {
            self.rebuild_transform(*id, true);
        }

        // Reflow moved every node's natural position, so the pins are
        // stale — recompute them against the new geometry. Quiet: the
        // allocation this pass is part of consumes the transforms.
        self.update_sticky(None, true);
    }

    /// Lay out a DETACHED subtree rooted at `id` against `width` ×
    /// `height`. Portal containers and virtualizer cells are Taffy
    /// orphans — the main [`run_layout`](Self::run_layout) pass is scoped
    /// to nodes reachable from the framework root, so it never frames
    /// them, and without this every widget inside a portal/cell stays
    /// 0×0. Mirrors `run_layout` but (a) computes from `id`'s layout node
    /// as a sub-root and (b) walks only `id` + its tracked descendants.
    /// Uses "quiet" transforms (no `queue_allocate`): callers invoke this
    /// from inside the subtree root's own `size_allocate` (portal) or
    /// immediately after placing a cell (virtualizer), where the result
    /// is consumed by the same/next allocation.
    pub fn layout_detached_root(&mut self, id: u64, width: f32, height: f32) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let Some(root_layout) = self.nodes.get(&id).map(|s| s.layout) else {
            return;
        };
        self.layout.compute(root_layout, width, height);

        // Collect `id` + every tracked descendant (self.children is the
        // parent→children linkage the walker's `insert` populates).
        let mut ids = Vec::new();
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            ids.push(n);
            if let Some(ch) = self.children.get(&n) {
                stack.extend(ch.iter().copied());
            }
        }

        for nid in &ids {
            let (frame, widget) = {
                let Some(st) = self.nodes.get(nid) else {
                    continue;
                };
                (self.layout.frame_of(st.layout), st.widget.clone())
            };
            if let Some(st) = self.nodes.get_mut(nid) {
                st.frame = (frame.x, frame.y, frame.width, frame.height);
            }
            let (w, h) = (
                (frame.width.round() as i32).max(0),
                (frame.height.round() as i32).max(0),
            );
            // Same frame-vs-intrinsic rule as `run_layout` — see there.
            // Portal/virtualizer content is real app content (wrapping
            // text included), so it needs the identical treatment.
            if let Some(v) = widget.downcast_ref::<IdealystView>() {
                v.set_layout_size(w, h);
            } else {
                let recorded = widget
                    .parent()
                    .and_then(|p| p.downcast::<IdealystView>().ok())
                    .is_some_and(|p| p.set_child_layout_size(&widget, w, h));
                if !recorded {
                    widget.set_size_request(w, h);
                }
            }
        }
        for nid in &ids {
            self.rebuild_transform(*nid, true);
        }
    }



    /// SDK extension helper: register an existing widget with the
    /// backend's layout tree so flex parents can size + position it.
    /// Returns the wrapped LinuxNode. Mirrors
    /// `IosBackend::register_external_view` /
    /// `WindowsBackend::register_external_view`.
    pub fn register_external_view(&mut self, widget: gtk4::Widget) -> LinuxNode {
        let node = self.wrap(widget.clone(), NodeKind::Other);
        // Give Taffy the widget's intrinsic size so `Element::External`
        // content (svg `Picture`, `Video`, codeblock/markdown `Label`)
        // sizes to its content when the author doesn't pin explicit
        // width/height — the same measure-fn treatment `create_text`
        // gives a plain label. Author `width`/`height` still win (they
        // land in Taffy's `size` via `apply_style`, which overrides the
        // measured intrinsic).
        if let Some(layout) = self.nodes.get(&node.id).map(|s| s.layout) {
            let w = widget.clone();
            self.layout
                .set_measure_fn(layout, Rc::new(move |known, available| {
                    widget_measure(&w, known, available)
                }));
        }
        node
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
    /// Nearest ancestor of `id` that is a `scroll_view`, or `None` when
    /// the node isn't inside one (a sticky node then stays relative).
    fn enclosing_scroll(&self, id: u64) -> Option<u64> {
        let mut cur = *self.parent_of.get(&id)?;
        loop {
            let st = self.nodes.get(&cur)?;
            if st.widget.is::<gtk4::ScrolledWindow>() {
                return Some(cur);
            }
            cur = *self.parent_of.get(&cur)?;
        }
    }

    /// A node's Y in its scroll container's *content* coordinate space —
    /// the sum of the Taffy frame offsets from the node up to (but not
    /// including) `scroll`. This is the position the pin is measured
    /// against, and it's why the pin has to be recomputed after every
    /// layout pass as well as on scroll: reflow moves it.
    fn content_y_within(&self, id: u64, scroll: u64) -> Option<f32> {
        let mut y = 0.0;
        let mut cur = id;
        loop {
            let st = self.nodes.get(&cur)?;
            y += st.frame.1;
            cur = *self.parent_of.get(&cur)?;
            if cur == scroll {
                return Some(y);
            }
        }
    }

    /// Recompute every sticky pin (optionally only those inside `only`)
    /// and push the resulting offsets into the nodes' transforms.
    ///
    /// `quiet` mirrors [`Self::rebuild_transform`]: quiet from inside the
    /// layout pass (the allocation that follows consumes it), loud from a
    /// scroll callback, where nothing else will re-allocate.
    pub fn update_sticky(&mut self, only: Option<u64>, quiet: bool) {
        // Resolve any entry still waiting for its scroll ancestor. By the
        // time this runs (end of a layout pass, or a scroll event) the
        // node is inserted, so the walk succeeds. An entry with no
        // enclosing scroll_view stays unresolved forever and is simply
        // never pinned — CSS falls back to `relative` the same way.
        let pending: Vec<u64> = self
            .sticky_nodes
            .iter()
            .filter(|(_, e)| e.scroll.is_none())
            .map(|(id, _)| *id)
            .collect();
        for id in pending {
            if let Some(scroll) = self.enclosing_scroll(id) {
                if let Some(e) = self.sticky_nodes.get_mut(&id) {
                    e.scroll = Some(scroll);
                }
            }
        }

        let entries: Vec<(u64, u64, f32)> = self
            .sticky_nodes
            .iter()
            .filter_map(|(id, e)| e.scroll.map(|s| (*id, s, e.top)))
            .filter(|(_, scroll, _)| only.is_none_or(|want| *scroll == want))
            .collect();

        for (id, scroll, top) in entries {
            let Some(offset) = self
                .nodes
                .get(&scroll)
                .and_then(|s| s.widget.downcast_ref::<gtk4::ScrolledWindow>())
                .map(|sw| sw.vadjustment().value() as f32)
            else {
                continue;
            };
            let Some(content_y) = self.content_y_within(id, scroll) else {
                continue;
            };
            let dy = sticky::pin_offset(content_y, offset, top);
            let changed = match self.nodes.get_mut(&id) {
                Some(st) if st.transform.sticky_dy != dy => {
                    st.transform.sticky_dy = dy;
                    true
                }
                _ => false,
            };
            if changed {
                self.rebuild_transform(id, quiet);
            }
        }
    }

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
// GTK mechanism — formerly `impl Backend for LinuxBackend`.
//
// v2 deleted the 159-method mega-trait; these bodies are unchanged and now
// live as inherent methods. `newcore.rs` implements `runtime_scene::Host` +
// the caps traits on top by delegating here, so the same scene builds the
// same widget tree.
// =========================================================================

impl LinuxBackend {

    fn color_scheme(&self) -> ColorScheme {
        ColorScheme::Auto
    }

    fn platform(&self) -> Platform {
        Platform::Custom("linux")
    }

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> LinuxNode {
        let widget = IdealystView::new();
        self.wrap(widget.upcast::<gtk4::Widget>(), NodeKind::View)
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> LinuxNode {
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
        _leading_icon: Option<&runtime_shared::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_shared::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> LinuxNode {
        let button = gtk4::Button::with_label(label);
        let fire = on_click.fire.clone();
        button.connect_clicked(move |_| (fire)());
        self.wrap(button.upcast::<gtk4::Widget>(), NodeKind::Other)
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> LinuxNode {
        // A styled container (IdealystView) with a click gesture — the
        // framework's Pressable is "a View that fires a callback".
        let widget = IdealystView::new();
        let gesture = gtk4::GestureClick::new();
        let fire = on_click.clone();
        gesture.connect_released(move |_, _, _, _| (fire)());
        widget.add_controller(gesture);
        self.wrap(widget.upcast::<gtk4::Widget>(), NodeKind::Pressable)
    }

    fn install_touch_handler(
        &mut self,
        node: &LinuxNode,
        handler: runtime_shared::TouchHandler,
    ) {
        // The trait's default body is a NO-OP, so leaving this
        // unimplemented is invisible: `.on_touch()` views render fine
        // and simply never fire. See `touch.rs`.
        touch::install(&node.widget, handler);
    }

    fn install_file_drop_handler(
        &mut self,
        node: &LinuxNode,
        handler: runtime_shared::FileDropHandler,
    ) {
        // Like `install_touch_handler`, the trait default is a NO-OP — which
        // is exactly what left the file-picker SDK's `FileDropZone` inert on
        // Linux. Attach a GTK `DropTarget` so an OS file drag reaches the
        // handler (and thence `picked_from_dropped`). See `file_drop.rs`.
        file_drop::install(&node.widget, handler);
    }

    fn insert(&mut self, parent: &mut LinuxNode, child: LinuxNode) {
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
            if let Some(inner) = scroll_document(scrolled) {
                inner.put(&child.widget, 0.0, 0.0);
            } else {
                // Never silently drop a child: an unparented widget is
                // laid out but invisible, which reads as "the backend
                // rendered nothing" and is near-impossible to spot.
                eprintln!(
                    "[backend-linux] scroll_view has no Fixed document; \
                     dropping child {} (this is a bug)",
                    child.id,
                );
            }
        }
    }

    fn clear_children(&mut self, node: &LinuxNode) {
        if let Some(iv) = node.widget.downcast_ref::<IdealystView>() {
            iv.remove_all_children();
        } else if let Some(scrolled) = node.widget.downcast_ref::<gtk4::ScrolledWindow>() {
            if let Some(inner) = scroll_document(scrolled) {
                let mut child = inner.first_child();
                while let Some(c) = child {
                    let next = c.next_sibling();
                    inner.remove(&c);
                    child = next;
                }
            }
        }
        // Detach from the LAYOUT tree too, not just GTK and the tracking
        // maps. Taffy is a separate tree: unparenting a widget doesn't
        // remove its layout node, so a cleared subtree keeps reserving
        // its old space forever. The website's `lazy!`-loaded Simulator
        // showed this exactly — its pre-load placeholder was cleared and
        // unparented, but its 681px-tall layout node stayed a child, so
        // the real phone rendered pushed 681px down the column from where
        // every other backend puts it.
        let parent_layout = self.nodes.get(&node.id).map(|s| s.layout);
        if let Some(ids) = self.children.remove(&node.id) {
            for id in ids {
                self.parent_of.remove(&id);
                if let (Some(parent_layout), Some(child_layout)) =
                    (parent_layout, self.nodes.get(&id).map(|s| s.layout))
                {
                    self.layout.remove_child(parent_layout, child_layout);
                }
            }
        }
    }

    fn update_text(&mut self, node: &LinuxNode, content: &str) {
        if let Some(label) = node.widget.downcast_ref::<gtk4::Label>() {
            label.set_text(content);
        }
    }

    fn update_button_label(&mut self, node: &LinuxNode, label: &str) {
        if let Some(btn) = node.widget.downcast_ref::<gtk4::Button>() {
            btn.set_label(label);
        }
    }

    fn apply_style(&mut self, node: &LinuxNode, style: &Rc<StyleRules>) {
        let id = node.id;
        let Some((layout, kind)) = self.nodes.get(&id).map(|s| (s.layout, s.kind)) else {
            return;
        };

        // 1. Layout — push flex/size/position/padding/margin into Taffy.
        self.layout.set_style(layout, style);

        // 1a. Padding on a LEAF widget → GTK margins.
        //
        // Taffy sizes a node's box to INCLUDE its padding and insets any
        // children accordingly — which is the whole story for an
        // `IdealystView`, since its children are separate widgets we
        // position ourselves. A leaf paints its own content across its
        // entire allocation instead, so a `GtkLabel` (plain text, and the
        // `codeblock` SDK's rendered source) ignored padding completely
        // and sat flush against its background edge, unlike every other
        // backend.
        //
        // GTK margins are the right target: `gtk_widget_allocate` takes
        // the full box and the widget subtracts its margins to get the
        // content area, so allocating the Taffy frame (padding included)
        // and setting margins equal to the padding insets the content by
        // exactly the padding — no double-counting.
        if let Some(st) = self.nodes.get(&id) {
            if !st.widget.is::<IdealystView>() {
                let w = st.widget.clone();
                w.set_margin_top(len_px(&style.padding_top).round().max(0.0) as i32);
                w.set_margin_bottom(len_px(&style.padding_bottom).round().max(0.0) as i32);
                w.set_margin_start(len_px(&style.padding_left).round().max(0.0) as i32);
                w.set_margin_end(len_px(&style.padding_right).round().max(0.0) as i32);
            }
        }

        // 1b. `position: sticky` — register (or drop) the pin. Taffy
        // treats sticky as relative and places the node normally; the pin
        // is a purely visual offset applied on scroll, so the flow around
        // it never reflows. Re-registering on every restyle keeps the
        // threshold live and drops the entry when a node stops being
        // sticky.
        self.sticky_nodes.remove(&id);
        if matches!(style.position, Some(runtime_shared::Position::Sticky)) {
            // Record the intent only — the scroll container is resolved
            // lazily (see `StickyEntry::scroll`). Re-inserting on every
            // restyle keeps the threshold live and re-resolves the
            // ancestor in case the node was reparented.
            self.sticky_nodes.insert(
                id,
                StickyEntry {
                    scroll: None,
                    top: len_px(&style.top),
                },
            );
        }

        // 2. Opacity (all kinds) + static transform (all kinds).
        if let Some(st) = self.nodes.get_mut(&id) {
            st.static_opacity = style.opacity.as_ref().map(|t| t.resolve()).unwrap_or(1.0);
            let eff = st.anim_opacity.unwrap_or(st.static_opacity);
            st.widget.set_opacity(eff as f64);

            // `pointer_events: None` → the widget must be transparent to
            // input. GTK hit-tests purely on geometry; opacity 0 still
            // swallows events, so an "invisible" overlay stays a wall.
            // `AppShell`'s scrim is exactly this: a full-window Pressable
            // painted OVER the content and marked `PointerEvents::None`
            // while the sidebar is pinned (see its
            // `pinned_overlay_shows_panel_and_inerts_scrim` test). Until
            // this was honored the scrim ate every scroll and click over
            // the main content — the page couldn't scroll and no link in
            // it could be followed, while the sidebar (painted ABOVE the
            // scrim) worked fine and made it look like a scroll bug.
            //
            // Divergence from CSS, deliberate and narrow: `pointer-events`
            // inherits, and a child can opt back in with `Auto`. GTK's
            // `can-target` is per-widget and picking skips a
            // non-targetable widget's subtree, so a re-enabling child
            // inside a `None` parent stays inert here. No framework
            // surface builds that shape today (the Apple backends model
            // the full chain in `backend-apple-core`'s
            // `pointer_events_policy`); revisit if one does.
            st.widget
                .set_can_target(!matches!(style.pointer_events, Some(PointerEvents::None)));
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
            NodeKind::Other => {
                // Image nodes are `gtk::Picture` (NodeKind::Other). Honor
                // `object_fit` → `content-fit` here — the framework calls
                // `apply_style` separately from `create_image`, so this is
                // where a reactive or static `object_fit` lands (default
                // `Contain`, matching the framework-wide default).
                if let Some(st) = self.nodes.get(&id) {
                    if let Some(pic) = st.widget.downcast_ref::<gtk4::Picture>() {
                        let fit = style.object_fit.unwrap_or_default();
                        pic.set_content_fit(image::map_fit(fit));
                    }
                }
            }
        }
    }

    fn set_animated_f32(&mut self, node: &LinuxNode, prop: AnimProp, value: f32) {
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

    fn set_animated_color(&mut self, node: &LinuxNode, prop: AnimProp, value: [f32; 4]) {
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

    fn make_view_handle(&self, node: &LinuxNode) -> runtime_shared::ViewHandle {
        handles::make_view_handle(self, node)
    }

    fn make_scroll_view_handle(
        &self,
        node: &LinuxNode,
    ) -> runtime_shared::primitives::scroll_view::ScrollViewHandle {
        handles::make_scroll_view_handle(self, node)
    }

    fn make_text_handle(&self, node: &LinuxNode) -> runtime_shared::TextHandle {
        handles::make_text_handle(self, node)
    }

    fn finish(&mut self, root: LinuxNode) {
        self.root_id = Some(root.id);

        // First mount: make the framework root the window's child so
        // GtkWindow stretches it to fill the content area, and install
        // the layout callback so the Taffy pass runs inside the root's
        // `size_allocate` (GTK's own layout cycle — fires on first map +
        // every resize). This is what keeps the external layout engine in
        // step with GTK allocation without fighting the frame clock.
        if root.widget.parent().is_none() {
            install_canvas_background(&self.host_window);
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
        src: &str,
        alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> LinuxNode {
        // `gtk::Picture` with a `content-fit` for `object_fit` (see
        // `image.rs`). `register_external_view` installs the intrinsic-
        // size measure fn so an unpinned image sizes to its bitmap;
        // author `width`/`height` still win via Taffy.
        let pic = image::build_picture(src, alt);
        self.register_external_view(pic.upcast::<gtk4::Widget>())
    }

    fn update_image_src(&mut self, node: &LinuxNode, src: &str) {
        if let Some(pic) = node.widget.downcast_ref::<gtk4::Picture>() {
            image::set_source(pic, src);
        }
    }

    fn update_image_alt(&mut self, node: &LinuxNode, alt: Option<&str>) {
        if let Some(pic) = node.widget.downcast_ref::<gtk4::Picture>() {
            pic.set_alternative_text(alt);
        }
    }

    fn create_icon(
        &mut self,
        data: &runtime_shared::primitives::icon::IconData,
        color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> LinuxNode {
        // Default color = opaque black (the text-node default / Linux
        // analogue of web `currentColor`); a set color parses through the
        // shared sRGB parser. The custom widget scales + strokes/fills the
        // glyph in its own `snapshot` — see `icon.rs`.
        let rgba = color
            .map(color::to_srgb)
            .unwrap_or(icon::DEFAULT_ICON_COLOR);
        let widget = icon::IdealystIcon::new_from_data(data, rgba);
        let node = self.wrap(widget.upcast::<gtk4::Widget>(), NodeKind::Other);

        // Pin a 24x24 default intrinsic size, matching iOS/macOS
        // (`backend-macos`'s `create_icon` sets the same constant).
        //
        // Without a measure fn an icon's Taffy node has NO size source at
        // all — `IdealystIcon` is a bare GTK widget with no natural size,
        // and the icon primitive carries its dimensions in a viewBox
        // rather than in style. So every icon laid out at 0x0 and simply
        // did not appear: the whiteboard demo's entire toolbar rendered
        // as blank buttons.
        //
        // Style-driven `width`/`height` still win, because Taffy passes
        // them as `known` and the closure short-circuits to them.
        const ICON_SIZE: f32 = 24.0;
        if let Some(layout) = self.nodes.get(&node.id).map(|s| s.layout) {
            self.layout.set_measure_fn(
                layout,
                Rc::new(move |known: Size<Option<f32>>, _available| Size {
                    width: known.width.unwrap_or(ICON_SIZE),
                    height: known.height.unwrap_or(ICON_SIZE),
                }),
            );
        }
        node
    }

    fn update_icon_color(&mut self, node: &LinuxNode, color: &Color) {
        if let Some(w) = node.widget.downcast_ref::<icon::IdealystIcon>() {
            w.set_color(color::to_srgb(color));
        }
    }

    fn update_icon_data(
        &mut self,
        node: &LinuxNode,
        data: &runtime_shared::primitives::icon::IconData,
    ) {
        if let Some(w) = node.widget.downcast_ref::<icon::IdealystIcon>() {
            w.set_data(data);
        }
    }

    // Stroke draw-on animation (dash-offset trim) is not yet wired on the
    // GTK backend. GSK can express it — `gsk::Stroke::set_dash` +
    // `set_dash_offset` with the total path length from `gsk::PathMeasure`,
    // ticked by a `gtk::TickCallback` — but the length/dash bookkeeping and
    // an easing clock are non-trivial and out of scope for the initial
    // primitive. The icon renders fully drawn (the documented fallback for
    // backends without stroke animation), so `draw_in` / reactive `stroke`
    // degrade gracefully rather than break.
    // TODO(icon-stroke-anim): implement update_icon_stroke / animate_icon_stroke
    // via PathMeasure-driven dash trimming + a tick-callback easing clock.

    fn create_text_input(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        _on_blur: Option<runtime_shared::primitives::text_input::BlurHandler>,
        secure: bool,
        _a11y: &AccessibilityProps,
    ) -> LinuxNode {
        let entry = gtk4::Entry::new();
        entry.set_text(initial_value);
        if secure {
            entry.set_visibility(false);
        }
        self.wrap(entry.upcast::<gtk4::Widget>(), NodeKind::Other)
    }

    fn update_text_input_secure(&mut self, node: &LinuxNode, secure: bool) {
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
        _on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> LinuxNode {
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
    ) -> LinuxNode {
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
    ) -> LinuxNode {
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
    ) -> LinuxNode {
        let scrolled = gtk4::ScrolledWindow::new();
        if horizontal {
            scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Never);
        } else {
            scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        }
        let inner = gtk4::Fixed::new();
        scrolled.set_child(Some(&inner));

        if let Some(cb) = on_scroll {
            // Deferred to an idle, NOT called inline. Author `on_scroll`
            // handlers write signals (the website's scroll-spy does
            // `scroll_y.set(y)`), and a signal write runs its dependent
            // effects synchronously. GTK emits `value-changed` from
            // inside allocation too — `upper`/`page_size` shift whenever
            // the viewport resizes — so an inline call re-enters the
            // reactive runtime mid-update and aborts with "RefCell
            // already borrowed" (`reactive.rs`). Verified by crashing the
            // app on exactly this once the TOC's scroll-spy started
            // reading real frames. Same deferral as the viewport publish
            // in `run_layout`.
            let fire = move |x: f32, y: f32| {
                let cb = cb.clone();
                gtk4::glib::source::idle_add_local_once(move || cb(x, y));
            };
            let fire_for_h = fire.clone();
            let scrolled_for_h = scrolled.clone();
            scrolled.hadjustment().connect_value_changed(move |adj| {
                let x = adj.value() as f32;
                let y = scrolled_for_h.vadjustment().value() as f32;
                fire_for_h(x, y);
            });
            let scrolled_for_v = scrolled.clone();
            scrolled.vadjustment().connect_value_changed(move |adj| {
                let x = scrolled_for_v.hadjustment().value() as f32;
                let y = adj.value() as f32;
                fire(x, y);
            });
        }

        let node = self.wrap(scrolled.clone().upcast::<gtk4::Widget>(), NodeKind::Other);

        // Drive `position: sticky` from this container's own scroll
        // offset. Separate from the `on_scroll` prop wiring above because
        // sticky must work whether or not the author asked for scroll
        // callbacks. `value-changed` fires only when the offset actually
        // moves, so this costs nothing while the view is still — the same
        // event-driven model as the macOS/Android backends (no vsync
        // polling).
        {
            let me = self.self_ref();
            let sticky_id = node.id;
            scrolled.vadjustment().connect_value_changed(move |_| {
                if let Some(b) = me.upgrade() {
                    if let Ok(mut b) = b.try_borrow_mut() {
                        b.update_sticky(Some(sticky_id), false);
                    }
                }
            });
        }

        // Mark the node `overflow: scroll` in Taffy so it's sized by its
        // PARENT, not by its content. The content is parented under this
        // node, so its size feeds the scroll node's automatic minimum —
        // without this the scroll view grows to its full content height
        // and has nothing left to scroll. The website's page was a
        // 3437px-tall `GtkScrolledWindow` inside a 712px slot: clipped,
        // and completely unscrollable. iOS/Android/macOS/terminal all
        // call this (see `LayoutTree::set_overflow_scroll`); GTK was the
        // odd one out. `set_style` clones the existing Taffy style and
        // never touches `overflow`, so later restyles preserve it.
        if let Some(l) = self.nodes.get(&node.id).map(|s| s.layout) {
            self.layout.set_overflow_scroll(l, horizontal);
        }
        node
    }

    fn create_link(
        &mut self,
        config: runtime_shared::primitives::link::LinkConfig,
        _a11y: &AccessibilityProps,
    ) -> LinuxNode {
        // A Link is "a Pressable that navigates". The trait default
        // collapses to `create_view` and DROPS `on_activate`, so every
        // link rendered as inert text and nothing in the app could be
        // navigated to — the same bug the terminal backend fixed (see
        // its `create_link`). `config.url` is deliberately ignored
        // (documented as web-only); `on_activate` already wraps in-app
        // push/replace dispatch, and `open_url` for `external` links.
        // Identical gesture wiring to `create_pressable` so both behave
        // the same under hit-testing.
        let widget = IdealystView::new();
        let gesture = gtk4::GestureClick::new();
        let fire = config.on_activate.clone();
        gesture.connect_released(move |_, _, _, _| (fire)());
        widget.add_controller(gesture);
        self.wrap(widget.upcast::<gtk4::Widget>(), NodeKind::Pressable)
    }

    fn create_activity_indicator(
        &mut self,
        _size: runtime_shared::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> LinuxNode {
        let spinner = gtk4::Spinner::new();
        spinner.start();
        self.wrap(spinner.upcast::<gtk4::Widget>(), NodeKind::Other)
    }

    fn create_virtualizer(
        &mut self,
        callbacks: runtime_shared::VirtualizerCallbacks<LinuxNode>,
        overscan: f32,
        layout: runtime_shared::VirtualLayout,
        _a11y: &AccessibilityProps,
    ) -> LinuxNode {
        // ScrolledWindow over a Fixed "document"; `virtualizer.rs` drives
        // windowed realization + recycling from the scroll adjustments.
        let horizontal = layout.axis.is_horizontal();
        let scrolled = gtk4::ScrolledWindow::new();
        if horizontal {
            scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Never);
        } else {
            scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        }
        let fixed = gtk4::Fixed::new();
        scrolled.set_child(Some(&fixed));
        let node = self.wrap(scrolled.clone().upcast::<gtk4::Widget>(), NodeKind::Other);
        virtualizer::create(
            node.id,
            scrolled,
            fixed,
            callbacks,
            overscan,
            layout,
            self.self_ref(),
        );
        node
    }

    fn virtualizer_data_changed(&mut self, node: &LinuxNode) {
        virtualizer::data_changed(node.id);
    }

    fn release_virtualizer(&mut self, node: &LinuxNode) {
        virtualizer::release(node.id);
    }

    fn create_graphics(
        &mut self,
        on_ready: runtime_shared::primitives::graphics::OnReady,
        on_resize: runtime_shared::primitives::graphics::OnResize,
        on_lost: runtime_shared::primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> LinuxNode {
        // A `GtkGLArea` render surface. See [`graphics`] for why this can't
        // yet satisfy the raw-window-handle `on_ready` contract on GTK4 — the
        // widget acquires a live GL context + FBO (proven by clearing to a
        // color) but exposes no `HasWindowHandle` for wgpu's `create_surface`.
        let widget = graphics::build_gl_area(on_ready, on_resize, on_lost);
        self.wrap(widget, NodeKind::Other)
    }


    fn create_portal(
        &mut self,
        target: runtime_shared::primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> LinuxNode {
        // A full-viewport flex container (see `portal.rs`) mounted into a
        // window-level `gtk::Overlay`. NodeKind::View so its background /
        // flex style apply through the normal `apply_style` path.
        let view = IdealystView::new();
        let node = self.wrap(view.clone().upcast::<gtk4::Widget>(), NodeKind::View);
        // Base placement flex from the target (author/composition style
        // overrides via a later `apply_style`).
        if let Some(layout) = self.nodes.get(&node.id).map(|s| s.layout) {
            let style = Rc::new(portal::placement_style(&target));
            self.layout.set_style(layout, &style);
        }
        portal::configure(
            &view,
            node.id,
            self.self_ref(),
            self.host_window.clone(),
            on_dismiss,
            trap_focus,
        );
        node
    }

    fn release_portal(&mut self, node: &LinuxNode) {
        if let Some(v) = node.widget.downcast_ref::<IdealystView>() {
            portal::release(v);
        }
    }

    fn set_portal_hidden(&mut self, node: &LinuxNode, hidden: bool) {
        // Hide without teardown (navigation off the portal's screen).
        node.widget.set_visible(!hidden);
    }





}

#[cfg(test)]
mod layout_tests {
    use super::{leaf_alloc_size, scroll_document};
    use gtk4::prelude::*;

    // The website's entire page was invisible: every child inserted into
    // a `scroll_view` was silently dropped from the widget tree. GTK
    // auto-wraps a non-scrollable child (our `GtkFixed` document) in a
    // `GtkViewport`, so `scrolled.child()` returns the VIEWPORT and the
    // old `downcast::<GtkFixed>()` failed — the `if let` just didn't
    // fire. Symptom was maximally confusing: Taffy framed the content
    // correctly (1024x2886) while GTK reported ALLOC=0x0, map=false,
    // parent=None for it.
    //
    // Needs a display (GObject construction requires an initialized
    // GTK); headless CI skips rather than fails.
    // ===================================================================
    // GTK-dependent regressions — ONE test on purpose.
    //
    // GTK4 requires every call to happen on the thread that ran
    // `gtk::init`, and cargo runs each `#[test]` on its own thread. Split
    // across several tests these segfault (verified). So all the checks
    // that need a live GTK live in this single function; headless CI
    // skips the lot. Each section names the bug it guards.
    // ===================================================================
    #[test]
    fn regression_gtk_layout_behaviors() {
        if gtk4::init().is_err() {
            eprintln!("skipping: no display available to initialize GTK");
            return;
        }
        use runtime_shared::accessibility::AccessibilityProps;
        use runtime_shared::{PointerEvents, StyleRules};

        // --- 1. Non-root nodes must report their Taffy size as the
        // MINIMUM. `GtkFixedLayout` (the GtkFixed document inside a
        // scroll_view) allocates children at their minimum, so the old
        // blanket `min = 0` allocated the whole scrollable page 0x0 and
        // nothing painted. Only the root needs 0, so the WINDOW can still
        // be resized smaller than its content.
        let child = crate::IdealystView::new();
        child.set_layout_size(1024, 2886);
        assert_eq!(
            child.measure(gtk4::Orientation::Horizontal, -1),
            (1024, 1024, -1, -1),
            "non-root must report Taffy width as min AND natural",
        );
        let root = crate::IdealystView::new();
        root.set_layout_callback(std::rc::Rc::new(|_, _| {}));
        root.set_layout_size(1024, 768);
        let (min_w, nat_w, _, _) = root.measure(gtk4::Orientation::Horizontal, -1);
        assert_eq!((min_w, nat_w), (0, 1024), "root keeps min 0 so the window can shrink");

        // --- 2. A changed layout size must invalidate GTK's cached
        // `measure()`. Parents we implement read `layout_size` directly,
        // but GTK-managed ones (GtkFixed/GtkViewport/GtkOverlay) allocate
        // from the cache, which only `queue_resize` clears.
        let cached = crate::IdealystView::new();
        cached.set_layout_size(100, 50);
        assert_eq!(cached.measure(gtk4::Orientation::Horizontal, -1).1, 100);
        cached.set_layout_size(200, 50);
        assert_eq!(
            cached.measure(gtk4::Orientation::Horizontal, -1).1,
            200,
            "stale cached measure - queue_resize missing from set_layout_size",
        );

        // --- 3. `scroll_document` must find the GtkFixed THROUGH the
        // GtkViewport GTK wraps it in. The old direct downcast failed
        // silently, so every child inserted into a scroll_view was never
        // parented: laid out correctly, allocated 0x0, never mapped. The
        // website's entire page was invisible.
        let scrolled = gtk4::ScrolledWindow::new();
        let fixed = gtk4::Fixed::new();
        scrolled.set_child(Some(&fixed));
        assert!(
            scrolled.child().unwrap().downcast_ref::<gtk4::Fixed>().is_none(),
            "GTK stopped wrapping the child - re-check scroll_document",
        );
        assert_eq!(
            scroll_document(&scrolled).expect("must find the Fixed through the viewport"),
            fixed,
        );

        // --- 4. `clear_children` must detach LAYOUT nodes, not just
        // widgets. Taffy is a separate tree: leaving the layout node
        // attached kept a cleared subtree reserving its old space. The
        // website's lazy-loaded Simulator placeholder did exactly that
        // and pushed the phone 681px down its column.
        let mut backend = crate::LinuxBackend::new(gtk4::Window::new());
        let a11y = AccessibilityProps::default();
        let mut parent = backend.create_view(&a11y);
        let kid = backend.create_view(&a11y);
        let parent_layout = backend.nodes.get(&parent.id).unwrap().layout;
        backend.insert(&mut parent, kid.clone());
        assert_eq!(backend.layout.children_of(parent_layout).len(), 1);
        backend.clear_children(&parent);
        assert!(
            backend.layout.children_of(parent_layout).is_empty(),
            "cleared subtree still reserves layout space and displaces siblings",
        );
        assert!(kid.widget.parent().is_none(), "and must still unparent the widget");

        // --- 5. `pointer_events: None` must make a widget transparent to
        // input. GTK hit-tests on geometry alone, so `AppShell`'s scrim —
        // a full-window Pressable painted over the content and inerted
        // while the sidebar is pinned — swallowed every scroll and click
        // over the main content until this was honored.
        let scrim = backend.create_pressable(std::rc::Rc::new(|| {}), &a11y);
        let inert = std::rc::Rc::new(StyleRules {
            pointer_events: Some(PointerEvents::None),
            ..StyleRules::default()
        });
        backend.apply_style(&scrim, &inert);
        assert!(
            !scrim.widget.can_target(),
            "pointer_events:None must clear can-target, or an invisible \
             overlay keeps eating input meant for the content beneath it",
        );

        let interactive = std::rc::Rc::new(StyleRules {
            pointer_events: Some(PointerEvents::Auto),
            ..StyleRules::default()
        });
        backend.apply_style(&scrim, &interactive);
        assert!(scrim.widget.can_target(), "Auto must restore hit-testing");

        // Unset is Auto: the overwhelmingly common case must stay
        // clickable, so this can't regress into "nothing is interactive".
        let plain = backend.create_pressable(std::rc::Rc::new(|| {}), &a11y);
        backend.apply_style(&plain, &std::rc::Rc::new(StyleRules::default()));
        assert!(plain.widget.can_target(), "unset pointer_events must hit-test");

        // --- 6. `position: sticky` must find its enclosing scroll_view
        // and register a pin. Nothing in the GTK backend handled
        // `Position` at all, so a sticky table-of-contents just scrolled
        // away with the content.
        let mut scroll = backend.create_scroll_view(false, None, &a11y);
        let mut row = backend.create_view(&a11y);
        let toc = backend.create_view(&a11y);
        backend.insert(&mut scroll, row.clone());
        backend.insert(&mut row, toc.clone());

        let sticky_style = std::rc::Rc::new(StyleRules {
            position: Some(runtime_shared::Position::Sticky),
            top: Some(runtime_shared::Tokenized::Literal(runtime_shared::Length::Px(16.0))),
            ..StyleRules::default()
        });
        backend.apply_style(&toc, &sticky_style);
        let entry = backend
            .sticky_nodes
            .get(&toc.id)
            .copied()
            .expect("a sticky node must register");
        assert_eq!(entry.top, 16.0, "must carry the `top` threshold");
        // Resolution is deliberately deferred: the walker styles a node
        // BEFORE inserting it, so at apply_style time it has no ancestry
        // (this is why the TOC never pinned — it registered against
        // nothing and was dropped).
        assert_eq!(entry.scroll, None, "ancestor is resolved lazily, not at apply_style");

        backend.update_sticky(None, true);
        assert_eq!(
            backend.sticky_nodes.get(&toc.id).unwrap().scroll,
            Some(scroll.id),
            "must resolve to the scroll ANCESTOR, not the direct parent",
        );

        // Ceasing to be sticky must drop the entry, or a stale pin keeps
        // dragging the node around on every scroll.
        backend.apply_style(&toc, &std::rc::Rc::new(StyleRules::default()));
        assert!(!backend.sticky_nodes.contains_key(&toc.id), "unregister on restyle");

        // Re-register (the unregister check above dropped it) and give
        // the nodes real frames.
        //
        // End-to-end: a pinned node's transform must actually move once
        // the container scrolls past it. Frames are set directly here
        // because a real Taffy pass needs a mapped window; everything
        // downstream of the frame (ancestor resolution, content-space y,
        // pin math, transform write) is the production path.
        backend.apply_style(&toc, &sticky_style);
        backend.nodes.get_mut(&row.id).unwrap().frame = (0.0, 100.0, 200.0, 400.0);
        backend.nodes.get_mut(&toc.id).unwrap().frame = (0.0, 50.0, 200.0, 300.0);
        let adj = backend
            .nodes
            .get(&scroll.id)
            .unwrap()
            .widget
            .downcast_ref::<gtk4::ScrolledWindow>()
            .unwrap()
            .vadjustment();

        // content_y = 50 (toc) + 100 (row) = 150. Not scrolled → no pin.
        adj.set_upper(5000.0);
        adj.set_value(0.0);
        backend.update_sticky(None, true);
        assert_eq!(
            backend.nodes.get(&toc.id).unwrap().transform.sticky_dy,
            0.0,
            "unscrolled: the node must ride the content",
        );

        // Scrolled 400px: the node is 250px above the pin line (150 - 400
        // = -250), plus a 16px threshold → pushed back down 266px.
        adj.set_value(400.0);
        backend.update_sticky(None, true);
        assert_eq!(
            backend.nodes.get(&toc.id).unwrap().transform.sticky_dy,
            266.0,
            "scrolled past: the node must pin, not scroll away",
        );

        // --- 6b. Padding on a LEAF must inset its content. Taffy puts
        // padding in the node's box, but a GtkLabel paints across its
        // whole allocation — so the `codeblock` SDK's source text sat
        // flush against its background edge with no padding at all,
        // unlike every other backend. Mapped to GTK margins.
        let leaf = backend.create_text("fn main() {}", &a11y);
        let padded = std::rc::Rc::new(StyleRules {
            padding_top: Some(runtime_shared::Tokenized::Literal(runtime_shared::Length::Px(16.0))),
            padding_bottom: Some(runtime_shared::Tokenized::Literal(runtime_shared::Length::Px(16.0))),
            padding_left: Some(runtime_shared::Tokenized::Literal(runtime_shared::Length::Px(12.0))),
            padding_right: Some(runtime_shared::Tokenized::Literal(runtime_shared::Length::Px(12.0))),
            ..StyleRules::default()
        });
        backend.apply_style(&leaf, &padded);
        assert_eq!(leaf.widget.margin_top(), 16, "top padding must inset the text");
        assert_eq!(leaf.widget.margin_bottom(), 16);
        assert_eq!(leaf.widget.margin_start(), 12, "left padding → start margin");
        assert_eq!(leaf.widget.margin_end(), 12);

        // GTK's own `measure()` INCLUDES the widget's margins. So once
        // padding became margins, the Taffy measure fn started reporting
        // content+padding, and Taffy — which adds padding to a leaf's
        // measured content size itself — added it a second time. Result:
        // every padded leaf on Linux was inset by twice its padding.
        // `text::measure` must report the CONTENT size.
        let raw = leaf.widget.measure(gtk4::Orientation::Horizontal, -1).1;
        let label = leaf.widget.clone().downcast::<gtk4::Label>().unwrap();
        let measured = crate::text::measure(
            &label,
            runtime_layout::Size { width: None, height: None },
            runtime_layout::Size {
                width: runtime_layout::AvailableSpace::MaxContent,
                height: runtime_layout::AvailableSpace::MaxContent,
            },
        );
        assert_eq!(
            measured.width.round() as i32,
            raw - 24,
            "measure must subtract the 12+12 horizontal margins — GTK includes \
             them, and Taffy re-adds padding on top of whatever we report",
        );

        // Removing the padding must remove the inset (same unset-reverts
        // rule as the text-style props).
        backend.apply_style(&leaf, &std::rc::Rc::new(StyleRules::default()));
        assert_eq!(leaf.widget.margin_top(), 0, "dropping padding must un-inset");

        // --- 6c. End-to-end: a padded leaf's Taffy FRAME must be
        // content + padding ONCE. This is the shape the website's TOC
        // links use (`TocLink`: padding_vertical 6, padding_left 12 on
        // the `text` itself), and it is what the measure fix above
        // exists to protect: with GTK's margin-inclusive measure feeding
        // Taffy, the frame came out content + 2x padding and every OTP
        // entry looked twice as padded on Linux as on web.
        let mut toc_root = backend.create_view(&a11y);
        let toc_text = backend.create_text("Getting started", &a11y);
        backend.insert(&mut toc_root, toc_text.clone());
        let toc_style = std::rc::Rc::new(StyleRules {
            padding_top: Some(runtime_shared::Tokenized::Literal(runtime_shared::Length::Px(6.0))),
            padding_bottom: Some(runtime_shared::Tokenized::Literal(runtime_shared::Length::Px(6.0))),
            padding_left: Some(runtime_shared::Tokenized::Literal(runtime_shared::Length::Px(12.0))),
            ..StyleRules::default()
        });
        backend.apply_style(&toc_text, &toc_style);

        // Content size the label reports with the padding applied.
        let toc_label = toc_text.widget.clone().downcast::<gtk4::Label>().unwrap();
        let content = crate::text::measure(
            &toc_label,
            runtime_layout::Size { width: None, height: None },
            runtime_layout::Size {
                width: runtime_layout::AvailableSpace::MaxContent,
                height: runtime_layout::AvailableSpace::MaxContent,
            },
        );

        let toc_root_layout = backend.nodes.get(&toc_root.id).unwrap().layout;
        backend.layout.compute(toc_root_layout, 220.0, 400.0);
        let frame = backend
            .layout
            .frame_of(backend.nodes.get(&toc_text.id).unwrap().layout);
        assert_eq!(
            frame.height.round(),
            (content.height + 12.0).round(),
            "leaf frame height must be content + padding ONCE (content \
             {:.0} + 6 + 6), got {:.0}",
            content.height,
            frame.height,
        );
        // Width is NOT asserted against content: a column flex parent
        // stretches its children cross-axis, so the frame is the
        // container's 220 regardless of the text. That's correct (web
        // does the same); height is where the double-count showed.
        assert!(
            frame.width >= content.width,
            "stretched frame must still fit its content",
        );

        // A container's padding stays Taffy's job — margins there would
        // double-count against the child frames Taffy already inset.
        let container = backend.create_view(&a11y);
        backend.apply_style(&container, &padded);
        assert_eq!(
            container.widget.margin_top(),
            0,
            "IdealystView padding is handled by Taffy, not GTK margins",
        );

        // --- 6d. An icon must have a default intrinsic size. `IdealystIcon`
        // is a bare GTK widget with no natural size, and the icon
        // primitive carries its dimensions in a viewBox rather than in
        // style — so with no measure fn Taffy laid EVERY icon out at
        // 0x0 and none of them appeared (the whiteboard demo's toolbar
        // was a row of blank buttons). 24x24 matches iOS/macOS.
        // Minimal valid glyph: a single closed path in a 24-unit viewBox.
        const ICON_FIXTURE: runtime_shared::primitives::icon::IconData =
            runtime_shared::primitives::icon::IconData {
                view_box: (24, 24),
                paths: &["M4 4 L20 4 L20 20 L4 20 Z"],
                fill_rule: runtime_shared::primitives::icon::FillRule::NonZero,
                filled: false,
            };
        let mut icon_root = backend.create_view(&a11y);
        let plain_icon = backend.create_icon(&ICON_FIXTURE, None, &a11y);
        backend.insert(&mut icon_root, plain_icon.clone());
        let icon_root_layout = backend.nodes.get(&icon_root.id).unwrap().layout;
        backend.layout.compute(icon_root_layout, 200.0, 200.0);
        let icon_frame = backend
            .layout
            .frame_of(backend.nodes.get(&plain_icon.id).unwrap().layout);
        // Height only: the column-flex parent stretches children on the
        // cross axis, so width is the container's, exactly as on web.
        // Height is where the 0x0 collapse showed.
        assert_eq!(
            icon_frame.height, 24.0,
            "an unstyled icon must fall back to a 24px intrinsic height, \
             not collapse to 0",
        );

        // An explicit size still wins — Taffy passes it as `known` and the
        // measure fn short-circuits to it.
        let sized_icon = backend.create_icon(&ICON_FIXTURE, None, &a11y);
        backend.insert(&mut icon_root, sized_icon.clone());
        backend.apply_style(
            &sized_icon,
            &std::rc::Rc::new(StyleRules {
                width: Some(runtime_shared::Tokenized::Literal(runtime_shared::Length::Px(16.0))),
                height: Some(runtime_shared::Tokenized::Literal(runtime_shared::Length::Px(16.0))),
                ..StyleRules::default()
            }),
        );
        backend.layout.compute(icon_root_layout, 200.0, 200.0);
        let sized_frame = backend
            .layout
            .frame_of(backend.nodes.get(&sized_icon.id).unwrap().layout);
        assert_eq!(
            (sized_frame.width, sized_frame.height),
            (16.0, 16.0),
            "an explicit width/height must override the 24x24 default",
        );

        // --- 7. `absolute_frame` must be WINDOW-relative, accumulating
        // ancestor offsets and subtracting scroll offsets. It used to
        // return the parent-relative frame, so the website's TOC
        // scroll-spy compared a section's offset-within-its-column
        // against the viewport and never tracked the scroll position.
        // (`row` is at y=100 inside `scroll`, `toc` at y=50 inside
        // `row`; the container is scrolled 400.)
        let abs = backend
            .node_absolute_frame(toc.id)
            .expect("absolute frame must resolve");
        assert_eq!(
            abs.1, -250.0,
            "absolute y = 50 + 100 - 400 scrolled; parent-relative (50) is the bug",
        );
        let parent_rel = backend.node_frame(toc.id).unwrap();
        assert_ne!(
            abs.1, parent_rel.1,
            "absolute_frame must not just echo the parent-relative frame",
        );

        // --- 8. `scroll_to` must actually move the container. The
        // framework installs `NoopScrollViewOps` unless the backend
        // provides a handle, so clicking a TOC entry silently did
        // nothing.
        use runtime_shared::primitives::scroll_view::ScrollViewOps;
        let sv_handle = backend.make_scroll_view_handle(&scroll);
        let _ = &sv_handle;
        let ops = crate::handles::scroll_view_ops_for_test();
        let state = crate::handles::handle_state_for_test(&backend, &scroll);
        ops.scroll_to(&state, 0.0, 250.0);
        assert_eq!(adj.value(), 250.0, "scroll_to must move the adjustment");

        // Beyond the end clamps to the last scrollable offset rather
        // than leaving the adjustment in an impossible state.
        adj.set_page_size(500.0);
        ops.scroll_to(&state, 0.0, 99_999.0);
        assert_eq!(adj.value(), 4500.0, "clamped to upper - page_size");

        // Sticky OUTSIDE any scroll container falls back to relative
        // (what CSS does) rather than registering a pin that can never
        // resolve.
        let orphan = backend.create_view(&a11y);
        backend.apply_style(&orphan, &sticky_style);
        backend.update_sticky(None, true);
        assert_eq!(
            backend.sticky_nodes.get(&orphan.id).unwrap().scroll,
            None,
            "no enclosing scroll_view - stays unresolved and is never pinned",
        );
    }
}
