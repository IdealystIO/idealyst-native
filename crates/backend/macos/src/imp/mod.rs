//! macOS backend implementation. AppKit-flavored.
//!
//! Mirrors the iOS backend's shape (Taffy-driven layout, per-view
//! layout-node map, intrinsic-size measurers) with AppKit primitives
//! and desktop idioms — cursor instead of touch, NSToolbar instead
//! of UINavigationController, NSSplitView for drawer-style layouts.
//!
//! See `docs/macos-backend-plan.md` for the design.

pub(crate) mod a11y;
pub(crate) mod animated;
pub(crate) mod border;
pub(crate) mod callbacks;
pub(crate) mod gradient;
pub(crate) mod graphics;
pub(crate) mod handles;
pub(crate) mod icon;
pub(crate) mod image;
pub(crate) mod keyboard;
pub(crate) mod node;
pub(crate) mod screenshot;
pub(crate) mod text_style;
pub(crate) mod transitions;
pub(crate) mod view;
pub(crate) mod virtualizer;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::{Backend, Color, StateBits, StyleRules};
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id, ClassType};
use objc2_app_kit::{NSColor, NSTextField, NSView};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSObject, NSString};

use backend_apple_core::color::parse_color;

pub use node::MacosNode;
pub use view::FlippedView;

// =========================================================================
// MacosBackend
// =========================================================================

pub struct MacosBackend {
    mtm: MainThreadMarker,
    host_root: Option<Retained<NSView>>,
    /// Parallel layout tree. Same shape as iOS — every NSView pointer
    /// maps to a Taffy node; `finish()` runs `compute(host_bounds)`
    /// and walks the map to assign `frame` on every registered view.
    pub(crate) layout: runtime_layout::LayoutTree,
    /// Map from view pointer → (retained NSView, layout node).
    /// Retained explicitly so views that aren't yet attached to the
    /// host (e.g. mid-build) survive the layout walk.
    pub(crate) view_to_layout:
        HashMap<usize, (Retained<NSView>, runtime_layout::LayoutNode)>,
    /// Process-registered custom fonts + per-`Typeface` lookup table.
    /// Same shape as iOS — driven by `register_asset` / `register_typeface`.
    /// Read by the text-style applier when constructing NSFont.
    pub(crate) font_registry: backend_apple_core::font::FontRegistry,
    /// Strong refs to retained Obj-C objects we own (callback targets,
    /// gesture recognizers, observers). Kept alive for the backend's
    /// lifetime so they outlive any closures they back.
    #[allow(dead_code)]
    callback_targets: Vec<Retained<NSObject>>,
    /// The app-level key-event monitor installed by `set_app_key_handler`
    /// (`NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskKeyDown`). The
    /// returned monitor object is retained here and passed to
    /// `NSEvent removeMonitor:` when the handler is replaced or cleared.
    app_key_monitor: Option<Retained<NSObject>>,
    /// Per-view cached animation state. Keyed by view pointer;
    /// holds the translate/scale/rotate components so writing one
    /// doesn't destroy the others (CALayer's `transform` is a single
    /// matrix). Mirrors the iOS `animated_states` map.
    pub(crate) animated_states: animated::AnimatedStateMap,
    /// Per-view cached gradient state. Keyed by view pointer; holds
    /// the gradient `CAGradientLayer` retained handle plus the
    /// current sRGB stop colors so `AnimProp::GradientStopColor(idx)`
    /// can rewrite a single stop without rebuilding the sublayer.
    pub(crate) gradient_states: HashMap<usize, gradient::GradientState>,
    /// Decoded `NSImage` cache keyed by `AssetId`. Populated by
    /// `register_asset` for `AssetTag::Image`; queried by
    /// `create_image` / `update_image_src` when the framework hands
    /// us an `asset://{id}` src.
    pub(crate) image_cache: image::ImageCache,
    /// Third-party `Element::External` registry. Populated by
    /// `register_external::<T>(...)` calls from per-platform leaf
    /// crates (e.g. a future `maps-macos::register`). `create_external`
    /// looks up the handler by payload TypeId; unregistered kinds
    /// fall through to a "not supported" placeholder NSTextField.
    /// Mirrors the iOS pattern.
    pub(crate) external_handlers: runtime_core::ExternalRegistry<MacosBackend>,
    /// Per-virtualizer side state. NSCollectionView's `dataSource`
    /// and `delegate` are weak refs, so the data source needs to
    /// outlive the collection view via this map. Keyed by the
    /// outer NSScrollView's pointer (the mountable node).
    /// `release_virtualizer` removes the entry; on map removal the
    /// data source's Retained drops + the underlying Objective-C
    /// release count goes to zero.
    pub(crate) virtualizer_instances:
        HashMap<usize, virtualizer::VirtualizerInstance>,
    /// Registry of `Element::Navigator` handler factories. SDK
    /// leaf crates (`stack_navigator::register`, `tab_navigator::
    /// register`, `drawer_navigator::register`, …) install factories
    /// keyed by their presentation TypeId at app bootstrap. The
    /// macOS handlers — once added to those SDKs — implement the
    /// single-window-with-sidebar shape per
    /// `project_macos_navigator_design`. Until those SDK leaves
    /// land, `create_navigator` falls through to a "kind not
    /// registered" placeholder text node.
    pub(crate) navigator_handlers:
        runtime_core::NavigatorRegistry<MacosBackend>,
    /// Per-navigator-instance handler. Mirrors iOS's
    /// `nav_handler_instances`. Keyed by the navigator container's
    /// NSView pointer. `create_navigator` resolves the factory,
    /// runs `init`, stashes the handler here so subsequent
    /// `navigator_attach_initial` / `release_navigator` /
    /// `apply_navigator_slot_style` trait methods can route through
    /// it.
    pub(crate) nav_handler_instances: HashMap<
        usize,
        std::rc::Rc<
            std::cell::RefCell<Box<dyn runtime_core::NavigatorHandler<MacosBackend>>>,
        >,
    >,
    /// screen_recorder `PrivateLayer` overlay windows, keyed by their
    /// content view's pointer → the borderless `NSWindow` that hosts it.
    /// Mirrors iOS's `detached_window_roots`. Two jobs:
    ///   1. Keeps the `NSWindow` retained for the overlay's lifetime (a
    ///      child window is otherwise only weakly held by its parent).
    ///   2. `insert` / `clear_children` consult it to SKIP the native
    ///      reparent: the External walker would otherwise yank the
    ///      content view out of its own (capture-excludable) window and
    ///      into the main recorded tree.
    /// The content view is also registered as a Taffy root sized to the
    /// window, so the layout pass lays the toolbar out inside it.
    pub(crate) detached_window_roots:
        HashMap<usize, Retained<objc2_app_kit::NSWindow>>,
    /// View-pointer keys of `create_portal` content views. A portal mounts
    /// itself into the host window's content view and is its OWN Taffy root, so
    /// (mirroring iOS's `portal_instances`) `insert` must SKIP the walker's
    /// attempt to reparent it into the surrounding tree — otherwise the overlay
    /// renders inline at the declaration site (e.g. a Modal landing at the bottom
    /// of a Settings list with no backdrop). The layout pass computes each against
    /// the viewport so a FullScreen portal fills the window.
    pub(crate) portal_roots: std::collections::HashSet<usize>,
    /// Layout node of the top-of-tree root, stashed by `finish` so a
    /// post-mount `schedule_layout_pass()` can recompute from it without
    /// the `root: Node` argument `finish` receives. `finish` runs exactly
    /// once at mount (the walker calls it after the build); reactive
    /// Effects that resize a node afterwards have no `root` to hand us, so
    /// we remember it here. `None` until the first `finish`.
    pub(crate) root_layout: Option<runtime_layout::LayoutNode>,
}

// =========================================================================
// Global self-handle — lets navigator/drawer dispatch closures
// schedule a layout pass after they mount new screens. Mirrors the
// iOS pattern; populated by `install_global_self` after the host
// wraps the backend in Rc<RefCell<>>.
// =========================================================================

/// Process-global registry of the `windowNumber`s of every live
/// `PrivateLayer` overlay `NSWindow`. The `screen-recorder` SDK's
/// ScreenCaptureKit capture path reads this (via [`private_layer_window_ids`])
/// to match each id against an `SCWindow.windowID` and exclude the overlay
/// from the recording — otherwise the recorded preview shows itself (a
/// feedback mirror).
///
/// A *process-global* `Mutex<Vec<i64>>` rather than a `MacosBackend` field
/// because the capture SDK is a separate crate that doesn't borrow the
/// backend; it needs a stand-alone accessor it can call from its async
/// `start`. `create_private_layer_window` inserts on build,
/// `release_private_layer_window` removes on teardown, so the set always
/// reflects the live overlays.
///
/// `windowNumber` is an `NSInteger` (isize); stored as `i64` to match
/// `SCWindow.windowID`'s `CGWindowID` domain when the SDK compares them.
static PRIVATE_LAYER_WINDOW_IDS: std::sync::Mutex<Vec<i64>> = std::sync::Mutex::new(Vec::new());

/// The `windowNumber`s of every live `PrivateLayer` overlay window. The
/// `screen-recorder` macOS capture backend passes these to
/// `SCContentFilter(excludingWindows:)` (matched against
/// `SCWindow.windowID`) so the overlay is omitted from the recording.
/// Returns an empty vec when no overlay is mounted.
pub fn private_layer_window_ids() -> Vec<i64> {
    PRIVATE_LAYER_WINDOW_IDS
        .lock()
        .map(|v| v.clone())
        .unwrap_or_default()
}

thread_local! {
    static MACOS_BACKEND_SELF: RefCell<Option<std::rc::Weak<RefCell<MacosBackend>>>> =
        const { RefCell::new(None) };

    /// Coalescing flag for [`schedule_layout_pass`]: set when a deferred
    /// layout pass is queued but not yet fired. A reactive batch that calls
    /// `apply_style` on many resized nodes posts ONE pass, not N — the flag
    /// is claimed by the first caller and cleared when the microtask runs.
    /// Mirrors iOS's `LAYOUT_PASS_QUEUED`.
    static LAYOUT_PASS_QUEUED: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };

    /// Profiling (§6): set `IDEALYST_LAYOUT_TRACE=1` to log every layout pass
    /// that actually runs (origin, duration, running totals) plus how many
    /// times the reactive-idle hook fired. The smoking gun for an O(N²)
    /// regression is many *real* passes (not early-returns) inside one
    /// interaction. Read once, cached — zero cost when unset.
    static LAYOUT_TRACE: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
    static PASS_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static IDLE_FIRE_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };

    /// Last viewport size mirrored into `runtime_core::set_viewport_size`, so
    /// `finish` schedules exactly one deferred mirror per actual change and
    /// none in the steady state (unchanged bounds every paint). See the long
    /// comment at the call site in `finish` for why the mirror is deferred.
    static LAST_MIRRORED_VIEWPORT: std::cell::Cell<Option<(f32, f32)>> =
        const { std::cell::Cell::new(None) };
}

/// Install the backend's self-reference. Hosts call this once after
/// wrapping the backend in `Rc<RefCell<>>` so navigator-side closures
/// can reach back into the backend without capturing it directly.
pub fn install_global_self(weak: std::rc::Weak<RefCell<MacosBackend>>) {
    MACOS_BACKEND_SELF.with(|s| {
        *s.borrow_mut() = Some(weak);
    });
    // Flush any coalesced layout pass synchronously when a reactive mutation
    // window closes — so views inserted by ANY reactive update (event, timer,
    // live-update push, hot-reload) are laid out before the turn paints, not
    // flashed at (0,0) and repositioned a frame later. The deferred
    // `schedule_layout_pass` microtask remains the fallback for the rare
    // change that isn't inside a reactive window. No-op when nothing's
    // pending, so it's cheap on every update.
    runtime_core::install_reactive_idle_hook(std::rc::Rc::new(|| {
        if layout_trace_enabled() {
            IDLE_FIRE_COUNT.with(|c| c.set(c.get() + 1));
        }
        flush_pending_layout_pass();
    }));
}

/// Push a scalar animation property update to `node` on the
/// installed global backend. The cross-platform animation system's
/// per-frame subscribers reach the macOS backend through this —
/// same shape as the iOS routing in `backend_ios_mobile`.
///
/// Quietly no-ops if no backend is installed yet (pre-render) or
/// the install has been dropped (post-teardown), or if the backend
/// is already borrowed (the in-flight Rust call will see the new
/// value on its next frame).
pub fn set_animated_f32(
    node: &MacosNode,
    prop: runtime_core::animation::AnimProp,
    value: f32,
) {
    let weak = MACOS_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    // Bind the try_borrow_mut result to a let so its temp drops
    // before `rc`. Without the binding, the if-let's scrutinee
    // outlives `rc` per the new borrow rules.
    let borrow = rc.try_borrow_mut();
    if let Ok(mut b) = borrow {
        use runtime_core::Backend;
        b.set_animated_f32(node, prop, value);
    }
}

/// Color-family counterpart of [`set_animated_f32`]. Routes through
/// the global backend's `set_animated_color`.
pub fn set_animated_color(
    node: &MacosNode,
    prop: runtime_core::animation::AnimProp,
    value: [f32; 4],
) {
    let weak = MACOS_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    let borrow = rc.try_borrow_mut();
    if let Ok(mut b) = borrow {
        use runtime_core::Backend;
        b.set_animated_color(node, prop, value);
    }
}

/// Re-enter the globally-installed backend from a deferred closure
/// (microtask, NSTimer callback, NotificationCenter observer, …).
/// Used by SDK leaves (drawer/stack/tab navigator handlers) whose
/// per-frame work needs `&mut MacosBackend` but runs outside the
/// framework's borrow window.
///
/// Quietly no-ops if no backend is installed (pre-render), if the
/// install has been dropped (post-teardown), or if the backend is
/// already borrowed (call site re-entered itself; the outer borrow
/// will complete its own work).
///
/// Mirrors `backend_terminal::with_global_backend` and
/// `backend_ios::with_backend`.
pub fn with_global_backend<F: FnOnce(&mut MacosBackend)>(f: F) {
    let weak = MACOS_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    let borrow = rc.try_borrow_mut();
    if let Ok(mut b) = borrow {
        f(&mut *b);
    }
}

/// Queue a coalesced, deferred layout pass on the globally-installed backend.
///
/// `Backend::finish` lays the tree out once, at mount. Reactive Effects that
/// resize a node afterwards (a `when`/reactive-style toggle growing a collapsed
/// `0×0` box to its real size — e.g. the whiteboard-demo recording preview)
/// push the new size into Taffy via `apply_style` but have no way to drive a
/// recompute; the NSView keeps its stale frame and the change is invisible.
/// This schedules `run_layout_pass_global` for the next main-loop turn so the
/// updated Taffy tree is committed to the view hierarchy.
///
/// Coalesced via `LAYOUT_PASS_QUEUED`: a reactive batch touching N nodes (or
/// inserting N rows) posts exactly one pass.
///
/// Two drains, and the difference is the whole story for flicker-free dynamic
/// updates:
///
/// 1. **[`flush_pending_layout_pass`] — synchronous, before paint.** The
///    macOS event dispatch (e.g. a Pressable mouse-up in `make_tap_handler`)
///    calls it right after the user handler returns — i.e. after the reactive
///    flush + all its `insert`s, but still inside the same run-loop turn,
///    BEFORE AppKit commits + displays. So freshly-inserted rows are sized
///    before they're ever painted. This is the path tree expand/collapse takes.
///
/// 2. **`schedule_microtask` — deferred, next turn.** The fallback for updates
///    NOT driven by a macOS event (timers, async, the inspector's own poll):
///    nothing calls `flush_pending_layout_pass` for those, so this guarantees
///    the pass still runs. `dispatch_async(main_queue)` fires next turn (one
///    frame after paint), which is why event-driven updates use path 1 — the
///    microtask alone would show the inserted rows at (0,0) for a frame.
///
/// Both drain the same coalescing flag, so whichever runs first wins and the
/// other no-ops — never a double pass. Deferring (vs. running inline here) is
/// also required because `with_global_backend`'s `try_borrow_mut` would bail
/// while the framework still holds the backend mid-`apply_style`/`insert`.
pub fn schedule_layout_pass() {
    let should_post = LAYOUT_PASS_QUEUED
        .with(|q| crate::layout_policy::claim_coalesced_pass(q));
    if !should_post {
        return;
    }
    runtime_core::schedule_microtask(|| {
        run_pending_layout_pass("microtask");
    });
}

/// Synchronously run the coalesced layout pass if one is queued, NOW. Called
/// from the macOS event dispatch after a user handler returns (see
/// `make_tap_handler`) so an insert triggered by that event is laid out before
/// the turn's paint — no "rows snap from the top" flicker. No-op when nothing
/// is queued (the common case for a click that didn't mutate the tree). Safe:
/// runs outside any `apply_style`/`insert` borrow (the handler has returned).
pub fn flush_pending_layout_pass() {
    run_pending_layout_pass("event-flush");
}

/// Cached read of `IDEALYST_LAYOUT_TRACE` (§6 profiling toggle). Returns
/// `false` unless the env var is set to a non-empty, non-`0` value.
fn layout_trace_enabled() -> bool {
    LAYOUT_TRACE.with(|c| match c.get() {
        Some(v) => v,
        None => {
            let v = std::env::var("IDEALYST_LAYOUT_TRACE")
                .map(|s| !s.is_empty() && s != "0")
                .unwrap_or(false);
            c.set(Some(v));
            v
        }
    })
}

/// Shared drain for both [`schedule_layout_pass`]'s microtask and
/// [`flush_pending_layout_pass`]. Clears the coalescing flag BEFORE running so
/// a `schedule_layout_pass` arriving during the pass re-arms and fires again
/// (post-layout state this pass couldn't capture). `catch_unwind` + `abort`
/// because the microtask path crosses libdispatch (C) where a Rust unwind is
/// UB; per the crash-loud policy we log + abort rather than continue half-
/// applied.
fn run_pending_layout_pass(origin: &str) {
    if !LAYOUT_PASS_QUEUED.with(|q| q.get()) {
        return;
    }
    LAYOUT_PASS_QUEUED.with(|q| crate::layout_policy::release_coalesced_pass(q));
    let trace = layout_trace_enabled();
    let started = if trace { Some(std::time::Instant::now()) } else { None };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_global_backend(|b| b.run_layout_pass_global());
    }));
    if let Some(started) = started {
        let n = PASS_COUNT.with(|c| {
            let n = c.get() + 1;
            c.set(n);
            n
        });
        let fires = IDLE_FIRE_COUNT.with(|c| c.get());
        eprintln!(
            "[layout-trace] pass #{n} ({origin}) {:.2}ms — idle-fires so far this run: {fires}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    if let Err(payload) = result {
        let msg = payload
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| payload.downcast_ref::<&'static str>().copied())
            .unwrap_or("<non-string panic payload>");
        eprintln!("[backend-macos] layout-pass {origin} panic: {msg}");
        std::process::abort();
    }
}

// =========================================================================
// Construction + host wiring
// =========================================================================

/// An inventory-collected external registrar. An SDK's macOS module
/// `inventory::submit!`s one of these (carrying a `fn(&mut MacosBackend)`);
/// `MacosBackend::new` drains them so the SDK self-registers its
/// `Element::External` handler without the app naming the concrete backend.
/// See [[project_inventory_self_registration]].
pub struct MacosExternalRegistrar(pub fn(&mut MacosBackend));
inventory::collect!(MacosExternalRegistrar);

/// Navigator analogue of [`MacosExternalRegistrar`]; a navigator SDK's macOS
/// module submits one so the app needn't call `<nav>::register` per platform.
/// See [[project_inventory_self_registration]].
pub struct MacosNavigatorRegistrar(pub fn(&mut MacosBackend));
inventory::collect!(MacosNavigatorRegistrar);

impl MacosBackend {
    /// Install every SDK-submitted external + navigator handler. Native
    /// (non-wasm) so inventory's link-time ctors populate the slices before
    /// construction.
    fn drain_self_registrars(&mut self) {
        for r in inventory::iter::<MacosExternalRegistrar> {
            (r.0)(self);
        }
        for r in inventory::iter::<MacosNavigatorRegistrar> {
            (r.0)(self);
        }
    }

    pub fn new(mtm: MainThreadMarker) -> Self {
        let mut backend = Self {
            mtm,
            host_root: None,
            layout: runtime_layout::LayoutTree::new(),
            view_to_layout: HashMap::new(),
            font_registry: backend_apple_core::font::FontRegistry::new(),
            callback_targets: Vec::new(),
            app_key_monitor: None,
            animated_states: HashMap::new(),
            gradient_states: HashMap::new(),
            image_cache: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
            virtualizer_instances: HashMap::new(),
            navigator_handlers: runtime_core::NavigatorRegistry::new(),
            nav_handler_instances: HashMap::new(),
            detached_window_roots: HashMap::new(),
            portal_roots: std::collections::HashSet::new(),
            root_layout: None,
        };
        backend.drain_self_registrars();
        backend
    }

    /// Register a `Element::Navigator` handler factory keyed by
    /// the presentation type `P`. SDK leaf crates call this once at
    /// bootstrap. Mirrors `IosBackend::register_navigator`.
    pub fn register_navigator<P, F>(&mut self, factory: F)
    where
        P: 'static,
        F: Fn() -> Box<dyn runtime_core::NavigatorHandler<MacosBackend>> + 'static,
    {
        self.navigator_handlers.register::<P, _>(factory);
    }

    /// Register a handler for the third-party external primitive whose
    /// payload type is `T`. Called by per-platform leaf crates (e.g.
    /// a future `maps_macos::register`) during app bootstrap. The
    /// handler receives the typed payload plus a mutable borrow of
    /// the backend and produces the `MacosNode` to mount.
    pub fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&std::rc::Rc<T>, &mut MacosBackend) -> MacosNode + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }

    /// `true` if a handler for payload type `T` has been registered.
    /// Useful for opt-in graceful degradation in user code (render a
    /// static fallback if the SDK isn't available on macOS).
    pub fn has_external<T: 'static>(&self) -> bool {
        self.external_handlers.has::<T>()
    }

    /// SDK extension helper: register an NSView (or subclass) with
    /// the backend's Taffy layout tree so flex parents can size +
    /// position it. Third-party `register_external` handlers call
    /// this once after constructing their native view so the layout
    /// pass picks it up. Without it, the view is laid out as 0×0.
    ///
    /// The view's `frame` is written by the layout pass — leaf
    /// widgets that don't need a custom measure function are fully
    /// serviced by this call alone. Mirrors
    /// `IosBackend::register_external_view`.
    pub fn register_external_view(&mut self, view: &NSView) {
        let _ = self.layout_for_view(view);
    }

    /// SDK extension helper for an external primitive whose own view has no
    /// intrinsic size (e.g. an `NSScrollView`) but whose `content` view does
    /// (e.g. the codeblock's text label). Installs a Taffy `measure_fn` on
    /// `node` driven by the content's natural size, so the wrapper fills its
    /// parent's offered width and scrolls content wider than that, and reports
    /// the content height. Without a measure a bare scroll view collapses to
    /// 0×0 in a flex column and the primitive renders blank. Mirrors
    /// `IosBackend::install_external_content_measure`.
    ///
    /// `pad` (points) is added on each axis to match a `contentInset` the
    /// handler draws inside the scroll view.
    pub fn install_external_content_measure(
        &mut self,
        node: &NSView,
        content: &NSView,
        pad: f32,
    ) {
        let layout = self.layout_for_view(node);
        let content: Retained<NSView> =
            unsafe { Retained::retain(content as *const NSView as *mut NSView).unwrap() };
        self.layout.set_measure_fn(
            layout,
            std::rc::Rc::new(move |known_dimensions, available_space| {
                // Probe the content's natural size. For a text control, ask its
                // cell at an effectively-unbounded width so multi-line code
                // reports the longest line + full height (single-axis scrollers
                // don't wrap); otherwise fall back to the view's fittingSize.
                let cell: *mut NSObject = unsafe { msg_send![&*content, cell] };
                let fit: CGSize = if cell.is_null() {
                    unsafe { msg_send![&*content, fittingSize] }
                } else {
                    let huge = CGRect {
                        origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
                        size: CGSize { width: 1.0e6, height: 1.0e6 },
                    };
                    unsafe { msg_send![cell, cellSizeForBounds: huge] }
                };
                let content_w = (fit.width as f32).max(0.0).ceil() + pad * 2.0;
                let content_h = (fit.height as f32).max(0.0).ceil() + pad * 2.0;
                let avail_w = match available_space.width {
                    runtime_layout::AvailableSpace::Definite(w) => Some(w),
                    _ => None,
                };
                runtime_layout::Size {
                    width: known_dimensions
                        .width
                        .unwrap_or_else(|| avail_w.unwrap_or(content_w)),
                    height: known_dimensions.height.unwrap_or(content_h),
                }
            }),
        );
    }

    /// `MainThreadMarker` accessor for third-party SDK extension code
    /// that needs to construct main-thread-only Obj-C objects. The
    /// marker is `Copy` so handing it out doesn't tie the SDK to the
    /// backend's borrow lifetime.
    pub fn mtm(&self) -> MainThreadMarker {
        self.mtm
    }

    /// Install the host's root NSView. Subsequent `finish()` calls
    /// compute layout against this view's bounds and apply frames
    /// down the registered tree.
    pub fn set_host_root(&mut self, view: Retained<NSView>) {
        // Attach a size-change observer so we re-run layout when the host's
        // bounds change (window resize, full-screen toggle, titlebar/toolbar
        // show-hide). Without it, `finish` lays out once at mount and a raw
        // window resize — which triggers no reactive render — leaves every
        // frame stale. The observer is a hidden, zero-impact subview pinned to
        // fill the host via its autoresizing mask; AppKit calls `setFrameSize:`
        // on it whenever the host resizes, which dispatches a coalesced layout
        // pass. Mirrors the iOS backend's `LayoutObserverView`.
        let bounds: CGRect = unsafe { msg_send![&view, bounds] };
        let observer = callbacks::LayoutObserverView::new(self.mtm, bounds.size);
        let _: () = unsafe { msg_send![&observer, setFrame: bounds] };
        // NSViewWidthSizable (2) | NSViewHeightSizable (16) = 0x12 — keep the
        // observer the same size as the host across every resize.
        let _: () = unsafe { msg_send![&observer, setAutoresizingMask: 0x12u64] };
        // Hidden → excluded from drawing AND hit-testing (so it never
        // intercepts a click), while still receiving autoresize `setFrameSize:`.
        let _: () = unsafe { msg_send![&observer, setHidden: true] };
        unsafe { view.addSubview(&observer) };
        // Retain alongside other backend-owned ObjC objects so the observer
        // outlives this scope (the host view keeps a strong ref too, but the
        // backend owning it matches how the other callback targets are held).
        let obj: Retained<NSObject> = unsafe {
            let ptr = Retained::as_ptr(&observer) as *mut NSObject;
            Retained::retain(ptr).unwrap()
        };
        self.callback_targets.push(obj);
        self.host_root = Some(view);
    }

    /// Borrow the host's root NSView. Third-party SDK extensions that
    /// need to reach the containing `NSWindow` (e.g. the `toolbar` SDK
    /// attaching an `NSToolbar` to window chrome) walk up via
    /// `host_root.window` — the contentView's `window` property is
    /// already set by the host's `setContentView:` before render
    /// starts, so this reaches the window even before
    /// `makeKeyAndOrderFront:`.
    pub fn host_root(&self) -> Option<&NSView> {
        self.host_root.as_deref()
    }

    /// Convenience for host crates: construct a flipped NSView the
    /// host can mount as its NSWindow contentView. The host then
    /// calls [`MacosBackend::set_host_root`] with the same view so
    /// the layout pass knows where to compute against.
    ///
    /// Using `FlippedView` here (top-left origin) means every
    /// subview placed via the layout pass lands in the same
    /// coordinate space the framework / Taffy emits — no per-frame
    /// Y inversion needed.
    pub fn create_host_root(&self) -> Option<Retained<NSView>> {
        let view = FlippedView::new(self.mtm);
        let view: Retained<NSView> = Retained::into_super(view);
        Some(view)
    }

    /// Get or create a layout node for an NSView. Called from every
    /// `create_*` method so each native view has a corresponding node
    /// in the layout tree.
    pub(crate) fn layout_for_view(&mut self, view: &NSView) -> runtime_layout::LayoutNode {
        let key = view as *const NSView as usize;
        if let Some((_, node)) = self.view_to_layout.get(&key) {
            return *node;
        }
        let node = self.layout.new_node();
        let retained = unsafe {
            Retained::retain(view as *const NSView as *mut NSView).expect("retain NSView")
        };
        self.view_to_layout.insert(key, (retained, node));
        node
    }

    /// Look up an existing layout node by view pointer. Returns
    /// `None` for views that weren't created by this backend.
    #[allow(dead_code)]
    pub(crate) fn layout_of(&self, view: &NSView) -> Option<runtime_layout::LayoutNode> {
        let key = view as *const NSView as usize;
        self.view_to_layout.get(&key).map(|(_, n)| *n)
    }

    /// Install a Taffy `measure_fn` for an `NSImageView` so flex
    /// layout reads its `intrinsicContentSize` (driven by the
    /// assigned `NSImage.size`) instead of collapsing to 0×0.
    /// Re-installable — `update_image_src` calls this again after
    /// swapping the image so a new bitmap's size is picked up.
    ///
    /// Mirrors `IosBackend::install_image_measure` per the
    /// `project_ios_intrinsic_size_measurer` memory.
    pub(crate) fn install_image_measure(&mut self, view: &Retained<NSView>) {
        let layout = self.layout_for_view(view);
        let view_for_measure = view.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&view_for_measure, intrinsicContentSize] };
                // `intrinsicContentSize` returns `{-1, -1}`
                // (NSViewNoIntrinsicMetric) when no image is set or
                // for views without a natural size. Clamp negative
                // dimensions to 0 so Taffy doesn't try to size a
                // slot against an impossible value.
                let w = (intrinsic.width as f32).max(0.0);
                let h = (intrinsic.height as f32).max(0.0);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );
    }

    /// Build the screen_recorder `PrivateLayer` overlay: a separate,
    /// borderless `NSWindow` pinned above the app's window whose content view
    /// is a recursive-passthrough `NSView`. The macOS analogue of
    /// `IosBackend::create_private_layer_window`.
    ///
    /// ## Why a child window
    ///
    /// On iOS the trick is a second non-key `UIWindow` at a high
    /// `windowLevel` (ReplayKit records only the key window). On macOS the
    /// equivalent is a borderless **child** window added via
    /// `addChildWindow:ordered:Above`: it tracks the parent window's moves +
    /// Space changes for free and composites above it, so the toolbar stays
    /// pinned over the app. ScreenCaptureKit exclusion is a *separate*
    /// `SCContentFilter(excludingWindows:)` step (a later task); this method
    /// only has to make the overlay show + be interactive, and REGISTER the
    /// window so that future task can find it via
    /// [`Self::private_layer_windows`].
    ///
    /// ## Layout
    ///
    /// The content view is registered in `view_to_layout` as a Taffy ROOT and
    /// the `finish` layout pass computes every detached root at its window's
    /// content size, so the content view fills the overlay and the author
    /// positions the layer's controls inside it with normal flex/absolute
    /// style — exactly like iOS.
    ///
    /// ## Passthrough
    ///
    /// The content view is a [`callbacks::PrivateLayerPassthroughView`]: its
    /// `hitTest:` returns `nil` everywhere except over a real control or a
    /// painted (non-clear) view, so clicks that miss the layer fall through to
    /// the app window beneath — the app stays interactive (e.g. the canvas
    /// stays drawable) everywhere except where the toolbar sits.
    pub fn create_private_layer_window(&mut self) -> MacosNode {
        let mtm = self.mtm;

        // Resolve the host NSWindow + its content size. The host's
        // `setContentView:` runs before render, so `host_root.window` is
        // non-nil here (same guarantee the `toolbar` SDK relies on). We size
        // the overlay to the host's content view bounds so the Taffy root that
        // lays out the toolbar matches the app's drawable area.
        let host_window: Option<Retained<objc2_app_kit::NSWindow>> =
            self.host_root().and_then(|root| {
                let win_ptr: *mut objc2_app_kit::NSWindow =
                    unsafe { msg_send![root, window] };
                if win_ptr.is_null() {
                    None
                } else {
                    unsafe { Retained::retain(win_ptr) }
                }
            });

        // Content rect in SCREEN coordinates (NSWindow init wants screen
        // coords). Fall back to the host content view's frame size at a
        // best-effort origin; if there's no host window the overlay still
        // builds (it just never shows) so the External never blanks.
        let content_rect: CGRect = match &host_window {
            Some(win) => {
                // `contentView.frame` in window base coords → convert to
                // screen via the window's `convertRectToScreen:`-style
                // `frame`/content inset. Simplest robust path: the window's
                // `contentRectForFrameRect:` against its on-screen frame.
                let win_frame: CGRect = unsafe { msg_send![win, frame] };
                let content_rect: CGRect =
                    unsafe { msg_send![win, contentRectForFrameRect: win_frame] };
                content_rect
            }
            None => CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: 800.0, height: 600.0 },
            },
        };

        // The borderless overlay window. Borderless + clear background +
        // non-opaque + no shadow so the app shows through the passthrough
        // regions. `defer: false` so the window backing is created up front
        // (it's about to be shown as a child window).
        let window: Retained<objc2_app_kit::NSWindow> = unsafe {
            let alloc = mtm.alloc::<objc2_app_kit::NSWindow>();
            objc2_app_kit::NSWindow::initWithContentRect_styleMask_backing_defer(
                alloc,
                content_rect,
                objc2_app_kit::NSWindowStyleMask::Borderless,
                objc2_app_kit::NSBackingStoreType::NSBackingStoreBuffered,
                false,
            )
        };
        let clear = unsafe { NSColor::clearColor() };
        window.setBackgroundColor(Some(&clear));
        window.setOpaque(false);
        window.setHasShadow(false);
        // Don't pull it into AppKit's "release on close" pool — we own the
        // `Retained` in `detached_window_roots` and release it ourselves.
        unsafe { window.setReleasedWhenClosed(false) };

        // Belt-and-suspenders capture exclusion: `NSWindowSharingNone` tells
        // the window server this window's contents may not be read by another
        // process / capture path. ScreenCaptureKit's `SCContentFilter`
        // exclusion (driven by `screen-recorder`) is the primary mechanism;
        // setting `sharingType` here means even a non-SCK capture (legacy
        // `CGWindowListCreateImage`, AirPlay mirroring) skips the overlay too.
        window.setSharingType(objc2_app_kit::NSWindowSharingType::NSWindowSharingNone);

        // Content view = recursive-passthrough flipped NSView, sized to the
        // window's content rect. `setContentView:` makes AppKit own its
        // layout/resize; the Taffy pass additionally drives its frame.
        let content: Retained<NSView> = {
            let v = callbacks::PrivateLayerPassthroughView::new(mtm);
            Retained::into_super(v)
        };
        let local_bounds = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: content_rect.size,
        };
        let _: () = unsafe { msg_send![&content, setFrame: local_bounds] };
        // flexibleWidth (2) | flexibleHeight (16) so it tracks the window even
        // before the next Taffy pass writes a frame.
        let _: () = unsafe { msg_send![&content, setAutoresizingMask: 0x12u64] };
        window.setContentView(Some(&content));

        // Pin above the host window as a child window (tracks the parent's
        // moves + Spaces; composites above it). If there's no host window we
        // still order it onto the screen so the toolbar is at least visible.
        match &host_window {
            Some(host) => unsafe {
                host.addChildWindow_ordered(
                    &window,
                    objc2_app_kit::NSWindowOrderingMode::NSWindowAbove,
                );
            },
            None => {
                window.orderFront(None);
            }
        }

        // Register the content view as a Taffy root + detached window root so
        // the layout pass sizes it and `insert` skips its reparent. The
        // window is retained on the entry so it lives as long as the layer.
        self.register_external_view(&content);
        let key = &*content as *const NSView as usize;
        // Record the overlay's `windowNumber` in the process-global registry so
        // the `screen-recorder` capture backend can exclude it from a
        // ScreenCaptureKit recording. Read here (after the window is fully
        // built + ordered onto the screen) so `windowNumber` is assigned — an
        // off-screen NSWindow reports 0 until it acquires a backing window
        // number, which `addChildWindow:`/`orderFront:` above guarantees.
        let window_number: isize = unsafe { window.windowNumber() };
        if let Ok(mut ids) = PRIVATE_LAYER_WINDOW_IDS.lock() {
            ids.push(window_number as i64);
        }
        self.detached_window_roots.insert(key, window);

        // Lay the new detached root out immediately against the window size —
        // it never enters the main tree, so `finish`'s host-root compute won't
        // touch it unless we kick a pass. The host re-invokes `finish` after
        // the External's children are inserted; that pass (see the detached-
        // root loop in `finish`) recomputes it at the right size.
        self.layout_detached_root(key, content_rect.size);

        MacosNode::View(content)
    }

    /// Tear down a `PrivateLayer` overlay created by
    /// [`Self::create_private_layer_window`]. Removes it from the registries,
    /// orders it off the screen, and drops the Taffy node so the next layout
    /// pass doesn't lay out a detached subtree. Mirrors
    /// `IosBackend::release_private_layer_window`.
    pub fn release_private_layer_window(&mut self, node: &MacosNode) {
        let key = node.view_key();
        let Some(window) = self.detached_window_roots.remove(&key) else {
            return;
        };
        // Drop this overlay's id from the process-global capture-exclusion
        // registry so a subsequent recording doesn't try to exclude a window
        // number that no longer exists.
        let window_number: isize = unsafe { window.windowNumber() };
        if let Ok(mut ids) = PRIVATE_LAYER_WINDOW_IDS.lock() {
            ids.retain(|&id| id != window_number as i64);
        }
        // Detach from the parent so AppKit stops tracking it, then order out.
        let parent: Option<Retained<objc2_app_kit::NSWindow>> =
            unsafe { window.parentWindow() };
        if let Some(parent) = parent {
            unsafe { parent.removeChildWindow(&window) };
        }
        window.orderOut(None);
        if let Some((_, layout_node)) = self.view_to_layout.remove(&key) {
            self.layout.remove_node(layout_node);
        }
    }

    /// Compute layout for one detached (private-layer) root against `size` and
    /// apply the resulting frames to the views inside that subtree. Detached
    /// roots never enter the main tree, so the host-root `finish` pass doesn't
    /// reach them; `finish` calls this for each registered detached root, and
    /// `create_private_layer_window` calls it once on creation.
    pub(crate) fn layout_detached_root(&mut self, root_key: usize, size: CGSize) {
        let w = size.width as f32;
        let h = size.height as f32;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let Some(root_layout) = self.view_to_layout.get(&root_key).map(|(_, n)| *n) else {
            return;
        };
        self.layout.compute(root_layout, w, h);

        // Apply frames to every view in this detached subtree. Collect the
        // Taffy nodes reachable from the root (BFS over `children_of`), then
        // write each corresponding registered view's frame — the same per-view
        // frame-write the host pass does, scoped to the detached tree so we
        // don't disturb the main (recorded) tree's frames or transforms.
        let mut subtree: std::collections::HashSet<runtime_layout::LayoutNode> =
            std::collections::HashSet::new();
        let mut stack = vec![root_layout];
        while let Some(n) = stack.pop() {
            if subtree.insert(n) {
                stack.extend(self.layout.children_of(n));
            }
        }
        let snapshot: Vec<(usize, runtime_layout::LayoutNode)> = self
            .view_to_layout
            .iter()
            .filter(|(_, (_, n))| subtree.contains(n))
            .map(|(k, (_, n))| (*k, *n))
            .collect();
        // Suppress implicit CALayer actions for the apply-frames pass (see the
        // host-tree pass in `compute_and_apply_layout` for the rationale).
        unsafe {
            let _: () = msg_send![objc2::class!(CATransaction), begin];
            let _: () = msg_send![objc2::class!(CATransaction), setDisableActions: true];
        }
        for (key, layout_node) in snapshot {
            let frame = self.layout.frame_of(layout_node);
            let Some((view, _)) = self.view_to_layout.get(&key) else {
                continue;
            };
            let rect = CGRect {
                origin: CGPoint { x: frame.x as f64, y: frame.y as f64 },
                size: CGSize {
                    width: frame.width as f64,
                    height: frame.height as f64,
                },
            };
            let _: () = unsafe { msg_send![&**view, setFrame: rect] };
            gradient::sync_gradient_sublayer(view);
            icon::sync_icon_sublayer(view, frame.width as f64, frame.height as f64);
        }
        unsafe {
            let _: () = msg_send![objc2::class!(CATransaction), commit];
        }
    }

    /// The registered `PrivateLayer` overlay `NSWindow`s. The future
    /// ScreenCaptureKit capture path passes these to
    /// `SCContentFilter(excludingWindows:)` so the overlay is omitted from the
    /// recording (the macOS analogue of iOS's non-key-window exclusion).
    pub fn private_layer_windows(&self) -> Vec<Retained<objc2_app_kit::NSWindow>> {
        self.detached_window_roots.values().cloned().collect()
    }
}

// =========================================================================
// Helpers
// =========================================================================

/// Opaque wrapper for CoreGraphics' `CGColorRef` so `msg_send!`'s
/// debug-mode encoding check sees `^{CGColor=}` instead of `^v`.
/// Mirrors the iOS-side wrapper in `backend-ios-core/src/style.rs`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct CGColorRef(pub *const std::ffi::c_void);

unsafe impl Encode for CGColorRef {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
}

/// Build an `NSColor` from a framework `Color`. Uses the shared
/// apple-core color parser (sRGB float tuple) then routes through
/// AppKit's RGB initializer.
/// Inert `NavigatorOps` for `make_navigator_handle` callers that
/// land on a navigator container with no registered handler. The
/// trait is empty, so the inert impl is just a marker; the handle
/// constructed against it ignores all dispatch attempts.
struct NoopNavOps;
impl runtime_core::primitives::navigator::NavigatorOps for NoopNavOps {}
static NOOP_NAV_OPS: NoopNavOps = NoopNavOps;

/// Maximum pointer travel (in points, window space) between mouse-down and
/// mouse-up for the gesture to still count as a tap. A press that drags farther
/// is a scroll/drag and must NOT fire the click — matches the cancel behavior of
/// iOS's `UITapGestureRecognizer`.
const TAP_SLOP_PT: f32 = 10.0;

/// Build a tap-detecting [`TouchHandler`] that fires `on_click` on a mouse-up
/// which stayed within [`TAP_SLOP_PT`] of the mouse-down. macOS pressables /
/// links have no native AppKit control to lean on (NSButton is used only for
/// `create_button`), so the `FlippedView` `mouseDown`→handler path is the click
/// delivery mechanism. Consuming every phase keeps the gesture on this view for
/// the whole down→up sequence; a drag beyond slop disarms the tap so dragging a
/// finger off a link doesn't navigate.
fn make_tap_handler(on_click: Rc<dyn Fn()>) -> runtime_core::TouchHandler {
    use runtime_core::{TouchEvent, TouchPhase, TouchResponse};
    use std::cell::Cell;
    let start = Rc::new(Cell::new((0.0f32, 0.0f32)));
    let armed = Rc::new(Cell::new(false));
    Rc::new(move |ev: &TouchEvent| match ev.phase {
        TouchPhase::Began => {
            start.set((ev.window_position.x, ev.window_position.y));
            armed.set(true);
            TouchResponse::CONSUMED
        }
        TouchPhase::Moved => {
            let (sx, sy) = start.get();
            let (dx, dy) = (ev.window_position.x - sx, ev.window_position.y - sy);
            if (dx * dx + dy * dy).sqrt() > TAP_SLOP_PT {
                armed.set(false);
            }
            TouchResponse::CONSUMED
        }
        TouchPhase::Ended => {
            if armed.replace(false) {
                (on_click)();
                // The click handler's signal mutations close a reactive window,
                // which fires the reactive-idle hook (installed in
                // `install_global_self`) → `flush_pending_layout_pass()`. So any
                // inserts it triggered are laid out before this turn paints,
                // with no per-event flush needed here.
            }
            TouchResponse::CONSUMED
        }
        TouchPhase::Cancelled => {
            armed.set(false);
            TouchResponse::CONSUMED
        }
    })
}

/// `true` if `view` is an `NSScrollView`. Used by `insert` to
/// redirect children into the scroll view's documentView so
/// the scroll machinery actually works. Implemented via
/// `isKindOfClass:` so subclasses (rare but possible) are also
/// recognized.
fn is_scroll_view(view: &NSView) -> bool {
    let cls = match objc2::runtime::AnyClass::get("NSScrollView") {
        Some(c) => c,
        None => return false,
    };
    unsafe { msg_send![view, isKindOfClass: cls] }
}

pub(crate) fn color_to_nscolor(color: &Color) -> Retained<NSColor> {
    let (r, g, b, a) = parse_color(&color.0);
    unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a) }
}

/// Resolve a `Color` to sRGB `[r, g, b, a]` floats in 0..=1 — the form the
/// transition tween interpolates. Same parse path as `color_to_nscolor`.
pub(crate) fn style_color_rgba(color: &Color) -> [f32; 4] {
    let (r, g, b, a) = parse_color(&color.0);
    [r as f32, g as f32, b as f32, a as f32]
}

/// Well-known theme token for body text color. The framework theme crate
/// (`idea-theme`) installs `color-text` in every variant's token table
/// (`#1a1a1f` light / `#e8eaf0` dark), and idea-ui's `Typography` resolves the
/// same token for its `color`. Using this name here means an UNSTYLED `text()`
/// resolves to the *theme's* text color through the identical
/// `Tokenized<Color>::resolve()` path a styled token goes through — uniform with
/// web/iOS/Android, NOT the OS system label color.
pub(crate) const THEME_TEXT_TOKEN: &str = "color-text";

/// Fallback used when the `color-text` token isn't installed yet (theme not
/// installed, or an external placeholder rendered before mount). It is the
/// framework light theme's text color — a near-black that's legible on the
/// default light surface. CRUCIAL that this is a real dark color and NOT a
/// system-appearance color: the whole point of defaulting raw `text()` to the
/// theme is that a light-theme app must not render white text just because the
/// user's macOS is in dark mode.
const THEME_TEXT_FALLBACK: &str = "#1a1a1f";

/// Resolve the editable text control's effective BACKGROUND `Color` through the
/// shared, host-tested `backend_apple_core::text_control_style` decision and the
/// same `Tokenized<Color>::resolve()` machinery a styled `background:` token
/// uses: explicit author background wins, else the theme's `color-surface`
/// token (NOT AppKit's system text-control fill, dark in dark mode). This is
/// what stops the idea-ui `Textarea` rendering as a near-black box. iOS shares
/// the same decision module (CLAUDE.md §7).
pub(crate) fn input_background_color(
    explicit: Option<&runtime_core::Tokenized<Color>>,
) -> Color {
    backend_apple_core::text_control_style::effective_input_background(explicit).resolve()
}

/// Resolve the editable text control's effective TEXT `Color`: explicit author
/// color wins, else the theme's `color-text` token (NOT the OS system label
/// color, white in dark mode). Shared decision; mirrors `theme_text_color`'s
/// fallback by design.
pub(crate) fn input_text_color(
    explicit: Option<&runtime_core::Tokenized<Color>>,
) -> Color {
    backend_apple_core::text_control_style::effective_input_text_color(explicit).resolve()
}

/// True when `view` is an AppKit editable text control (NSTextField — which
/// includes the NSSecureTextField password subclass — or NSTextView). These
/// paint their own background + text through AppKit (NOT the CALayer
/// `backgroundColor` that `apply_style_to_view` set), so `apply_style` must
/// mirror the author's / theme's colors onto the AppKit-level properties.
fn is_editable_text_control(view: &NSView) -> bool {
    for name in ["NSTextField", "NSTextView"] {
        if let Some(cls) = objc2::runtime::AnyClass::get(name) {
            let is: bool = unsafe { msg_send![view, isKindOfClass: cls] };
            if is {
                return true;
            }
        }
    }
    false
}

/// The installed theme's body-text `Color`, resolved through the SAME token
/// machinery a styled `color:` token resolves through (`Tokenized<Color>::
/// resolve()` against the `color-text` token). Returns the framework light
/// theme's text color when no theme is installed yet.
///
/// Used by `create_text` to give a raw `text()` node a default color so it
/// doesn't inherit `NSColor.labelColor` (the OS system label color — WHITE in
/// dark mode), which would make a light-theme app's text invisible on a
/// dark-appearance Mac. An EXPLICIT `style.color` set later in `apply_style`
/// still wins (that path only writes when `style.color.is_some()`).
pub(crate) fn theme_text_color() -> Color {
    runtime_core::Tokenized::Token {
        name: THEME_TEXT_TOKEN,
        fallback: Color(THEME_TEXT_FALLBACK.to_string()),
    }
    .resolve()
}

/// Apply the framework's `StyleRules` to an NSView's CALayer-backed
/// presentation. Minimum viable set: background color, opacity,
/// corner radius. More properties (gradients, shadows, borders) will
/// follow the iOS shape.
fn apply_style_to_view(view: &NSView, style: &StyleRules) {
    // Force CALayer backing so `backgroundColor`, `cornerRadius`,
    // etc. work. NSView is layer-optional by default; we want the
    // same CALayer-backed presentation iOS / Android have so styling
    // is uniform.
    let _: () = unsafe { msg_send![view, setWantsLayer: true] };
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };

    // Background color → layer's `backgroundColor`, via the transition system
    // (animates over `background_transition` if set, else snaps). Scroll views
    // paint their background through AppKit `drawsBackground`, NOT the layer, so
    // they're handled in `apply_style`'s scroll branch instead — skip here.
    if let Some(bg) = &style.background {
        if !is_scroll_view(view) {
            let rgba = style_color_rgba(&bg.resolve());
            transitions::apply_color(
                view,
                transitions::ColorProp::Background,
                false,
                rgba,
                style.background_transition.as_ref(),
            );
        }
    }

    // Gradient background is handled by the caller (`apply_style`)
    // because the returned `GradientState` lives in the backend's
    // per-view cache — `apply_style_to_view` doesn't have access.

    // Opacity. Use `NSView.setAlphaValue:` (matches iOS's
    // `UIView.setAlpha:`), NOT `CALayer.setOpacity:`. The two
    // multiply in AppKit's rendering pipeline, so routing static
    // opacity through the layer while animated opacity goes through
    // the view (`AnimProp::Opacity` → `setAlphaValue:`) would make
    // any author who set both static AND animated opacity get
    // (static * animated) — visibly broken whenever the static
    // value is < 1. Welcome's planets hit this case exactly: the
    // sheet declares `opacity: 0.0` as the resting state and the
    // raf body animates `setAlphaValue:` up to `fade_in`. Pre-fix
    // the planets stayed invisible because the layer opacity
    // stayed at 0 regardless of `setAlphaValue:`. Both backends
    // now route static + animated through the same field.
    if let Some(opacity) = style.opacity.as_ref().map(|t| t.resolve()) {
        let _: () = unsafe { msg_send![view, setAlphaValue: opacity as f64] };
    }

    // Corner radius. Same caveat as iOS: AppKit's `cornerRadius`
    // isn't clamped to `min(W, H)/2`, so the CSS-idiomatic
    // `border-radius: 999px` renders nothing on a small box. We
    // clamp to half the smaller explicit dimension when available;
    // percent/auto dims defer to the layout pass.
    let radius = [
        style.border_top_left_radius.as_ref(),
        style.border_top_right_radius.as_ref(),
        style.border_bottom_left_radius.as_ref(),
        style.border_bottom_right_radius.as_ref(),
    ]
    .iter()
    .filter_map(|r| r.map(|t| length_to_px(&t.resolve())))
    .fold(0.0_f64, f64::max);
    if radius > 0.0 {
        fn px_half(t: &runtime_core::Tokenized<runtime_core::Length>) -> Option<f64> {
            match t.resolve() {
                runtime_core::Length::Px(v) => Some(v as f64 / 2.0),
                _ => None,
            }
        }
        let half_w = style.width.as_ref().and_then(px_half);
        let half_h = style.height.as_ref().and_then(px_half);
        let cap = match (half_w, half_h) {
            (Some(w), Some(h)) => Some(w.min(h)),
            (Some(w), None) => Some(w),
            (None, Some(h)) => Some(h),
            (None, None) => None,
        };
        match cap {
            Some(c) => {
                let effective = radius.min(c);
                let _: () = unsafe { msg_send![&layer, setCornerRadius: effective] };
            }
            None => {
                // Defer clamping to the layout pass — without explicit
                // px dimensions, we can't know `min(W, H)/2` until
                // Taffy assigns a frame. Stash the requested value as
                // an associated `NSNumber` on the layer; the layout
                // pass reads it back in `sync_corner_radius` and
                // clamps against the real bounds. Same pattern iOS
                // uses; see [[project_ios_cornerradius_unclamped]].
                //
                // We set cornerRadius=0 in the meantime so the layer
                // doesn't render the broken "999 on tiny view → blank"
                // state during the first paint before layout lands.
                let key = NSString::from_str("idealyst_requested_corner_radius");
                let number: Retained<NSObject> = unsafe {
                    msg_send_id![objc2::class!(NSNumber), numberWithDouble: radius]
                };
                let _: () = unsafe { msg_send![&layer, setValue: &*number, forKey: &*key] };
                let _: () = unsafe { msg_send![&layer, setCornerRadius: 0.0_f64] };
            }
        }
        // AppKit's NSView has no `setMasksToBounds:` (that's UIView).
        // Equivalent is `layer.masksToBounds = true` on the CALayer
        // directly. Required so cornerRadius clips child content,
        // matching the UIView `clipsToBounds` behavior.
        let _: () = unsafe { msg_send![&layer, setMasksToBounds: true] };
    }

    // Border. Routes uniform→CALayer stroke (follows cornerRadius) /
    // asymmetric→per-side NSView bars (e.g. a `border-bottom`-only
    // underline) / none→clear — the SAME shape as iOS, via the shared
    // `backend_apple_core::border::uniform_border` decision so the two
    // backends converge (Rule #7). The macOS backend previously applied NO
    // border at all, so every bordered component (Card outlines, the
    // SegmentedControl/Tabs active underline, Field borders, the whiteboard
    // swatch ring) rendered borderless on macOS while iOS/web showed it.
    border::apply_border(view, &layer, style);

    // Overflow → CALayer `masksToBounds` (AppKit's equivalent of UIView's
    // `clipsToBounds`). Mirrors iOS `style.rs` so the backends converge (Rule
    // #7): a plain `overflow: Hidden` view clips its children on macOS too. The
    // macOS backend previously honored overflow ONLY on scroll views, so a
    // styled non-scroll view silently never clipped (iOS/Android/web did). When
    // overflow is unset we leave `masksToBounds` as the cornerRadius branch above
    // decided. Clipping is a layer mask — it does NOT detach a child's hosted
    // `CAMetalLayer`; the view is already layer-backed (`setWantsLayer` above) and
    // the GPU surface keeps presenting, clipped to bounds.
    if let Some(clip) = overflow_masks_to_bounds(style) {
        let _: () = unsafe { msg_send![&layer, setMasksToBounds: clip] };
    }

    // Interaction (desktop affordances; touch backends no-op these). Mirrors
    // the web `cursor` / `user-select` CSS so the backends converge (Rule #7).
    //
    // Cursor: only our `FlippedView` host (view / pressable / link) carries the
    // cursor-rect override — native controls keep their own system cursor (an
    // `NSTextField` shows the iBeam). `Some(Auto)` clears any prior cursor; an
    // unset `cursor` leaves whatever a previous apply installed.
    if let Some(c) = style.cursor {
        if let Some(fv) = as_flipped_view(view) {
            fv.set_cursor(view::cursor_for(c));
        }
    }

    // Text selection: macOS controls it per text widget via `setSelectable:`
    // (NSTextField / NSTextView respond; other views ignore it). `Auto` leaves
    // the widget default — labels are already non-selectable, so a button's
    // text isn't selectable without any opt-in, matching `user-select: none`
    // on web.
    if let Some(u) = style.user_select {
        use runtime_core::UserSelect;
        let selectable = match u {
            UserSelect::None => Some(false),
            UserSelect::Text | UserSelect::All => Some(true),
            UserSelect::Auto => None,
        };
        if let Some(sel) = selectable {
            let responds: bool =
                unsafe { msg_send![view, respondsToSelector: objc2::sel!(setSelectable:)] };
            if responds {
                let _: () = unsafe { msg_send![view, setSelectable: sel] };
            }
        }
    }
}

/// Downcast an `&NSView` to `&FlippedView` when its dynamic class is our
/// `IdealystFlippedView` (the host for `view` / `pressable` / `link`).
/// Returns `None` for native controls (`NSTextField`, `NSSwitch`, …). Mirrors
/// the class-check + pointer-cast `install_touch_handler` already uses.
fn as_flipped_view(view: &NSView) -> Option<&FlippedView> {
    let is_flipped: bool =
        unsafe { msg_send![view, isKindOfClass: objc2::class!(IdealystFlippedView)] };
    if is_flipped {
        // SAFETY: just confirmed the dynamic class is `IdealystFlippedView`.
        Some(unsafe { &*(view as *const NSView as *const FlippedView) })
    } else {
        None
    }
}

/// The `masksToBounds` value an explicit `overflow` requests: `Hidden` clips,
/// `Visible` doesn't, and an unset overflow returns `None` so the caller leaves
/// whatever the cornerRadius branch decided. Pure so it's host-testable — a full
/// NSView mount needs a main thread + live layer (see `imp::icon` tests), so the
/// clip *decision* is the closest reachable guard for this branch.
fn overflow_masks_to_bounds(style: &StyleRules) -> Option<bool> {
    style
        .overflow
        .as_ref()
        .map(|o| matches!(o, runtime_core::Overflow::Hidden))
}

#[cfg(test)]
mod overflow_tests {
    use super::overflow_masks_to_bounds;
    use runtime_core::{Overflow, StyleRules};

    // Regression: macOS used to ignore `overflow` on non-scroll views entirely,
    // so a styled `overflow: Hidden` parent never clipped its children (iOS /
    // Android / web all did). `apply_style_to_view` now routes the decision here.
    #[test]
    fn overflow_decides_masks_to_bounds() {
        assert_eq!(
            overflow_masks_to_bounds(&StyleRules { overflow: Some(Overflow::Hidden), ..Default::default() }),
            Some(true),
            "overflow: Hidden must clip (masksToBounds = true)",
        );
        assert_eq!(
            overflow_masks_to_bounds(&StyleRules { overflow: Some(Overflow::Visible), ..Default::default() }),
            Some(false),
            "overflow: Visible must not clip",
        );
        assert_eq!(
            overflow_masks_to_bounds(&StyleRules::default()),
            None,
            "unset overflow leaves the cornerRadius decision untouched",
        );
    }
}

/// The Taffy style a `create_portal` container view gets, from its target. The
/// container is computed as an orphan root against the viewport (Auto axes fill
/// to the window), so this only carries the flex justify/align that positions the
/// single content child within that frame. Mirrors iOS
/// `portal::container_style_for_placement` / `container_style_for_anchor` (Rule
/// #7 — keep the two backends' portal placement identical). Pure → host-testable.
fn portal_container_style(target: &runtime_core::primitives::portal::PortalTarget) -> StyleRules {
    use runtime_core::primitives::portal::{PortalTarget, ViewportPlacement};
    use runtime_core::{AlignItems, FlexDirection, JustifyContent};

    let mut rules = StyleRules { flex_direction: Some(FlexDirection::Column), ..Default::default() };
    let placement = match target {
        PortalTarget::Viewport(p) => *p,
        // Anchored/named portals span the viewport with neutral flex; the content
        // positions itself (absolute insets). FullScreen is the closest neutral.
        _ => ViewportPlacement::FullScreen,
    };
    match placement {
        ViewportPlacement::Center => {
            rules.justify_content = Some(JustifyContent::Center);
            rules.align_items = Some(AlignItems::Center);
        }
        ViewportPlacement::Top => {
            rules.justify_content = Some(JustifyContent::FlexStart);
            rules.align_items = Some(AlignItems::Stretch);
        }
        ViewportPlacement::Bottom => {
            rules.justify_content = Some(JustifyContent::FlexEnd);
            rules.align_items = Some(AlignItems::Stretch);
        }
        ViewportPlacement::Left => {
            rules.justify_content = Some(JustifyContent::FlexStart);
            rules.align_items = Some(AlignItems::FlexStart);
        }
        ViewportPlacement::Right => {
            rules.justify_content = Some(JustifyContent::FlexStart);
            rules.align_items = Some(AlignItems::FlexEnd);
        }
        ViewportPlacement::FullScreen => {
            rules.justify_content = Some(JustifyContent::FlexStart);
            rules.align_items = Some(AlignItems::Stretch);
        }
    }
    rules
}

#[cfg(test)]
mod portal_tests {
    use super::portal_container_style;
    use runtime_core::primitives::portal::{PortalTarget, ViewportPlacement};
    use runtime_core::{AlignItems, JustifyContent};

    // Regression: a `FullScreen` portal (idea-ui `Modal`) rendered inline at the
    // bottom of its screen on macOS — `insert` reparented it into the main tree
    // instead of keeping it an escaped, viewport-sized root. The container style
    // must stretch its child to fill the window so the Modal's own centering
    // container can center the card. (The escape itself — the `portal_roots`
    // `insert` skip + per-root layout — needs a live AppKit window to verify; this
    // pins the placement style, mirroring iOS.)
    #[test]
    fn fullscreen_portal_stretches_its_child() {
        let s = portal_container_style(&PortalTarget::Viewport(ViewportPlacement::FullScreen));
        assert_eq!(s.align_items, Some(AlignItems::Stretch));
        assert_eq!(s.justify_content, Some(JustifyContent::FlexStart));
    }

    #[test]
    fn center_portal_centers_its_child() {
        let s = portal_container_style(&PortalTarget::Viewport(ViewportPlacement::Center));
        assert_eq!(s.align_items, Some(AlignItems::Center));
        assert_eq!(s.justify_content, Some(JustifyContent::Center));
    }
}

/// Resolve a deferred cornerRadius (stashed as `idealyst_requested_
/// corner_radius` on the layer by [`apply_style_to_view`] when the
/// view's dimensions weren't known at apply-style time) against the
/// view's now-laid-out bounds. Mirrors `backend_ios_core::style::
/// sync_corner_radius`.
fn sync_corner_radius(view: &NSView) {
    // NSView is layer-optional on AppKit. A view without a layer
    // can't have a stashed `idealyst_requested_corner_radius` either
    // (the apply-style path that writes the key calls `setWantsLayer`
    // first), so the sync is a no-op for unlayered views. Use raw
    // `msg_send!` + null-check instead of `msg_send_id!` (which
    // asserts non-nil and panics on the unlayered case — hit by
    // every external-primitive placeholder and any root NSView that
    // hasn't gone through `apply_style_to_view` yet).
    let layer_ptr: *mut NSObject = unsafe { msg_send![view, layer] };
    if layer_ptr.is_null() {
        return;
    }
    let layer: &NSObject = unsafe { &*layer_ptr };
    let key = NSString::from_str("idealyst_requested_corner_radius");
    let value_ptr: *mut NSObject = unsafe { msg_send![layer, valueForKey: &*key] };
    if value_ptr.is_null() {
        return;
    }
    let value: &NSObject = unsafe { &*value_ptr };
    let requested: f64 = unsafe { msg_send![value, doubleValue] };
    if requested <= 0.0 {
        return;
    }
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    let half_w = bounds.size.width / 2.0;
    let half_h = bounds.size.height / 2.0;
    let cap = half_w.min(half_h);
    let effective = requested.min(cap.max(0.0));
    let _: () = unsafe { msg_send![layer, setCornerRadius: effective] };
}

fn length_to_px(len: &runtime_core::Length) -> CGFloat {
    match len {
        runtime_core::Length::Px(v) => *v as CGFloat,
        runtime_core::Length::Percent(_) | runtime_core::Length::Auto => 0.0,
    }
}

// =========================================================================
// Backend trait implementation
// =========================================================================

// =========================================================================
// Layout pass — shared mount + deferred post-mount recompute. Inherent (not
// trait) methods so the trait impl below stays a contiguous block.
// =========================================================================

impl MacosBackend {
    /// Resize every `NSScrollView`'s documentView to its content's bounding
    /// box so AppKit can scroll. The framework parents a scroll view's children
    /// under the OUTER scroll-view Taffy node (see `insert`), so the layout pass
    /// gives each child a real frame relative to the scroll view's content
    /// origin — but Taffy never positions the inner documentView (it has no
    /// Taffy parent). Without sizing it here the documentView stays 0×0 and
    /// clips every laid-out child to nothing. We walk each scroll view's Taffy
    /// descendants (not just direct children — a `min_height: 100%` inner
    /// container clamps to the clip bounds while a Spacer-pushed footer
    /// overflows past it), accumulate the bounding box, and set the documentView
    /// frame to it. Mirrors the iOS backend's `contentSize` sync.
    fn sync_scroll_document_views(&mut self) {
        // Snapshot scroll views first so we don't hold a `view_to_layout`
        // borrow across the per-view frame writes.
        let scrolls: Vec<(Retained<NSView>, runtime_layout::LayoutNode)> = self
            .view_to_layout
            .values()
            .filter(|(v, _)| is_scroll_view(v))
            .map(|(v, n)| (v.clone(), *n))
            .collect();
        for (scroll_view, scroll_layout) in scrolls {
            // Bounding box across the scroll view's Taffy descendants. The
            // walk + projection lives in `layout_policy` so `cargo test` pins
            // it (the AppKit `setFrame:` below needs the main thread + a live
            // window). See `scroll_content_bbox` for the deep-descendant
            // rationale.
            let layout = &self.layout;
            let roots = layout.children_of(scroll_layout);
            let (max_x, max_y) = crate::layout_policy::scroll_content_bbox(
                &roots,
                |n| {
                    let f = layout.frame_of(n);
                    (f.x, f.y, f.width, f.height)
                },
                |n| layout.children_of(n),
            );
            // The documentView fills at least the clip view so short content
            // still paints edge-to-edge; it grows past that to enable scroll.
            // The clamp itself lives in `layout_policy` so it's host-testable —
            // it's the piece that makes a TOP-LEVEL scroll view paint (a
            // window-filling scroll view with short content has a bbox smaller
            // than the viewport; without the clip clamp the documentView would
            // be content-tall and the rest of the window a blank gap).
            let clip: CGRect = unsafe { msg_send![&*scroll_view, bounds] };
            let clip_size = (clip.size.width as f32, clip.size.height as f32);
            let (content_w, content_h) =
                crate::layout_policy::scroll_document_view_size((max_x, max_y), clip_size);
            let doc_ptr: *mut NSView = unsafe { msg_send![&*scroll_view, documentView] };
            if doc_ptr.is_null() {
                continue;
            }
            // Guard: a documentView that lands 0×0 while the scroll view has
            // real, laid-out children (and a real clip) IS the "top-level scroll
            // page renders blank" regression. Warn loudly at the exact pass that
            // caused it so it never again presents as an inexplicably empty
            // window. `scroll_document_view_size` only yields a degenerate size
            // when the clip is also degenerate (pre-first-paint), which the
            // guard excludes — so this fires only on a genuine regression.
            if crate::layout_policy::scroll_document_view_is_degenerate(
                (max_x, max_y),
                (content_w, content_h),
                clip_size,
            ) {
                backend_apple_core::log::apple_log(
                    "[macos] WARNING: scroll view documentView sized 0×0 with \
                     non-empty children — content will render BLANK. This is the \
                     top-level-scroll-blank regression; check scroll_content_bbox \
                     and the apply-frames order in compute_and_apply_layout.",
                );
            }
            let rect = CGRect {
                origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: content_w as f64, height: content_h as f64 },
            };
            let _: () = unsafe { msg_send![doc_ptr, setFrame: rect] };
            // Commit the programmatic documentView resize to the scroll
            // machinery. AppKit only recomputes the clip view's relationship to
            // the documentView (scroller knobs, contentView bounds, the
            // documentView's visible placement) when the scroll view is told its
            // content changed — `setFrame:` on the documentView alone doesn't
            // trigger that. Without this, a documentView resized AFTER the scroll
            // view's own `tile` (which is exactly our order: apply-frames sets the
            // scroll view frame, then we resize the documentView) can keep stale
            // clip geometry until the first user scroll nudges it. `clipView`
            // (contentView) is the argument AppKit's own scrollers pass.
            let clip_view_ptr: *mut NSObject =
                unsafe { msg_send![&*scroll_view, contentView] };
            if !clip_view_ptr.is_null() {
                let _: () =
                    unsafe { msg_send![&*scroll_view, reflectScrolledClipView: clip_view_ptr] };
            }
        }
    }

    /// Recompute Taffy from `root_layout` against `(width, height)` and commit
    /// every registered view's frame (plus gradient/corner-radius/transform
    /// post-layout sync), then lay out the detached private-layer roots.
    ///
    /// Shared by `finish` (the one-time mount pass) and `run_layout_pass_global`
    /// (the deferred post-mount pass scheduled by reactive `apply_style`). Pure
    /// view-tree commit — the caller owns viewport derivation, root parenting,
    /// and the reactive-viewport mirror.
    pub(crate) fn compute_and_apply_layout(
        &mut self,
        root_layout: runtime_layout::LayoutNode,
        width: f32,
        height: f32,
    ) {
        self.layout.compute(root_layout, width, height);

        // Apply frames to every registered view. We don't recurse
        // through `NSView.subviews` because some views may not yet
        // be attached at finish time (matches the iOS rationale).
        let snapshot: Vec<(usize, runtime_layout::LayoutNode)> = self
            .view_to_layout
            .iter()
            .map(|(k, (_, n))| (*k, *n))
            .collect();
        // Disable Core Animation's implicit actions for the whole apply-frames
        // pass: a layout-driven `setFrame:` on a layer-backed view otherwise
        // eases to its new origin over ~0.25s, gliding a beat behind sibling
        // GPU content (e.g. the media selection ring lagging the dragged image).
        // Explicit animations (the animation system's `addAnimation`) are
        // unaffected — this only suppresses the default implicit ones.
        unsafe {
            let _: () = msg_send![objc2::class!(CATransaction), begin];
            let _: () = msg_send![objc2::class!(CATransaction), setDisableActions: true];
        }
        for (key, layout_node) in snapshot {
            let frame = self.layout.frame_of(layout_node);
            // The map's retained NSView is the source of truth for
            // applying the frame — we keep that strong ref so the
            // view is alive while we touch it.
            let Some((view, _)) = self.view_to_layout.get(&key) else {
                continue;
            };
            // Static percent translates (`transform: translate(50%, -50%)`
            // from style rules) are applied as frame-origin offsets
            // here rather than via layer.transform. Layer-backed
            // NSViews don't honor pure-static transform translates
            // reliably; baking them into the frame is unambiguous.
            // See `animated::static_translate_offset` for details.
            let (static_tx, static_ty) = animated::static_translate_offset(
                view,
                &self.animated_states,
                frame.width,
                frame.height,
            );
            let rect = CGRect {
                origin: objc2_foundation::CGPoint {
                    x: frame.x as f64 + static_tx,
                    y: frame.y as f64 + static_ty,
                },
                size: CGSize {
                    width: frame.width as f64,
                    height: frame.height as f64,
                },
            };
            let _: () = unsafe { msg_send![&**view, setFrame: rect] };
            // Sync any gradient sublayer to the new bounds. CALayer
            // autoresizingMask doesn't drive automatic sublayer
            // resizing in practice — mirror the resize here. No-op
            // when the view has no `idealyst_gradient` sublayer.
            gradient::sync_gradient_sublayer(view);
            // Scale + center an icon's glyph sublayer to the new box so
            // `Icon(size = N)` renders at N px (the path is baked at 24).
            // No-op when the view has no `idealyst_icon` sublayer.
            icon::sync_icon_sublayer(view, frame.width as f64, frame.height as f64);
            // Re-apply a deferred cornerRadius (stashed when apply_style
            // ran before bounds were known). Clamps to half the
            // smaller bound — required so `border-radius: 999px` on a
            // percent-sized view ("make it a circle") actually
            // renders instead of blanking the layer. No-op when no
            // deferred radius was stashed.
            sync_corner_radius(view);
            // Re-resolve any percent transforms against the new
            // bounds. The sun glare's wrapper uses `translate(50%,
            // -50%)` to offset itself off-screen; that resolves to
            // 0 at apply-style time (bounds 0×0) and only becomes
            // correct here, post-layout. No-op for views with
            // identity transforms.
            animated::sync_transform_after_layout(view, &self.animated_states);
            // Feed any `on_layout` subscribers (a `.container()` view's
            // inline-size signal) with this view's resolved size. The
            // callbacks change-guard, so re-firing at an unchanged width
            // is a no-op — which is what keeps the container-query
            // restyle→relayout loop convergent.
            handles::fire_layout_for_view(
                handles::view_key(&**view),
                frame.width,
                frame.height,
            );
        }
        unsafe {
            let _: () = msg_send![objc2::class!(CATransaction), commit];
        }

        // (Scroll documentView sizing is deferred to the END of this method — it
        // must run AFTER the detached + portal subtrees are laid out, or a scroll
        // view inside a portal (e.g. an idea-ui Modal's body) gets a documentView
        // sized against stale/uncomputed frames and its content is clipped /
        // un-hit-testable. See the `sync_scroll_document_views` call below.)

        // Keep every detached (screen_recorder private-layer) overlay window
        // covering the host's content area BEFORE we recompute its contents.
        // `addChildWindow:ordered:` tracks the host's moves (origin) but not
        // its size, so on a window resize the overlay would keep its old size
        // and the toolbar inside it would lay out against stale bounds. Rewrite
        // each overlay's frame to the host's current content rect (screen
        // coords) when the size drifts — gated by `detached_overlay_needs_resize`
        // so we don't fight AppKit's per-frame child-window move tracking.
        if !self.detached_window_roots.is_empty() {
            if let Some(host_content) = self.host_content_rect_screen() {
                let target = (host_content.size.width as f32, host_content.size.height as f32);
                for window in self.detached_window_roots.values() {
                    let cur: CGRect = unsafe { msg_send![&**window, frame] };
                    let current = (cur.size.width as f32, cur.size.height as f32);
                    if crate::layout_policy::detached_overlay_needs_resize(current, target) {
                        // `display: true` so the resized backing repaints this
                        // pass rather than on the next event-loop turn.
                        let _: () = unsafe {
                            msg_send![&**window, setFrame: host_content, display: true]
                        };
                    }
                }
            }
        }

        // Lay out every detached private-layer root. These live in their own
        // borderless overlay windows and never enter the host tree, so the
        // host-root compute above doesn't reach them — but the External walker
        // has by now inserted their children (toolbar, preview). Compute each
        // against its overlay window's (now host-synced) content size so the
        // toolbar fills + positions correctly inside its window.
        let detached: Vec<(usize, CGSize)> = self
            .detached_window_roots
            .iter()
            .map(|(key, window)| {
                let frame: CGRect = unsafe { msg_send![&**window, frame] };
                let content: CGRect =
                    unsafe { msg_send![&**window, contentRectForFrameRect: frame] };
                (*key, content.size)
            })
            .collect();
        for (key, size) in detached {
            self.layout_detached_root(key, size);
        }

        // Lay out each portal container against the FULL viewport. A portal is
        // its own orphan Taffy root (not reachable from the host root computed
        // above; `insert` keeps it escaped via `portal_roots`), so it needs an
        // explicit compute — `layout_detached_root` computes it against the given
        // size (Auto axes fill to the viewport) and frames its subtree. Mirrors
        // iOS, where `run_layout_pass_global` computes every Taffy root against
        // the viewport. Without this the portal subtree kept a 0/stale frame.
        if !self.portal_roots.is_empty() {
            let portal_size = CGSize { width: width as f64, height: height as f64 };
            let portal_keys: Vec<usize> = self.portal_roots.iter().copied().collect();
            for key in portal_keys {
                self.layout_detached_root(key, portal_size);
            }
        }

        // Size every NSScrollView's documentView to its content's bounding box —
        // LAST, so it sees the final frames of EVERY subtree (main, detached, and
        // portal). The framework parents a scroll view's children under the OUTER
        // scroll-view Taffy node (mirroring iOS's single-UIScrollView model), so
        // the layout passes above gave each child a real frame; the inner
        // documentView is a native-only container Taffy never positions, so
        // without sizing it here it stays 0×0 and clips its children to nothing.
        // Critically it must run after the portal pass: a scroll view inside a
        // portal (an idea-ui Modal's scrollable body) would otherwise be sized
        // against stale frames and its content — including the action buttons —
        // would be clipped and un-hit-testable (the "modal renders but nothing is
        // pressable" bug). Mirrors iOS's `contentSize` sync.
        self.sync_scroll_document_views();
    }

    /// The host window's current content rect in SCREEN coordinates, or `None`
    /// before the host root is attached to a window. Used to keep the
    /// private-layer overlay child windows sized to the app's drawable area
    /// across host resizes (`addChildWindow:` only tracks moves, not size).
    fn host_content_rect_screen(&self) -> Option<CGRect> {
        let host = self.host_root.as_ref()?;
        let win_ptr: *mut objc2_app_kit::NSWindow = unsafe { msg_send![&**host, window] };
        if win_ptr.is_null() {
            return None;
        }
        let frame: CGRect = unsafe { msg_send![win_ptr, frame] };
        let content: CGRect = unsafe { msg_send![win_ptr, contentRectForFrameRect: frame] };
        Some(content)
    }

    /// Deferred post-mount layout pass: recompute from the stashed root against
    /// the host's *current* bounds and re-commit frames. Scheduled (coalesced)
    /// by [`schedule_layout_pass`] when a reactive `apply_style` resizes an
    /// already-mounted node — the case `finish` (mount-only) never revisits.
    /// Bails quietly before the first `finish` (no root yet) or when the host
    /// has no usable bounds.
    pub(crate) fn run_layout_pass_global(&mut self) {
        let Some(root_layout) = self.root_layout else {
            return;
        };
        let Some(host) = self.host_root.clone() else {
            return;
        };
        let mut bounds: CGRect = unsafe { msg_send![&host, bounds] };
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            let frame: CGRect = unsafe { msg_send![&host, frame] };
            bounds = frame;
        }
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return;
        }
        self.compute_and_apply_layout(
            root_layout,
            bounds.size.width as f32,
            bounds.size.height as f32,
        );
    }

    /// Run a layout pass **synchronously** — compute from the stashed root and
    /// commit frames now, instead of the coalesced microtask that
    /// [`schedule_layout_pass`] posts. A navigator screen-swap calls this right
    /// after inserting the incoming screen so it is laid out BEFORE the next
    /// paint; without it the freshly-inserted subtree shows for one runloop turn
    /// unsized (the navigation "flash"/delay). Safe to call from inside a
    /// `with_global_backend` block — it only needs `&mut self`.
    pub fn run_layout_pass_now(&mut self) {
        self.run_layout_pass_global();
    }
}

/// Lets SDKs register an `Element::External` handler via the generic
/// `register<B: RegisterExternal>(b)` entry without naming `MacosBackend` —
/// the same path web/ssr expose. Forwards to the same `external_handlers`
/// registry as the inherent [`MacosBackend::register_external`], so an explicit
/// call (e.g. `canvas_vello::register`) overrides an inventory-registered
/// handler for the same payload type (last-registration-wins). Mirrors
/// `impl RegisterExternal for WebBackend`.
impl runtime_core::RegisterExternal for MacosBackend {
    fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&std::rc::Rc<T>, &mut MacosBackend) -> MacosNode + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }
}

impl Backend for MacosBackend {
    type Node = MacosNode;

    /// Navigator abstraction calls this after every command (see the trait doc).
    fn schedule_layout_pass() {
        crate::imp::schedule_layout_pass();
    }

    fn platform(&self) -> runtime_core::Platform {
        runtime_core::Platform::MacOs
    }

    /// Theme the host surface behind the rendered tree — the AppKit window's
    /// content area. On web the page `<body>` shows through a background-less
    /// root; an AppKit window instead clears to the system window background
    /// (dark in dark mode), so a root view with no `background` leaves the
    /// window dark. The theme SDK routes the theme's `color-background` token
    /// here (see `idea-theme`'s `apply_host_surface_from_tokens`); we paint the
    /// host root's CALayer with it so a bg-less root defaults to the theme
    /// background, matching web. Native backends apply `color.value()` directly
    /// (no `var(--…)` indirection), and the SDK re-calls this on theme swap so
    /// the window re-resolves. No-op before the host root is installed — the
    /// next call (every swap re-invokes) repaints once it exists.
    fn set_app_background(&mut self, color: &runtime_core::Tokenized<runtime_core::Color>) {
        let Some(host) = self.host_root.as_ref() else {
            return;
        };
        // `.value()` is the resolved literal/fallback — the form native backends
        // apply directly (the same `Tokenized` value a styled `background` token
        // resolves to).
        //
        // Paint the host **NSWindow's** `backgroundColor`, NOT the host root
        // view's layer. Forcing `setWantsLayer:true` on the host root (the old
        // impl) makes AppKit give the root an implicit CALayer and re-host its
        // subtree — which DETACHES a child `CAMetalLayer` (the vello GPU canvas)
        // from the render server, blanking the canvas. The window's content
        // background sits behind the (layer-optional, bg-less) root view and
        // shows through exactly the same, without touching any view's layer.
        // See [[project_macos_appkit_uikit_diffs]] #21.
        //
        // `host_root.window` is non-nil after `setContentView:` (the same
        // guarantee `create_private_layer_window` + the toolbar SDK rely on);
        // no-op before then — every theme swap re-invokes once it exists.
        let ns_color = color_to_nscolor(color.value());
        unsafe {
            let win_ptr: *mut objc2_app_kit::NSWindow = msg_send![&**host, window];
            if win_ptr.is_null() {
                return;
            }
            let _: () = msg_send![win_ptr, setBackgroundColor: &*ns_color];
        }
    }

    fn set_app_key_handler(&mut self, handler: Option<runtime_core::primitives::key::KeyDownHandler>) {
        keyboard::set_app_key_handler(self, handler);
    }

    fn supports_screenshot(&self) -> bool {
        // Capability, not current state: AppKit can always rasterize a
        // view hierarchy. A capture before the host root is installed
        // (or before first layout) returns an error rather than failing
        // this gate — see `capture_screenshot`.
        true
    }

    fn capture_screenshot(
        &self,
        done: Box<dyn FnOnce(Result<runtime_core::Screenshot, String>)>,
    ) {
        let result = match self.host_root.as_ref() {
            Some(view) => screenshot::capture(view),
            None => Err("no host root installed yet".into()),
        };
        done(result);
    }

    fn url_opener(&self) -> Option<std::rc::Rc<dyn Fn(&str)>> {
        Some(std::rc::Rc::new(|url: &str| {
            // NSWorkspace.sharedWorkspace.openURL: hands the URL to the
            // user's default handler (browser, mail client, …). Raw
            // msg_send + class!() to avoid pulling NSWorkspace/NSURL
            // typed bindings — same style as `color_scheme` below. The
            // autoreleased NSURL is consumed by `openURL:` immediately,
            // within the same autorelease scope.
            let ns_url_str = NSString::from_str(url);
            let url_obj: *mut NSObject =
                unsafe { msg_send![objc2::class!(NSURL), URLWithString: &*ns_url_str] };
            if url_obj.is_null() {
                return;
            }
            let workspace: *mut NSObject =
                unsafe { msg_send![objc2::class!(NSWorkspace), sharedWorkspace] };
            if workspace.is_null() {
                return;
            }
            let _: bool = unsafe { msg_send![workspace, openURL: url_obj] };
        }))
    }

    fn fullscreen_setter(&self) -> Option<std::rc::Rc<dyn Fn(bool)>> {
        Some(std::rc::Rc::new(|enabled: bool| {
            // NSApp.mainWindow.toggleFullScreen: — raw msg_send + class!()
            // to avoid pulling typed NSApplication/NSWindow bindings,
            // matching `url_opener` / `color_scheme`. `toggleFullScreen:`
            // FLIPS state, so read the current state first
            // (NSWindowStyleMaskFullScreen = 1 << 14) and only toggle when
            // it differs from the requested one — keeps `set_fullscreen`
            // idempotent.
            unsafe {
                let app: *mut NSObject =
                    msg_send![objc2::class!(NSApplication), sharedApplication];
                if app.is_null() {
                    return;
                }
                let window: *mut NSObject = msg_send![app, mainWindow];
                if window.is_null() {
                    return;
                }
                let style_mask: usize = msg_send![window, styleMask];
                let is_fullscreen = (style_mask & (1 << 14)) != 0;
                if is_fullscreen != enabled {
                    let _: () =
                        msg_send![window, toggleFullScreen: std::ptr::null::<NSObject>()];
                }
            }
        }))
    }

    fn color_scheme(&self) -> runtime_core::ColorScheme {
        // Read the *application's* effective appearance, NOT
        // `NSAppearance.currentAppearance`: the latter is a per-draw thread-local
        // that is `nil` outside a drawing pass (e.g. at mount, when the app reads
        // `color_scheme()` to pick its initial theme), so it would always report
        // `Auto`. `NSApp.effectiveAppearance` reflects the system Dark Mode at any
        // time. Map its name to light/dark.
        let appearance: *const NSObject = unsafe {
            let app_cls = objc2::class!(NSApplication);
            let app: *const NSObject = msg_send![app_cls, sharedApplication];
            if app.is_null() {
                // No app yet — fall back to the drawing-context appearance.
                let cls = objc2::class!(NSAppearance);
                msg_send![cls, currentAppearance]
            } else {
                msg_send![app, effectiveAppearance]
            }
        };
        if appearance.is_null() {
            return runtime_core::ColorScheme::Auto;
        }
        let name_ptr: *const NSString = unsafe { msg_send![appearance, name] };
        if name_ptr.is_null() {
            return runtime_core::ColorScheme::Auto;
        }
        let s = unsafe { (*name_ptr).to_string() };
        if s.contains("Dark") {
            runtime_core::ColorScheme::Dark
        } else if s.contains("Aqua") {
            runtime_core::ColorScheme::Light
        } else {
            runtime_core::ColorScheme::Auto
        }
    }

    fn create_view(&mut self, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        let view = FlippedView::new(self.mtm);
        let view: Retained<NSView> = Retained::into_super(view);
        let _ = self.layout_for_view(&view);
        let node = MacosNode::View(view);
        // View has no default a11y role (transparent container); pass
        // None and let `a11y::apply` skip role writing entirely when
        // author code hasn't supplied one.
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::View,
            ),
        );
        node
    }

    fn install_touch_handler(
        &mut self,
        node: &Self::Node,
        handler: runtime_core::TouchHandler,
    ) {
        // Every `create_view` mints a `FlippedView`, which translates
        // `mouseDown/Dragged/Up` into the handler (see `view.rs`). Other node
        // kinds (NSTextField labels, native controls) don't carry an `on_touch`
        // slot, so the walker never calls us with them.
        let MacosNode::View(view) = node else {
            return;
        };
        let cls = objc2::class!(IdealystFlippedView);
        let is_flipped: bool = unsafe { msg_send![&**view, isKindOfClass: cls] };
        if !is_flipped {
            return;
        }
        // SAFETY: just confirmed the dynamic class is `IdealystFlippedView`;
        // its layout is `NSView` extended with our ivars, ABI-compatible here.
        let flipped: &FlippedView = unsafe { &*(Retained::as_ptr(view) as *const FlippedView) };
        flipped.set_handler(handler);
    }

    fn install_wheel_handler(
        &mut self,
        node: &Self::Node,
        handler: runtime_core::WheelHandler,
    ) {
        // Same FlippedView path as `install_touch_handler`: the view's
        // `magnify:` / `scrollWheel:` overrides route to this handler.
        let MacosNode::View(view) = node else {
            return;
        };
        let cls = objc2::class!(IdealystFlippedView);
        let is_flipped: bool = unsafe { msg_send![&**view, isKindOfClass: cls] };
        if !is_flipped {
            return;
        }
        // SAFETY: dynamic class confirmed `IdealystFlippedView`; layout is
        // `NSView` + our ivars, ABI-compatible here.
        let flipped: &FlippedView = unsafe { &*(Retained::as_ptr(view) as *const FlippedView) };
        flipped.set_wheel_handler(handler);
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // The trait-default `create_pressable` drops `on_click` (it just calls
        // `create_view`), so Pressable-backed controls (idea-ui Button, Tabs, …)
        // never fired on macOS. Mirror iOS: a tappable view wired to the click.
        // We reuse the FlippedView `on_touch` path (`mouseDown`→`Began`,
        // `mouseUp`→`Ended`) rather than an NSClickGestureRecognizer so no extra
        // AppKit feature is needed and the responder semantics match every other
        // framework view. (Labels are hit-transparent — see `IdealystLabel` — so
        // a click on the visible text reaches this view.)
        let flipped = FlippedView::new(self.mtm);
        flipped.set_handler(make_tap_handler(on_click));
        let view: Retained<NSView> = Retained::into_super(flipped);
        let _ = self.layout_for_view(&view);
        let node = MacosNode::View(view);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::Button));
        node
    }

    fn create_link(
        &mut self,
        config: runtime_core::primitives::link::LinkConfig,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Same tappable-view wiring as `create_pressable`; navigation lives in
        // `config.on_activate` (the framework's Link primitive turns it into the
        // right `NavCommand` — `Select` for the drawer). The trait default drops
        // it, so sidebar / in-body links never navigated on macOS.
        let flipped = FlippedView::new(self.mtm);
        flipped.set_handler(make_tap_handler(config.on_activate));
        let view: Retained<NSView> = Retained::into_super(flipped);
        let _ = self.layout_for_view(&view);
        let node = MacosNode::View(view);
        // Default the a11y label to the route / URL when the author gave none,
        // matching iOS so VoiceOver announces the link target.
        let resolved_label = a11y.label.clone().unwrap_or_else(|| {
            if config.external {
                config.url.clone()
            } else {
                config.route.to_string()
            }
        });
        let effective_a11y = runtime_core::accessibility::AccessibilityProps {
            label: Some(resolved_label),
            ..a11y.clone()
        };
        a11y::apply(
            &node,
            &effective_a11y,
            Some(runtime_core::accessibility::Role::Link),
        );
        node
    }

    fn create_text(&mut self, content: &str, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        // NSTextField in label mode is AppKit's UILabel equivalent.
        // `+[NSTextField labelWithString:]` configures it as
        // non-editable, non-selectable, no border, no background.
        let ns = NSString::from_str(content);
        // Use the hit-transparent `IdealystLabel` subclass (not plain
        // NSTextField) so a label sitting inside a Link / Pressable doesn't
        // swallow the click (and so scroll-wheel events under the text still
        // reach the enclosing NSScrollView). See `view::IdealystLabel`.
        let label: Retained<NSTextField> =
            crate::imp::view::IdealystLabel::label_with_string(self.mtm, &ns);

        // Multi-line wrap: NSTextField's cell needs `wraps = true` +
        // `usesSingleLineMode = false` for the same behavior iOS's
        // `numberOfLines = 0 + lineBreakMode = byWordWrapping` gives.
        let cell: Retained<NSObject> = unsafe { msg_send_id![&label, cell] };
        let _: () = unsafe { msg_send![&cell, setWraps: true] };
        let _: () = unsafe { msg_send![&cell, setUsesSingleLineMode: false] };

        // Default the text color to the INSTALLED THEME's `color-text`, not
        // AppKit's `NSColor.labelColor`. `+labelWithString:` leaves the label on
        // the system label color, which tracks the OS appearance — WHITE in dark
        // mode — so a light-theme app would render invisible white text on a
        // dark-mode Mac. Resolving the `color-text` token here (the same path a
        // styled `color:` token resolves through) makes a raw `text()` match
        // web/iOS/Android. An explicit `style.color` still wins: `apply_style`'s
        // text path only writes a color when `style.color.is_some()`.
        let theme_color = color_to_nscolor(&theme_text_color());
        let _: () = unsafe { msg_send![&label, setTextColor: &*theme_color] };

        // Install a measure_fn so Taffy queries `cellSizeForBounds:`
        // at compute time and gives the label the right wrap height.
        let view: Retained<NSView> = unsafe {
            // NSTextField inherits from NSControl → NSView, so this
            // upcast is safe via the ObjC class hierarchy.
            Retained::retain(Retained::as_ptr(&label) as *mut NSView).unwrap()
        };
        let layout = self.layout_for_view(&view);
        let label_for_measure = label.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, available_space| {
                let avail_w = known_dimensions
                    .width
                    .unwrap_or(match available_space.width {
                        runtime_layout::AvailableSpace::Definite(w) => w,
                        runtime_layout::AvailableSpace::MaxContent => f32::INFINITY,
                        runtime_layout::AvailableSpace::MinContent => 0.0,
                    });
                // A wrapping label's height is a function of its WIDTH (how many
                // lines the text breaks into), never of the available height.
                // Measure against an effectively-unbounded height so
                // `cellSizeForBounds:` returns the natural wrapped height.
                //
                // CRITICAL: do NOT feed `available_space.height` into the bounds.
                // Taffy probes a flex item's MIN-content height (available height
                // = MinContent) to apply the flexbox `min-height: auto` floor
                // that stops items shrinking below their content. Mapping
                // MinContent → height 0 and passing it here makes
                // `cellSizeForBounds:` clip to ~0, so Taffy thinks the label's
                // minimum height is ~0 and flex-shrinks every paragraph/heading
                // to a sliver (H1 worst — shrink is weighted by base size). The
                // height the cell reports must be width-driven only.
                let bounds = CGRect {
                    origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: if avail_w.is_finite() { avail_w as f64 } else { 10_000.0 },
                        height: 10_000.0,
                    },
                };
                let cell: *mut NSObject = unsafe { msg_send![&label_for_measure, cell] };
                let fitted: CGSize = unsafe { msg_send![cell, cellSizeForBounds: bounds] };
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or((fitted.width as f32).ceil()),
                    height: known_dimensions.height.unwrap_or((fitted.height as f32).ceil()),
                }
            }),
        );

        let node = MacosNode::Label(label);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::Text,
            ),
        );
        node
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &runtime_core::Action,
        _leading_icon: Option<&runtime_core::IconData>,
        _trailing_icon: Option<&runtime_core::IconData>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Real NSButton. `+[NSButton buttonWithTitle:target:action:]`
        // produces a system-styled push button (rounded bezel, system
        // font) and wires the target/action in one call. Leading /
        // trailing icons are not yet rendered — NSButton's image slot
        // is single-position; we'd need a custom container view to
        // composite icon + label like UIButton's intrinsic layout.
        // Tracked as a follow-up; the label-only call site is what
        // user code hits today.
        let ns_title = NSString::from_str(label);
        let target = callbacks::CallbackTarget::new(self.mtm, on_click.fire.clone());
        let action = objc2::sel!(invoke:);
        let button: Retained<objc2_app_kit::NSButton> = unsafe {
            msg_send_id![
                objc2::class!(NSButton),
                buttonWithTitle: &*ns_title,
                target: &*target,
                action: action,
            ]
        };
        // Keep the target alive — NSButton holds it as a weak ref.
        // Once the backend drops, the Vec drops, the target drops; by
        // that point the button has also been dropped from the view
        // tree, so the weak ref is irrelevant.
        self.callback_targets.push(unsafe {
            Retained::cast::<NSObject>(target)
        });
        // Upcast NSButton → NSView via the ObjC class hierarchy
        // (NSButton : NSControl : NSView). Same trick as the
        // NSTextField → NSView upcast in `create_text`.
        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&button) as *mut NSView)
                .expect("retain NSButton as NSView")
        };
        // Install an intrinsic-size measurer so Taffy gives the
        // button a sensible default rect. NSButton has a real
        // `intrinsicContentSize` (computed from label + bezel
        // padding); read it through the measure_fn so style-driven
        // sizes can still override.
        let layout = self.layout_for_view(&view);
        let button_for_measure = button.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&button_for_measure, intrinsicContentSize] };
                runtime_layout::Size {
                    width: known_dimensions
                        .width
                        .unwrap_or((intrinsic.width as f32).ceil()),
                    height: known_dimensions
                        .height
                        .unwrap_or((intrinsic.height as f32).ceil()),
                }
            }),
        );
        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::Button,
            ),
        );
        node
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        // NSButton's title is set via `setTitle:` (NSString). Same
        // selector works whether the button was created by us or by
        // someone else's code — Obj-C dispatch on the concrete class.
        let view = node.as_view();
        let ns = NSString::from_str(label);
        let _: () = unsafe { msg_send![view, setTitle: &*ns] };
        // Title change can shift the intrinsic width; mark the layout
        // node dirty so the next pass picks up the new intrinsic size.
        if let Some(layout) = self.layout_of(view) {
            self.layout.mark_dirty(layout);
        }
    }

    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = image::create_image(&self.image_cache, src, alt);
        if let MacosNode::View(view) = &node {
            let view_clone = view.clone();
            self.install_image_measure(&view_clone);
        }
        a11y::apply(
            &node,
            a11y,
            Some(runtime_core::accessibility::Role::Image),
        );
        node
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        image::update_image_src(node, &self.image_cache, src);
        if let MacosNode::View(view) = node {
            // Image swap can change intrinsicContentSize; re-mark the
            // measurer so the next layout pass picks up the new size.
            let view_clone = view.clone();
            self.install_image_measure(&view_clone);
        }
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        secure: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Build an editable NSTextField. `+[NSTextField alloc, init]`
        // gives a bezeled, editable field by default — the same shape
        // as iOS's `UITextField::new` (which starts editable but
        // unbordered; we set `setBorderStyle: 3` for the round-rect
        // bezel on iOS). On macOS the default bezel is appropriate.
        //
        // Password masking: NSSecureTextField is a drop-in NSTextField
        // subclass that renders bullets for typed characters, so we
        // instantiate it instead when `secure`. The binding stays typed
        // as NSTextField (the superclass) since every subsequent
        // msg_send uses NSTextField/NSControl/NSView API.
        let cls = if secure {
            objc2::class!(NSSecureTextField)
        } else {
            objc2::class!(NSTextField)
        };
        let field: Retained<objc2_app_kit::NSTextField> =
            unsafe { msg_send_id![msg_send_id![cls, alloc], init] };

        // Set initial value + placeholder. `setStringValue:` writes
        // the editable string; `setPlaceholderString:` (on the field
        // directly, not the cell, since macOS 10.10) shows when
        // empty.
        let ns_val = NSString::from_str(initial_value);
        let _: () = unsafe { msg_send![&field, setStringValue: &*ns_val] };
        if let Some(ph) = placeholder {
            let ns_ph = NSString::from_str(ph);
            let _: () = unsafe { msg_send![&field, setPlaceholderString: &*ns_ph] };
        }

        // Wire change notification. AppKit fires
        // `NSControlTextDidChangeNotification` (object = the field)
        // on every keystroke; route through the
        // `StringCallbackTarget` whose `controlTextDidChange:`
        // method reads the sender's `stringValue` and forwards.
        let target =
            callbacks::StringCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(controlTextDidChange:);
        let notification_name = NSString::from_str("NSControlTextDidChangeNotification");
        let center: Retained<NSObject> = unsafe {
            msg_send_id![
                objc2::class!(NSNotificationCenter),
                defaultCenter
            ]
        };
        let _: () = unsafe {
            msg_send![
                &center,
                addObserver: &*target,
                selector: sel,
                name: &*notification_name,
                object: &*field,
            ]
        };
        self.callback_targets.push(unsafe {
            Retained::cast::<NSObject>(target)
        });

        // Create-time theme default: an NSTextField with no authored style
        // still gets the theme's surface fill + text color (drawsBackground
        // forced on) so a BARE `text_input` is never AppKit's dark-in-dark-mode
        // system fill before/without an `apply_style`. Explicit author colors
        // override in `apply_style`'s editable-text-control arm. Mirrors
        // `create_text`'s `theme_text_color` create-time default.
        let bg = color_to_nscolor(&input_background_color(None));
        let _: () = unsafe { msg_send![&field, setDrawsBackground: true] };
        let _: () = unsafe { msg_send![&field, setBackgroundColor: &*bg] };
        let fg = color_to_nscolor(&input_text_color(None));
        let _: () = unsafe { msg_send![&field, setTextColor: &*fg] };

        // Upcast NSTextField → NSView via the ObjC class hierarchy.
        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&field) as *mut NSView)
                .expect("retain NSTextField as NSView")
        };

        // Intrinsic-size measurer. NSTextField's
        // `intrinsicContentSize` reports a sensible default height
        // and (for the editable variant) ~100pt width — same shape
        // as the Button measurer.
        let layout = self.layout_for_view(&view);
        let field_for_measure = field.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&field_for_measure, intrinsicContentSize] };
                let w = (intrinsic.width as f32).max(0.0);
                let h = (intrinsic.height as f32).max(0.0);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );

        let node = MacosNode::View(view);
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        let view = node.as_view();
        // Read the current `stringValue` first; only write if it
        // differs, otherwise we re-fire `controlTextDidChange:` and
        // loop with the framework's reactive update.
        let current: *mut NSString = unsafe { msg_send![view, stringValue] };
        if !current.is_null() {
            let current_ref: &NSString = unsafe { &*current };
            if current_ref.to_string() == value {
                return;
            }
        }
        let ns = NSString::from_str(value);
        let _: () = unsafe { msg_send![view, setStringValue: &*ns] };
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        // `wrap` is the no-wrap / soft-wrap toggle. The macOS NSTextView
        // path mounts bare today (see the v1 note below) and already
        // sizes to its content via the intrinsic measure_fn installed
        // below, so the wrap toggle lands in the same follow-up that
        // adds the NSScrollView wrapping. (There is no `auto_grow` flag:
        // content-height growth is just the intrinsic measure with no
        // pinned height — which this path already does.)
        _wrap: bool,
        on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // NSTextView is the multi-line editable text widget. It's
        // typically embedded in an NSScrollView for clipping +
        // scrollbars; for v1 we mount it bare and let the layout
        // pass size it directly. Wrap-in-scroll-view lands as a
        // follow-up alongside the ScrollView wiring.
        let view: Retained<NSView> = unsafe {
            let allocated: *mut objc2::runtime::AnyObject =
                msg_send![objc2::class!(NSTextView), alloc];
            let inited: *mut objc2::runtime::AnyObject = msg_send![allocated, init];
            Retained::from_raw(inited.cast::<NSView>())
                .expect("NSTextView init returned nil")
        };
        let ns_val = NSString::from_str(initial_value);
        let _: () = unsafe { msg_send![&view, setString: &*ns_val] };
        let _: () = unsafe { msg_send![&view, setEditable: true] };
        let _: () = unsafe { msg_send![&view, setRichText: false] };

        // Create-time theme default (see `create_text_input`): a bare
        // `text_area` gets the theme surface + text color with drawsBackground
        // forced on, so an NSTextView is never AppKit's dark system fill in
        // dark mode. Explicit author colors override in `apply_style`.
        let bg = color_to_nscolor(&input_background_color(None));
        let _: () = unsafe { msg_send![&view, setDrawsBackground: true] };
        let _: () = unsafe { msg_send![&view, setBackgroundColor: &*bg] };
        let fg = color_to_nscolor(&input_text_color(None));
        let _: () = unsafe { msg_send![&view, setTextColor: &*fg] };

        // Wire change notification.
        // `NSTextDidChangeNotification` fires every edit on the
        // text view; the StringCallbackTarget's `textDidChange:`
        // method reads the sender's `string` and forwards. The
        // `notification.object` filter is the NSTextView itself,
        // so a sibling TextArea's edits don't fire this handler.
        let target = callbacks::StringCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(textDidChange:);
        let notification_name = NSString::from_str("NSTextDidChangeNotification");
        let center: Retained<NSObject> = unsafe {
            msg_send_id![
                objc2::class!(NSNotificationCenter),
                defaultCenter
            ]
        };
        let _: () = unsafe {
            msg_send![
                &center,
                addObserver: &*target,
                selector: sel,
                name: &*notification_name,
                object: &*view,
            ]
        };
        self.callback_targets.push(unsafe {
            Retained::cast::<NSObject>(target)
        });

        let layout = self.layout_for_view(&view);
        let view_for_measure = view.clone();
        // NSTextView's `intrinsicContentSize` reports its natural
        // text-content size; clamp negatives and bias toward a
        // reasonable default (matches the wgpu TEXT_AREA_DEFAULT_HEIGHT
        // posture — 4× line ≈ 80pt).
        const TEXT_AREA_DEFAULT_HEIGHT: f32 = 80.0;
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&view_for_measure, intrinsicContentSize] };
                let w = (intrinsic.width as f32).max(0.0);
                let h = (intrinsic.height as f32).max(TEXT_AREA_DEFAULT_HEIGHT);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );

        let node = MacosNode::View(view);
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        let view = node.as_view();
        let ns = NSString::from_str(value);
        let _: () = unsafe { msg_send![view, setString: &*ns] };
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // NSSwitch is the macOS 10.15+ equivalent of UISwitch — same
        // rounded-pill toggle visual. It's an NSControl so
        // target/action wiring is the same pattern as NSButton.
        let switch: Retained<objc2_app_kit::NSSwitch> = unsafe {
            msg_send_id![msg_send_id![objc2::class!(NSSwitch), alloc], init]
        };
        // NSControlStateValueOn = 1, Off = 0.
        let state: isize = if initial_value { 1 } else { 0 };
        let _: () = unsafe { msg_send![&switch, setState: state] };

        let target = callbacks::BoolCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(invoke:);
        let _: () = unsafe { msg_send![&switch, setTarget: &*target] };
        let _: () = unsafe { msg_send![&switch, setAction: sel] };
        self.callback_targets.push(unsafe {
            Retained::cast::<NSObject>(target)
        });

        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&switch) as *mut NSView)
                .expect("retain NSSwitch as NSView")
        };
        let layout = self.layout_for_view(&view);
        let switch_for_measure = switch.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&switch_for_measure, intrinsicContentSize] };
                let w = (intrinsic.width as f32).max(0.0);
                let h = (intrinsic.height as f32).max(0.0);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );
        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::Toggle,
            ),
        );
        node
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        let view = node.as_view();
        let state: isize = if value { 1 } else { 0 };
        // Read first; skip write if the state already matches so the
        // target/action callback doesn't re-fire and loop with the
        // framework's reactive update.
        let current: isize = unsafe { msg_send![view, state] };
        if current == state {
            return;
        }
        let _: () = unsafe { msg_send![view, setState: state] };
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // NSSlider with continuous tracking. `setMinValue:` /
        // `setMaxValue:` / `setDoubleValue:` configure the range +
        // initial. `setContinuous:true` makes the action fire as the
        // user drags (matches UISlider's default on iOS).
        //
        // Step snapping isn't wired yet — NSSlider has
        // `setAltIncrementValue:` for keyboard step but no native
        // continuous-drag snap. Authors expecting `step` semantics
        // get continuous output for now; a wrapper closure could
        // round in the callback if needed.
        let slider: Retained<objc2_app_kit::NSSlider> = unsafe {
            msg_send_id![msg_send_id![objc2::class!(NSSlider), alloc], init]
        };
        let _: () = unsafe { msg_send![&slider, setMinValue: min as f64] };
        let _: () = unsafe { msg_send![&slider, setMaxValue: max as f64] };
        let _: () = unsafe { msg_send![&slider, setDoubleValue: initial_value as f64] };
        let _: () = unsafe { msg_send![&slider, setContinuous: true] };

        let target = callbacks::FloatCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(invoke:);
        let _: () = unsafe { msg_send![&slider, setTarget: &*target] };
        let _: () = unsafe { msg_send![&slider, setAction: sel] };
        self.callback_targets.push(unsafe {
            Retained::cast::<NSObject>(target)
        });

        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&slider) as *mut NSView)
                .expect("retain NSSlider as NSView")
        };
        let layout = self.layout_for_view(&view);
        let slider_for_measure = slider.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&slider_for_measure, intrinsicContentSize] };
                // NSSlider's intrinsic width is `-1`
                // (NSViewNoIntrinsicMetric); height is a real value.
                // Default the width to a reasonable touch-friendly
                // size matching iOS's SLIDER_DEFAULT_WIDTH.
                const SLIDER_DEFAULT_WIDTH: f32 = 200.0;
                let w = if intrinsic.width >= 0.0 {
                    intrinsic.width as f32
                } else {
                    SLIDER_DEFAULT_WIDTH
                };
                let h = (intrinsic.height as f32).max(0.0);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );
        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::Slider,
            ),
        );
        node
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        let view = node.as_view();
        let current: f64 = unsafe { msg_send![view, doubleValue] };
        if (current - value as f64).abs() < f64::EPSILON {
            return;
        }
        let _: () = unsafe { msg_send![view, setDoubleValue: value as f64] };
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Real two-view shape: outer NSScrollView wraps a
        // `FlippedView` documentView. Children mount inside the
        // documentView (via `insert`'s scroll-view-aware
        // redirect); the NSScrollView's clip view handles
        // overflow + scroll-bar machinery.
        //
        // We return the OUTER NSScrollView as the MacosNode so
        // layout / style / hit-testing target the scrolling
        // container (the framework's mental model). `insert`
        // checks `documentView` and routes children into the
        // documentView's subview tree + the Taffy graph.
        let document_view = FlippedView::new(self.mtm);
        let document_view: Retained<NSView> = Retained::into_super(document_view);

        // Build NSScrollView via alloc/initWithFrame:CGRect.zero.
        // Frame is set by the layout pass in `finish()`.
        let scroll_view: Retained<NSView> = unsafe {
            let allocated: *mut objc2::runtime::AnyObject =
                msg_send![objc2::class!(NSScrollView), alloc];
            let zero_rect = CGRect {
                origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: 0.0, height: 0.0 },
            };
            let inited: *mut objc2::runtime::AnyObject =
                msg_send![allocated, initWithFrame: zero_rect];
            Retained::from_raw(inited.cast::<NSView>())
                .expect("NSScrollView init returned nil")
        };

        // Configure scrollers per axis. Vertical-default matches
        // iOS's default ScrollView. NSScrollView's API:
        //  - setHasVerticalScroller: / setHasHorizontalScroller:
        //  - setAutohidesScrollers: true so the scrollers fade
        //    when not actively scrolling (modern macOS feel).
        let _: () = unsafe {
            msg_send![&scroll_view, setHasVerticalScroller: !horizontal]
        };
        let _: () = unsafe {
            msg_send![&scroll_view, setHasHorizontalScroller: horizontal]
        };
        let _: () = unsafe { msg_send![&scroll_view, setAutohidesScrollers: true] };

        // Transparent by default — matching iOS's UIScrollView. By default an
        // NSScrollView AND its NSClipView (`contentView`) both fill with the
        // opaque `controlBackgroundColor` (dark in dark mode), which composites
        // OVER the author's `background` and reads as a dark void. We disable
        // BOTH here; `apply_style` re-enables painting with the author's color
        // when a `background` style is present. Disabling only the scroll view
        // (not the clip view) leaves the clip painting dark — the bug where the
        // body stayed dark even with `background: #f7f5ef`.
        let _: () = unsafe { msg_send![&scroll_view, setDrawsBackground: false] };
        let clip_for_bg: *mut NSObject = unsafe { msg_send![&scroll_view, contentView] };
        if !clip_for_bg.is_null() {
            let _: () = unsafe { msg_send![clip_for_bg, setDrawsBackground: false] };
        }

        // Install the documentView. NSScrollView retains it; we
        // can drop our local Retained after this call without
        // losing the document.
        let _: () = unsafe { msg_send![&scroll_view, setDocumentView: &*document_view] };

        // Register ONLY the outer NSScrollView in the layout map — it is what
        // Taffy positions, and (mirroring iOS's single-`UIScrollView` model)
        // the scroll view's children are parented directly under its Taffy
        // node by `insert`. The inner documentView is deliberately NOT a Taffy
        // node: it is a native-only container that AppKit scrolls, sized to its
        // children's bounding box each layout pass by `sync_scroll_document_views`.
        // Registering it would make the apply loop stamp it with an
        // uncomputed-orphan frame.
        let scroll_layout = self.layout_for_view(&scroll_view);

        // Mark the Taffy node as a scroll container on the scroll axis. Because
        // we parent the scroll view's children directly under THIS node
        // (iOS-style), without `Overflow::Scroll` Taffy treats them as ordinary
        // flex items in a viewport-height column and shrinks them to fit (the
        // content is taller than the viewport), crushing each label to a sliver
        // — H1 worst, since flex-shrink is weighted by base size. `Overflow::Scroll`
        // makes Taffy give the node a definite size from its parent and lets the
        // children overflow at their natural size (which `sync_scroll_document_views`
        // then reads as the documentView's scrollable content size). `set_style`
        // (called later by `apply_style`) is a partial merge that never writes
        // `overflow`, so this survives the author's `ScreenScroll` style.
        self.layout.set_overflow_scroll(scroll_layout, horizontal);

        // Wire `on_scroll` via NSViewBoundsDidChangeNotification on
        // the scroll view's contentView (the NSClipView). We flip
        // `postsBoundsChangedNotifications` on, then register a
        // ScrollObserverTarget against the default NotificationCenter
        // keyed on the clip view. The target's `boundsDidChange:`
        // selector reads the clip view's bounds.origin and forwards
        // it as the (x, y) scroll offset \u{2014} same units as web
        // (CSS pixels) and iOS (UIKit points), so author code reads
        // identical values across platforms.
        if let Some(cb) = on_scroll {
            let target = crate::imp::callbacks::ScrollObserverTarget::new(self.mtm, cb);
            let clip_view: *mut objc2::runtime::AnyObject =
                unsafe { msg_send![&scroll_view, contentView] };
            if !clip_view.is_null() {
                let _: () =
                    unsafe { msg_send![clip_view, setPostsBoundsChangedNotifications: true] };
                let center: *mut objc2::runtime::AnyObject =
                    unsafe { msg_send![objc2::class!(NSNotificationCenter), defaultCenter] };
                let name: Retained<objc2_foundation::NSString> =
                    objc2_foundation::NSString::from_str("NSViewBoundsDidChangeNotification");
                let sel = objc2::sel!(boundsDidChange:);
                let _: () = unsafe {
                    msg_send![
                        center,
                        addObserver: &*target,
                        selector: sel,
                        name: &*name,
                        object: clip_view,
                    ]
                };
            }
            // Retain across the backend's lifetime so the observer
            // outlives the scroll view it watches. The notification
            // center holds a non-owning ref \u{2014} same pattern as
            // every other macOS callback target.
            self.callback_targets
                .push(unsafe { Retained::cast::<NSObject>(target) });
        }

        let node = MacosNode::View(scroll_view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::ScrollView,
            ),
        );
        node
    }

    fn create_activity_indicator(
        &mut self,
        size: runtime_core::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        use runtime_core::primitives::activity_indicator::ActivityIndicatorSize;
        // NSProgressIndicator in spinning style — same shape as
        // UIActivityIndicatorView. `setStyle: 1` =
        // NSProgressIndicatorStyleSpinning. `setIndeterminate: true`
        // is the default for spinning style but we set it
        // explicitly for safety.
        let spinner: Retained<objc2_app_kit::NSProgressIndicator> = unsafe {
            msg_send_id![
                msg_send_id![objc2::class!(NSProgressIndicator), alloc],
                init
            ]
        };
        // INVARIANT: `setStyle:`/`setControlSize:` take NSUInteger args
        // (`NSProgressIndicatorStyle` / `NSControlSize`, encoding `Q`,
        // unsigned). The previous raw `msg_send![…, setStyle: 1isize]` passed
        // a signed `isize` (`q`); objc2's runtime encoding check rejects the
        // mismatch with a NON-UNWINDING panic → the whole process SIGABRTs
        // (uncatchable) the instant a spinner is created. We use the typed
        // objc2 enums so the unsigned type is COMPILE-enforced — a stray
        // integer literal can't reintroduce the crash.
        unsafe { spinner.setStyle(objc2_app_kit::NSProgressIndicatorStyle::Spinning) };
        let _: () = unsafe { msg_send![&spinner, setIndeterminate: true] };
        // Map ActivityIndicatorSize → NSControlSize. macOS's spinner has no
        // explicit "large" variant, so ::Large maps to Regular.
        let control_size = match size {
            ActivityIndicatorSize::Small => objc2_app_kit::NSControlSize::Small,
            ActivityIndicatorSize::Large => objc2_app_kit::NSControlSize::Regular,
        };
        unsafe { spinner.setControlSize(control_size) };
        let _: () = unsafe { msg_send![&spinner, startAnimation: std::ptr::null::<NSObject>()] };

        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&spinner) as *mut NSView)
                .expect("retain NSProgressIndicator as NSView")
        };
        let layout = self.layout_for_view(&view);
        let spinner_for_measure = spinner.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&spinner_for_measure, intrinsicContentSize] };
                let w = (intrinsic.width as f32).max(0.0);
                let h = (intrinsic.height as f32).max(0.0);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );
        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::ActivityIndicator,
            ),
        );
        node
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let parent_view = parent.as_view();
        let child_view = child.as_view();

        // Detached window root (screen_recorder private layer): the content
        // view already lives in its OWN borderless overlay window. The
        // External walker calls `insert(parent, external_node)` to splice the
        // handler's returned node into the surrounding tree — but `addSubview`
        // here would reparent the content view OUT of its overlay window and
        // INTO the main (recorded/capturable) tree. Skip the native reparent;
        // the root stays in its window. Its Taffy node remains registered
        // (sized to the window by the detached-root pass in `finish`) and the
        // walker's child-insert already populated it. Mirror of iOS.
        let child_key = child_view as *const NSView as usize;

        // Portal container: it already mounted ITSELF into the host window's
        // content view in `create_portal` and is its own viewport-sized Taffy
        // root (laid out in `compute_and_apply_layout`). The walker still calls
        // `insert(declaration_parent, portal_node)` for it — doing the reparent
        // here would yank the overlay out of the host overlay and INTO the
        // surrounding tree, so it'd render inline at the declaration site (a
        // Modal at the bottom of its screen, no full-window backdrop). Skip it.
        // Mirrors iOS's `portal_instances` guard.
        if self.portal_roots.contains(&child_key) {
            return;
        }

        if self.detached_window_roots.contains_key(&child_key) {
            return;
        }

        // ScrollView routing: if the framework's logical parent is an
        // NSScrollView (created via `create_scroll_view`), children mount
        // inside its documentView so they participate in the scroll machinery.
        // `documentView` returns the inner FlippedView we installed. Without
        // this redirect, addSubview would add the child to the scroll view's
        // clip view at fixed coordinates and the scroll wouldn't take effect.
        //
        // NOTE: this affects only the *native* mount target. The *Taffy*
        // parent stays the scroll view itself (see below).
        let is_scroll = is_scroll_view(parent_view);
        let native_target: Retained<NSView> = if is_scroll {
            let doc_ptr: *mut NSView =
                unsafe { msg_send![parent_view, documentView] };
            if doc_ptr.is_null() {
                // Defensive: fall back to the scroll view itself if
                // documentView is somehow nil. Shouldn't happen
                // because `create_scroll_view` always installs one.
                unsafe { Retained::retain(parent_view as *const NSView as *mut NSView) }
                    .unwrap()
            } else {
                unsafe { Retained::retain(doc_ptr) }
                    .expect("NSScrollView documentView retain")
            }
        } else {
            unsafe { Retained::retain(parent_view as *const NSView as *mut NSView) }
                .unwrap()
        };

        // AppKit: `addSubview:` mounts the child. Frame is determined
        // by the layout pass; at initial build that's `finish()`, but a
        // POST-mount insert (a `presence`/`when` mount — e.g. the
        // whiteboard Settings/Preview screens) happens after `finish` has
        // already run once and won't recompute on its own. Here we just
        // establish the parent/child relationship in both the view tree
        // AND the Taffy tree, then (below) kick a coalesced pass when the
        // parent is already in a window.
        unsafe { native_target.addSubview(child_view) };

        // Mirror the parenting in Taffy against the framework's LOGICAL
        // parent — the scroll view itself, NOT its documentView. This mirrors
        // iOS's single-`UIScrollView` model: children are direct Taffy children
        // of the scroll node, so the layout pass reaches and sizes them, and
        // their frames are relative to the scroll view's content origin (== the
        // documentView origin, which sits at the clip view's top-left). The
        // documentView is then resized to the children's bounding box in
        // `sync_scroll_document_views` so AppKit can scroll. Parenting children
        // under the documentView instead orphans the whole subtree (the
        // documentView has no Taffy parent), and every child computes to 0×0 —
        // the macOS "scroll page renders blank" bug.
        let parent_layout = self.layout_for_view(parent_view);
        let child_layout = self.layout_for_view(child_view);
        self.layout.add_child(parent_layout, child_layout);

        // Post-mount insert into a window-attached parent → queue a coalesced
        // layout pass so the freshly-mounted subtree gets sized. During the
        // initial build the target isn't in a window yet (the root is parented
        // to the host in `finish`), so this no-ops then and the mount pass
        // handles it.
        //
        // `schedule_layout_pass` now runs the pass from a run-loop observer
        // BEFORE this turn's Core Animation commit (see its docs), so a burst
        // of inserts in one batch produces exactly ONE pass that lands before
        // any paint: no per-insert O(N) re-layout (the lag), and no
        // paint-at-(0,0)-then-reposition (the flicker).
        let host_window: *mut objc2_app_kit::NSWindow =
            unsafe { msg_send![&native_target, window] };
        if crate::layout_policy::insert_needs_layout_pass(!host_window.is_null()) {
            schedule_layout_pass();
        }
    }

    /// Opt into ANCHORLESS reactive regions (the runtime-decided control-flow
    /// lowering). With this on, a style-less `when`/`for` in a children list
    /// splices its branch/rows DIRECTLY into the real parent via `insert_at` /
    /// `remove_child` — no `create_reactive_anchor` wrapper view.
    ///
    /// Why macOS needs it (it was the last backend on the anchored path): a
    /// wrapper view is a real box that AUTO-sizes to its IN-FLOW children, so a
    /// `when` whose active branch is `position: Absolute` (every popover/overlay
    /// — Settings, camera panel, media inspector) collapses the wrapper to 0×0.
    /// AppKit still PAINTS the absolute child (NSView doesn't clip subviews to
    /// bounds), but `hitTest:` won't descend into a subview unless the click is
    /// inside the 0×0 wrapper's frame — so the popover rendered fine yet
    /// swallowed every click. Splicing gives the absolute branch the real parent
    /// as its containing block (matching web's `display: contents` anchor), so it
    /// both paints AND hit-tests. Mirrors iOS/Android, which already opt in.
    fn supports_child_splice(&self) -> bool {
        true
    }

    /// Remove a SPECIFIC `child` from `parent` — the removal half of an
    /// anchorless region's per-toggle rebuild. Detaches the native view AND the
    /// parallel Taffy edge, then marks the parent dirty (Taffy doesn't
    /// auto-invalidate a parent's cached size on a child-set change) and reflows
    /// so a content-sized ancestor shrinks to fit the now-shorter child set.
    /// Mirror of the per-child teardown `clear_children` does. Mirrors iOS.
    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        let parent_view = parent.as_view();
        let child_view = child.as_view();
        let child_key = child.view_key();

        // Portal / detached-window roots aren't real children of `parent` (their
        // `insert` was skipped): a portal mounts itself into the host window, a
        // detached root (screen_recorder private layer) lives in its own
        // borderless window. `removeFromSuperview` here would yank them out of
        // their host window, and their Taffy node isn't `parent`'s child. Their
        // teardown runs via the scope-tied release paths. Symmetric with the
        // `insert` skips; mirrors iOS.
        if self.portal_roots.contains(&child_key)
            || self.detached_window_roots.contains_key(&child_key)
        {
            return;
        }

        // Detach the Taffy edge BEFORE removeFromSuperview (mirror
        // `clear_children`) so a stale child-set + cached parent size can't drive
        // a ghost layout on the next pass. `mark_dirty` recomputes the parent's
        // measured size.
        if let (Some(p_layout), Some(c_layout)) =
            (self.layout_of(parent_view), self.layout_of(child_view))
        {
            self.layout.remove_child(p_layout, c_layout);
            self.layout.mark_dirty(p_layout);
        }
        // Drop any pending color transition keyed on this view pointer so a
        // recycled NSView can't inherit a stale `from` color.
        transitions::forget_view(child_view);
        let _: () = unsafe { msg_send![child_view, removeFromSuperview] };

        // Reflow after the removal — symmetric with `insert` / `insert_at`. The
        // coalesced pass runs before this turn's Core Animation commit, so the
        // shrink lands in the same frame with no flicker; a mid-build removal on
        // a floating parent no-ops here and defers to `finish()`.
        let host_window: *mut objc2_app_kit::NSWindow =
            unsafe { msg_send![parent_view, window] };
        if crate::layout_policy::insert_needs_layout_pass(!host_window.is_null()) {
            schedule_layout_pass();
        }
    }

    /// Insert `child` into `parent` at child-array `index` — companion to
    /// `remove_child`. This is `insert` (above) with one difference: it lands the
    /// child at the region's stable `base_index` (so a region with trailing
    /// static siblings rebuilds in the right place / z-order) instead of always
    /// appending. Preserves every special case `insert` has: the portal /
    /// detached-window-root skips, the scroll-view `documentView` routing, and
    /// the window-attached layout-pass gate. Mirrors iOS.
    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        let parent_view = parent.as_view();
        let child_view = child.as_view();
        let child_key = child.view_key();

        // Portal / detached-window roots mount themselves into the host window;
        // skip the parent-tree splice the walker tries for them. (Mirror of
        // `insert`.)
        if self.portal_roots.contains(&child_key)
            || self.detached_window_roots.contains_key(&child_key)
        {
            return;
        }

        // ScrollView routing: children mount inside the inner documentView so
        // they participate in the scroll machinery, while the Taffy parent stays
        // the scroll view itself. (Mirror of `insert`.)
        let is_scroll = is_scroll_view(parent_view);
        let native_target: Retained<NSView> = if is_scroll {
            let doc_ptr: *mut NSView = unsafe { msg_send![parent_view, documentView] };
            if doc_ptr.is_null() {
                unsafe { Retained::retain(parent_view as *const NSView as *mut NSView) }.unwrap()
            } else {
                unsafe { Retained::retain(doc_ptr) }.expect("NSScrollView documentView retain")
            }
        } else {
            unsafe { Retained::retain(parent_view as *const NSView as *mut NSView) }.unwrap()
        };

        // AppKit has no `insertSubview:atIndex:`. Subviews are ordered
        // back-to-front, so to land `child` at array index `index` we insert it
        // BELOW the sibling currently at that index (which shifts to `index+1`).
        // At or past the end there's no such sibling — append on top, matching
        // `insert`'s plain `addSubview:`. The Taffy `add_child_at_index` below
        // clamps `index` identically.
        let subviews_arr: *mut NSObject = unsafe { msg_send![&*native_target, subviews] };
        let count: usize = if subviews_arr.is_null() {
            0
        } else {
            unsafe { msg_send![subviews_arr, count] }
        };
        if index >= count {
            unsafe { native_target.addSubview(child_view) };
        } else {
            let sibling: *mut NSView = unsafe { msg_send![subviews_arr, objectAtIndex: index] };
            let _: () = unsafe {
                msg_send![
                    &*native_target,
                    addSubview: child_view,
                    positioned: objc2_app_kit::NSWindowOrderingMode::NSWindowBelow,
                    relativeTo: sibling,
                ]
            };
        }

        // Mirror the parenting in Taffy against the framework's LOGICAL parent
        // (the scroll view itself, NOT its documentView — same as `insert`), at
        // the matching index so flex order tracks the native subview order.
        let parent_layout = self.layout_for_view(parent_view);
        let child_layout = self.layout_for_view(child_view);
        self.layout.add_child_at_index(parent_layout, child_layout, index);
        self.layout.mark_dirty(parent_layout);

        // Same window-attached layout-pass gate as `insert`: a splice into a
        // live parent (the post-mount `when`/`for` toggle) kicks a coalesced
        // pass so the new branch is sized + painted in the same frame; a
        // mid-build splice into a floating parent defers to `finish()`.
        let host_window: *mut objc2_app_kit::NSWindow =
            unsafe { msg_send![&native_target, window] };
        if crate::layout_policy::insert_needs_layout_pass(!host_window.is_null()) {
            schedule_layout_pass();
        }
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        if let MacosNode::Label(label) = node {
            let ns = NSString::from_str(content);
            let _: () = unsafe { msg_send![label, setStringValue: &*ns] };
            // Text-content change can shift wrapped height; mark the
            // layout node dirty so the next layout pass re-measures.
            let view = node.as_view();
            if let Some(layout) = self.layout_of(view) {
                self.layout.mark_dirty(layout);
            }
        }
    }

    fn clear_children(&mut self, node: &Self::Node) {
        let view = node.as_view();
        // A scroll view hosts its content inside the documentView (see
        // `insert`), so walk THAT subview list — `view.subviews` is just the
        // clip view. The Taffy parent stays the scroll node itself (its
        // children are parented there), so `remove_child` below still uses
        // `layout_of(view)`.
        let native_host: Retained<NSView> = if is_scroll_view(view) {
            let doc_ptr: *mut NSView = unsafe { msg_send![view, documentView] };
            if doc_ptr.is_null() {
                unsafe { Retained::retain(view as *const NSView as *mut NSView) }.unwrap()
            } else {
                unsafe { Retained::retain(doc_ptr) }.expect("documentView retain")
            }
        } else {
            unsafe { Retained::retain(view as *const NSView as *mut NSView) }.unwrap()
        };
        let native_host: &NSView = &native_host;
        // AppKit: removeFromSuperview on each subview. Walk a
        // snapshot because removeFromSuperview mutates the
        // subviews array we'd otherwise iterate.
        let subviews_arr: *mut NSObject = unsafe { msg_send![native_host, subviews] };
        if subviews_arr.is_null() {
            return;
        }
        // NSArray copy → safe-to-iterate snapshot.
        let copy: Retained<NSObject> = unsafe {
            msg_send_id![subviews_arr, copy]
        };
        let count: usize = unsafe { msg_send![&copy, count] };
        for i in 0..count {
            let sub_ptr: *mut NSView = unsafe { msg_send![&copy, objectAtIndex: i] };
            if sub_ptr.is_null() {
                continue;
            }
            // Detached window root (screen_recorder private layer): never
            // detach an overlay content view that lives in its own borderless
            // window. It isn't a subview of the recorded tree to begin with
            // (its `insert` was skipped), but a reactive region rebuild that
            // clears a shared parent must not pull it out of its window or
            // unregister its Taffy root. Mirror of the `insert` skip + iOS.
            if self
                .detached_window_roots
                .contains_key(&(sub_ptr as *const NSView as usize))
            {
                continue;
            }
            // Mirror iOS's clear_children Taffy sync (see
            // [[project_ios_clear_children_taffy_sync]]): detach the
            // Taffy edge BEFORE removeFromSuperview so a stale
            // child-set + cached parent size doesn't drive a ghost
            // layout on the next pass.
            let sub_ref: &NSView = unsafe { &*sub_ptr };
            if let (Some(p_layout), Some(c_layout)) =
                (self.layout_of(view), self.layout_of(sub_ref))
            {
                self.layout.remove_child(p_layout, c_layout);
                self.layout.mark_dirty(p_layout);
            }
            // Drop any pending color transition keyed on this view pointer so a
            // recycled NSView can't inherit a stale `from` color.
            transitions::forget_view(sub_ref);
            let _: () = unsafe { msg_send![sub_ptr, removeFromSuperview] };
        }
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let view = node.as_view();
        apply_style_to_view(view, style);

        // Gradient install lives outside `apply_style_to_view` because
        // we need to stash the returned state in the per-view cache —
        // the helper module returns the layer + per-stop sRGB cache
        // for later `set_animated_gradient_stop` writes.
        if let Some(g) = &style.background_gradient {
            if let Some(state) = gradient::install_gradient(view, g) {
                let key = view as *const NSView as usize;
                self.gradient_states.insert(key, state);
            }
        }

        // Static `transform: [...]` from the style. Px translates,
        // scale, and rotate go straight into the CALayer transform;
        // percent translates are stashed for resolution after layout
        // (bounds not known here). `sync_transform_after_layout` in
        // the layout pass re-applies once frames are computed.
        animated::apply_static_transform(node, style, &mut self.animated_states);

        // Push the framework's style into Taffy so flex props
        // (direction, gap, justify, align, width/height) flow into
        // the layout pass. `LayoutTree::set_style` accepts
        // `&StyleRules` directly — the translation lives inside
        // `runtime-layout`.
        if let Some(layout_node) = self.layout_of(view) {
            self.layout.set_style(layout_node, style);
        }

        // Per-node-type text styling. Labels and text views need
        // font/color/alignment applied directly to the AppKit
        // widget; generic views skip this. Mirrors the iOS
        // `match node { IosNode::Label(_) => apply_text_style(...) }`
        // pattern.
        match node {
            MacosNode::Label(_) => {
                text_style::apply_text_style(view, style, true, &self.font_registry);
                // Author `padding_*` on a `text()` node: Taffy reserves the
                // padding (sizing the outer label frame), but the glyphs would
                // paint flush in a corner without an inset. Push the same per-
                // side padding into the cell's draw inset (see
                // `view::IdealystLabelCell`) so the text lands in the content
                // rect. `length_to_px` yields 0 for Percent/Auto (no defined
                // sizing parent for a leaf), matching the iOS handler.
                let resolve = |t: &Option<runtime_core::Tokenized<runtime_core::Length>>| {
                    t.as_ref().map(|tok| length_to_px(&tok.resolve())).unwrap_or(0.0)
                };
                view::set_label_insets(
                    view,
                    view::LabelInsets {
                        top: resolve(&style.padding_top),
                        left: resolve(&style.padding_left),
                        bottom: resolve(&style.padding_bottom),
                        right: resolve(&style.padding_right),
                    },
                );
                // Label height depends on font + width; both can
                // change via apply_style. Dirty the layout node so
                // the measure_fn runs again.
                if let Some(layout_node) = self.layout_of(view) {
                    self.layout.mark_dirty(layout_node);
                }
            }
            MacosNode::View(_) if is_editable_text_control(view) => {
                // NSTextField / NSTextView paint their OWN background + text
                // through AppKit (the system text-control fill + `labelColor`),
                // which composites OVER the CALayer `backgroundColor` that
                // `apply_style_to_view` set and tracks the OS appearance — so a
                // light-theme app's input rendered as a near-black box with
                // invisible text under dark mode (the idea-ui `Textarea` bug).
                // Mirror the THEME-resolved colors onto the AppKit-level
                // properties: background→color-surface (explicit wins),
                // text→color-text (explicit wins), and force `drawsBackground`
                // so the fill actually paints. Editing behaviour (caret,
                // selection, secure entry) is untouched — these are pure
                // appearance setters. Shared decision with iOS (§7).
                let bg = color_to_nscolor(&input_background_color(style.background.as_ref()));
                let _: () = unsafe { msg_send![view, setDrawsBackground: true] };
                let _: () = unsafe { msg_send![view, setBackgroundColor: &*bg] };
                let fg = color_to_nscolor(&input_text_color(style.color.as_ref()));
                let _: () = unsafe { msg_send![view, setTextColor: &*fg] };
            }
            MacosNode::View(_) => {
                // NSScrollView + its NSClipView paint their background through
                // AppKit's `drawsBackground`, NOT the CALayer `backgroundColor`
                // that `apply_style_to_view` set — and AppKit's fill composites
                // over the layer. So mirror the author's `background` onto both
                // AppKit backgrounds (and re-enable drawing, which
                // `create_scroll_view` disabled for a transparent default). A
                // scroll view with no `background` stays transparent and shows
                // its parent, matching iOS's UIScrollView.
                if is_scroll_view(view) {
                    if let Some(bg) = &style.background {
                        let bg_val = bg.resolve();
                        // Animate the scroll + clip AppKit background over
                        // `background_transition` (the visible theme body/sidebar
                        // fade), or snap. `apply_color` re-enables drawsBackground
                        // on both (create_scroll_view left them transparent).
                        transitions::apply_color(
                            view,
                            transitions::ColorProp::Background,
                            true,
                            style_color_rgba(&bg_val),
                            style.background_transition.as_ref(),
                        );
                        // Match the scroll view's appearance to its background
                        // luminance so the OVERLAY SCROLLER's knob contrasts — a
                        // dark knob on a light surface, light on dark —
                        // regardless of the SYSTEM light/dark setting. Without
                        // this, a light sidebar viewed under a dark-mode system
                        // inherits a light knob that's invisible on the white
                        // background (the "scrollbar disappeared" report). The
                        // forced appearance also keeps any native controls in the
                        // scrolled content consistent with the author's theme.
                        let rgba = runtime_core::color::parse_or(
                            &bg_val.0,
                            runtime_core::color::Rgba::BLACK,
                        );
                        let lum = 0.299 * rgba.r as f32
                            + 0.587 * rgba.g as f32
                            + 0.114 * rgba.b as f32;
                        let name = if lum > 140.0 {
                            "NSAppearanceNameAqua"
                        } else {
                            "NSAppearanceNameDarkAqua"
                        };
                        let ns_name = NSString::from_str(name);
                        let appearance: *mut NSObject = unsafe {
                            msg_send![objc2::class!(NSAppearance), appearanceNamed: &*ns_name]
                        };
                        if !appearance.is_null() {
                            let _: () = unsafe { msg_send![view, setAppearance: appearance] };
                        }
                    }
                }
            }
        }

        // Post-mount layout commit. `finish` lays the tree out exactly once, at
        // mount. A reactive `apply_style` that changes a layout property
        // afterward (size / position / flex — e.g. the whiteboard-demo
        // recording-preview box growing from a collapsed 0×0 to its real size
        // when Record is pressed) updated Taffy via `set_style` above, but
        // nothing else drives a recompute, so the NSView keeps its stale frame
        // and the change is invisible. If this view is already in a window —
        // i.e. we're past the initial build, where views are still floating and
        // the upcoming `finish` will lay them out — schedule a coalesced pass to
        // commit the new frames. The window check is what keeps the initial
        // build from posting N redundant passes. Mirrors the Android
        // dynamic-update fix.
        let host_window: *mut objc2_app_kit::NSWindow =
            unsafe { msg_send![view, window] };
        if crate::layout_policy::reactive_change_needs_layout_pass(!host_window.is_null()) {
            schedule_layout_pass();
        }
    }

    /// Wire native hover/press events to the framework's per-node state
    /// machine. Only our `FlippedView` host (view / pressable / link) tracks
    /// them — `set_state_setter` installs an `NSTrackingArea` for hover and
    /// the view's `mouseDown`/`Up` drive press. Native controls (NSTextField,
    /// NSSwitch, …) render their own system states, so they're skipped.
    /// macOS has no touch, so this is the desktop analogue of web's CSS
    /// `:hover`/`:active`.
    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        if let Some(fv) = as_flipped_view(node.as_view()) {
            fv.set_state_setter(setter);
        }
    }

    fn set_animated_f32(
        &mut self,
        node: &Self::Node,
        prop: runtime_core::animation::AnimProp,
        value: f32,
    ) {
        animated::set_animated_f32(node, prop, value, &mut self.animated_states);
    }

    fn set_animated_color(
        &mut self,
        node: &Self::Node,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        // Gradient stop writes need the per-view cache (layer +
        // sRGB stops), so they route here at the backend level
        // rather than into the standalone `animated::set_animated_color`
        // helper. Background / foreground writes still go through
        // the helper.
        if let runtime_core::animation::AnimProp::GradientStopColor(idx) = prop {
            let view = node.as_view();
            let key = view as *const NSView as usize;
            if let Some(state) = self.gradient_states.get_mut(&key) {
                gradient::set_animated_gradient_stop(state, idx as usize, value);
            }
            return;
        }
        animated::set_animated_color(node, prop, value);
    }

    // -----------------------------------------------------------------
    // Accessibility
    // -----------------------------------------------------------------
    //
    // `dump_accessibility_tree` stays at its default (returns `None`).
    // AppKit walks each NSView's NSAccessibility attributes directly —
    // there's no parallel semantics tree to dump.

    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y_props: &runtime_core::accessibility::AccessibilityProps,
        inferred_role: Option<runtime_core::accessibility::Role>,
    ) {
        a11y::apply(node, a11y_props, inferred_role);
    }

    fn announce_for_accessibility(
        &mut self,
        msg: &str,
        priority: runtime_core::accessibility::LiveRegionPriority,
    ) {
        a11y::announce(msg, priority);
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        handles::make_view_handle(node)
    }

    /// Node's rect in its parent's coordinate system. The framework's
    /// views are layer-backed + flipped (top-left origin, matching Taffy),
    /// so `-[NSView frame]` reads in the same top-left space the layout
    /// pass writes. Enables the robot bridge's `get_frame` verb on macOS
    /// (used by the inspector + e2e drivers); previously the default
    /// `None` stub left every element frame-less here.
    fn frame(&self, node: &Self::Node) -> Option<runtime_core::primitives::portal::ViewportRect> {
        let view = node.as_view();
        let frame: CGRect = unsafe { msg_send![view, frame] };
        Some(runtime_core::primitives::portal::ViewportRect {
            x: frame.origin.x as f32,
            y: frame.origin.y as f32,
            width: frame.size.width as f32,
            height: frame.size.height as f32,
        })
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        handles::make_text_handle(node)
    }

    fn create_virtualizer(
        &mut self,
        callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Real NSCollectionView wrap with cell reuse — see
        // `imp/virtualizer.rs` for the AppKit-flavored adapter.
        // Mirrors iOS's UICollectionView pattern: data source +
        // delegate routed via a custom NSObject, items dequeued
        // from a reuse pool, per-item Scope released on
        // displayEnd / reuse / teardown.
        let view = virtualizer::create(
            self.mtm,
            &mut self.virtualizer_instances,
            callbacks,
            overscan,
            horizontal,
        );
        let _ = self.layout_for_view(&view);
        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            Some(runtime_core::accessibility::Role::List),
        );
        node
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        // Tell the NSCollectionView to `reloadData`. Future
        // performBatchUpdates optimisation lands when the iOS
        // counterpart does — same shape (key-keyed diff, batched
        // insert/remove ops).
        let view = node.as_view();
        virtualizer::data_changed(view);
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        // Mirrors iOS's teardown — disconnect dataSource/delegate so
        // queued AppKit events become no-ops, drain mounted scopes,
        // drop the side-state entry. Without this, AppKit's lingering
        // layout pass after the framework's scope drop would invoke
        // released `Signal` closures and panic.
        let view = node.as_view();
        virtualizer::release(&mut self.virtualizer_instances, view);
    }

    fn create_navigator(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        presentation: Rc<dyn std::any::Any>,
        host: runtime_core::primitives::navigator::NavigatorHost<Self::Node>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Consult the registry. SDK leaf crates (`drawer_navigator`,
        // `stack_navigator`, `tab_navigator`) call
        // `register_navigator::<TheirPresentation, _>(factory)` at
        // bootstrap; when one matches, we run `init` and stash the
        // handler under the returned view's pointer for subsequent
        // `navigator_attach_initial` / `release_navigator` /
        // `apply_navigator_slot_style` dispatches.
        //
        // No-match: render a visible placeholder noting WHICH kind
        // wasn't registered. This is the path for navigator SDKs
        // that haven't shipped a macOS handler yet — author code
        // running on macOS sees the missing wiring at runtime
        // (matching the External + placeholder posture across the
        // workspace; see `feedback_cpu_unsupported_placeholders`).
        if let Some(factory) = self.navigator_handlers.get(type_id) {
            let mut handler: Box<dyn runtime_core::NavigatorHandler<MacosBackend>> =
                (factory)();
            let node = handler.init(self, host, presentation);

            // Stash the handler keyed by the resolved node's NSView
            // pointer. Future trait calls
            // (`navigator_attach_initial`, etc.) look the handler up
            // by the same key.
            let view = node.as_view();
            let key = view as *const NSView as usize;
            self.nav_handler_instances.insert(
                key,
                std::rc::Rc::new(std::cell::RefCell::new(handler)),
            );

            a11y::apply(&node, a11y, None);
            return node;
        }

        let text = format!(
            "Navigator kind \"{type_name}\" not registered for macOS \
             — SDK leaf needs `register_navigator(&mut backend)` \
             on macOS targets (per `project_macos_navigator_design`)"
        );
        let ns = NSString::from_str(&text);
        let label: Retained<NSTextField> = unsafe {
            msg_send_id![objc2::class!(NSTextField), labelWithString: &*ns]
        };
        let cell: Retained<NSObject> = unsafe { msg_send_id![&label, cell] };
        let _: () = unsafe { msg_send![&cell, setWraps: true] };
        let _: () = unsafe { msg_send![&cell, setUsesSingleLineMode: false] };
        let red: Retained<NSColor> = unsafe {
            msg_send_id![objc2::class!(NSColor), systemRedColor]
        };
        let _: () = unsafe { msg_send![&label, setTextColor: &*red] };
        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&label) as *mut NSView)
                .expect("retain NSTextField as NSView")
        };
        let _ = self.layout_for_view(&view);
        let node = MacosNode::Label(label);
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        let view = node.as_view();
        let key = view as *const NSView as usize;
        if let Some(handler_cell) = self.nav_handler_instances.remove(&key) {
            // Run the SDK's `release` so it can drop native
            // resources. The handler's Box drops after the
            // borrow_mut block returns (the Rc's strong count
            // falls to zero once the map entry is gone).
            handler_cell.borrow_mut().release(self);
        }
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        let view = navigator.as_view();
        let key = view as *const NSView as usize;
        if let Some(handler_cell) = self.nav_handler_instances.get(&key).cloned() {
            handler_cell
                .borrow_mut()
                .attach_initial(self, screen, scope_id, options);
        }
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let view = node.as_view();
        let key = view as *const NSView as usize;
        if let Some(handler_cell) = self.nav_handler_instances.get(&key).cloned() {
            handler_cell
                .borrow_mut()
                .apply_slot_style(self, slot, style);
        }
    }

    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::navigator::NavigatorHandle {
        let view = node.as_view();
        let key = view as *const NSView as usize;
        if let Some(handler_cell) = self.nav_handler_instances.get(&key) {
            return handler_cell.borrow().make_handle();
        }
        // No handler — return the trait's default inert handle.
        // Author code that bound a `Ref<NavigatorHandle>` against
        // an unregistered navigator kind silently gets a no-op
        // handle (matches what the trait default would do).
        runtime_core::primitives::navigator::NavigatorHandle::new(
            std::rc::Rc::new(()),
            &NOOP_NAV_OPS,
        )
    }

    fn create_graphics(
        &mut self,
        on_ready: runtime_core::primitives::graphics::OnReady,
        on_resize: runtime_core::primitives::graphics::OnResize,
        on_lost: runtime_core::primitives::graphics::OnLost,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // CAMetalLayer-backed NSView wrapping a wgpu Surface.
        // Mirrors the iOS `imp/graphics.rs` pattern — MetalView
        // (NSView subclass with `-makeBackingLayer` returning
        // CAMetalLayer) + `raw_window_handle::AppKitWindowHandle`
        // provider + scheduled `on_ready` callback fired on the
        // next runloop turn once AppKit's layout has assigned a
        // real frame.
        let node = graphics::create_graphics(
            self.mtm,
            &mut self.callback_targets,
            on_ready,
            on_resize,
            on_lost,
        );
        if let MacosNode::View(view) = &node {
            let _ = self.layout_for_view(view);
        }
        a11y::apply(&node, a11y, None);
        node
    }

    fn create_icon(
        &mut self,
        data: &runtime_core::primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Real vector-path rendering via `CAShapeLayer` with a
        // CGPath built by the shared SVG parser in
        // `backend_apple_core::icon_path`. Identical render output
        // to iOS — both backends now drive off the same parser, so
        // a Lucide icon looks the same on iOS and macOS without
        // duplicated path-handling code.
        let view = icon::create_icon(self.mtm, data, color);

        // Pin a 24×24 default intrinsic size so flex layout gives
        // the icon a stable footprint matching iOS. Style-driven
        // overrides on `width`/`height` still win because
        // known_dimensions short-circuits the closure body.
        const ICON_SIZE: f32 = 24.0;
        let layout = self.layout_for_view(&view);
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(ICON_SIZE),
                    height: known_dimensions.height.unwrap_or(ICON_SIZE),
                }
            }),
        );

        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::Icon,
            ),
        );
        node
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        icon::update_icon_color(node, color);
    }

    fn create_portal(
        &mut self,
        target: runtime_core::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Full-viewport overlay attached to the host window's contentView. Scrim
        // styling, named-slot routing, on_dismiss event firing, and focus
        // trapping are deferred — match iOS's portal_instances surface when those
        // land. The container's flex style (from the placement) positions its
        // single content child within the viewport; FullScreen stretches it so a
        // self-centering child (idea-ui `Modal`) fills the window.
        let container_rules = portal_container_style(&target);

        let content = FlippedView::new(self.mtm);
        let content: Retained<NSView> = Retained::into_super(content);

        // Attach to the host window's contentView if available. The
        // overlay is added as the topmost subview so it draws above
        // the rest of the tree. If no host yet (mid-build), defer —
        // the layout pass / `set_host_root` will attach the parent
        // chain later. Here we still return the orphan content view
        // so the framework can `insert` children into it.
        if let Some(host) = self.host_root.as_ref() {
            unsafe { host.addSubview(&content) };
        }

        // Register the container as its own Taffy root, sized via the placement
        // flex style. It's an orphan (no Taffy parent — `insert` skips the
        // walker's reparent via `portal_roots`), so `compute_and_apply_layout`
        // computes it against the viewport (Auto axes fill to the window) and the
        // placement's justify/align positions the content child inside it.
        let layout_node = self.layout_for_view(&content);
        self.layout.set_style(layout_node, &container_rules);
        let portal_key = &*content as *const NSView as usize;
        self.portal_roots.insert(portal_key);

        // Hold a strong ref so a future `release_portal` can
        // detach it cleanly. Without a side-map entry, removal
        // would require walking the tree.
        let node = MacosNode::View(content);
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_portal(&mut self, node: &Self::Node) {
        // v1 cleanup: detach the overlay from its superview. Once
        // `portal_instances` lands (mirroring iOS's per-portal
        // entry map), we'll also drop any KVO observers / dismiss
        // gesture recognizers attached at create time.
        self.portal_roots.remove(&node.view_key());
        let view = node.as_view();
        unsafe { view.removeFromSuperview() };
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = if let Some(handler) = self.external_handlers.get(type_id) {
            handler(payload, self)
        } else {
            // No handler registered — render an explicit placeholder
            // so the missing SDK binding is visible at run-time. The
            // user-facing pattern for graceful degradation is
            // `backend.has_external::<T>()` BEFORE building the
            // primitive; this placeholder is the safety net for code
            // that mounts unconditionally.
            external_placeholder_node(self, type_name)
        };
        // Third-party externals declare their own role via
        // `props.role` if needed — we don't infer one here.
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_external(&mut self, node: &Self::Node) {
        // Detached window root (screen_recorder private layer): tear down its
        // borderless overlay window so it stops compositing when the layer
        // unmounts. `release_private_layer_window` returns early for any node
        // that isn't a registered detached root, so this is a cheap no-op for
        // every other external. Mirrors iOS. Future SDK leaves that hold
        // instance state would also clean up here, keyed by `view_key`.
        if self.detached_window_roots.contains_key(&node.view_key()) {
            self.release_private_layer_window(node);
        }
    }

    fn finish(&mut self, root: Self::Node) {
        // Compute layout against the host's bounds and walk every
        // registered view to assign frames. The iOS backend does the
        // same thing with `apply_frames`; this is the AppKit-flavored
        // equivalent.
        let host = match &self.host_root {
            Some(v) => v.clone(),
            None => {
                // Without a host root, treat the root node's NSView
                // as the host. Useful for one-off rendering tests.
                let view = root.as_view();
                unsafe {
                    Retained::retain(view as *const NSView as *mut NSView).expect("retain root")
                }
            }
        };

        // Parent the root view to the host's content view. The
        // framework's walker calls `insert(parent, child)` only for
        // child-of-child relationships within the user's primitive
        // tree — the *top* node has no framework parent, so it never
        // becomes a subview of the host without this explicit
        // `addSubview`. iOS does the same with `pin_to_edges`; AppKit
        // doesn't need Auto Layout here because we drive frames via
        // Taffy.
        let root_view = root.as_view();
        if !std::ptr::eq(root_view as *const NSView, &*host as *const NSView) {
            // Check if already a subview to avoid double-parenting on
            // re-renders. AppKit's `addSubview:` is idempotent for the
            // same parent (it reorders rather than dupes), but the
            // logical contract is clearer if we no-op when already
            // mounted.
            let superview: *mut NSView = unsafe { msg_send![root_view, superview] };
            if superview != (&*host as *const NSView as *mut NSView) {
                unsafe { host.addSubview(root_view) };
            }
        }

        // Read viewport from host bounds. If the host hasn't laid out
        // yet (bounds == 0×0 at first render before the window is
        // visible), fall back to the host's frame, then the window's
        // contentRectForFrameRect. The bail-on-zero in the iOS
        // backend works because UIKit lays out before render; AppKit
        // can run render before the first window paint.
        let mut bounds: CGRect = unsafe { msg_send![&host, bounds] };
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            let frame: CGRect = unsafe { msg_send![&host, frame] };
            bounds = frame;
        }
        let viewport = runtime_layout::Size {
            width: bounds.size.width as f32,
            height: bounds.size.height as f32,
        };
        // Mirror into the framework's reactive viewport signal so
        // `viewport_size()` subscribers (breakpoint hooks, responsive
        // containers, theme-cohort restyle) re-fire on window resize.
        //
        // CRITICAL: `finish` runs with the backend RefCell borrowed (the host
        // calls `backend.borrow_mut().finish(...)`), and `set_viewport_size`
        // notifies subscribers SYNCHRONOUSLY. On the first paint the viewport
        // changes from its default, which fires the breakpoint memo → theme-
        // cohort driver → `apply_style`, and `apply_style` re-borrows the
        // backend → "RefCell already borrowed" panic that aborts the process at
        // startup. So defer the mirror to a microtask: it runs after `finish`
        // returns and the borrow is released. This mirrors the Android backend
        // (`run_layout_pass`, which carries the same comment) and how iOS
        // mirrors the viewport from a UIKit resize callback rather than inside
        // layout. The local `viewport` below still drives THIS pass; only the
        // reactive signal mirror is deferred. Dedup by last-mirrored size so a
        // resize schedules exactly one microtask and the steady state none.
        if viewport.width > 0.0 && viewport.height > 0.0 {
            let next = (viewport.width, viewport.height);
            let changed = LAST_MIRRORED_VIEWPORT.with(|c| c.get()) != Some(next);
            if changed {
                LAST_MIRRORED_VIEWPORT.with(|c| c.set(Some(next)));
                runtime_core::schedule_microtask(move || {
                    runtime_core::set_viewport_size(runtime_core::ViewportSize {
                        width: next.0,
                        height: next.1,
                    });
                });
            }
        }
        if viewport.width <= 0.0 || viewport.height <= 0.0 {
            // Still nothing — nothing to compute against. The next
            // window resize will trigger a layout pass with real
            // bounds; bail rather than feeding Taffy zeros.
            backend_apple_core::log::apple_log(
                "[macos] finish: viewport is zero, skipping layout pass",
            );
            return;
        }

        // Compute layout starting from the root node.
        let Some(root_layout) = self.layout_of(root_view) else {
            return;
        };
        // Remember the root for post-mount passes (`run_layout_pass_global`):
        // reactive resizes after this `finish` have no `root` argument to hand
        // us, so they recompute from this stashed node instead.
        self.root_layout = Some(root_layout);
        self.compute_and_apply_layout(root_layout, viewport.width, viewport.height);
    }

    // ---------------------------------------------------------------
    // Asset / typeface registry hooks. Same forwarding as iOS, just
    // routed through the cross-Apple `FontRegistry` lifted in Phase 0.
    // ---------------------------------------------------------------

    fn register_asset(
        &mut self,
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
        source: &runtime_core::AssetSource,
    ) {
        // Font branch routes through `apple-core`'s shared registry;
        // image branch decodes `NSImage` into the per-backend cache
        // for `create_image` to resolve. Other asset tags are no-op
        // here (registered by future SDK leaves on demand).
        let _ = self.font_registry.register_asset(id, kind, source);
        image::register_asset(&mut self.image_cache, id, kind, source);
    }

    fn unregister_asset(
        &mut self,
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
    ) {
        self.font_registry.unregister_asset(id, kind);
    }

    fn register_typeface(
        &mut self,
        id: runtime_core::assets::TypefaceId,
        family_name: &str,
        faces: &[runtime_core::assets::TypefaceFace],
        fallback: runtime_core::assets::SystemFallback,
    ) {
        self.font_registry
            .register_typeface(id, family_name, faces, fallback);
    }

    fn unregister_typeface(&mut self, id: runtime_core::assets::TypefaceId) {
        self.font_registry.unregister_typeface(id);
    }
}

/// Build a visible "External X not supported on macOS" placeholder
/// for `create_external` when no handler is registered for the given
/// payload type. Mirrors the iOS placeholder pattern — single line
/// of red text so the dev / user immediately sees that a SDK binding
/// is missing rather than hitting an invisible empty rect.
fn external_placeholder_node(b: &mut MacosBackend, type_name: &'static str) -> MacosNode {
    let text = format!("External \"{type_name}\" not supported on macOS");
    let ns = NSString::from_str(&text);
    let label: Retained<NSTextField> = unsafe {
        msg_send_id![objc2::class!(NSTextField), labelWithString: &*ns]
    };
    // Multi-line wrap so a long type name (e.g. `webview::WebViewProps`)
    // doesn't get clipped.
    let cell: Retained<NSObject> = unsafe { msg_send_id![&label, cell] };
    let _: () = unsafe { msg_send![&cell, setWraps: true] };
    let _: () = unsafe { msg_send![&cell, setUsesSingleLineMode: false] };
    // System red — matches iOS placeholder intent.
    let red: Retained<NSColor> = unsafe {
        msg_send_id![objc2::class!(NSColor), systemRedColor]
    };
    let _: () = unsafe { msg_send![&label, setTextColor: &*red] };

    let view: Retained<NSView> = unsafe {
        Retained::retain(Retained::as_ptr(&label) as *mut NSView)
            .expect("retain NSTextField as NSView")
    };
    let _ = b.layout_for_view(&view);
    MacosNode::Label(label)
}


