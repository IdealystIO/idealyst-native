pub(crate) mod a11y;
pub(crate) mod animated;
pub mod callbacks;
pub(crate) mod graphics;
pub(crate) mod handles;
pub(crate) mod icon;
pub(crate) mod image;
pub(crate) mod portal;
pub(crate) mod phase_timer;
pub(crate) mod sticky;
pub(crate) mod text_inset;
pub(crate) mod touch;
pub(crate) mod virtualizer;

/// Platform log with format. Forwards to `backend_ios_core::ios_log`
/// which wraps NSLog.
#[allow(dead_code)]
macro_rules! ios_log {
    ($($arg:tt)*) => {
        backend_ios_core::ios_log(&format!($($arg)*))
    };
}

use runtime_core::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_core::primitives::graphics::{OnLost, OnReady, OnResize};
use runtime_core::primitives::link::LinkConfig;
use runtime_core::primitives::navigator::NavigatorOps;
use runtime_core::{Backend, Color, StyleRules};

/// No-op `NavigatorOps` returned by `make_navigator_handle` when no
/// SDK handler is stored for the requested node (e.g. the node id
/// doesn't appear in `nav_handler_instances`). Keeps the fallback
/// handle inert without panicking.
struct NoopNavOps;
impl NavigatorOps for NoopNavOps {}
static NOOP_NAV_OPS: NoopNavOps = NoopNavOps;
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::{
    UIActivityIndicatorView, UIActivityIndicatorViewStyle, UIButton, UIButtonType,
    UILabel, UIScrollView, UISlider, UISwitch,
    UITextField, UITextView, UIView, UIViewController,
};
use std::collections::HashMap;
use std::rc::Rc;

use callbacks::{
    BoolCallbackTarget, CallbackTarget, FloatCallbackTarget, StringCallbackTarget, TextKeyDelegate,
};
use backend_ios_core::style::{
    animate, apply_style_to_view, apply_text_style, color_to_uicolor,
};

// =========================================================================
// UIKit enum values we use via `msg_send`. UIKit's Swift/ObjC enums
// aren't exposed by `objc2_ui_kit` as numeric constants, so we mirror
// the raw integer values here with the named UIKit constant in scope.
// =========================================================================

/// `UIScrollViewContentInsetAdjustmentBehavior.never` — `contentInset`
/// is the only thing setting the inset; no auto-adjustment for
/// safeAreaInsets or scrollIndicators.
const SCROLL_VIEW_INSET_ADJUSTMENT_NEVER: i64 = 2;

/// `UIScrollViewContentInsetAdjustmentBehavior.always` — UIKit always
/// adds `safeAreaInsets` (status bar + nav bar + home indicator) to
/// the scroll view's effective content inset, regardless of position
/// or scroll-axis orientation.
const SCROLL_VIEW_INSET_ADJUSTMENT_ALWAYS: i64 = 3;

// =========================================================================
// IosBackend
// =========================================================================

pub struct IosBackend {
    mtm: MainThreadMarker,
    host_root: Option<Retained<UIView>>,
    callback_targets: Vec<Retained<NSObject>>,
    /// Set of view pointers that are UIScrollViews. Used in the
    /// post-layout pass to sync `contentSize` from Taffy children.
    scroll_views: std::collections::HashSet<usize>,
    /// Cache of rasterized icon UIImages keyed by (icon identity, size).
    /// Icon identity = pointer address of the `paths` static slice.
    /// Size = point size as u16 (half-point granularity is enough).
    /// Only used by `render_to_uiimage` — the standalone `create_icon`
    /// uses CAShapeLayer (vector, no raster needed).
    icon_image_cache: HashMap<(usize, u16), Retained<NSObject>>,
    /// Cache of decoded `UIImage`s for asset-backed `Image` primitives.
    /// Filled by [`Backend::register_asset`] when an
    /// `Asset<kinds::Image>` is observed; queried by
    /// [`Backend::create_image`] when the `src` is the
    /// `asset://{id}` sentinel.
    pub(crate) image_cache: image::ImageCache,
    /// Process-registered custom fonts + per-`Typeface` lookup table.
    /// Filled by [`Backend::register_asset`] for `AssetTag::Font`
    /// (CGFont → CTFontManager) and [`Backend::register_typeface`]
    /// (records PostScript name per (weight, style) face). Read by
    /// `apply_text_style` to build `UIFont(name:size:)`.
    pub(crate) font_registry: backend_ios_core::font::FontRegistry,
    /// Active portals keyed by container view pointer. Holds both
    /// viewport-placed and anchor-positioned portals — `PortalEntry`
    /// discriminates via its `anchor: Option<...>` field, and the
    /// tracker (CADisplayLink) lives on the same entry for the
    /// anchored case.
    portal_instances: portal::PortalInstances,
    /// Taffy-backed flex layout tree, parallel to the UIView tree.
    /// `view_to_layout` maps a view pointer to its layout node so the
    /// `apply_style` / `insert` / `finish` paths can update it. After
    /// build, `finish` calls `layout.compute(...)` and walks the
    /// UIView tree to set each subview's `frame`.
    pub(crate) layout: runtime_layout::LayoutTree,
    /// Map from view pointer (as key) to (retained reference, layout node).
    /// We hold a `Retained<UIView>` so the layout pass can iterate every
    /// registered view directly — recursing through `UIView.subviews`
    /// misses subtrees that aren't yet attached to the host (e.g. a
    /// `UINavigationController`'s top VC view, which only gets added
    /// on UIKit's first layout pass, after our `finish()` returns).
    pub(crate) view_to_layout: HashMap<usize, (Retained<UIView>, runtime_layout::LayoutNode)>,
    /// Last-applied Taffy frame per view, keyed by the same view
    /// pointer that keys `view_to_layout`. `apply_frames` consults
    /// this and skips the `setBounds:` / `setCenter:` / gradient /
    /// corner-radius / transform-percent sync trio when the new
    /// frame matches — every persistent hidden screen
    /// (LazyPersistent mount policy keeps them around for
    /// re-Selects) goes through the apply loop on every relayout,
    /// and their frames don't change, so writing the same bounds
    /// every pass burns N obj-c message sends per stale view per
    /// pass. Cumulative cost grew ~2–3 ms per navigation in
    /// profiling, dwarfing every other phase by round 4.
    pub(crate) applied_frames: HashMap<usize, (f32, f32, f32, f32)>,
    /// Viewport size at the last layout pass. When this changes
    /// (device rotation, window resize) every persistent root needs
    /// `mark_dirty` before the dirty-skip in `run_layout_pass_global`
    /// can safely opt out of computing clean roots — otherwise a
    /// rotation would leave hidden screens cached at the old
    /// dimensions and they'd render with stale sizes the moment the
    /// user navigates back.
    pub(crate) last_viewport: Option<(f32, f32)>,
    /// Per-view cached animation state. Mirrors the web backend's
    /// `animated_states` map; see [`animated`] for the routing
    /// from [`AnimProp`](runtime_core::animation::AnimProp) to
    /// UIKit setters and the rationale for caching the transform
    /// components.
    pub(crate) animated_states: animated::AnimatedStateMap,
    /// Registry of third-party `Element::External` handlers,
    /// populated by `register_external::<T>(...)` calls from
    /// per-platform leaf crates (e.g. `webview-ios::register`).
    /// `create_external` looks the handler up by payload TypeId;
    /// unregistered kinds fall through to a "not supported" placeholder
    /// UILabel.
    pub(crate) external_handlers:
        runtime_core::ExternalRegistry<IosBackend>,
    /// Registry of `Element::Navigator` handler factories.
    /// SDK leaf crates (`stack_navigator::register`, etc.) install
    /// factories keyed by their presentation TypeId.
    pub(crate) navigator_handlers:
        runtime_core::NavigatorRegistry<IosBackend>,
    /// Per-navigator-instance SDK handler. Keyed by the navigator
    /// container's `IosNode::view_key()`. `Backend::create_navigator`
    /// resolves the factory, runs `init`, and stores the returned
    /// handler here so subsequent `navigator_attach_initial` /
    /// `release_navigator` / `make_navigator_handle` /
    /// `apply_navigator_slot_style` trait methods can route through
    /// the handler's kind-specific logic instead of branching on a
    /// kind discriminant + calling per-kind inherent helpers.
    pub(crate) nav_handler_instances: HashMap<
        usize,
        std::rc::Rc<
            std::cell::RefCell<Box<dyn runtime_core::NavigatorHandler<IosBackend>>>,
        >,
    >,
    /// Per-virtualizer side state — keyed by the `UICollectionView`'s
    /// pointer. UIKit holds dataSource + delegate as weak refs, so
    /// we keep the `VirtualizerDataSource` retained here for the
    /// collection view's lifetime. `release_virtualizer` removes
    /// the entry; that drops the data source's `Retained`, which
    /// frees it after the next ObjC autorelease drain.
    pub(crate) virtualizer_instances:
        HashMap<usize, virtualizer::VirtualizerInstance>,
    /// Set of UICollectionView pointers we created via
    /// `create_virtualizer`. Listed separately from `scroll_views`
    /// because UICollectionView IS a UIScrollView (so `bounds.origin`
    /// = contentOffset and must be preserved across `apply_frames`),
    /// but its content layout is owned by `UICollectionViewLayout`,
    /// NOT by Taffy — meaning the contentSize-sync loop in
    /// `apply_frames` would otherwise compute `0×0` (no Taffy
    /// children registered for cells) and clobber UIKit's own
    /// contentSize. Membership in this set opts a view into
    /// origin-preservation but out of the contentSize sync.
    pub(crate) collection_views: std::collections::HashSet<usize>,
    /// `Position::Sticky` bookkeeping. Keyed by the enclosing
    /// `UIScrollView`'s pointer; the entry holds a `CADisplayLink`
    /// that re-evaluates each sticky child's translate against the
    /// scroll view's live `contentOffset` per vsync. See
    /// [`sticky`] for the rationale (side registry over UIScrollView
    /// subclass).
    pub(crate) sticky_registry: sticky::StickyRegistry,
    /// Sticky views whose `apply_style` ran BEFORE their first
    /// `insert`, so the superview walk couldn't yet find an
    /// enclosing `UIScrollView`. The walker calls `apply_style`
    /// (via `attach_style`) inside the per-primitive `build`, then
    /// the parent's `insert_children` does `backend.insert(...)`
    /// afterwards — so at apply-style time the child is still a
    /// detached floating view. We stash `(view_ptr, threshold)`
    /// here and complete the registration in `insert` once the
    /// view is actually in a parent chain. The map empties as
    /// each pending entry is promoted to the live registry.
    pub(crate) pending_sticky: std::collections::HashMap<usize, f32>,
    /// Content-view pointers of "detached window roots" — views that
    /// live in their OWN `UIWindow` (the `screen_recorder` private
    /// layer's ReplayKit-excluded overlay) rather than in the host's
    /// view tree. `insert` consults this to SKIP the native
    /// `addSubview` reparent when the External walker tries to splice
    /// such a root into its surrounding parent: the walker's
    /// `insert(parent, external_node)` would otherwise yank the
    /// window-root out of its private `UIWindow` and into the main
    /// (recorded) tree, defeating capture exclusion. Everything else
    /// proceeds normally — the root is still a Taffy root in
    /// `view_to_layout`, so the layout pass sizes it to the window
    /// (= viewport) and its children lay out inside it. The entry
    /// retains the owning `UIWindow` so it stays on screen for the
    /// layer's lifetime; dropping it (in `release_private_layer_window`)
    /// tears the window down.
    pub(crate) detached_window_roots:
        std::collections::HashMap<usize, Retained<NSObject>>,
}

// =========================================================================
// IosNode
// =========================================================================

#[derive(Clone)]
pub enum IosNode {
    View(Retained<UIView>),
    Label(Retained<UILabel>),
    Button(Retained<UIButton>),
    TextField(Retained<UITextField>),
    /// `TextArea` materialised as `UITextView` — the multi-line
    /// equivalent of `UITextField`. UITextView accepts newlines, has
    /// scrollable content, and uses a `UITextViewDelegate` rather
    /// than the target/action pattern UITextField uses for change
    /// notifications.
    TextView(Retained<objc2_ui_kit::UITextView>),
    Switch(Retained<UISwitch>),
    Slider(Retained<UISlider>),
    ScrollView(Retained<UIScrollView>),
    ActivityIndicator(Retained<UIActivityIndicatorView>),
}

impl IosNode {
    pub fn as_view(&self) -> &UIView {
        match self {
            IosNode::View(v) => v,
            IosNode::Label(l) => l,
            IosNode::Button(b) => b,
            IosNode::TextField(t) => t,
            IosNode::TextView(t) => t,
            IosNode::Switch(s) => s,
            IosNode::Slider(s) => s,
            IosNode::ScrollView(s) => s,
            IosNode::ActivityIndicator(a) => a,
        }
    }

    pub fn view_key(&self) -> usize {
        self.as_view() as *const UIView as usize
    }
}

// =========================================================================
// Global self-handle — lets navigator/drawer dispatch closures schedule
// a layout pass after they mount new screens. The framework's render
// flow only calls `finish()` once (initial render); subsequent pushes
// have to trigger layout themselves, and the closures don't capture
// the backend Rc otherwise.
// =========================================================================

thread_local! {
    static IOS_BACKEND_SELF: std::cell::RefCell<Option<std::rc::Weak<std::cell::RefCell<IosBackend>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the backend's self-reference. The user code (typically
/// `ios_main` in the example) must call this once after wrapping the
/// backend in `Rc<RefCell<>>` so subsequent navigation mounts can
/// reach back in and re-run layout. Without this, screens pushed
/// after initial render render with zero-sized children.
pub fn install_global_self(weak: std::rc::Weak<std::cell::RefCell<IosBackend>>) {
    IOS_BACKEND_SELF.with(|s| {
        *s.borrow_mut() = Some(weak);
    });
}

/// Run `f` with a mutable borrow of the installed global backend. Same
/// pattern as `set_animated_f32` / `set_animated_color` but exposed for
/// SDK code that needs to reach the backend outside the framework's
/// usual call paths (e.g. drawer-navigator's deferred sidebar attach,
/// fired from a `schedule_microtask` after `init` returns).
///
/// Returns `Some(f(...))` on success, `None` if the backend hasn't
/// been installed, has been dropped, or is currently borrowed by
/// another caller (the borrow_mut fails silently rather than
/// panicking). Callers that need the result should match on the
/// `Option`; otherwise it's fine to ignore.
pub fn with_backend<R>(f: impl FnOnce(&mut IosBackend) -> R) -> Option<R> {
    let weak = IOS_BACKEND_SELF.with(|s| s.borrow().clone())?;
    let rc = weak.upgrade()?;
    let mut b = rc.try_borrow_mut().ok()?;
    Some(f(&mut b))
}

/// Push a scalar animation property update to `node` on the
/// installed global backend. The cross-platform animation system's
/// per-frame subscribers reach the iOS backend through this — same
/// shape as `backend_web::set_animated_f32` but routed via the
/// global self-handle rather than a thread-local backend stash the
/// wrapper would have to wire up.
///
/// Quietly no-ops if no backend has been installed yet (pre-render)
/// or the install has been dropped (post-teardown), or if the
/// backend is already borrowed (the in-flight Rust call will see
/// the new value on its next frame).
pub fn set_animated_f32(node: &IosNode, prop: runtime_core::animation::AnimProp, value: f32) {
    let weak = IOS_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        use runtime_core::Backend;
        b.set_animated_f32(node, prop, value);
    };
}

/// Color-family counterpart of [`set_animated_f32`]. Routes through
/// the global backend's `set_animated_color`.
pub fn set_animated_color(
    node: &IosNode,
    prop: runtime_core::animation::AnimProp,
    value: [f32; 4],
) {
    let weak = IOS_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        use runtime_core::Backend;
        b.set_animated_color(node, prop, value);
    };
}

/// Schedule a fresh layout pass on the next main-queue turn. Safe to
/// call from anywhere on the main thread; no-op if the backend has
/// been dropped or no self ref is installed.
///
/// We **always defer** rather than running synchronously: many
/// callers are reached while the framework holds `backend.borrow_mut()`
/// (e.g. inside `Backend::insert` from the build walker). Running
/// `run_layout_pass_global` immediately in that state hits
/// `RefCell::try_borrow_mut` → `Err` and silently drops the pass.
/// Deferring to the next runloop turn ensures the framework's borrow
/// is released first.
thread_local! {
    /// Coalescing flag: set when a layout pass is queued but not yet
    /// fired. Subsequent `schedule_layout_pass()` calls are dropped
    /// until the queued pass clears it on entry. Without this, the
    /// build walker's many `Backend::insert` calls each post their
    /// own pass to libdispatch, producing N passes for one tree
    /// build (one per insertion) instead of one. The wasted passes
    /// fire back-to-back after the build returns, blocking the main
    /// thread for hundreds of ms on a large screen.
    static LAYOUT_PASS_QUEUED: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };

    /// Whether the current runloop turn has already executed a
    /// synchronous full-tree layout pass via `Backend::insert`'s
    /// window-attached fast-path. The first insert into a live parent
    /// still syncs (so `switch` / `when` toggles paint without flicker),
    /// but every subsequent insert in the same turn falls through to the
    /// coalesced `schedule_layout_pass()` instead — otherwise a fresh
    /// screen mount with N children does N full-tree layouts in a row.
    /// Cleared on the next libdispatch turn.
    static SYNC_LAYOUT_DONE_THIS_TURN: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Mark "a sync layout already ran this runloop turn" and arm a
/// libdispatch callback to clear the flag at the start of the next
/// turn. Idempotent — repeated calls in the same turn re-arm nothing.
fn arm_sync_layout_done_reset() {
    if SYNC_LAYOUT_DONE_THIS_TURN.with(|c| c.replace(true)) {
        return; // already armed
    }
    extern "C" {
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }
    extern "C" fn reset(_ctx: *mut std::ffi::c_void) {
        SYNC_LAYOUT_DONE_THIS_TURN.with(|c| c.set(false));
    }
    unsafe {
        dispatch_async_f(
            &_dispatch_main_q as *const _ as *const std::ffi::c_void,
            std::ptr::null_mut(),
            reset,
        );
    }
}

/// True iff a sync layout has already run this turn — caller should
/// fall through to the deferred path instead of triggering another
/// full-tree layout.
fn sync_layout_already_done_this_turn() -> bool {
    SYNC_LAYOUT_DONE_THIS_TURN.with(|c| c.get())
}

pub fn schedule_layout_pass() {
    if LAYOUT_PASS_QUEUED.with(|q| q.replace(true)) {
        // Already queued — drop this call. The pending pass will
        // pick up whatever state changes our caller just made.
        return;
    }
    extern "C" {
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }

    extern "C" fn trampoline(_ctx: *mut std::ffi::c_void) {
        // Clear the queued flag BEFORE running the pass. Any
        // `schedule_layout_pass` invocations that arrive during the
        // pass itself will re-arm and fire AFTER this one — they
        // reflect post-layout state we couldn't have captured here.
        LAYOUT_PASS_QUEUED.with(|q| q.set(false));
        // libdispatch is C and a Rust panic unwinding back into it is
        // undefined behavior. catch_unwind here only prints the panic
        // message before we abort \u{2014} project policy is crash-loud
        // so the layout pass never silently keeps running on top of
        // a partially-mutated reactive state.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let weak = IOS_BACKEND_SELF.with(|s| s.borrow().clone());
            let Some(weak) = weak else { return };
            let Some(rc) = weak.upgrade() else { return };
            // By the time this fires the original `borrow_mut()` should
            // have ended. If something else is mid-borrow we still bail
            // rather than panic.
            let mut backend = match rc.try_borrow_mut() {
                Ok(b) => b,
                Err(_) => return,
            };
            backend.run_layout_pass_global();
        }));
        if let Err(payload) = result {
            let msg = if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else {
                "<non-string panic payload>".to_string()
            };
            eprintln!("[backend-ios] layout-pass trampoline panic: {msg}");
            std::process::abort();
        }
    }

    unsafe {
        dispatch_async_f(
            &_dispatch_main_q as *const _ as *const std::ffi::c_void,
            std::ptr::null_mut(),
            trampoline,
        );
    }
}

/// Walk the subtree rooted at `view`, checking each subview's
/// pointer against `pending_sticky`. Any pending entry whose view
/// can now resolve a UIScrollView ancestor (i.e. the just-inserted
/// subtree is now wired into one) gets promoted into the live
/// registry via [`sticky::register`]. The view keys to remove from
/// `pending_sticky` are collected in `to_remove` so the caller can
/// drop them after the walk (avoids borrowing `pending_sticky`
/// mutably across the recursion).
///
/// Subtree walk (not just the root view): a `Element::View`
/// containing a `View { position: Sticky }` child will see the
/// outer View as `child_view` in `insert`, with the sticky child
/// nested inside. Both flagged in `pending_sticky` until this walk
/// promotes them.
fn promote_pending_sticky_recursive(
    mtm: MainThreadMarker,
    view: &UIView,
    pending: &mut std::collections::HashMap<usize, f32>,
    registry: &mut sticky::StickyRegistry,
    to_remove: &mut Vec<usize>,
) {
    let key = view as *const UIView as usize;
    if let Some(&threshold) = pending.get(&key) {
        if sticky::register(mtm, registry, view, threshold) {
            to_remove.push(key);
        }
        // If register returned false, the view STILL has no
        // scroll ancestor — leave it in `pending` so a future
        // re-parent (rare but possible) could pick it up.
    }
    let subs = view.subviews();
    for sub in subs.iter() {
        promote_pending_sticky_recursive(mtm, &sub, pending, registry, to_remove);
    }
}

// =========================================================================
// Helpers
// =========================================================================

impl IosBackend {
    pub fn new(mtm: MainThreadMarker) -> Self {
        phase_timer::install_core_bridge();
        Self {
            mtm,
            host_root: None,
            callback_targets: Vec::new(),
            scroll_views: std::collections::HashSet::new(),
            icon_image_cache: HashMap::new(),
            image_cache: HashMap::new(),
            font_registry: backend_ios_core::font::FontRegistry::new(),
            portal_instances: HashMap::new(),
            layout: runtime_layout::LayoutTree::new(),
            view_to_layout: HashMap::new(),
            applied_frames: HashMap::new(),
            last_viewport: None,
            animated_states: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
            navigator_handlers: runtime_core::NavigatorRegistry::new(),
            nav_handler_instances: HashMap::new(),
            virtualizer_instances: HashMap::new(),
            collection_views: std::collections::HashSet::new(),
            sticky_registry: HashMap::new(),
            pending_sticky: HashMap::new(),
            detached_window_roots: HashMap::new(),
        }
    }

    /// Register a handler for the third-party external primitive whose
    /// payload type is `T`. Called by per-platform leaf crates (e.g.
    /// `webview_ios::register`) during app bootstrap. The handler
    /// receives the typed payload + a mutable borrow of the backend
    /// and produces the `IosNode` to mount.
    pub fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&std::rc::Rc<T>, &mut IosBackend) -> IosNode + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }

    /// `true` if a handler for payload type `T` has been registered.
    /// Useful for opt-in graceful degradation in user code (render a
    /// static fallback if the SDK isn't available on iOS).
    pub fn has_external<T: 'static>(&self) -> bool {
        self.external_handlers.has::<T>()
    }

    /// Register a navigator-kind handler factory for the per-backend
    /// `NavigatorRegistry`. Mirrors `register_external` but for
    /// `Element::Navigator`. SDK leaf crates
    /// (`stack_navigator::register`, `tab_navigator::register`,
    /// `drawer_navigator::register`) call this once during app
    /// bootstrap.
    pub fn register_navigator<P, F>(&mut self, factory: F)
    where
        P: 'static,
        F: Fn() -> Box<dyn runtime_core::NavigatorHandler<IosBackend>> + 'static,
    {
        self.navigator_handlers.register::<P, _>(factory);
    }

    /// `MainThreadMarker` accessor for third-party SDK extension code
    /// that needs to construct main-thread-only Obj-C objects (e.g.
    /// `WKWebView::initWithFrame_configuration`). Mirrors the backend's
    /// internal `mtm` field; the marker is `Copy` so handing it out
    /// doesn't tie the SDK to the backend's borrow lifetime.
    pub fn mtm(&self) -> objc2_foundation::MainThreadMarker {
        self.mtm
    }

    /// SDK extension helper: register a UIView (or subclass) with the
    /// backend's Taffy layout tree so flex parents can size + position
    /// it. Third-party `register_external` handlers call this once
    /// after constructing their native view so the layout pass picks
    /// it up. Without it, the view is laid out as 0×0.
    ///
    /// The view's `frame` is written by `Backend::finish` /
    /// `apply_style`'s layout pass — leaf widgets that don't need a
    /// custom measure function are fully serviced by this call alone.
    pub fn register_external_view(&mut self, view: &objc2_ui_kit::UIView) {
        let _ = self.layout_for_view(view);
    }

    /// Get or create a layout node for a UIView. Called from every
    /// `create_*` method so each native view has a corresponding
    /// node in the layout tree.
    pub(crate) fn layout_for_view(&mut self, view: &UIView) -> runtime_layout::LayoutNode {
        let key = view as *const UIView as usize;
        if let Some((_, node)) = self.view_to_layout.get(&key) {
            return *node;
        }
        let node = self.layout.new_node();
        let retained = unsafe {
            Retained::retain(view as *const UIView as *mut UIView).expect("retain UIView")
        };
        self.view_to_layout.insert(key, (retained, node));
        node
    }

    /// Look up an existing layout node by view pointer. Returns
    /// `None` for views that weren't created by this backend
    /// (e.g. UIKit-internal scroll view internals).
    pub(crate) fn layout_of(&self, view: &UIView) -> Option<runtime_layout::LayoutNode> {
        let key = view as *const UIView as usize;
        self.view_to_layout.get(&key).map(|(_, n)| *n)
    }

    /// Create a capture-excluded overlay surface for the
    /// `screen_recorder` private layer and return its content view as
    /// an [`IosNode`]. The `screen_recorder` SDK calls this from its
    /// `PrivateLayer` external handler; the walker then parents the
    /// layer's children into the returned content view and tries to
    /// `insert` the content view into the surrounding tree — which
    /// `insert` skips because we register the content view as a
    /// detached window root.
    ///
    /// ## Why a separate `UIWindow` excludes it from the recording
    ///
    /// ReplayKit (`RPScreenRecorder.startCapture`) records the app's
    /// **key window only**. We build a second `UIWindow` at a high
    /// `windowLevel` (above the normal/alert key window), make it
    /// visible but deliberately **never** call `makeKeyAndVisible`, so
    /// it stays a non-key window — ReplayKit's capture omits it while
    /// the user still sees it composited on screen. This is the iOS
    /// "overlay on a separate UIWindow" trick the user shipped at
    /// Critiq; the orchestrator verifies the exclusion on a real
    /// device (a Simulator can't run ReplayKit capture). If the device
    /// run shows the layer IS captured, the fix is a property tweak on
    /// THIS window (e.g. a private capture-exclusion flag), not a
    /// change to the framework's tree handling.
    ///
    /// ## Layout
    ///
    /// The content view is registered in `view_to_layout`, making it a
    /// Taffy ROOT. `run_layout_pass_global` computes every root at the
    /// viewport size (`viewport_size()` == screen bounds here), so the
    /// content view fills the overlay window and the author positions
    /// the layer's controls inside it with normal flex/absolute style.
    ///
    /// ## Touch passthrough
    ///
    /// The content view is an [`OverlayPassthroughView`]: its
    /// `pointInside:` returns YES only over a child subview's frame, so
    /// taps that miss the private-layer content fall through this
    /// window to the app window beneath — the app stays interactive
    /// everywhere except where the layer's controls sit.
    pub fn create_private_layer_window(&mut self) -> IosNode {
        let (vw, vh) = self.viewport_size();
        let screen_bounds = objc2_foundation::CGRect {
            origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
            size: objc2_foundation::CGSize {
                width: vw as f64,
                height: vh as f64,
            },
        };

        // Content view = passthrough overlay. Hosting it as the window's
        // rootViewController.view means UIKit owns its lifecycle and
        // resizes it to the window; we additionally drive its frame via
        // the Taffy layout pass (it's a registered root).
        let content: Retained<UIView> = {
            let v = callbacks::OverlayPassthroughView::new(self.mtm);
            unsafe { Retained::cast::<UIView>(v) }
        };
        let _: () = unsafe { msg_send![&content, setFrame: screen_bounds] };
        // flexibleWidth (2) | flexibleHeight (16) — track the window on
        // rotation even before the next Taffy pass writes a frame.
        let _: () = unsafe { msg_send![&content, setAutoresizingMask: 0x12u64] };

        // Root view controller whose view IS the content view. A bare
        // UIWindow with no rootViewController logs a runtime warning and
        // declines to display; the VC is the documented host.
        let vc: Retained<NSObject> =
            unsafe { msg_send_id![msg_send_id![objc2::class!(UIViewController), alloc], init] };
        let _: () = unsafe { msg_send![&vc, setView: &*content] };

        // The separate UIWindow. Prefer the active window scene so the
        // window joins the same scene as the app (required on iOS 13+
        // for the window to actually display); fall back to the
        // frame-based initializer if no scene is resolvable.
        let window: Retained<NSObject> = unsafe {
            let window: Retained<NSObject> =
                msg_send_id![msg_send_id![objc2::class!(UIWindow), alloc], initWithFrame: screen_bounds];
            if let Some(scene) = self.active_window_scene() {
                let _: () = msg_send![&window, setWindowScene: &*scene];
            }
            window
        };

        // Clear background so the app shows through the passthrough
        // regions, and a high windowLevel so the overlay composites
        // above the key window. UIWindowLevelAlert is 2000; go above it.
        let clear: Retained<NSObject> =
            unsafe { msg_send_id![objc2::class!(UIColor), clearColor] };
        let _: () = unsafe { msg_send![&window, setBackgroundColor: &*clear] };
        // `windowLevel` is a CGFloat (f64 on 64-bit). Above
        // UIWindowLevelAlert (2000) so it sits over alerts/sheets too.
        let _: () = unsafe { msg_send![&window, setWindowLevel: 3000.0f64] };
        let _: () = unsafe { msg_send![&window, setRootViewController: &*vc] };
        // Make it VISIBLE but NOT key — `setHidden: NO` shows the
        // window without making it the key window. ReplayKit records
        // the key window only, so this stays excluded. (Calling
        // `makeKeyAndVisible` here would defeat the whole mechanism.)
        let _: () = unsafe { msg_send![&window, setHidden: false] };

        // Register the content view as a Taffy root + detached window
        // root so the layout pass sizes it and `insert` skips its
        // reparent. Retain the window on the entry so it lives as long
        // as the layer.
        self.register_external_view(&content);
        let key = &*content as *const UIView as usize;
        self.detached_window_roots.insert(key, window);

        // Kick a layout pass so the new root computes against the
        // viewport even though it never entered the main tree (the
        // window-attached-insert sync in `insert` won't fire for a
        // detached root).
        schedule_layout_pass();

        IosNode::View(content)
    }

    /// Tear down the private-layer overlay window created by
    /// [`Self::create_private_layer_window`]. Dropping the retained
    /// `UIWindow` removes it from the screen; we also drop the Taffy
    /// node + view-table entry so the next layout pass doesn't lay out
    /// a detached subtree (mirrors the portal `release` path).
    pub fn release_private_layer_window(&mut self, node: &IosNode) {
        let key = node.view_key();
        if self.detached_window_roots.remove(&key).is_none() {
            return;
        }
        // Hide the window before dropping so it stops compositing even
        // if some stray retain keeps the object alive briefly.
        if let Some((view, layout_node)) = self.view_to_layout.remove(&key) {
            let window: Option<Retained<NSObject>> =
                unsafe { msg_send_id![&*view, window] };
            if let Some(window) = window {
                let _: () = unsafe { msg_send![&window, setHidden: true] };
            }
            self.layout.remove_node(layout_node);
        }
        self.applied_frames.remove(&key);
    }

    /// Resolve the app's active `UIWindowScene` (iOS 13+) so the
    /// private-layer window can join the same scene as the app. Walks
    /// `UIApplication.sharedApplication.connectedScenes` for the first
    /// `UIWindowScene`. Returns `None` on older OSes or if no scene is
    /// foreground-active yet (the window then falls back to its
    /// frame-based init, which still displays on single-scene apps).
    fn active_window_scene(&self) -> Option<Retained<NSObject>> {
        unsafe {
            let app: Retained<NSObject> =
                msg_send_id![objc2::class!(UIApplication), sharedApplication];
            let scenes: Retained<objc2_foundation::NSSet<NSObject>> =
                msg_send_id![&app, connectedScenes];
            let enumerator: Retained<NSObject> = msg_send_id![&scenes, objectEnumerator];
            let window_scene_class = objc2::class!(UIWindowScene);
            loop {
                let next: Option<Retained<NSObject>> = msg_send_id![&enumerator, nextObject];
                let scene = next?;
                let is_window_scene: bool =
                    msg_send![&scene, isKindOfClass: window_scene_class];
                if is_window_scene {
                    return Some(scene);
                }
            }
        }
    }

    pub fn set_host_root(&mut self, view: Retained<UIView>) {
        // Attach a size-change observer so we re-run layout when the
        // host's bounds change (orientation flip, iPad split-view
        // resize, keyboard frame change, dynamic-island insets, etc.).
        // The observer is a zero-impact sibling at the bottom of the
        // host's subview list — invisible, user-interaction disabled,
        // and resized to match the host via autoresizing mask. UIKit
        // calls `layoutSubviews` on it whenever the host's bounds
        // change, which is our cue to dispatch a layout pass.
        let host_bounds: objc2_foundation::CGRect = unsafe { msg_send![&view, bounds] };
        let observer = callbacks::LayoutObserverView::new(self.mtm);
        let _: () = unsafe { msg_send![&observer, setFrame: host_bounds] };
        // autoresizingMask = .flexibleWidth | .flexibleHeight = 0x12
        let _: () = unsafe { msg_send![&observer, setAutoresizingMask: 0x12u64] };
        let _: () = unsafe { msg_send![&observer, setHidden: true] };
        let _: () = unsafe { msg_send![&observer, setUserInteractionEnabled: false] };
        unsafe { view.addSubview(&observer) };
        // Retain alongside other backend-owned ObjC objects so the
        // observer outlives this scope.
        let obj: Retained<NSObject> = unsafe {
            let ptr = Retained::as_ptr(&observer) as *mut NSObject;
            Retained::retain(ptr).unwrap()
        };
        self.callback_targets.push(obj);
        self.host_root = Some(view);
    }

    /// Install a Taffy `measure_fn` for an image view so flex layout
    /// reads its `intrinsicContentSize` (driven by the assigned
    /// `UIImage.size`) instead of collapsing it to 0×0. Re-installable
    /// — `update_image_src` calls this again after swapping the
    /// image so a new bitmap's size is picked up immediately.
    pub(crate) fn install_image_measure(&mut self, view: &objc2::rc::Retained<UIView>) {
        let layout = self.layout_for_view(view);
        let view_for_measure = view.clone();
        self.layout.set_measure_fn(
            layout,
            std::rc::Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: objc2_foundation::CGSize =
                    unsafe { msg_send![&view_for_measure, intrinsicContentSize] };
                let w = intrinsic.width as f32;
                let h = intrinsic.height as f32;
                // `intrinsicContentSize` is `{-1, -1}` (UIViewNoIntrinsicMetric)
                // before an image is assigned. Fall back to a zero
                // measurement in that case so Taffy doesn't try to
                // size the slot against a negative dimension.
                let w = if w < 0.0 { 0.0 } else { w };
                let h = if h < 0.0 { 0.0 } else { h };
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );
    }

    /// Install a Taffy `measure_fn` for an external primitive whose own
    /// view has no intrinsic size (e.g. a `UIScrollView`) but whose
    /// `content` subview does (e.g. the codeblock's `UILabel`). Without a
    /// measure the wrapper collapses to 0×0 in a flex column and the
    /// primitive renders blank — the bug that made the over-wire codeblock
    /// "missing outright" once its real handler ran (the old not-available
    /// placeholder was a self-sizing text node).
    ///
    /// We probe the content's `sizeThatFits:` at its natural (unbounded)
    /// size — single-axis scrollers don't wrap, so this yields the true
    /// content extent — and add `pad` on each side to match the scroll
    /// view's `contentInset`. The node fills any parent-known width and
    /// scrolls content wider than that.
    pub fn install_external_content_measure(
        &mut self,
        node: &objc2_ui_kit::UIView,
        content: &objc2_ui_kit::UIView,
        pad: f32,
    ) {
        let layout = self.layout_for_view(node);
        let content: objc2::rc::Retained<objc2_ui_kit::UIView> = unsafe {
            objc2::rc::Retained::retain(content as *const _ as *mut objc2_ui_kit::UIView)
                .unwrap()
        };
        self.layout.set_measure_fn(
            layout,
            std::rc::Rc::new(move |known_dimensions, available_space| {
                let probe = objc2_foundation::CGSize {
                    width: f64::MAX,
                    height: f64::MAX,
                };
                let fit: objc2_foundation::CGSize =
                    unsafe { msg_send![&content, sizeThatFits: probe] };
                let content_w = (fit.width as f32).max(0.0).ceil();
                let content_h = (fit.height as f32).max(0.0).ceil();
                // Fill the parent's offered width (so the scroller spans
                // the column and scrolls content wider than it); fall back
                // to the content's own width when the parent leaves it
                // unconstrained.
                let avail_w = match available_space.width {
                    runtime_layout::AvailableSpace::Definite(w) => Some(w),
                    _ => None,
                };
                runtime_layout::Size {
                    width: known_dimensions
                        .width
                        .or(avail_w)
                        .unwrap_or(content_w + 2.0 * pad),
                    height: known_dimensions
                        .height
                        .unwrap_or(content_h + 2.0 * pad),
                }
            }),
        );
    }

    /// Install a Taffy `measure_fn` for a standalone icon view so flex
    /// layout reserves the icon's intrinsic box. Without it the icon
    /// node had no size Taffy understood (the 24×24 Auto Layout
    /// constraints in `icon::create_icon` are invisible to Taffy): in a
    /// flex row the glyph collapsed to 0 width — letting the sibling
    /// label overlap it — and stretched on the cross axis. We report the
    /// CAShapeLayer's build size (`icon::DEFAULT_SIZE`) as the intrinsic
    /// size. An explicit `width`/`height` in the author's style still
    /// wins — Taffy uses a definite size over the measure result.
    pub(crate) fn install_icon_measure(&mut self, view: &objc2::rc::Retained<UIView>) {
        let layout = self.layout_for_view(view);
        self.layout.set_measure_fn(
            layout,
            std::rc::Rc::new(move |known_dimensions, _available_space| {
                let d = icon::DEFAULT_SIZE as f32;
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(d),
                    height: known_dimensions.height.unwrap_or(d),
                }
            }),
        );
    }

    fn retain_target<T: objc2::Message>(&mut self, target: &Retained<T>) {
        let obj: Retained<NSObject> = unsafe {
            let ptr = Retained::as_ptr(target) as *mut NSObject;
            Retained::retain(ptr).unwrap()
        };
        self.callback_targets.push(obj);
    }

    fn node_key(node: &IosNode) -> usize {
        node.as_view() as *const UIView as usize
    }
}

/// Pin `child` inside `parent` using Auto Layout (fills parent).
pub fn pin_to_edges(parent: &UIView, child: &UIView) {
    let _: () = unsafe {
        msg_send![child, setTranslatesAutoresizingMaskIntoConstraints: false]
    };
    unsafe { parent.addSubview(child) };

    let p_top: Retained<NSObject> = unsafe { msg_send_id![parent, topAnchor] };
    let p_bot: Retained<NSObject> = unsafe { msg_send_id![parent, bottomAnchor] };
    let p_lead: Retained<NSObject> = unsafe { msg_send_id![parent, leadingAnchor] };
    let p_trail: Retained<NSObject> = unsafe { msg_send_id![parent, trailingAnchor] };
    let c_top: Retained<NSObject> = unsafe { msg_send_id![child, topAnchor] };
    let c_bot: Retained<NSObject> = unsafe { msg_send_id![child, bottomAnchor] };
    let c_lead: Retained<NSObject> = unsafe { msg_send_id![child, leadingAnchor] };
    let c_trail: Retained<NSObject> = unsafe { msg_send_id![child, trailingAnchor] };

    for (a, b) in [(&c_top, &p_top), (&c_bot, &p_bot), (&c_lead, &p_lead), (&c_trail, &p_trail)] {
        let c: Retained<NSObject> = unsafe { msg_send_id![a, constraintEqualToAnchor: &**b] };
        let _: () = unsafe { msg_send![&c, setActive: true] };
    }
}

/// Mount a framework screen node into a UIViewController.
/// Pins to the safe area so content sits below the nav bar and
/// above the home indicator. The navigator's header_style slot
/// handles the nav bar background color separately.
pub fn mount_screen_in_vc(mtm: MainThreadMarker, screen: &UIView) -> Retained<UIViewController> {
    let vc = unsafe { UIViewController::new(mtm) };
    let vc_view = vc.view().expect("vc.view");

    let _: () = unsafe {
        objc2::msg_send![screen, setTranslatesAutoresizingMaskIntoConstraints: false]
    };
    unsafe { vc_view.addSubview(screen) };

    // Pin to the VC view's *edges* (not safeAreaLayoutGuide) so the
    // screen fills the nav controller's content area edge-to-edge.
    // Per-screen safe-area handling is the screen's job — a `View`
    // opts in via `.safe_area(...)` (outer padding); a `ScrollView`
    // opts in via the same method (UIKit-native contentInset, so
    // content slides under the nav bar when scrolled). Double-inset
    // is no longer possible because the framework only applies the
    // safe area at one place: wherever the author opted in.
    //
    // Pin all four edges (not just top/leading) so screens with a
    // zero-intrinsic child — UIScrollView, Graphics surface — get a
    // concrete height instead of collapsing to fit intrinsic
    // siblings.
    let v_top: Retained<NSObject> = unsafe { msg_send_id![&vc_view, topAnchor] };
    let v_bot: Retained<NSObject> = unsafe { msg_send_id![&vc_view, bottomAnchor] };
    let v_lead: Retained<NSObject> = unsafe { msg_send_id![&vc_view, leadingAnchor] };
    let v_trail: Retained<NSObject> = unsafe { msg_send_id![&vc_view, trailingAnchor] };
    let s_top: Retained<NSObject> = unsafe { msg_send_id![screen, topAnchor] };
    let s_bot: Retained<NSObject> = unsafe { msg_send_id![screen, bottomAnchor] };
    let s_lead: Retained<NSObject> = unsafe { msg_send_id![screen, leadingAnchor] };
    let s_trail: Retained<NSObject> = unsafe { msg_send_id![screen, trailingAnchor] };

    for (a, b) in [(&s_top, &v_top), (&s_bot, &v_bot), (&s_lead, &v_lead), (&s_trail, &v_trail)] {
        let c: Retained<NSObject> = unsafe { msg_send_id![a, constraintEqualToAnchor: &**b] };
        let _: () = unsafe { msg_send![&c, setActive: true] };
    }

    vc
}

// `apply_header_options` / `apply_header_options_with_nav` and the
// per-kind navigator inherent helpers (`create_stack_navigator`, etc.)
// moved to the `ios-navigator-helpers` crate as part of the navigator-
// substrate refactor. The framework reaches the helpers through the
// per-instance SDK handlers stashed on `nav_handler_instances`.

// =========================================================================
// Backend trait implementation
// =========================================================================

impl Backend for IosBackend {
    type Node = IosNode;

    fn platform(&self) -> runtime_core::Platform {
        // `target_abi = "sim"` is set for `aarch64-apple-ios-sim` and
        // `x86_64-apple-ios` simulator targets; absent on real devices.
        // The sim self-reports via `Custom("Sim")` so author code
        // (and the welcome example) can distinguish it from a real
        // device — there's no separate `is_simulator` signal, just
        // the `Platform` value the backend returns.
        if cfg!(all(target_os = "ios", target_abi = "sim")) {
            runtime_core::Platform::Custom("Sim")
        } else {
            runtime_core::Platform::Ios
        }
    }

    fn url_opener(&self) -> Option<std::rc::Rc<dyn Fn(&str)>> {
        Some(std::rc::Rc::new(|url: &str| {
            // [[UIApplication sharedApplication] openURL:] hands the URL
            // to the system (Safari, Mail, the app registered for the
            // scheme). Raw msg_send + class!() — same style as
            // `color_scheme` below — so no extra objc2-ui-kit typed
            // feature is needed. We use the single-arg form rather than
            // openURL:options:completionHandler: to keep the call ABI
            // trivially correct (one object arg in, BOOL out, no block
            // to marshal). Must run on the main thread; `open_url` is
            // only invoked from main-thread event handlers.
            let ns_url_str = NSString::from_str(url);
            let url_obj: *mut NSObject =
                unsafe { msg_send![objc2::class!(NSURL), URLWithString: &*ns_url_str] };
            if url_obj.is_null() {
                return;
            }
            let app: *mut NSObject =
                unsafe { msg_send![objc2::class!(UIApplication), sharedApplication] };
            if app.is_null() {
                return;
            }
            let _: bool = unsafe { msg_send![app, openURL: url_obj] };
        }))
    }

    fn color_scheme(&self) -> runtime_core::ColorScheme {
        // UITraitCollection.currentTraitCollection.userInterfaceStyle
        // 0 = Unspecified, 1 = Light, 2 = Dark (UIUserInterfaceStyle).
        let tc: Retained<NSObject> =
            unsafe { msg_send_id![objc2::class!(UITraitCollection), currentTraitCollection] };
        let style: isize = unsafe { msg_send![&tc, userInterfaceStyle] };
        match style {
            1 => runtime_core::ColorScheme::Light,
            2 => runtime_core::ColorScheme::Dark,
            _ => runtime_core::ColorScheme::Auto,
        }
    }

    fn create_view(&mut self, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        // IdealystTouchView is a UIView subclass that overrides the
        // four `touchesBegan:/Moved:/Ended:/Cancelled:` entry points
        // so a later `install_touch_handler` can attach a raw-touch
        // handler without re-creating the view. Views with no
        // handler installed pay one extra method dispatch per touch
        // event (the override calls super immediately) — touch
        // events fire only during active gestures so this isn't
        // hot. See `imp/touch.rs` and `docs/native-touch-backends-plan.md`.
        //
        // Children are positioned via Taffy-computed frames in
        // `finish`. We no longer use UIStackView — its arranged-
        // subview model fights with flex semantics (no flex-grow/
        // shrink, no wrap, zero-intrinsic-collapsing children,
        // opaque constraint conflicts).
        let touch_view = touch::IdealystTouchView::new(self.mtm);
        // `translatesAutoresizingMaskIntoConstraints` defaults to
        // YES on `[UIView new]`, which is what we want — frame
        // assignment becomes authoritative.
        let view: Retained<UIView> = Retained::into_super(touch_view);
        let _ = self.layout_for_view(&view);
        let node = IosNode::View(view);
        a11y::apply(&node, a11y, None);
        node
    }

    fn create_text(&mut self, content: &str, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        // `IdealystLabel` is a UILabel subclass with per-side text
        // insets. The framework's `StyleRules.padding_*` values get
        // applied by Taffy as the node's padding rect, which insets
        // children inside their parent's bounds — but UILabel has no
        // children, so Taffy padding would otherwise just grow the
        // label's outer frame without pushing the glyphs in. The
        // subclass honors the insets in `drawText(in:)`,
        // `sizeThatFits:`, and `intrinsicContentSize`. See
        // `imp::text_inset` for the override details.
        let custom_label = text_inset::IdealystLabel::new(self.mtm);
        let label: Retained<UILabel> = Retained::into_super(custom_label);
        let ns_text = NSString::from_str(content);
        unsafe { label.setText(Some(&ns_text)) };
        let _: () = unsafe { msg_send![&label, setNumberOfLines: 0isize] };
        // UILabel's default `lineBreakMode` is `byTruncatingTail` —
        // any line wider than the assigned frame becomes "…". That
        // makes us vulnerable to the case where our Taffy
        // `measure_fn` returns a width fractionally smaller than
        // what `sizeThatFits:` would round up to (e.g. 19.5 → 19),
        // and the label silently ellipsizes instead of wrapping.
        // `byWordWrapping` (= 0) wraps to additional lines when a
        // line is too wide, which combined with `numberOfLines: 0`
        // gives us "size to content, never truncate". The
        // measure_fn's height return value tells Taffy how much
        // vertical space the wrapped text needs.
        let _: () = unsafe { msg_send![&label, setLineBreakMode: 0isize] };

        // Install a measure function so Taffy can ask UILabel how
        // tall it needs to be for a given available width. Without
        // this, multi-line text gets sized to its single-line
        // intrinsicContentSize (one line ~1300pt wide for a paragraph),
        // which both prevents wrap and breaks every flex sibling
        // around it.
        let layout = self.layout_for_view(&label);
        let label_for_measure = label.clone();
        self.layout.set_measure_fn(
            layout,
            std::rc::Rc::new(move |known_dimensions, available_space| {
                let avail_w = known_dimensions
                    .width
                    .unwrap_or(match available_space.width {
                        runtime_layout::AvailableSpace::Definite(w) => w,
                        runtime_layout::AvailableSpace::MaxContent => f32::INFINITY,
                        runtime_layout::AvailableSpace::MinContent => 0.0,
                    });
                let avail_h = known_dimensions
                    .height
                    .unwrap_or(match available_space.height {
                        runtime_layout::AvailableSpace::Definite(h) => h,
                        runtime_layout::AvailableSpace::MaxContent => f32::INFINITY,
                        runtime_layout::AvailableSpace::MinContent => 0.0,
                    });

                // Ask UIKit how the label fits in this much space.
                // sizeThatFits: returns the smallest rect needed to
                // render the current text+font, wrapping within the
                // given width.
                let target = objc2_foundation::CGSize {
                    width: if avail_w.is_finite() { avail_w as f64 } else { f64::MAX },
                    height: if avail_h.is_finite() { avail_h as f64 } else { f64::MAX },
                };
                let fitted: objc2_foundation::CGSize =
                    unsafe { msg_send![&label_for_measure, sizeThatFits: target] };
                // Ceil to whole points. `sizeThatFits:` returns a
                // theoretical fit (often fractional); the assigned
                // frame rounds when rendered, which can clip the
                // last character/line by a fractional point. Always
                // round up so the frame is at least the size the
                // text needs.
                let result = runtime_layout::Size {
                    width: known_dimensions
                        .width
                        .unwrap_or((fitted.width as f32).ceil()),
                    height: known_dimensions
                        .height
                        .unwrap_or((fitted.height as f32).ceil()),
                };
                result
            }),
        );

        let node = IosNode::Label(label);
        // Text role has no first-class UIAccessibilityTrait equivalent
        // — the helper emits nothing role-derived for it. Hint /
        // identifier / live_region label still apply.
        a11y::apply(&node, a11y, None);
        node
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &runtime_core::Action,
        leading_icon: Option<&runtime_core::IconData>,
        _trailing_icon: Option<&runtime_core::IconData>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let button = unsafe {
            UIButton::buttonWithType(UIButtonType::System, self.mtm)
        };
        let ns_label = NSString::from_str(label);
        let _: () = unsafe { msg_send![&button, setTitle: &*ns_label, forState: 0u64] };

        // Leading icon → UIButton.setImage (renders before title).
        if let Some(icon_data) = leading_icon {
            let image = icon::render_to_uiimage(
                icon_data, 20.0, &mut self.icon_image_cache,
            );
            let _: () = unsafe { msg_send![&button, setImage: &*image, forState: 0u64] };
        }

        let target = CallbackTarget::new(self.mtm, on_click.fire.clone());
        let sel = objc2::sel!(invoke);
        let _: () = unsafe {
            msg_send![&button, addTarget: &*target, action: sel, forControlEvents: 64u64]
        };
        self.retain_target(&target);

        // Install a measure function so Taffy queries UIButton's
        // sizeThatFits: at compute time. We can't use a static
        // intrinsicContentSize captured here — it reports a tiny
        // default (~32×16) for a freshly-constructed UIButton because
        // the button hasn't been mounted and its title-based layout
        // hasn't materialized yet. By the time Taffy calls the
        // measure_fn during layout, the title + font are set and
        // sizeThatFits: returns the real content size.
        let layout = self.layout_for_view(&button);
        let button_for_measure = button.clone();
        self.layout.set_measure_fn(
            layout,
            std::rc::Rc::new(move |known_dimensions, available_space| {
                let avail_w = known_dimensions
                    .width
                    .unwrap_or(match available_space.width {
                        runtime_layout::AvailableSpace::Definite(w) => w,
                        runtime_layout::AvailableSpace::MaxContent => f32::INFINITY,
                        runtime_layout::AvailableSpace::MinContent => 0.0,
                    });
                let avail_h = known_dimensions
                    .height
                    .unwrap_or(match available_space.height {
                        runtime_layout::AvailableSpace::Definite(h) => h,
                        runtime_layout::AvailableSpace::MaxContent => f32::INFINITY,
                        runtime_layout::AvailableSpace::MinContent => 0.0,
                    });
                let target = objc2_foundation::CGSize {
                    width: if avail_w.is_finite() { avail_w as f64 } else { f64::MAX },
                    height: if avail_h.is_finite() { avail_h as f64 } else { f64::MAX },
                };
                let fitted: objc2_foundation::CGSize =
                    unsafe { msg_send![&button_for_measure, sizeThatFits: target] };
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(fitted.width as f32),
                    height: known_dimensions.height.unwrap_or(fitted.height as f32),
                }
            }),
        );

        let node = IosNode::Button(button);
        // UIButton implicitly has the Button trait; we still call
        // apply so author label/hint/identifier/state flags override
        // UIKit's defaults.
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        if let IosNode::Button(button) = node {
            let ns = NSString::from_str(label);
            let _: () = unsafe { msg_send![button, setTitle: &*ns, forState: 0u64] };
        }
        // Same reasoning as `update_text` — the button's intrinsic
        // content size depends on its label, and Taffy caches.
        let view = node.as_view();
        if let Some(layout) = self.layout_of(view) {
            self.layout.mark_dirty(layout);
            schedule_layout_pass();
        }
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        secure: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let field = unsafe { UITextField::new(self.mtm) };
        let ns_val = NSString::from_str(initial_value);
        unsafe { field.setText(Some(&ns_val)) };

        // Password masking: UITextField renders dots for typed chars
        // when secure text entry is on.
        let _: () =
            unsafe { msg_send![&field, setSecureTextEntry: objc2::runtime::Bool::new(secure)] };

        if let Some(ph) = placeholder {
            let ns_ph = NSString::from_str(ph);
            unsafe { field.setPlaceholder(Some(&ns_ph)) };
        }

        let _: () = unsafe { msg_send![&field, setBorderStyle: 3isize] };

        let target = StringCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(invoke:);
        let _: () = unsafe {
            msg_send![&field, addTarget: &*target, action: sel, forControlEvents: 131072u64]
        };
        self.retain_target(&target);

        // Keydown bridge — UITextField's delegate's
        // `shouldChangeCharactersInRange:` fires for every keystroke
        // (Tab/Enter/printable/Backspace) before UIKit applies the
        // change. The delegate carries `None` for on_change because
        // UITextField already reports change via target/action above.
        if let Some(handler) = on_key_down {
            let delegate = TextKeyDelegate::new(self.mtm, Some(handler), None);
            let _: () = unsafe { msg_send![&field, setDelegate: &*delegate] };
            self.retain_target(&delegate);
        }

        let node = IosNode::TextField(field);
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        if let IosNode::TextField(field) = node {
            let current: Option<Retained<NSString>> = unsafe { msg_send_id![field, text] };
            let current_str = current.map(|ns| ns.to_string()).unwrap_or_default();
            if current_str != value {
                let ns = NSString::from_str(value);
                unsafe { field.setText(Some(&ns)) };
            }
        }
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        wrap: bool,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // UITextView is the multi-line equivalent of UITextField. It
        // ships with `editable: true` already, so we don't need to
        // flip it. Note: UITextView has no `placeholder` property —
        // matching the framework primitive shape requires a manual
        // overlay-label hack; for v1 we accept the `placeholder` arg
        // but ignore it (callers shouldn't depend on placeholder
        // text rendering on iOS yet — flagged on Element::TextArea).
        let view: Retained<UITextView> = unsafe { UITextView::new(self.mtm) };
        let ns_val = NSString::from_str(initial_value);
        unsafe { view.setText(Some(&ns_val)) };

        // Wrapping: UITextView wraps by default. For the code-editor
        // shape (`wrap == false`) the text container must stop tracking
        // the view width and the layout must not break lines, so long
        // lines extend horizontally (UITextView then scrolls them).
        if !wrap {
            unsafe {
                let container: Retained<NSObject> = msg_send_id![&view, textContainer];
                let _: () = msg_send![&container, setWidthTracksTextView: false];
                let huge = objc2_foundation::CGSize { width: f64::MAX, height: f64::MAX };
                let _: () = msg_send![&container, setSize: huge];
                // `NSLineBreakByClipping` (= 2): don't wrap, clip the
                // line — the container scrolls to reveal the rest.
                let _: () = msg_send![&container, setLineBreakMode: 2isize];
            }
        }

        // One delegate carries BOTH on_change (via textViewDidChange:)
        // and on_key_down (via shouldChangeTextInRange:). UITextView
        // has no target/action editing-changed event; the delegate is
        // the only canonical change-notification path.
        let delegate = TextKeyDelegate::new(self.mtm, on_key_down, Some(on_change));
        let _: () = unsafe { msg_send![&view, setDelegate: &*delegate] };
        self.retain_target(&delegate);

        // Intrinsic content sizing (only in wrap mode — a code editor
        // is a fixed-height scroller, like the web `wrap == off` path).
        // Drive the view's height from its content via a Taffy
        // `measure_fn` (`sizeThatFits:`), exactly the UILabel / UIButton
        // intrinsic-size pattern. With no height pinned the box grows to
        // fit; with a `max_height` on the style Taffy clamps it and the
        // content scrolls (UITextView keeps its default
        // `scrollEnabled = true`, which only bites once the frame is
        // shorter than the content); with a pinned `height` Taffy
        // ignores the measured height entirely. `update_text_area_value`
        // re-measures on change, exactly like `update_text` for labels.
        if wrap {
            let layout = self.layout_for_view(&view);
            let view_for_measure = view.clone();
            self.layout.set_measure_fn(
                layout,
                std::rc::Rc::new(move |known_dimensions, available_space| {
                    let avail_w = known_dimensions
                        .width
                        .unwrap_or(match available_space.width {
                            runtime_layout::AvailableSpace::Definite(w) => w,
                            runtime_layout::AvailableSpace::MaxContent => f32::INFINITY,
                            runtime_layout::AvailableSpace::MinContent => 0.0,
                        });
                    // Ask for the height the text needs at this width,
                    // height unbounded — Taffy applies the max clamp.
                    let target = objc2_foundation::CGSize {
                        width: if avail_w.is_finite() { avail_w as f64 } else { f64::MAX },
                        height: f64::MAX,
                    };
                    let fitted: objc2_foundation::CGSize =
                        unsafe { msg_send![&view_for_measure, sizeThatFits: target] };
                    runtime_layout::Size {
                        width: known_dimensions.width.unwrap_or((fitted.width as f32).ceil()),
                        height: known_dimensions.height.unwrap_or((fitted.height as f32).ceil()),
                    }
                }),
            );
        }

        let node = IosNode::TextView(view);
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        if let IosNode::TextView(view) = node {
            let current: Option<Retained<NSString>> = unsafe { msg_send_id![view, text] };
            let current_str = current.map(|ns| ns.to_string()).unwrap_or_default();
            if current_str != value {
                let ns = NSString::from_str(value);
                unsafe { view.setText(Some(&ns)) };
            }
        }
        // Same invalidation as `update_text`: setting the text changes
        // the widget's intrinsic content size, but UIKit doesn't tell
        // Taffy. Mark the node dirty so its `measure_fn` re-runs on the
        // next (coalesced) layout pass. This is what makes a content-
        // sized (wrapping) textview track its text; a code-mode textview
        // has no measure_fn, so the re-layout reproduces its style-given
        // size — harmless.
        let view = node.as_view();
        if let Some(layout) = self.layout_of(view) {
            self.layout.mark_dirty(layout);
            schedule_layout_pass();
        }
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let switch = unsafe { UISwitch::new(self.mtm) };
        unsafe { switch.setOn_animated(initial_value, false) };

        let target = BoolCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(invoke:);
        let _: () = unsafe {
            msg_send![&switch, addTarget: &*target, action: sel, forControlEvents: 4096u64]
        };
        self.retain_target(&target);

        // Install an intrinsic-size measurer so Taffy gives the
        // UISwitch a real frame (≈51×31). Without it, Taffy assigns
        // 0×0 — UISwitch still *draws* at its intrinsic size (UIKit
        // doesn't clip rendering to bounds), but its hit-test region
        // is the empty bounds rect, so every tap slides off and the
        // switch never fires its `valueChanged` event.
        let layout = self.layout_for_view(&switch);
        let switch_for_measure = switch.clone();
        self.layout.set_measure_fn(
            layout,
            std::rc::Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: objc2_foundation::CGSize =
                    unsafe { msg_send![&switch_for_measure, intrinsicContentSize] };
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(intrinsic.width as f32),
                    height: known_dimensions.height.unwrap_or(intrinsic.height as f32),
                }
            }),
        );

        let node = IosNode::Switch(switch);
        // UISwitch already exposes the "switch" role to UIKit via its
        // implicit ToggleButton trait. apply() folds in the author's
        // CHECKED/DISABLED/etc. on top.
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        if let IosNode::Switch(switch) = node {
            let current: bool = unsafe { msg_send![switch, isOn] };
            if current != value {
                unsafe { switch.setOn_animated(value, true) };
            }
        }
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Plain UIScrollView, frame-based. Children are added
        // directly as subviews (no inner UIStackView). Their frames
        // come from Taffy via `apply_frames`. We sync the scroll
        // view's `contentSize` to the bounding rect of its Taffy
        // children at the end of every layout pass so scrolling
        // works.
        let scroll = unsafe { UIScrollView::new(self.mtm) };

        // Always allow scroll gestures even when content fits — UIKit
        // otherwise disables them when contentSize ≤ bounds, which
        // makes the scroll view feel "dead" when content happens to
        // be short. Matches typical iOS app behavior (Settings, Mail).
        if horizontal {
            let _: () = unsafe { msg_send![&scroll, setAlwaysBounceHorizontal: true] };
        } else {
            let _: () = unsafe { msg_send![&scroll, setAlwaysBounceVertical: true] };
        }

        // Wire `on_scroll` via UIScrollViewDelegate. The delegate
        // forwards `scrollViewDidScroll:` into the Rust closure with
        // (contentOffset.x, contentOffset.y) in UIKit points \u{2014}
        // same units as the web backend's CSS-pixel offset.
        if let Some(cb) = on_scroll {
            let delegate = crate::imp::callbacks::ScrollDelegate::new(self.mtm, cb);
            let _: () = unsafe { msg_send![&scroll, setDelegate: &*delegate] };
            self.retain_target(&delegate);
        }

        let _ = self.layout_for_view(&scroll);
        let key = &*scroll as *const UIScrollView as *const UIView as usize;
        self.scroll_views.insert(key);
        let node = IosNode::ScrollView(scroll);
        // ScrollView has no first-class role — UIKit handles the
        // scrolling chrome itself. apply() still writes label / hint /
        // identifier when set, which lets authors mark a scroll
        // container (e.g. a TabPanel) for assistive tech.
        a11y::apply(&node, a11y, None);
        node
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
        let slider = unsafe { UISlider::new(self.mtm) };
        unsafe {
            slider.setMinimumValue(min);
            slider.setMaximumValue(max);
            slider.setValue_animated(initial_value, false);
        };

        let target = FloatCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(invoke:);
        let _: () = unsafe {
            msg_send![&slider, addTarget: &*target, action: sel, forControlEvents: 4096u64]
        };
        self.retain_target(&target);

        // Same intrinsic-size measurer rationale as `create_toggle`
        // — UISlider has a real `intrinsicContentSize` but Taffy
        // doesn't know about it. Without this, a sliderup with no
        // explicit width style hit-tests against an empty rect.
        let layout = self.layout_for_view(&slider);
        let slider_for_measure = slider.clone();
        self.layout.set_measure_fn(
            layout,
            std::rc::Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: objc2_foundation::CGSize =
                    unsafe { msg_send![&slider_for_measure, intrinsicContentSize] };
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(intrinsic.width as f32),
                    height: known_dimensions.height.unwrap_or(intrinsic.height as f32),
                }
            }),
        );

        let node = IosNode::Slider(slider);
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        if let IosNode::Slider(slider) = node {
            unsafe { slider.setValue_animated(value, true) };
        }
    }

    fn create_activity_indicator(
        &mut self,
        size: ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let style = match size {
            ActivityIndicatorSize::Small => UIActivityIndicatorViewStyle::Medium,
            ActivityIndicatorSize::Large => UIActivityIndicatorViewStyle::Large,
        };
        let indicator = unsafe {
            UIActivityIndicatorView::initWithActivityIndicatorStyle(
                self.mtm.alloc(),
                style,
            )
        };
        if let Some(c) = color {
            let ui_color = color_to_uicolor(c);
            unsafe { indicator.setColor(Some(&ui_color)) };
        }
        unsafe { indicator.startAnimating() };

        let node = IosNode::ActivityIndicator(indicator);
        a11y::apply(
            &node,
            a11y,
            Some(runtime_core::accessibility::Role::Spinner),
        );
        node
    }

    fn create_icon(
        &mut self,
        data: &runtime_core::primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = icon::create_icon(self.mtm, data, color);
        // Give the icon a Taffy intrinsic size so flex layout reserves
        // its box (otherwise the glyph collapses to 0 width and row
        // siblings overlap it — see `install_icon_measure`).
        if let IosNode::View(ref view) = node {
            self.install_icon_measure(view);
        }
        a11y::apply(
            &node,
            a11y,
            Some(runtime_core::accessibility::Role::Image),
        );
        node
    }

    fn register_asset(
        &mut self,
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
        source: &runtime_core::AssetSource,
    ) {
        // Font branch routes into the CoreText-backed registry first;
        // when the asset isn't a font, the call falls through to the
        // image cache. `register_asset` returns `true` once it has
        // handled the font tag so the image branch can be skipped.
        let handled = self.font_registry.register_asset(id, kind, source);
        if !handled {
            image::register_asset(&mut self.image_cache, id, kind, source);
        }
    }

    fn unregister_asset(
        &mut self,
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
    ) {
        self.font_registry.unregister_asset(id, kind);
        if kind == runtime_core::AssetTag::Image {
            self.image_cache.remove(&id);
        }
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

    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        let node = image::create_image(self.mtm, &self.image_cache, src, alt);
        // Register with the layout tree so Taffy gives it a frame.
        // Image views need an intrinsic-size measurer so they don't
        // collapse to 0×0 — see project_ios_intrinsic_size_measurer
        // memory for why.
        if let IosNode::View(view) = &node {
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
        if let IosNode::View(view) = node {
            // Image swap can change intrinsicContentSize → re-measure.
            let view_clone = view.clone();
            self.install_image_measure(&view_clone);
        }
    }

    fn create_virtualizer(
        &mut self,
        callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Build the UICollectionView + flow layout + data source.
        // Supports vertical and horizontal single-section lists with
        // `ItemSize::Known` sizing.
        //
        // Remaining gaps (documented in `imp/virtualizer.rs`):
        //   - `ItemSize::Measured`: blocked on framework-core exposing
        //     a measure-only pass over a detached subtree (cells live
        //     outside the Taffy tree).
        //   - Sections + sticky headers: blocked on a section-aware
        //     `VirtualizerCallbacks` shape in framework-core.
        //   - `performBatchUpdates` instead of `reloadData` on data
        //     changes, for animated row mutations.
        //   - Overscan tuning: UIKit's built-in prefetch covers the
        //     common case; revisit if a list needs finer control.
        // Clone the size-driving closures before `callbacks` moves into
        // `create`, so the measure_fn below can report the list's total
        // content size to Taffy.
        let item_count = callbacks.item_count.clone();
        let item_size = callbacks.item_size.clone();
        let view = virtualizer::create(
            self.mtm,
            &mut self.virtualizer_instances,
            callbacks,
            overscan,
            horizontal,
        );
        // Stage in the layout tree so Taffy gives the collection view
        // an outer frame. Cells inside the collection view are NOT
        // Taffy-managed — UICollectionViewLayout owns their layout.
        let layout = self.layout_for_view(&view);
        // Give the node a measure_fn that returns the list's total content
        // size along the scroll axis (sum of item sizes), mirroring the web
        // backend's content-driven height. A `UICollectionView` has no
        // intrinsic Taffy size, so without this the list collapses to 0 in a
        // flex column and renders nothing even with data. The cross axis
        // fills the parent-provided / available extent. Re-measured on data
        // change (see `virtualizer_data_changed`).
        self.layout.set_measure_fn(
            layout,
            std::rc::Rc::new(move |known: runtime_layout::Size<Option<f32>>,
                                   available: runtime_layout::Size<runtime_layout::AvailableSpace>| {
                let count = (item_count)();
                let total: f32 = (0..count).map(|i| (item_size)(i)).sum();
                let avail_w = match available.width {
                    runtime_layout::AvailableSpace::Definite(w) => w,
                    _ => 0.0,
                };
                let avail_h = match available.height {
                    runtime_layout::AvailableSpace::Definite(h) => h,
                    _ => 0.0,
                };
                if horizontal {
                    runtime_layout::Size {
                        width: known.width.unwrap_or(total),
                        height: known.height.unwrap_or(avail_h),
                    }
                } else {
                    runtime_layout::Size {
                        width: known.width.unwrap_or(avail_w),
                        height: known.height.unwrap_or(total),
                    }
                }
            }),
        );
        // Register for origin-preservation in `apply_frames` (so the
        // user's scroll position survives every relayout) but NOT in
        // `scroll_views` because that would also pull the view into
        // the contentSize-sync loop, which assumes Taffy-managed
        // children. See the comment on `IosBackend::collection_views`.
        let key = &*view as *const UIView as usize;
        self.collection_views.insert(key);
        let node = IosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            Some(runtime_core::accessibility::Role::List),
        );
        node
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        // Phase-1: full reload. Phase-2 would diff item keys against
        // the previous snapshot and issue `performBatchUpdates` so
        // surviving rows animate in place. `reloadData()` is correct
        // (UIKit re-queries item_count + sizes + cellForItem on every
        // visible index) but throws away every mounted cell — every
        // item gets remounted via a fresh per-item Scope. For lists
        // whose data churns rarely, that cost is fine; for live
        // streams it's the obvious next thing to optimize.
        let view = node.as_view();
        // Defer the UICollectionView `reloadData`. It synchronously fires
        // `didEndDisplayingCell` for every currently-visible cell, and our
        // handler calls `release_item` — which DROPS the per-item reactive
        // Scope. But `virtualizer_data_changed` is invoked from WITHIN the
        // reactive update that changed the data signal, so dropping a Scope
        // here re-enters the reactive runtime mid-update and panics (the
        // panic then aborts across UIKit's Obj-C frame). Hop to the next
        // main-loop turn so the current update finishes before any cell is
        // recycled. (This bug only surfaces once the list actually renders
        // cells — see the content-size measure_fn in `create_virtualizer`.)
        let view_retained: Retained<UIView> = unsafe {
            Retained::retain(view as *const UIView as *mut UIView).expect("retain UIView")
        };
        runtime_core::scheduling::schedule_microtask(move || {
            virtualizer::data_changed(&view_retained);
        });
        // The item count changed, so the list's content size changed. Mark
        // the Taffy node dirty and schedule a layout pass so the measure_fn
        // re-runs and the node resizes to the new content height — otherwise
        // a list that loaded its rows after first layout (e.g. an async
        // fetch) would stay sized to its initial (often empty → 0) extent.
        // `mark_dirty` only flags the node; the layout pass is itself
        // deferred, so this is safe to do synchronously.
        let layout = self.layout_for_view(view);
        self.layout.mark_dirty(layout);
        schedule_layout_pass();
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        // Tear down — runs from the cleanup Effect installed by the
        // walker when the surrounding Scope drops. We do this BEFORE
        // the UICollectionView itself goes out of scope so any UIKit
        // event already queued for the next runloop turn drains as a
        // no-op against `alive == false`. Without this hook, the
        // user's data closures (which captured per-item `Signal`s
        // scoped to the same teardown event) would be invoked by
        // UIKit's lingering layout pass and panic with "signal used
        // after its scope was dropped".
        let view = node.as_view();
        let key = view as *const UIView as usize;
        self.collection_views.remove(&key);
        virtualizer::release(&mut self.virtualizer_instances, view);
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        icon::update_icon_color(node, color)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        icon::update_icon_stroke(node, progress)
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: runtime_core::Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        icon::animate_icon_stroke(node, from, to, duration_ms, easing, infinite, autoreverses)
    }

    fn make_icon_handle(&self, node: &Self::Node) -> runtime_core::IconHandle {
        icon::make_handle(node)
    }

    fn create_graphics(
        &mut self,
        on_ready: OnReady,
        on_resize: OnResize,
        on_lost: OnLost,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = graphics::create_graphics(self.mtm, &mut self.callback_targets, on_ready, on_resize, on_lost);
        // Graphics surfaces are GPU-rendered content with no inherent
        // a11y role; authors opt in via props.role / props.label.
        a11y::apply(&node, a11y, None);
        node
    }

    fn create_link(&mut self, config: LinkConfig, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        // Plain UIView (was UIStackView). UIStackView injected internal
        // UISV-canvas-connection constraints that fought Taffy's
        // frame-based positioning — manifested as sibling links in the
        // drawer sidebar overlapping with gap=0 instead of honoring
        // the parent's `gap`, and the Link's own height collapsing.
        // Children now render via the normal addSubview + Taffy frame
        // path, identical to `create_view`.
        let view = unsafe { UIView::new(self.mtm) };
        let _: () = unsafe { msg_send![&view, setUserInteractionEnabled: true] };

        let target = CallbackTarget::new(self.mtm, config.on_activate);
        let tap_sel = objc2::sel!(invoke);
        let tap_gr = unsafe {
            objc2_ui_kit::UITapGestureRecognizer::initWithTarget_action(
                self.mtm.alloc(),
                Some(&target),
                Some(tap_sel),
            )
        };
        // Same mount-time phantom-tap gate as `create_pressable` — a
        // Link sitting under the viewport center must not auto-navigate
        // on the first run-loop turn it appears. See
        // `CallbackTarget::gesture_recognizer_should_begin`.
        let _: () = unsafe { msg_send![&*tap_gr, setDelegate: &*target] };
        let _: () = unsafe { msg_send![&view, addGestureRecognizer: &*tap_gr] };
        self.retain_target(&target);

        let _ = self.layout_for_view(&view);
        let node = IosNode::View(view);
        // Default Link label = the route (in-app) or the URL
        // (external), if no author label was given. `a11y::apply`
        // clears the label when `props.label.is_none()`; we re-set it
        // afterwards so reactive prop changes that explicitly clear
        // the label fall back rather than leaving the link unlabelled.
        // Author overrides still win.
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

    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        // Mirror `create_link`'s tap-gesture wiring so `Pressable`
        // children actually fire their click handlers. The default
        // `Backend::create_pressable` (see
        // `crates/framework/core/src/backend.rs:117`) explicitly
        // ignores `on_click` and falls through to `create_view()` —
        // the doc comment acknowledges "clicks won't fire". This
        // override is what makes `idea_ui::Tabs`, `CardTabs`, and
        // any other Pressable-backed control respond to taps on iOS.
        let view = unsafe { UIView::new(self.mtm) };
        let _: () = unsafe { msg_send![&view, setUserInteractionEnabled: true] };

        let target = CallbackTarget::new(self.mtm, on_click);
        let tap_sel = objc2::sel!(invoke);
        let tap_gr = unsafe {
            objc2_ui_kit::UITapGestureRecognizer::initWithTarget_action(
                self.mtm.alloc(),
                Some(&target),
                Some(tap_sel),
            )
        };
        // Gate the tap so a phantom touch UIKit delivers during the
        // view's first run-loop turn on screen can't fire `on_click`
        // (the mount-time auto-open bug). `CallbackTarget` doubles as
        // the recognizer's `UIGestureRecognizerDelegate`; see its
        // `gestureRecognizerShouldBegin:` + `TAP_GATE_SETTLE_SECS`.
        let _: () = unsafe { msg_send![&*tap_gr, setDelegate: &*target] };
        let _: () = unsafe { msg_send![&view, addGestureRecognizer: &*tap_gr] };
        self.retain_target(&target);

        let _ = self.layout_for_view(&view);
        let node = IosNode::View(view);
        // Pressable is a tappable UIView with no implicit UIKit
        // accessibility role — `Role::Button` tells UIKit it's
        // interactive so VoiceOver announces "Button" after the label.
        a11y::apply(
            &node,
            a11y,
            Some(runtime_core::accessibility::Role::Button),
        );
        node
    }

    fn install_touch_handler(
        &mut self,
        node: &Self::Node,
        handler: runtime_core::TouchHandler,
    ) {
        // `create_view` mints `IdealystTouchView` instances; every
        // framework View should pass this `isKindOfClass:` check.
        // Other primitives (Button, Pressable, etc.) don't carry
        // an `on_touch` slot so the walker never calls us with
        // their nodes — we don't need a fallback path.
        let view = node.as_view();
        let touch_cls = objc2::class!(IdealystTouchView);
        let is_touch_view: bool = unsafe { msg_send![view, isKindOfClass: touch_cls] };
        if !is_touch_view {
            // Defensive: log + drop. The framework shouldn't reach
            // here today, but adding new primitives that carry an
            // `on_touch` slot in the future without minting them as
            // IdealystTouchView would silently lose touches without
            // this guard.
            return;
        }
        // SAFETY: just confirmed the dynamic class is
        // `IdealystTouchView` (or a subclass). The layout is
        // ABI-identical to `UIView` extended with our ivars.
        let touch_view: &touch::IdealystTouchView =
            unsafe { &*(view as *const UIView as *const touch::IdealystTouchView) };
        touch_view.set_handler(handler);
    }

    fn claim_touch(
        &mut self,
        node: &Self::Node,
        _touch_id: runtime_core::TouchId,
    ) {
        // Walk up the responder chain looking for any UIScrollView
        // ancestor and force-cancel its in-flight pan. See
        // `imp/touch.rs::claim_touch_internal` for the rationale —
        // toggling `panGestureRecognizer.enabled` is the standard
        // iOS pattern for "stop the parent scroll immediately."
        touch::claim_touch_internal(node.as_view());
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let parent_view = parent.as_view();
        let parent_key = parent_view as *const UIView as usize;
        let child_view = child.as_view();
        let child_key = child_view as *const UIView as usize;

        // Portal containers mount themselves into the host window —
        // skip the parent-tree insert the walker tries for them.
        if self.portal_instances.contains_key(&child_key) {
            return;
        }

        // Detached window root (screen_recorder private layer): the
        // content view already lives in its OWN `UIWindow`. The
        // External walker calls `insert(parent, external_node)` to
        // splice the handler's returned node into the surrounding
        // view tree — but doing the `addSubview` here would reparent
        // the content view OUT of its private window and INTO the
        // main (recorded) tree, so ReplayKit would capture it. Skip
        // the native reparent; the root stays in its window. Its
        // Taffy node remains registered (sized to the window in the
        // layout pass) and the walker's `insert_children` already
        // populated it, so the private subtree renders correctly —
        // just on the excluded window.
        if self.detached_window_roots.contains_key(&child_key) {
            return;
        }

        // Portal parent: addSubview + Taffy add_child. For viewport
        // portals the container's flex style places the child via
        // justify/align. For anchored portals we additionally apply
        // an absolute-position style to the first non-backdrop child
        // and start the per-vsync anchor tracker.
        if self.portal_instances.contains_key(&parent_key) {
            unsafe { parent_view.addSubview(child_view) };
            let p_layout = self.layout_for_view(parent_view);
            let c_layout = self.layout_for_view(child_view);
            self.layout.add_child(p_layout, c_layout);

            // Anchored portals route the first inserted child through
            // the absolute-position + tracker path. Subsequent children
            // (rare — the typical portal has one content child plus an
            // optional backdrop) flow into the same container without
            // their own tracker; the backdrop child is usually inserted
            // first by the composition (it sits behind the content) and
            // sizes itself via the portal's flex style.
            //
            // For now we apply tracker treatment only when the entry
            // doesn't already have one. Composition convention puts the
            // anchored content as the last child; the tracker tracks
            // whichever child we wire it to. This works for the common
            // single-content-child case; if a future composition layers
            // multiple anchored children we'd need a per-child policy.
            let needs_tracker = {
                let entry = self.portal_instances.get(&parent_key).unwrap();
                entry.anchor.is_some() && entry.anchor_link.is_none()
            };
            if needs_tracker {
                let (target, side, align, offset) = {
                    let entry = self.portal_instances.get(&parent_key).unwrap();
                    let anchor = entry.anchor.as_ref().unwrap();
                    (anchor.target.clone(), anchor.side, anchor.align, anchor.offset)
                };
                let spec = portal::AnchorSpec {
                    target: target.clone(),
                    side,
                    align,
                    offset,
                };
                let child_rules = portal::child_style_for_anchor(&spec);
                self.layout.set_style(c_layout, &child_rules);

                let popover: Retained<UIView> = unsafe {
                    Retained::retain(child_view as *const UIView as *mut UIView)
                        .expect("retain popover view")
                };
                let link = portal::start_anchor_tracker(
                    self.mtm, popover, target, side, align, offset,
                );
                if let Some(entry_mut) = self.portal_instances.get_mut(&parent_key) {
                    entry_mut.anchor_link = Some(link);
                }
            }

            // Portals mount dynamically (when their open signal flips)
            // so the framework's `finish()` hook can't size them —
            // kick a layout pass now.
            schedule_layout_pass();
            return;
        }

        // Default path: addSubview + add to the parallel Taffy tree.
        // Children inside a UIScrollView use the same path — they
        // get positioned by Taffy, and `run_layout_pass_global`
        // syncs `scrollView.contentSize` from their bounding box
        // afterwards. Lazily allocate layout nodes for both views.
        unsafe { parent_view.addSubview(child_view) };
        let p_layout = self.layout_for_view(parent_view);
        let c_layout = self.layout_for_view(child_view);
        self.layout.add_child(p_layout, c_layout);
        // Mirror `clear_children`'s explicit `mark_dirty` here:
        // Taffy caches per-node measured size keyed by inputs, and
        // child-set changes don't always invalidate the parent's
        // cache. Without this, a `switch` swap can land the new
        // child in Taffy's tree but the parent reuses a stale
        // larger size from when the prior branch was active —
        // surfaced as a too-tall panel after switching from a
        // long-content tab to a short-content one.
        self.layout.mark_dirty(p_layout);
        // Layout strategy: sync only when the parent is already
        // attached to a window (i.e. live in the host view
        // hierarchy). That cleanly discriminates the two cases:
        //
        // - Mid-build insert (parent is a freshly-created floating
        //   UIView, not yet added to any superview): `parent.window`
        //   is nil. Defer — the build pass will keep inserting
        //   ancestors and eventually `finish()` runs the closing
        //   layout pass. A sync layout here would re-compute against
        //   a partial tree and cache wrong sizes for the new node's
        //   subtree (this was the user-visible "oversized CodeBlock"
        //   bug from an earlier sync-on-mount shortcut).
        //
        // - Post-mount insert into a live parent — `switch` branch
        //   swaps, `when` toggles, dynamic list inserts: `parent.window`
        //   is the app window. Sync — the new child needs a frame
        //   in the same frame as its UIKit insert, otherwise the
        //   one-tick deferred layout shows a visible flicker (blank
        //   between `clear_children` and the next-tick paint).
        let parent_window: *const NSObject = unsafe { msg_send![parent_view, window] };
        if !parent_window.is_null() && !sync_layout_already_done_this_turn() {
            // First window-attached insert this runloop turn — sync so
            // a `switch`/`when` toggle paints without flicker. Arm the
            // reset so subsequent inserts in the same turn coalesce.
            arm_sync_layout_done_reset();
            self.run_layout_pass_global();
        } else {
            schedule_layout_pass();
        }

        // Retry pending sticky registrations now that this subtree
        // is wired into the parent chain. The walker fires
        // `apply_style` before `insert`, so any `Position::Sticky`
        // child created in this build cycle deferred its
        // registration to `pending_sticky`. We walk the
        // just-inserted subtree's view tree (with the child as
        // root) and promote each pending entry that can now
        // resolve a scroll ancestor. Entries that still can't —
        // genuinely no scroll-view ancestor — stay in the pending
        // map until the view is removed; we don't keep churning
        // because the per-walk cost is the subtree size, not the
        // pending-map size.
        let mut to_remove = Vec::new();
        promote_pending_sticky_recursive(
            self.mtm,
            child_view,
            &mut self.pending_sticky,
            &mut self.sticky_registry,
            &mut to_remove,
        );
        for k in to_remove {
            self.pending_sticky.remove(&k);
        }
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        match node {
            IosNode::Label(label) => {
                let ns = NSString::from_str(content);
                unsafe { label.setText(Some(&ns)) };
            }
            IosNode::Button(button) => {
                let ns = NSString::from_str(content);
                let _: () = unsafe { msg_send![button, setTitle: &*ns, forState: 0u64] };
            }
            _ => {}
        }
        // The widget's intrinsic content size just changed. Taffy's
        // measure_fn for this node returns whatever `sizeThatFits:`
        // says, but it only re-invokes the measure_fn when the node
        // is dirty. Setting `UILabel.text` doesn't mark the Taffy
        // node dirty (Taffy doesn't know about UIKit) — so without
        // this the cached size from the previous content keeps
        // winning, and the label clips / overflows / leaves blank
        // space depending on whether new content is bigger or
        // smaller than old. Mark dirty + schedule a layout pass on
        // the next runloop turn to bring everything in sync.
        let view = node.as_view();
        if let Some(layout) = self.layout_of(view) {
            self.layout.mark_dirty(layout);
            schedule_layout_pass();
        }
    }

    fn clear_children(&mut self, node: &Self::Node) {
        // Mirror the UIKit teardown in Taffy. The earlier shape only
        // called `removeFromSuperview()` — UIKit dropped the child
        // views but Taffy still tracked them as children of the
        // parent's layout node, with their last-computed frames
        // intact. The next `insert()` would append the new child
        // *after* those stale entries, so the surviving Taffy
        // children would take the parent's flex budget and the
        // freshly-inserted view ended up off-screen or stacked
        // behind nothing.
        //
        // The user-visible symptom was a `switch()` branch swap:
        // press a tab → `clear_children` + `insert` runs → parent's
        // size changed (Taffy was recomputing) but the new branch's
        // content rendered invisibly.
        let parent = node.as_view();
        let parent_layout = self.layout_for_view(parent);
        // Snapshot child layout nodes before mutating UIKit, since
        // we'll be looking them up by view pointer.
        let child_layouts: Vec<runtime_layout::LayoutNode> = parent
            .subviews()
            .iter()
            .filter_map(|sub| self.layout_of(sub))
            .collect();
        for layout in child_layouts {
            self.layout.remove_child(parent_layout, layout);
        }
        // Invalidate the parent's cached layout. Taffy caches each
        // node's computed size keyed by the constraints; child-set
        // changes don't auto-invalidate, so without `mark_dirty`
        // the next layout pass reuses the parent's last-seen size
        // (from when its previous children were taller). The user-
        // visible symptom on `switch` swaps was the panel retaining
        // the largest historically-active branch's height — the
        // gray `CodeBlock` background extending far past the actual
        // text in the new (shorter) branch.
        self.layout.mark_dirty(parent_layout);
        // Drop any sticky bookkeeping for the entire subtree
        // we're about to remove. Walk recursively so a sticky
        // child nested inside an intermediate View also
        // deregisters (otherwise its registry entry survives the
        // unmount and the CADisplayLink keeps trying to apply
        // transforms to a detached view). If any descendant IS a
        // scroll view, deregister it as a scroll-host so its
        // descendants' sticky bookkeeping is cleaned up too.
        let scroll_class = objc2::class!(UIScrollView);
        fn walk_and_deregister(
            view: &UIView,
            registry: &mut sticky::StickyRegistry,
            pending: &mut std::collections::HashMap<usize, f32>,
            scroll_class: &objc2::runtime::AnyClass,
        ) {
            sticky::deregister(registry, view);
            // Also drop any pending entry — the view is about to
            // be unmounted, so a deferred-not-yet-promoted entry
            // would otherwise survive the unmount and try to
            // attach to a stale pointer on a later re-parent
            // attempt.
            pending.remove(&(view as *const UIView as usize));
            let is_scroll: bool =
                unsafe { msg_send![view, isKindOfClass: scroll_class] };
            if is_scroll {
                sticky::deregister_scroll_view(registry, view);
            }
            let subs = view.subviews();
            for sub in subs.iter() {
                walk_and_deregister(&sub, registry, pending, scroll_class);
            }
        }
        let subviews_for_sticky = parent.subviews();
        for sub in subviews_for_sticky.iter() {
            walk_and_deregister(
                &sub,
                &mut self.sticky_registry,
                &mut self.pending_sticky,
                scroll_class,
            );
        }
        // Now drop the UIKit subviews. Order matters: walk
        // `parent.subviews()` again because Taffy mutations don't
        // affect UIKit, and grabbing the list before the loop
        // freezes it against in-loop removals.
        let subviews = parent.subviews();
        for sub in subviews.iter() {
            unsafe { sub.removeFromSuperview() };
        }
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let view = node.as_view();
        apply_style_to_view(view, style);

        // Position::Sticky → register against the enclosing
        // UIScrollView so the per-vsync sticky tick pins this view
        // when scrolled past the threshold. Any other Position
        // value (or `None`) must first deregister so a previous
        // Sticky → Relative transition cleans up its registry
        // entry + clears the carried transform. See `sticky.rs`.
        //
        // The walker fires `apply_style` (via `attach_style`)
        // BEFORE the parent's `insert(parent, child)` call (see
        // `walker/view.rs:124-126` — `build()` returns first, then
        // `insert` runs). At that moment the child is still a
        // floating UIView with no superview, so
        // `sticky::register`'s superview walk can't find the
        // scroll ancestor yet. We try anyway (it succeeds for
        // re-applies on already-mounted views — stylesheet
        // variant flips, theme changes) and fall back to
        // recording in `pending_sticky` for the first-mount case.
        // `insert` consults `pending_sticky` after attaching the
        // subtree and promotes any entries it can now resolve.
        let view_key = view as *const UIView as usize;
        match style.position {
            Some(runtime_core::Position::Sticky) => {
                let threshold_top = style
                    .top
                    .as_ref()
                    .map(|t| match t.resolve() {
                        runtime_core::Length::Px(v) => v,
                        // Percent / Auto for sticky's pin offset
                        // isn't meaningful (the spec resolves
                        // percent against the scroll container's
                        // padding box on web, but there's no
                        // common "the threshold is half the
                        // container" use case). Treat as 0.
                        _ => 0.0,
                    })
                    .unwrap_or(0.0);
                let registered = sticky::register(
                    self.mtm,
                    &mut self.sticky_registry,
                    view,
                    threshold_top,
                );
                if !registered {
                    // No enclosing scroll view *yet*. Could be a
                    // first-mount (insert hasn't run) or a
                    // genuinely-not-in-a-scroll-view view.
                    // Record either way; `insert` retries and
                    // `release_*` clears the entry.
                    self.pending_sticky.insert(view_key, threshold_top);
                }
            }
            _ => {
                sticky::deregister(&mut self.sticky_registry, view);
                self.pending_sticky.remove(&view_key);
            }
        }

        // Static `transform: …` ops from the stylesheet. iOS's
        // animation system composes a single `CGAffineTransform`
        // from per-axis slots in `AnimatedTransformState`; static
        // transforms share those slots (CSS semantics: animation
        // wins on conflict). Percent translates are stashed and
        // resolved in the layout pass once the box has a real size.
        self.apply_static_transform(node, style);

        // Background gradient: install (or refresh) the CAGradientLayer
        // sublayer and stash the layer ref + resolved sRGB stops on
        // this node's animation state. The per-frame
        // `set_animated_color(GradientStopColor)` path reads those
        // back and calls `setColors:` without rebuilding the sublayer.
        if let Some(installed) = backend_ios_core::style::install_gradient(
            view,
            style.background_gradient.as_ref(),
        ) {
            let key = node.view_key();
            let state = self.animated_states.entry(key).or_default();
            state.gradient_layer = Some(installed.0);
            state.gradient_stops = installed.1;
        }

        // Mirror the resolved style into the Taffy node so flex
        // properties (width/height/flex-direction/padding/gap/…) take
        // effect during the layout pass. For Text leaves padding is
        // stripped — the visual padding is handled by the
        // `IdealystLabel` subclass (its `textInsets` ivar gets the
        // style's `padding_*`, and `sizeThatFits:` returns
        // `content + insets`). Taffy's outer size then equals what
        // measure_fn returns (= sizeThatFits), so the padding is
        // accounted for exactly once. Stripping Taffy padding here
        // prevents the double-count that would otherwise inflate
        // the label's outer rect by 2× the padding.
        let layout_node = self.layout_for_view(view);
        if matches!(node, IosNode::Label(_)) {
            let mut text_style: StyleRules = (**style).clone();
            text_style.padding_left = None;
            text_style.padding_right = None;
            text_style.padding_top = None;
            text_style.padding_bottom = None;
            self.layout.set_style(layout_node, &text_style);
        } else {
            self.layout.set_style(layout_node, style);
        }

        match node {
            IosNode::Label(_) => apply_text_style(view, style, true, &self.font_registry),
            IosNode::Button(button) => {
                if let Some(color) = &style.color {
                    let color_val = color.resolve();
                    let c = color_to_uicolor(&color_val);
                    if let Some(trans) = &style.color_transition {
                        let btn_ref: Retained<UIButton> = button.clone();
                        let trans = *trans;
                        animate(&trans, Rc::new(move || {
                            let _: () = unsafe { msg_send![&btn_ref, setTitleColor: &*c, forState: 0u64] };
                        }));
                    } else {
                        let _: () = unsafe { msg_send![button, setTitleColor: &*c, forState: 0u64] };
                    }
                }
                // Buttons mirror Label typography: route through the
                // font registry so a custom typeface on a button-style
                // rule actually changes the title font.
                let has_typography = style.font_family.is_some()
                    || style.font_size.is_some()
                    || style.font_weight.is_some()
                    || style.font_style.is_some();
                if has_typography {
                    let title_label: Option<Retained<UILabel>> =
                        unsafe { msg_send_id![button, titleLabel] };
                    if let Some(tl) = title_label {
                        apply_text_style(&tl, style, true, &self.font_registry);
                    }
                }
            }
            IosNode::TextField(field) => {
                apply_text_style(view, style, false, &self.font_registry);
                // Caret color → UIKit `tintColor`. On a UITextField the
                // caret + selection handles both follow tintColor, so a
                // single setter covers them. Mirrors the web `caret-color`
                // mapping. The text color itself lives on `color` and is
                // already applied by `apply_text_style` above.
                if let Some(caret) = &style.caret_color {
                    let c = color_to_uicolor(&caret.resolve());
                    if let Some(trans) = &style.caret_color_transition {
                        let field_ref: Retained<UITextField> = field.clone();
                        let trans = *trans;
                        animate(&trans, Rc::new(move || {
                            let _: () = unsafe { msg_send![&field_ref, setTintColor: &*c] };
                        }));
                    } else {
                        let _: () = unsafe { msg_send![field, setTintColor: &*c] };
                    }
                }
            }
            IosNode::TextView(textview) => {
                // UITextView is the multi-line analogue. Same text
                // styling path applies (font, color, font-size); we
                // pass `is_label = false` because UITextView is an
                // editable widget, not a label.
                apply_text_style(view, style, false, &self.font_registry);
                if let Some(caret) = &style.caret_color {
                    let c = color_to_uicolor(&caret.resolve());
                    if let Some(trans) = &style.caret_color_transition {
                        let view_ref: Retained<UITextView> = textview.clone();
                        let trans = *trans;
                        animate(&trans, Rc::new(move || {
                            let _: () = unsafe { msg_send![&view_ref, setTintColor: &*c] };
                        }));
                    } else {
                        let _: () = unsafe { msg_send![textview, setTintColor: &*c] };
                    }
                }
            }
            _ => {}
        }
    }

    fn set_animated_f32(
        &mut self,
        node: &Self::Node,
        prop: runtime_core::animation::AnimProp,
        value: f32,
    ) {
        self.impl_set_animated_f32(node, prop, value);
    }

    fn set_animated_color(
        &mut self,
        node: &Self::Node,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        self.impl_set_animated_color(node, prop, value);
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: runtime_core::PresenceState,
        transition: Option<(u32, runtime_core::Easing)>,
    ) {
        self.impl_apply_presence(node, state, transition);
    }

    fn frame(&self, node: &Self::Node) -> Option<runtime_core::primitives::portal::ViewportRect> {
        // UIView.frame is already in superview coordinates — that's
        // the relative-to-parent rect.
        let view = node.as_view();
        let frame: objc2_foundation::CGRect = unsafe { msg_send![view, frame] };
        Some(runtime_core::primitives::portal::ViewportRect {
            x: frame.origin.x as f32,
            y: frame.origin.y as f32,
            width: frame.size.width as f32,
            height: frame.size.height as f32,
        })
    }

    fn absolute_frame(&self, node: &Self::Node) -> Option<runtime_core::primitives::portal::ViewportRect> {
        // Same conversion as `rect_of_node` in handles.rs: convert
        // bounds to window coordinates. Returns None if the view
        // isn't yet mounted in a window.
        let view = node.as_view();
        let bounds: objc2_foundation::CGRect = unsafe { msg_send![view, bounds] };
        let window: Option<Retained<UIView>> = unsafe {
            let w: *mut UIView = msg_send![view, window];
            if w.is_null() { None } else { Retained::retain(w) }
        };
        let window = window?;
        let frame_in_window: objc2_foundation::CGRect = unsafe {
            msg_send![view, convertRect: bounds, toView: &*window]
        };
        Some(runtime_core::primitives::portal::ViewportRect {
            x: frame_in_window.origin.x as f32,
            y: frame_in_window.origin.y as f32,
            width: frame_in_window.size.width as f32,
            height: frame_in_window.size.height as f32,
        })
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        let enabled = !disabled;
        match node {
            IosNode::Button(b) => {
                let _: () = unsafe { msg_send![b, setEnabled: enabled] };
            }
            IosNode::TextField(f) => {
                let _: () = unsafe { msg_send![f, setEnabled: enabled] };
            }
            IosNode::Switch(s) => {
                let _: () = unsafe { msg_send![s, setEnabled: enabled] };
            }
            IosNode::Slider(s) => {
                let _: () = unsafe { msg_send![s, setEnabled: enabled] };
            }
            _ => {}
        }
    }

    // =================================================================
    // Navigator
    // =================================================================


    // =================================================================
    // Portal
    // =================================================================

    fn create_portal(
        &mut self,
        target: runtime_core::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // On iOS-mobile we don't use `presentViewController:` for
        // portals — they're window-level `UIView` subviews. There's
        // no native dismiss event for that path (no swipe-down on
        // raw views, no hardware back). `on_dismiss` is effectively
        // host-signal-driven: the framework flips its open state in
        // response to whatever interaction the composition wires up
        // (backdrop tap, swipe handler on a sheet child, etc.). We
        // accept the callback but never fire it from this backend.
        use runtime_core::primitives::portal::PortalTarget;

        let (anchor_spec, container_rules) = match &target {
            PortalTarget::Viewport(placement) => {
                (None, portal::container_style_for_placement(*placement))
            }
            PortalTarget::Anchor { target, side, align, offset } => {
                let spec = portal::AnchorSpec {
                    target: target.clone(),
                    side: *side,
                    align: *align,
                    offset: *offset,
                };
                (Some(spec), portal::container_style_for_anchor())
            }
            PortalTarget::Named(name) => {
                // Reserved for future "slot" routing — no registry
                // yet. Fall back to a viewport-fullscreen portal so
                // the subtree still mounts somewhere visible. Log
                // once so the missing wiring is obvious in dev.
                eprintln!(
                    "[ios-portal] PortalTarget::Named({:?}) not implemented — falling back to FullScreen",
                    name
                );
                use runtime_core::primitives::portal::ViewportPlacement;
                (None, portal::container_style_for_placement(ViewportPlacement::FullScreen))
            }
        };

        let (content_view, entry) = portal::create_portal(
            self.mtm,
            self.host_root.as_ref(),
            anchor_spec,
            trap_focus,
        );
        let key = &*content_view as *const UIView as usize;
        self.portal_instances.insert(key, entry);

        // Register the container in the layout tree as a Taffy root.
        // It's orphan (no parent in Taffy because `insert` skips its
        // own insertion), so `compute()`'s viewport auto-fill resizes
        // it to the full viewport on every layout pass — including
        // orientation flips. The target-derived flex style places
        // the portal's content child within that frame.
        let layout_node = self.layout_for_view(&content_view);
        self.layout.set_style(layout_node, &container_rules);

        let node = IosNode::View(content_view);
        // Portal containers are transparent in the AX tree by
        // default; the mounted content carries its own role. `apply`
        // still writes author-set label / identifier when present.
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_portal(&mut self, node: &Self::Node) {
        let key = IosBackend::node_key(node);
        if let Some(entry) = self.portal_instances.remove(&key) {
            portal::release_portal(entry);
        }
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &std::rc::Rc<dyn std::any::Any>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = if let Some(handler) = self.external_handlers.get(type_id) {
            handler(payload, self)
        } else {
            // No handler registered → render a placeholder UILabel so
            // the dev/user sees that an SDK binding is missing on iOS
            // rather than a silent hole. `has_external::<T>()` is the
            // supported way to render custom degradation in user space.
            external_placeholder_node(self, type_name)
        };
        // Third-party externals declare their own role via
        // `props.role` if needed — we don't infer one here.
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_external(&mut self, _node: &Self::Node) {
        // No per-external bookkeeping today. Future SDK leaves that
        // hold instance state (KVO observers, CADisplayLink, etc.)
        // would clean up here, keyed by `node_key` like portals do.
    }

    fn apply_safe_area_padding(
        &mut self,
        node: &Self::Node,
        sides: runtime_core::SafeAreaSides,
    ) {
        // Read the platform's current safe-area insets from the host
        // root. `LayoutObserverView` mirrors the host's insets, so
        // this reflects the same value the framework signal carries
        // — but reading the host directly avoids a stale read if
        // this runs before the signal has propagated.
        let insets = self.platform_safe_area_insets();
        // Mask per-side: only contribute on sides the author opted
        // into. `set_safe_area_extra` always takes all four sides;
        // we pass zero for unopted ones so the math stays uniform.
        let top = if sides.contains(runtime_core::SafeAreaSides::TOP) { insets.top } else { 0.0 };
        let right = if sides.contains(runtime_core::SafeAreaSides::RIGHT) { insets.right } else { 0.0 };
        let bottom = if sides.contains(runtime_core::SafeAreaSides::BOTTOM) { insets.bottom } else { 0.0 };
        let left = if sides.contains(runtime_core::SafeAreaSides::LEFT) { insets.left } else { 0.0 };

        let view = node.as_view();
        let layout_node = self.layout_for_view(view);
        self.layout.set_safe_area_extra(layout_node, top, right, bottom, left);
        schedule_layout_pass();
    }

    fn apply_scroll_view_safe_area_inset(
        &mut self,
        node: &Self::Node,
        sides: runtime_core::SafeAreaSides,
    ) {
        // Delegate inset math to UIKit by toggling
        // `contentInsetAdjustmentBehavior` and leaving
        // `contentInset` untouched. With `.always`, UIScrollView
        // reads its own `safeAreaInsets` — which already propagate
        // through any wrapping `UINavigationController` (top inset
        // = status bar + nav bar) and through the host window
        // (bottom inset = home indicator) — and folds them into
        // `adjustedContentInset` automatically. UIKit re-runs that
        // every time the safe area changes (rotation, dynamic
        // island, sheet adaptation), so no Effect subscription is
        // needed and there's no stale-host-inset bug to hit.
        //
        // The earlier code read `host_root.safeAreaInsets` and wrote
        // `contentInset` manually. Two problems with that:
        //
        // 1. Host insets don't include the nav bar. Screens inside
        //    the drawer's `UINavigationController` ended up under
        //    the nav bar on every route change after the first.
        // 2. Calling `setContentInset:` shifts `contentOffset` to
        //    keep visible content visible. Combined with a layout
        //    pass that also reset the scroll view's `contentSize`,
        //    the sidebar's `contentOffset` flipped to `(0, 0)` on
        //    every route change — content jumped up under the
        //    status bar until the user touched the scroll view and
        //    UIKit snapped it back to a valid offset.
        //
        // We don't touch `contentInset` at all here. Author-set
        // padding inside the scroll view's content tree still works
        // normally; UIKit's `adjustedContentInset` is layered on top
        // of whatever the framework wrote.
        //
        // `sides` is treated as on/off rather than per-edge.
        // `contentInsetAdjustmentBehavior` is whole-area;
        // partial-side opt-in would need `additionalSafeAreaInsets`
        // overrides on a wrapping VC and isn't needed by current
        // examples. Revisit if a partial-side use case shows up.
        let view = node.as_view();
        let behavior: i64 = if sides.is_empty() {
            // Author opted out — scroll bleeds edge-to-edge with no
            // inset; the author is responsible for content offset.
            SCROLL_VIEW_INSET_ADJUSTMENT_NEVER
        } else {
            // UIKit insets for the effective safe area (status bar +
            // nav bar + tab bar + home indicator).
            SCROLL_VIEW_INSET_ADJUSTMENT_ALWAYS
        };
        let _: () = unsafe { msg_send![view, setContentInsetAdjustmentBehavior: behavior] };
    }

    // =================================================================
    // Handle factories — override defaults so handles carry the
    // real iOS node, enabling `AnchorableHandle::rect()` to read
    // viewport coords. Required for element-anchored overlays
    // (Popover, Select).
    // =================================================================

    fn make_button_handle(&self, node: &Self::Node) -> runtime_core::ButtonHandle {
        runtime_core::ButtonHandle::new(Rc::new(node.clone()), &handles::IOS_BUTTON_OPS)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_core::PressableHandle {
        runtime_core::PressableHandle::new(Rc::new(node.clone()), &handles::IOS_PRESSABLE_OPS)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        runtime_core::ViewHandle::new(Rc::new(node.clone()), &handles::IOS_VIEW_OPS)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        runtime_core::TextHandle::new(Rc::new(node.clone()), &handles::IOS_TEXT_OPS)
    }

    fn make_text_input_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::text_input::TextInputHandle {
        if let IosNode::TextField(field) = node {
            runtime_core::primitives::text_input::TextInputHandle::new(
                Rc::new(field.clone()),
                &handles::IOS_TEXT_INPUT_OPS,
            )
        } else {
            // Shouldn't happen — walker only calls this for TextInput
            // nodes. Fall back to a no-op handle wrapping an empty box.
            runtime_core::primitives::text_input::TextInputHandle::new(
                Rc::new(()),
                &handles::IOS_TEXT_INPUT_OPS,
            )
        }
    }

    fn make_text_area_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::text_area::TextAreaHandle {
        if let IosNode::TextView(view) = node {
            runtime_core::primitives::text_area::TextAreaHandle::new(
                Rc::new(view.clone()),
                &handles::IOS_TEXT_AREA_OPS,
            )
        } else {
            runtime_core::primitives::text_area::TextAreaHandle::new(
                Rc::new(()),
                &handles::IOS_TEXT_AREA_OPS,
            )
        }
    }

    // =================================================================
    // Tab Navigator
    // =================================================================


    // ------------------------------------------------------------------
    // Navigator — unified path for SDK-supplied navigator kinds.
    //
    // `create_navigator` resolves the SDK-registered factory, runs
    // `init`, and stashes the returned handler on
    // `nav_handler_instances` keyed by the container's `view_key`.
    // Subsequent post-init dispatch (`attach_initial` / `release` /
    // `make_handle` / `apply_slot_style`) looks the handler up and
    // forwards through it — the handler then calls whichever
    // per-kind inherent helper (`stack_navigator_attach_initial`,
    // `apply_drawer_sidebar_style`, …) is appropriate for its kind.
    // ------------------------------------------------------------------

    fn create_navigator(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        presentation: Rc<dyn std::any::Any>,
        host: runtime_core::NavigatorHost<Self::Node>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let factory = self
            .navigator_handlers
            .get(type_id)
            .unwrap_or_else(|| {
                panic!(
                    "IosBackend::create_navigator: navigator kind '{}' \
                     is not registered. Did the app forget to call \
                     `<navigator-sdk>::register(&mut backend)` during bootstrap?",
                    type_name
                )
            });
        let mut handler = factory();
        let node = handler.init(self, host, presentation);
        // Stash the handler keyed by the container's view key so
        // subsequent dispatch routes through the SDK handler instead
        // of through a kind switch. The handler internally remembers
        // its container `IosNode` so its post-init methods can call
        // back into the backend's legacy per-kind helpers.
        self.nav_handler_instances.insert(
            node.view_key(),
            std::rc::Rc::new(std::cell::RefCell::new(handler)),
        );
        node
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        let handler = self.nav_handler_instances.get(&navigator.view_key()).cloned();
        let Some(handler) = handler else { return };
        handler.borrow_mut().attach_initial(self, screen, scope_id, options);
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        let handler = self.nav_handler_instances.remove(&node.view_key());
        let Some(handler) = handler else { return };
        handler.borrow_mut().release(self);
    }

    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::NavigatorHandle {
        let handler = self.nav_handler_instances.get(&node.view_key()).cloned();
        match handler {
            Some(h) => h.borrow().make_handle(),
            None => runtime_core::NavigatorHandle::new(Rc::new(()), &NOOP_NAV_OPS),
        }
    }

    fn apply_navigator_slot_style(
        &mut self,
        navigator: &Self::Node,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let handler = self.nav_handler_instances.get(&navigator.view_key()).cloned();
        let Some(handler) = handler else { return };
        handler.borrow_mut().apply_slot_style(self, slot, style);
    }

    // =================================================================
    // Accessibility
    // =================================================================
    //
    // `dump_accessibility_tree` is intentionally left at its default
    // (returns `None`). UIKit walks each `UIView`'s
    // `accessibilityLabel`/`accessibilityHint`/`accessibilityTraits`
    // directly — there's no parallel semantics tree to dump.

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

    fn finish(&mut self, root: Self::Node) {
        if let Some(host) = &self.host_root {
            pin_to_edges(host, root.as_view());
        }
        self.run_layout_pass(&root);
    }

    /// Backend-trait entry point the runtime-server shell uses to drive layout
    /// when the deferred `schedule_layout_pass` path's
    /// `IOS_BACKEND_SELF.upgrade()` returns `None` (runtime-server mode owns
    /// the backend by-value inside `RuntimeServerClient`, so the global
    /// self-ref is never installed). Delegates to the existing
    /// public [`Self::run_layout`] wrapper around
    /// `run_layout_pass_global`.
    fn run_layout(&mut self) {
        IosBackend::run_layout(self);
    }
}

impl IosBackend {
    /// Run Taffy on every layout-tree root (the app root plus each
    /// screen mount that landed via `mount_screen_in_vc` rather than
    /// `insert`), then walk the UIView tree assigning computed
    /// frames. Uses the host view's bounds as the viewport (falls
    /// back to UIScreen.main.bounds when the host hasn't laid out
    /// yet).
    pub(crate) fn run_layout_pass(&mut self, _root: &IosNode) {
        self.run_layout_pass_global();
    }

    /// Public version of [`Self::run_layout_pass_global`] for hosts
    /// that drive layout synchronously rather than through
    /// [`schedule_layout_pass`] / `IOS_BACKEND_SELF`. The runtime-server iOS
    /// client uses this after each command batch: in runtime-server mode the
    /// `IosBackend` is moved into the `RuntimeServerClient` by value, so
    /// there's no `Rc<RefCell<IosBackend>>` to register globally and
    /// the deferred-via-dispatch_async path bails silently. Calling
    /// this synchronously after `apply_batch` finishes guarantees
    /// the new sizes propagate.
    pub fn run_layout(&mut self) {
        self.run_layout_pass_global();
    }

    /// Run layout for the whole registry. Called from `finish()` for
    /// the initial render and from `schedule_layout_pass()` whenever
    /// new views land after that (navigation pushes, drawer mounts).
    pub(crate) fn run_layout_pass_global(&mut self) {
        let _t = phase_timer::PhaseTimer::start("run_layout_pass_global");
        let (vw, vh) = self.viewport_size();
        backend_ios_core::ios_log(&format!(
            "[layout] run_layout_pass viewport=({:.1}, {:.1}) registered_views={}",
            vw, vh, self.view_to_layout.len()
        ));
        if vw <= 0.0 || vh <= 0.0 {
            backend_ios_core::ios_log("[layout] ABORT: viewport is zero");
            return;
        }

        // Find every Taffy root. The framework root is one; each screen
        // mounted via `mount_screen_in_vc` (which bypasses
        // `Backend::insert`) is another.
        let roots: Vec<runtime_layout::LayoutNode> = {
            let _t = phase_timer::PhaseTimer::start("collect_roots");
            self
                .view_to_layout
                .values()
                .map(|(_, n)| *n)
                .filter(|n| self.layout.is_root(*n))
                .collect()
        };

        // If the viewport changed since the last pass, mark every
        // root dirty so the skip-clean fast path doesn't lock hidden
        // screens at stale dimensions.
        let viewport_changed = self.last_viewport != Some((vw, vh));
        if viewport_changed {
            for root_node in &roots {
                self.layout.mark_dirty(*root_node);
            }
            self.last_viewport = Some((vw, vh));
        }

        let mut computed_count = 0usize;
        let mut skipped_count = 0usize;
        {
            let _t = phase_timer::PhaseTimer::start("taffy_compute_all_roots");
            for root_node in &roots {
                if !self.layout.is_dirty(*root_node) {
                    // Skip persistent hidden screens whose subtree
                    // hasn't been touched since the last pass. Taffy's
                    // mark_dirty propagation guarantees a dirty child
                    // anywhere in the subtree marks this root dirty,
                    // so a clean root is genuinely a no-op for compute.
                    skipped_count += 1;
                    continue;
                }
                let _t_one = phase_timer::PhaseTimer::start("taffy_compute_one_root");
                self.layout.compute(*root_node, vw, vh);
                computed_count += 1;
            }
        }
        backend_ios_core::ios_log(&format!(
            "[layout] {} taffy roots — computed {} skipped {}",
            roots.len(), computed_count, skipped_count
        ));

        // Iterate every registered view directly. Recursing via
        // `UIView.subviews` misses subtrees that aren't yet attached
        // to the framework root — e.g. a UINavigationController's
        // top VC view, which UIKit adds lazily after our `finish()`
        // returns. We hold a `Retained` ref to every view, so direct
        // iteration is both safe and exhaustive.
        //
        // We use `setBounds:` + `setCenter:` instead of `setFrame:`
        // because some framework-managed views (drawer sidebar,
        // overlays) carry a `CGAffineTransform` for slide-in
        // animations. Apple's documentation explicitly warns that
        // setting `.frame` on a transformed view is undefined — the
        // observed failure mode here was width collapsing to 0.
        // Bounds + center are stable regardless of transform.
        let mut applied = 0usize;
        {
        let _t = phase_timer::PhaseTimer::start("apply_frames_loop");
        // Track which views we actually touched this pass so the
        // applied-frames cache can drop entries for views that have
        // been removed (Backend::clear_children, screen unmounts,
        // etc.). Without this the cache would grow with every nav
        // that disposes a screen.
        let mut still_present: std::collections::HashSet<usize> =
            std::collections::HashSet::with_capacity(self.view_to_layout.len());
        for (key, (view, layout_node)) in self.view_to_layout.iter() {
            still_present.insert(*key);
            let frame = self.layout.frame_of(*layout_node);
            // Compare against the last frame we wrote for this
            // view. If it hasn't moved, skip the obj-c message
            // sends entirely — most relayouts only touch a small
            // fraction of the tree (the screen we just swapped, an
            // animated property's host view), but `apply_frames`
            // walks every registered view including persistent
            // hidden screens. For an idle pass that's hundreds of
            // unchanged views taking ~16 µs each = several ms of
            // wasted writes.
            let frame_key = (frame.x, frame.y, frame.width, frame.height);
            if self.applied_frames.get(key) == Some(&frame_key) {
                applied += 1;
                continue;
            }
            // Preserve bounds.origin. For a regular UIView the
            // origin is always (0, 0), but for a UIScrollView
            // `bounds.origin` IS `contentOffset` — overwriting it
            // with (0, 0) on every layout pass scrolls the view
            // back to the top, undoing both the user's scroll
            // position and the `adjustedContentInset` offset. That
            // bug surfaced as the sidebar "jumping" out of the
            // safe area on every route change: the active-route
            // signal change triggered a relayout, which reset
            // `contentOffset` to `(0, 0)`, which moved the top of
            // content from `y = adjustedContentInset.top` to
            // `y = 0` (under the status bar) until the next gesture
            // made UIKit re-clamp.
            // `collection_views` is the virtualizer/UICollectionView
            // set. UICollectionView inherits from UIScrollView, so the
            // same bounds.origin = contentOffset invariant applies —
            // overwriting it with (0, 0) every layout pass scrolls the
            // list back to row 0 on every relayout (every reactive
            // signal update, every navigation, every safe-area inset
            // change). Treat both sets the same way here; they only
            // differ in how `contentSize` is computed downstream (see
            // the scroll-view contentSize sync loop, which skips
            // collection_views because UICollectionViewLayout owns
            // contentSize, not Taffy).
            let is_scrollable =
                self.scroll_views.contains(key) || self.collection_views.contains(key);
            let origin = if is_scrollable {
                let current: objc2_foundation::CGRect =
                    unsafe { msg_send![view, bounds] };
                current.origin
            } else {
                objc2_foundation::CGPoint { x: 0.0, y: 0.0 }
            };
            let bounds = objc2_foundation::CGRect {
                origin,
                size: objc2_foundation::CGSize {
                    width: frame.width as f64,
                    height: frame.height as f64,
                },
            };
            let center = objc2_foundation::CGPoint {
                x: (frame.x + frame.width / 2.0) as f64,
                y: (frame.y + frame.height / 2.0) as f64,
            };
            let _: () = unsafe { msg_send![view, setBounds: bounds] };
            let _: () = unsafe { msg_send![view, setCenter: center] };
            // Resize any `idealyst_gradient` CAGradientLayer this view
            // owns to match the new bounds. The gradient was inserted
            // at apply-style time when bounds were still 0×0; without
            // this call it stays 0-sized and never paints (CALayer
            // doesn't auto-resize sublayers from `autoresizingMask`
            // on iOS in practice).
            backend_ios_core::style::sync_gradient_sublayer(view);
            // Recenter any `idealyst_icon` CAShapeLayer within the new
            // bounds. The icon's path is built at a fixed 24×24 top-left
            // origin; when flex sizes the icon view larger than the glyph
            // (cross-axis stretch in a row, or centered in a bigger
            // pressable like the drawer menu button) the glyph would hug
            // the top-left without this recenter.
            backend_ios_core::style::sync_icon_sublayer(view);
            // Re-clamp cornerRadius against the now-known bounds.
            // `apply_style_to_view` stashes the requested radius
            // when the view's size is percent-based; this call reads
            // it back and writes a properly-clamped value.
            backend_ios_core::style::sync_corner_radius(view);
            // Resolve any percent-valued static `transform: translate`
            // requests now that the box has real pixel dimensions.
            // CSS spec: translate-% is BOX-relative, so the shift
            // needs the box's own width / height — not knowable at
            // apply-style time when bounds are still zero.
            crate::imp::animated::sync_static_transform_percent(
                &mut self.animated_states,
                *key,
                view,
                frame.width,
                frame.height,
            );
            self.applied_frames.insert(*key, frame_key);
            applied += 1;
        }
        // Drop cache entries for views that aren't registered
        // anymore. Cheap iteration over a small map (entries only
        // grow with view count; never more than `view_to_layout`).
        self.applied_frames.retain(|k, _| still_present.contains(k));
        }
        backend_ios_core::ios_log(&format!("[layout] apply_frames done: applied={}", applied));

        // Sync UIScrollView contentSize: walk each scroll view's
        // Taffy children, compute the bounding box, set
        // `scrollView.contentSize` to that size. Without this the
        // scroll view doesn't know how tall its content is and
        // gestures don't scroll (or only bounce, when
        // `alwaysBounceVertical` is on).
        let _t_sync = phase_timer::PhaseTimer::start("scroll_contentsize_sync");
        for view_ptr in self.scroll_views.iter().copied() {
            let Some((_view_ref, scroll_layout)) = self.view_to_layout.values()
                .find(|(v, _)| (&**v as *const UIView as usize) == view_ptr)
                .cloned()
            else {
                continue;
            };
            let _ = scroll_layout; // currently not used; reserved for future per-axis adjustments

            // Bounding box across the scroll view's Taffy descendants.
            // We have to look past the direct children: author
            // sidebars frequently set `min_height: Percent(100)` on
            // their outermost container, which Taffy clamps to the
            // scroll view's bounds — overflowing grandchildren (a
            // Spacer-pushed footer, a Dark-mode toggle pinned at
            // the end of a tall list) sit past the direct child's
            // reported frame and won't drive `contentSize` if we
            // stop there. UIKit projects a deep subview's frame
            // into the scroll view's content coordinate space by
            // walking the chain of superview origins; we do the
            // same by accumulating the running origin while
            // descending.
            //
            // Perf note: this runs at the end of every layout pass
            // for every framework-created UIScrollView. The walk is
            // unconditional — a "skip if this node doesn't extend
            // the running max" pruning sounds tempting but breaks
            // the case the deep walk exists for: a 260×852 parent
            // clamped to the scroll view's bounds with a single
            // narrow descendant sitting at y = 900, which extends
            // the bbox on the height axis but not the width axis,
            // so a both-axis-must-extend gate would skip it. The
            // O(N) cost stays bounded by the tree size — author
            // scroll views inside the website are tens of nodes,
            // not thousands.
            let mut max_x = 0.0_f32;
            let mut max_y = 0.0_f32;
            let mut stack: Vec<(runtime_layout::LayoutNode, f32, f32)> = Vec::new();
            for c in self.layout.children_of(scroll_layout) {
                stack.push((c, 0.0_f32, 0.0_f32));
            }
            while let Some((node, origin_x, origin_y)) = stack.pop() {
                let f = self.layout.frame_of(node);
                let nx = origin_x + f.x;
                let ny = origin_y + f.y;
                let right = nx + f.width;
                let bottom = ny + f.height;
                if right > max_x {
                    max_x = right;
                }
                if bottom > max_y {
                    max_y = bottom;
                }
                for child in self.layout.children_of(node) {
                    stack.push((child, nx, ny));
                }
            }
            let scroll_view: Retained<UIScrollView> = unsafe {
                let ptr = view_ptr as *mut UIScrollView;
                Retained::retain(ptr).unwrap()
            };
            let size = objc2_foundation::CGSize {
                width: max_x as f64,
                height: max_y as f64,
            };
            // Skip the assignment if the value is unchanged. UIKit
            // documents that `setContentSize:` resets `contentOffset`
            // to `(0, 0)` when the new content size fits inside the
            // scroll view's bounds — which fires on every layout
            // pass for short content (sidebars, headers), even when
            // the size hasn't actually changed. The offset reset
            // then bypasses `adjustedContentInset`, so safe-area
            // insets stop being respected until the next gesture
            // makes UIKit re-clamp. Reading first + comparing
            // sidesteps the reset entirely; UIKit's own `setBounds:`
            // already no-ops on equal values, but `setContentSize:`
            // does not.
            let current: objc2_foundation::CGSize =
                unsafe { msg_send![&scroll_view, contentSize] };
            if (current.width - size.width).abs() > 0.5
                || (current.height - size.height).abs() > 0.5
            {
                let _: () = unsafe { msg_send![&scroll_view, setContentSize: size] };
            }
        }

        // Refresh `natural_y` for every Position::Sticky child now
        // that Taffy has re-laid out the tree. Without this, a
        // tree rebuild (route switch, branch swap) leaves stale
        // natural-y values and the sticky child pins to the wrong
        // place — most visibly when the user scrolls a freshly-
        // mounted screen for the first time. Cheap walk; the
        // registry is tiny by construction.
        {
            let _t = phase_timer::PhaseTimer::start("sticky_refresh");
            sticky::refresh_layout_positions(
                &mut self.sticky_registry,
                &self.layout,
                &self.view_to_layout,
            );
        }
        // Make sure the scroll-content sync timer drops before we
        // dump — without this the timer scope would still hold the
        // duration when `take_and_dump` runs and the value would
        // round to zero.
        drop(_t_sync);
        drop(_t);
        phase_timer::take_and_dump("layout pass");
    }

    /// Return the viewport size for layout. Tries host_root.bounds
    /// first (which is non-zero after UIKit has laid out the host),
    /// then UIScreen.main.bounds.
    fn viewport_size(&self) -> (f32, f32) {
        if let Some(host) = &self.host_root {
            let bounds: objc2_foundation::CGRect = unsafe { msg_send![host, bounds] };
            if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
                return (bounds.size.width as f32, bounds.size.height as f32);
            }
        }
        // UIScreen.main.bounds — device screen size.
        unsafe {
            let screen: Retained<NSObject> =
                msg_send_id![objc2::class!(UIScreen), mainScreen];
            let bounds: objc2_foundation::CGRect = msg_send![&screen, bounds];
            (bounds.size.width as f32, bounds.size.height as f32)
        }
    }

    /// Return the host root's current safe-area insets. Used by
    /// `apply_safe_area_padding` to avoid trusting a stale framework
    /// signal value during the build/layout flow — UIKit's value is
    /// the source of truth.
    fn platform_safe_area_insets(&self) -> runtime_core::EdgeInsets {
        let Some(host) = &self.host_root else {
            return runtime_core::EdgeInsets::ZERO;
        };
        let insets: callbacks::UIEdgeInsets =
            unsafe { msg_send![&**host, safeAreaInsets] };
        runtime_core::EdgeInsets {
            top: insets.top as f32,
            right: insets.right as f32,
            bottom: insets.bottom as f32,
            left: insets.left as f32,
        }
    }
}

/// Build a placeholder UILabel for an unregistered external primitive
/// — visible in dev so missing SDK bindings on iOS are obvious.
/// User-space `has_external::<T>()` discovery is the supported way to
/// render custom degradation instead of relying on this fallback.
fn external_placeholder_node(b: &mut IosBackend, type_name: &'static str) -> IosNode {
    let label = unsafe { UILabel::new(b.mtm) };
    let text = format!("External \"{type_name}\" not supported on iOS");
    let ns_text = NSString::from_str(&text);
    unsafe { label.setText(Some(&ns_text)) };
    let _: () = unsafe { msg_send![&label, setNumberOfLines: 0isize] };
    // Match the red intent of the web placeholder. UIColor.systemRed
    // is the dynamic color that adapts to light/dark — same intent
    // across platforms, no manual hex needed.
    let red: Retained<NSObject> =
        unsafe { msg_send_id![objc2::class!(UIColor), systemRedColor] };
    let _: () = unsafe { msg_send![&label, setTextColor: &*red] };
    let _ = b.layout_for_view(&label);
    IosNode::Label(label)
}

// All legacy per-kind navigator inherent helpers (`create_stack_navigator`,
// `tab_navigator_attach_initial`, `apply_drawer_sidebar_style`, etc.)
// moved to the `ios-navigator-helpers` crate as part of the
// navigator-substrate refactor. SDK iOS handlers
// (`stack_navigator::ios::IosStackHandler`, etc.) now call into that
// crate directly, and the framework reaches the handlers through the
// per-instance map stashed on `nav_handler_instances`.

#[cfg(test)]
mod backend_self_handle_tests {
    //! Regression coverage for the runtime-server drawer "scrim shows
    //! but sidebar panel is invisible" bug. Per CLAUDE.md §8 the test
    //! is named after the bug, not the function.
    //!
    //! Root cause: the runtime-server iOS shell (`runtime_server::
    //! ios_main_with_register`) spawned the `RuntimeServerShell` but
    //! never called `install_global_self`. SDK code reached outside
    //! the framework's normal call path — specifically the drawer
    //! handler's `schedule_microtask`-deferred `drawer_attach_sidebar`,
    //! which calls `with_backend(|b| b.run_layout())` to size the
    //! freshly-attached, *parentless* sidebar Taffy node — therefore
    //! found NO installed self, so `with_backend` returned `None` and
    //! the layout pass never ran. The sidebar UIView stayed 0×0: on
    //! open the modal scrim darkened (its node is part of the
    //! create_navigator batch the shell's per-tick `run_layout`
    //! covers) but the sidebar panel slid in invisibly.
    //!
    //! A tighter test would drive the real spawn path, but
    //! `IosBackend::new` needs a `MainThreadMarker` (only available on
    //! a live UIKit main thread) and the shell starts a WS worker
    //! thread — neither fits `cargo test`. So this asserts the precise
    //! mechanism that broke: `with_backend` is a no-op until a self
    //! handle is installed, and resolves once one is. The end-to-end
    //! behavior (panel visible on open over the wire) is covered by an
    //! on-simulator `idealyst dev --ios` screenshot, logged in
    //! [[project_navigator_over_wire_wip]].

    /// With no self installed, `with_backend` must short-circuit to
    /// `None` (never panic, never run the closure) — this IS the
    /// broken pre-fix runtime-server state, reproduced deterministically.
    #[test]
    fn regression_rs_drawer_sidebar_with_backend_noops_until_self_installed() {
        // Fresh thread → guaranteed-empty `IOS_BACKEND_SELF`
        // thread-local (the spawn path runs on its own thread too).
        std::thread::spawn(|| {
            let mut ran = false;
            let out = super::with_backend(|_b| {
                ran = true;
                42u32
            });
            assert!(
                out.is_none(),
                "with_backend must return None when no self is installed (pre-fix RS state)",
            );
            assert!(
                !ran,
                "the closure must NOT run when no self is installed — the silent no-op that left the drawer sidebar unsized",
            );
        })
        .join()
        .expect("test thread panicked");
    }
}
