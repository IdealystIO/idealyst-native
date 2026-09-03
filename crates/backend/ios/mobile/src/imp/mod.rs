pub(crate) mod a11y;
pub(crate) mod animated;
pub mod callbacks;
pub(crate) mod ffi_guard;
pub(crate) mod graphics;
pub(crate) mod handles;
pub(crate) mod icon;
pub(crate) mod image;
pub(crate) mod portal;
pub(crate) mod phase_timer;
pub(crate) mod keyboard;
pub(crate) mod screenshot;
pub(crate) mod sticky;
pub(crate) mod styled_text;
pub(crate) mod text_inset;
pub(crate) mod touch;
pub(crate) mod virtual_grid;
mod virtualizer;

/// Platform log with format. Forwards to `backend_ios_core::ios_log`
/// which wraps NSLog.
#[allow(dead_code)]
macro_rules! ios_log {
    ($($arg:tt)*) => {
        backend_ios_core::ios_log(&format!($($arg)*))
    };
}

use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_shared::primitives::graphics::{OnLost, OnReady, OnResize};
use runtime_shared::primitives::link::LinkConfig;
use runtime_shared::primitives::navigator::NavigatorOps;
use runtime_shared::{Color, StyleRules};

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
    /// The app-level key responder installed by `set_app_key_handler` (an
    /// invisible first-responder view overriding `pressesBegan:`). Retained here;
    /// removed + resigned when the handler is replaced or cleared.
    app_key_responder: Option<Retained<keyboard::IdealystKeyResponder>>,
    /// Set of view pointers that are UIScrollViews. Used in the
    /// post-layout pass to sync `contentSize` from Taffy children.
    scroll_views: std::collections::HashSet<usize>,
    /// External single-axis scrollers (SDK leaves like `codeblock`) keyed by
    /// scroll-view pointer, mapping to the content view whose `sizeThatFits:`
    /// drives the node's measure_fn. `apply_style` diverts the author's
    /// `padding_*` on these nodes to the content label's `textInsets` (so the
    /// padding scrolls WITH the content and the scroll viewport reaches the
    /// node's own edge — the web `<pre> { padding }` observable model) instead
    /// of Taffy padding (which would inset + clip the scroll viewport itself).
    /// Populated by [`Self::install_external_content_measure`]; cleaned up in
    /// `release_external`.
    ///
    /// The `Cell` caches the content's natural (unbounded) `sizeThatFits:`,
    /// written by the measure_fn each time Taffy re-measures the node. The
    /// post-layout `contentSize` sync reads it instead of re-running
    /// `sizeThatFits:` per pass — a full text layout over the highlighted
    /// attributed string, which turned every layout pass O(codeblocks) of text
    /// measurement and tanked scroll perf on codeblock-heavy pages.
    external_content_measures:
        HashMap<usize, (Retained<UIView>, std::rc::Rc<std::cell::Cell<(f32, f32)>>)>,
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
    /// Styled-text labels (`Backend::create_styled_text`), keyed by view
    /// pointer. Holds the run list + last paragraph style so `apply_style`
    /// and `update_styled_text` re-realize the attributed string with base
    /// + deltas merged. Evicted on pointer reuse alongside
    /// `layout_style_keys` (same recycling defense).
    pub(crate) styled_texts: HashMap<usize, styled_text::StyledTextEntry>,
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
    /// Per-view cached key over only the *layout-affecting* style
    /// fields (see `style_diff::layout_affecting_key`). `apply_style`
    /// compares the incoming style's key against this; if it's
    /// unchanged the delta is paint-only (background / opacity / color
    /// / shadow / corner radius), so we skip both the Taffy
    /// `set_style` and the coalesced layout pass. Without this gate a
    /// reactive paint-only re-style (selecting a chip, dimming a
    /// pressed button) scheduled a full layout pass on every press —
    /// the "layout runs on every press" churn. Keyed by the view
    /// pointer that keys `view_to_layout`; dropped in `release_view`.
    pub(crate) layout_style_keys: HashMap<usize, String>,
    /// Viewport size at the last layout pass. When this changes
    /// (device rotation, window resize) every persistent root needs
    /// `mark_dirty` before the dirty-skip in `run_layout_pass_global`
    /// can safely opt out of computing clean roots — otherwise a
    /// rotation would leave hidden screens cached at the old
    /// dimensions and they'd render with stale sizes the moment the
    /// user navigates back.
    pub(crate) last_viewport: Option<(f32, f32)>,
    /// Height (points) of the soft keyboard currently overlapping the host
    /// view's bottom, or `0.0` when no keyboard is shown. UIKit overlays the
    /// keyboard without resizing the host, so [`Self::viewport_size`]
    /// subtracts this to shrink the layout viewport — making content reflow
    /// above the keyboard and restore when it dismisses (the iOS analog of
    /// Android's window resize). Driven by the `KeyboardObserver` →
    /// [`Self::on_keyboard_frame_changed`].
    pub(crate) keyboard_overlap: f32,
    /// Per-view cached animation state. Mirrors the web backend's
    /// `animated_states` map; see [`animated`] for the routing
    /// from [`AnimProp`](runtime_shared::animation::AnimProp) to
    /// UIKit setters and the rationale for caching the transform
    /// components.
    pub(crate) animated_states: animated::AnimatedStateMap,
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
    /// the parent's `insert_children` does `backend.insert_impl(...)`
    /// afterwards — so at apply-style time the child is still a
    /// detached floating view. We stash `(view_ptr, threshold)`
    /// here and complete the registration in `insert` once the
    /// view is actually in a parent chain. The map empties as
    /// each pending entry is promoted to the live registry.
    pub(crate) pending_sticky:
        std::collections::HashMap<usize, runtime_shared::sticky::StickyInsets>,
    /// `virtual_grid` instances, keyed by their scroller's pointer.
    pub(crate) virtual_grid_registry: virtual_grid::GridRegistry,
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
pub fn set_animated_f32(node: &IosNode, prop: runtime_shared::animation::AnimProp, value: f32) {
    let weak = IOS_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        b.set_animated_f32_impl(node, prop, value);
    };
}

/// Color-family counterpart of [`set_animated_f32`]. Routes through
/// the global backend's `set_animated_color`.
pub fn set_animated_color(
    node: &IosNode,
    prop: runtime_shared::animation::AnimProp,
    value: [f32; 4],
) {
    let weak = IOS_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        b.set_animated_color_impl(node, prop, value);
    };
}

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
}

/// Post the main-queue backstop that drains a queued layout pass.
///
/// Unconditional — the coalescing decision belongs to the callers
/// ([`schedule_layout_pass`] posts once per queued pass;
/// [`drain_queued_layout_pass`] re-posts when it had to give up).
fn post_layout_drain() {
    extern "C" {
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }

    extern "C" fn trampoline(_ctx: *mut std::ffi::c_void) {
        // libdispatch is C and a Rust panic unwinding back into it is
        // undefined behavior. catch_unwind here only prints the panic
        // message before we abort \u{2014} project policy is crash-loud
        // so the layout pass never silently keeps running on top of
        // a partially-mutated reactive state.
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(drain_queued_layout_pass));
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

/// Run the queued layout pass, if one is still queued.
///
/// Called from two places that race deliberately — the pre-commit
/// runloop observer (early, same turn, so the frames land in the paint
/// that shows the new views) and the main-queue backstop (late, but
/// guaranteed to happen). Whichever arrives first does the work; the
/// other finds the flag clear and returns.
fn drain_queued_layout_pass() {
    use crate::layout_drain_policy::{decide, Drain};

    let queued = LAYOUT_PASS_QUEUED.with(|q| q.get());
    if !queued {
        // Hot path: the pre-commit observer calls this on EVERY runloop
        // turn, including every frame of a scroll. Bail on the flag read
        // alone rather than cloning the self-ref and taking a borrow to
        // reach the same `Drain::Nothing` below.
        return;
    }
    // `IOS_BACKEND_SELF` is unset in runtime-server mode (the backend is
    // owned by value inside `RuntimeServerClient`, which drives layout
    // itself) and dangling once the app tears down.
    let rc = IOS_BACKEND_SELF
        .with(|s| s.borrow().clone())
        .and_then(|weak| weak.upgrade());
    // Take the borrow up front: whether it is available IS one of the
    // three facts the decision needs, and there is no way to ask
    // without asking.
    let borrowed = rc.as_ref().map(|rc| rc.try_borrow_mut());
    let borrow_available = matches!(borrowed, Some(Ok(_)));

    match decide(queued, rc.is_some(), borrow_available) {
        Drain::Nothing => {}
        Drain::Abandon => LAYOUT_PASS_QUEUED.with(|q| q.set(false)),
        Drain::Retry => post_layout_drain(),
        Drain::Run => {
            // Clear the queued flag BEFORE running the pass. Any
            // `schedule_layout_pass` invocations that arrive during the
            // pass itself will re-arm and fire AFTER this one — they
            // reflect post-layout state we couldn't have captured here.
            LAYOUT_PASS_QUEUED.with(|q| q.set(false));
            let Some(Ok(mut backend)) = borrowed else {
                unreachable!("Drain::Run implies the borrow succeeded")
            };
            backend.run_layout_pass_global();
        }
    }
}

/// Schedule a fresh layout pass. Safe to call from anywhere on the
/// main thread; no-op if the backend has been dropped or no self ref
/// is installed.
///
/// We **always defer** rather than running synchronously: many
/// callers are reached while the framework holds `backend.borrow_mut()`
/// (e.g. inside `Backend::insert` from the build walker). Running
/// `run_layout_pass_global` immediately in that state hits
/// `RefCell::try_borrow_mut` → `Err` and drops the pass. Deferring
/// ensures the framework's borrow is released first.
///
/// The queued pass has two drain points, and the earliest one wins:
///
/// 1. **The pre-commit runloop observer** — end of the current turn,
///    before CoreAnimation commits. This is what keeps a freshly
///    mounted screen from painting once at its parent's origin before
///    anything has laid it out.
/// 2. **The main queue** — next turn. Slower, but it is the one that
///    always happens, including for work scheduled from inside the
///    commit phase (UIKit's `layoutSubviews`, i.e. rotation) which the
///    observer has already run past.
///
/// See `backend_apple_core::pre_commit` for why both are needed.
pub fn schedule_layout_pass() {
    if LAYOUT_PASS_QUEUED.with(|q| q.replace(true)) {
        // Already queued — drop this call. The pending pass will
        // pick up whatever state changes our caller just made.
        return;
    }
    // Drain before this turn's CoreAnimation commit when we can, so a
    // freshly mounted screen is positioned in the same frame UIKit
    // first shows it. Idempotent; costs one thread-local read after
    // the first call. See `backend_apple_core::pre_commit` for why the
    // dispatch below still has to exist.
    backend_apple_core::pre_commit::install_pre_commit_hook(drain_queued_layout_pass);
    post_layout_drain();
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
    pending: &mut std::collections::HashMap<usize, runtime_shared::sticky::StickyInsets>,
    registry: &mut sticky::StickyRegistry,
    to_remove: &mut Vec<usize>,
) {
    let key = view as *const UIView as usize;
    if let Some(&insets) = pending.get(&key) {
        if sticky::register(mtm, registry, view, insets) {
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
        let mut backend = Self {
            mtm,
            host_root: None,
            callback_targets: Vec::new(),
            app_key_responder: None,
            scroll_views: std::collections::HashSet::new(),
            external_content_measures: HashMap::new(),
            icon_image_cache: HashMap::new(),
            image_cache: HashMap::new(),
            font_registry: backend_ios_core::font::FontRegistry::new(),
            portal_instances: HashMap::new(),
            layout: runtime_layout::LayoutTree::new(),
            view_to_layout: HashMap::new(),
            styled_texts: HashMap::new(),
            applied_frames: HashMap::new(),
            layout_style_keys: HashMap::new(),
            last_viewport: None,
            keyboard_overlap: 0.0,
            animated_states: HashMap::new(),
            virtualizer_instances: HashMap::new(),
            collection_views: std::collections::HashSet::new(),
            sticky_registry: HashMap::new(),
            pending_sticky: HashMap::new(),
            virtual_grid_registry: HashMap::new(),
            detached_window_roots: HashMap::new(),
        };
        backend
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
        // Brand-new registration at this pointer. Clear any stale
        // layout-style-key a freed view left behind at the same address
        // (the allocator recycles UIView pointers), so this view's
        // first `apply_style` is correctly treated as layout-affecting
        // instead of matching the dead view's key and skipping layout.
        self.layout_style_keys.remove(&key);
        // Same recycling hazard for the external-scroller registry: a
        // recycled pointer that used to be a content-measured scroller
        // must not divert the new view's padding to a dead label.
        self.external_content_measures.remove(&key);
        // ...and for styled-text runs: a recycled pointer must not make
        // the new label re-realize a dead label's runs on theme swap.
        self.styled_texts.remove(&key);
        // ...and for the last-written frame. This is the one that bites
        // hardest, because the layout pass's short-circuit is an EQUALITY
        // test: `applied_frames.get(key) == Some(&frame_key)` skips the
        // write. A recycled pointer whose new frame happens to equal the
        // dead view's cached frame therefore never gets a frame written
        // at all, and the view sits at 0x0 — the same "empty card"
        // symptom the teardown path's `applied_frames.remove` documents.
        //
        // Clearing it HERE and not only at teardown is what makes it
        // sound: the end-of-pass `retain(still_present)` cannot catch
        // this case (the recycled address IS present — as the new view),
        // and any release path that misses the teardown plan leaves the
        // stale entry behind. Registration is the one point every view
        // passes through.
        //
        // Tables are the worst case and the reason this surfaced: cells
        // share column widths and row heights, so a recycled cell
        // pointer very often lands on geometry identical to the dead
        // cell's. What is a coincidence elsewhere is the norm in a grid,
        // which is why a table would render as a mostly-empty box with a
        // few stray cells — the ones whose frames happened to differ.
        self.applied_frames.remove(&key);
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
        // the Taffy layout pass (it's a registered root). Use the RECURSIVE
        // passthrough (not the portal `OverlayPassthroughView`): the private
        // layer's content is a viewport-spanning transparent flex root, so a
        // direct-children-frame check would report YES everywhere and swallow
        // all canvas-area touches (drawing dead). See
        // `PrivateLayerPassthroughView`.
        let content: Retained<UIView> = {
            let v = callbacks::PrivateLayerPassthroughView::new(self.mtm);
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
            // PassthroughWindow (not a plain UIWindow): its `hitTest:` returns
            // nil when nothing real is hit, so canvas touches fall through to
            // the app window. A plain UIWindow's `pointInside` is YES across
            // the screen, so it would consume every passed-through touch.
            let window = callbacks::PassthroughWindow::new_with_frame(self.mtm, screen_bounds);
            let window: Retained<NSObject> = Retained::cast(window);
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

        // Soft-keyboard awareness: observe the keyboard frame so content
        // reflows above the IME and restores on dismiss. UIKit overlays the
        // keyboard WITHOUT resizing the host, so the LayoutObserverView above
        // never fires for it — we need the notification. NSNotificationCenter
        // does NOT retain `addObserver:selector:…` observers, so we retain
        // ours in `callback_targets` (dropping it ends observation at
        // teardown).
        let kb_observer = callbacks::KeyboardObserver::new(self.mtm);
        unsafe {
            let center: Retained<NSObject> =
                msg_send_id![objc2::class!(NSNotificationCenter), defaultCenter];
            let name = NSString::from_str("UIKeyboardWillChangeFrameNotification");
            let nil_obj: Option<&NSObject> = None;
            let _: () = msg_send![
                &center,
                addObserver: &*kb_observer,
                selector: objc2::sel!(keyboardFrameWillChange:),
                name: &*name,
                object: nil_obj,
            ];
        }
        let kb_obj: Retained<NSObject> = unsafe {
            let ptr = Retained::as_ptr(&kb_observer) as *mut NSObject;
            Retained::retain(ptr).unwrap()
        };
        self.callback_targets.push(kb_obj);

        // Tap-to-dismiss: a tap outside any text input ends editing, blurring
        // the active field — iOS parity with web/macOS (clicking outside an
        // input blurs it). `cancelsTouchesInView = NO` so the tap still reaches
        // buttons; the target's `shouldReceiveTouch:` skips taps on text inputs
        // so field→field focus transfer stays a clean native handoff. The
        // field's `on_blur` veto still applies via `textFieldShouldEndEditing:`.
        let dismiss_target = callbacks::KeyboardDismissTarget::new(self.mtm);
        let tap: Retained<NSObject> = unsafe {
            let alloc: *mut NSObject = msg_send![objc2::class!(UITapGestureRecognizer), alloc];
            let inited: *mut NSObject = msg_send![
                alloc,
                initWithTarget: &*dismiss_target,
                action: objc2::sel!(dismiss:),
            ];
            Retained::from_raw(inited).expect("UITapGestureRecognizer init returned nil")
        };
        let _: () = unsafe { msg_send![&tap, setCancelsTouchesInView: false] };
        let _: () = unsafe { msg_send![&tap, setDelegate: &*dismiss_target] };
        let _: () = unsafe { msg_send![&view, addGestureRecognizer: &*tap] };
        let dt_obj: Retained<NSObject> = unsafe {
            let ptr = Retained::as_ptr(&dismiss_target) as *mut NSObject;
            Retained::retain(ptr).unwrap()
        };
        self.callback_targets.push(dt_obj);

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
    /// content extent. The node fills any parent-known width and scrolls
    /// content wider than that.
    ///
    /// Also registers `node` as an external scroller so `apply_style`
    /// diverts the author's `padding_*` to `content`'s `textInsets`
    /// (see the `external_content_measures` field doc). `content` should
    /// be an inset-honoring label from [`Self::new_text_inset_label`] —
    /// its `sizeThatFits:` then includes the padding, so the measure
    /// accounts for it exactly once.
    pub fn install_external_content_measure(
        &mut self,
        node: &objc2_ui_kit::UIView,
        content: &objc2_ui_kit::UIView,
    ) {
        let layout = self.layout_for_view(node);
        let content: objc2::rc::Retained<objc2_ui_kit::UIView> = unsafe {
            objc2::rc::Retained::retain(content as *const _ as *mut objc2_ui_kit::UIView)
                .unwrap()
        };
        // Cache of the content's natural size, written here by the measure_fn
        // and read by the post-layout `contentSize` sync — so the sync never
        // re-measures. See the field doc.
        let content_size = std::rc::Rc::new(std::cell::Cell::new((0.0f32, 0.0f32)));
        self.external_content_measures.insert(
            node as *const UIView as usize,
            (content.clone(), content_size.clone()),
        );
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
                // Publish the natural size for the contentSize sync. Constant
                // across probe kinds (unbounded `sizeThatFits:`), so the last
                // write in a pass is authoritative.
                content_size.set((content_w, content_h));
                // Fill the parent's offered width (so the scroller spans
                // the column and scrolls content wider than it); report ~0 for
                // MIN-content (a single-axis scroller can shrink to nothing —
                // its content scrolls — so it must NOT floor its flex ancestors
                // at its content width, the macOS "page overflows the outlet
                // until you resize" bug); fall back to the content's own width
                // for MAX-content.
                let avail_w = match available_space.width {
                    runtime_layout::AvailableSpace::Definite(w) => Some(w),
                    runtime_layout::AvailableSpace::MinContent => Some(0.0),
                    runtime_layout::AvailableSpace::MaxContent => None,
                };
                runtime_layout::Size {
                    width: known_dimensions
                        .width
                        .or(avail_w)
                        .unwrap_or(content_w),
                    height: known_dimensions.height.unwrap_or(content_h),
                }
            }),
        );
    }

    /// Construct an inset-honoring label (`IdealystLabel`) for SDK external
    /// handlers whose content view must honor the author's `padding_*` as
    /// `textInsets` (see [`Self::install_external_content_measure`]). Returned
    /// as a plain `UIView`; it responds to every `UILabel` selector plus
    /// `setTextInsets:`.
    pub fn new_text_inset_label(&self) -> Retained<UIView> {
        let label = text_inset::IdealystLabel::new(self.mtm);
        objc2::rc::Retained::into_super(objc2::rc::Retained::into_super(label))
    }

    /// Width-aware variant of [`install_external_content_measure`](Self::install_external_content_measure)
    /// for externals whose `content` **wraps** to the offered width (a
    /// multi-line `UILabel` / `UITextView`) rather than scrolling a
    /// single axis.
    ///
    /// The difference is the probe: the scrolling variant asks the
    /// content `sizeThatFits:` at unbounded width (it never wraps, so the
    /// natural extent is correct). A wrapping label MUST be probed at the
    /// parent's offered width minus padding, or it reports its
    /// single-line intrinsic width and the returned height is one line —
    /// the text then clips/overlaps below. We pass the definite available
    /// (or known) width into `sizeThatFits:` so the label wraps and
    /// returns the true multi-line height. Used by the `markdown` SDK,
    /// which renders a whole document as one `NSAttributedString` label.
    pub fn install_external_wrapping_measure(
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
                // The width we'll wrap to: prefer an ancestor-pinned
                // width, else the parent's definite offer, else
                // unbounded (no wrap — only happens when nothing
                // constrains us, e.g. a min/max-content probe).
                let constraint_w = match (known_dimensions.width, available_space.width) {
                    (Some(w), _) => Some(w),
                    (None, runtime_layout::AvailableSpace::Definite(w)) => Some(w),
                    _ => None,
                };
                let probe_w = match constraint_w {
                    Some(w) => ((w - 2.0 * pad).max(0.0)) as f64,
                    None => f64::MAX,
                };
                let probe = objc2_foundation::CGSize {
                    width: probe_w,
                    height: f64::MAX,
                };
                let fit: objc2_foundation::CGSize =
                    unsafe { msg_send![&content, sizeThatFits: probe] };
                let content_w = (fit.width as f32).max(0.0).ceil();
                let content_h = (fit.height as f32).max(0.0).ceil();
                runtime_layout::Size {
                    width: known_dimensions
                        .width
                        .or(constraint_w)
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

// The backend mechanism, as inherent methods (runtime v2: the `Backend`
// mega-trait is gone). Bodies are verbatim what the trait impl carried;
// `newcore.rs` adapts them onto `runtime_scene::Host` + the
// `runtime_vocabulary::caps::*Ops` capability traits, one delegation per
// method. `_impl` suffix keeps the adapter's call sites unambiguous.
impl IosBackend {

    /// Navigator abstraction calls this after every command (see the trait doc).
    pub(crate) fn schedule_layout_pass_impl() {
        crate::imp::schedule_layout_pass();
    }

    pub(crate) fn set_app_key_handler_impl(&mut self, handler: Option<runtime_shared::primitives::key::KeyDownHandler>) {
        keyboard::set_app_key_handler(self, handler);
    }

    pub(crate) fn platform_impl(&self) -> runtime_shared::Platform {
        // ALWAYS `Ios`, simulator included. The simulator IS iOS — same UIKit,
        // same touch model — so `platform()` must report `Ios` there too, or
        // `is_mobile()`-gated behavior silently flips on the sim. It did exactly
        // that: the sim used to self-report `Custom("Sim")`, which is not
        // `is_mobile()`, so `dnd::Activation::platform_default()` resolved to
        // Immediate (drag-starts-on-move) instead of LongPress (press-and-hold)
        // on the simulator only. Per CLAUDE.md §7, a sim/device predicate has no
        // place in the public `Platform` API; genuine sim-only code uses the
        // compile-time `cfg(target_abi = "sim")` marker (see the camera /
        // canvas-native / vello gates), which is precise and doesn't leak into
        // runtime author code.
        runtime_shared::Platform::Ios
    }

    pub(crate) fn supports_screenshot_impl(&self) -> bool {
        // Capability, not current state: UIKit can always rasterize a
        // view hierarchy. A capture before the host root is installed
        // returns an error rather than failing this gate.
        true
    }

    pub(crate) fn capture_screenshot_impl(
        &self,
        done: Box<dyn FnOnce(Result<runtime_shared::Screenshot, String>)>,
    ) {
        let result = match self.host_root.as_ref() {
            Some(view) => screenshot::capture(view),
            None => Err("no host root installed yet".into()),
        };
        done(result);
    }

    pub(crate) fn url_opener_impl(&self) -> Option<std::rc::Rc<dyn Fn(&str)>> {
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

    pub(crate) fn fullscreen_setter_impl(&self) -> Option<std::rc::Rc<dyn Fn(bool)>> {
        // Drive the host `ViewController`'s `prefersStatusBarHidden` /
        // `prefersHomeIndicatorAutoHidden` via its `applyFullscreen:`
        // method (defined in the generated `ViewController.swift`). The
        // `respondsToSelector:` guard makes this soft-fail on an older
        // generated template that lacks the method — no
        // unrecognized-selector crash — mirroring the Android JNI-skew
        // handling. Runs on the main thread (navigator transitions are
        // main-thread), as UIKit appearance updates require.
        //
        // iOS has no system back-gesture *arrow* to suppress (disabling
        // `interactivePopGestureRecognizer` already removes the swipe),
        // so here `set_fullscreen` is the cosmetic immersive parity:
        // status bar hidden + home indicator dimmed.
        Some(std::rc::Rc::new(|enabled: bool| unsafe {
            let app: *mut NSObject =
                msg_send![objc2::class!(UIApplication), sharedApplication];
            if app.is_null() {
                return;
            }
            // Window-based app (AppDelegate sets `self.window` +
            // makeKeyAndVisible), so `keyWindow` is populated; fall back
            // to the first window defensively.
            let mut window: *mut NSObject = msg_send![app, keyWindow];
            if window.is_null() {
                let windows: *mut NSObject = msg_send![app, windows];
                if !windows.is_null() {
                    window = msg_send![windows, firstObject];
                }
            }
            if window.is_null() {
                return;
            }
            let root_vc: *mut NSObject = msg_send![window, rootViewController];
            if root_vc.is_null() {
                return;
            }
            let responds: bool =
                msg_send![root_vc, respondsToSelector: objc2::sel!(applyFullscreen:)];
            if responds {
                let _: () = msg_send![root_vc, applyFullscreen: enabled];
            }
        }))
    }

    pub(crate) fn color_scheme_impl(&self) -> runtime_shared::ColorScheme {
        // UITraitCollection.currentTraitCollection.userInterfaceStyle
        // 0 = Unspecified, 1 = Light, 2 = Dark (UIUserInterfaceStyle).
        let tc: Retained<NSObject> =
            unsafe { msg_send_id![objc2::class!(UITraitCollection), currentTraitCollection] };
        let style: isize = unsafe { msg_send![&tc, userInterfaceStyle] };
        match style {
            1 => runtime_shared::ColorScheme::Light,
            2 => runtime_shared::ColorScheme::Dark,
            _ => runtime_shared::ColorScheme::Auto,
        }
    }

    pub(crate) fn create_view_impl(&mut self, a11y: &runtime_shared::accessibility::AccessibilityProps) -> IosNode {
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

    pub(crate) fn create_text_impl(&mut self, content: &str, a11y: &runtime_shared::accessibility::AccessibilityProps) -> IosNode {
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

    pub(crate) fn create_styled_text_impl(
        &mut self,
        runs: &[runtime_shared::TextRun],
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
        // Reuse the full `create_text` path (inset label subclass, wrap
        // config, measure_fn — `sizeThatFits:` measures the attributed
        // string once it's set), then replace the plain string with the
        // attributed realization. The paragraph style arrives via
        // `apply_style` right after and re-realizes with the real base.
        let plain = runtime_shared::styled_text::plain_text_of(runs);
        let node = self.create_text_impl(&plain, a11y);
        if let IosNode::Label(label) = &node {
            styled_text::realize(label, runs, None, &self.font_registry);
            let key = &**label as *const UILabel as *const UIView as usize;
            self.styled_texts.insert(
                key,
                styled_text::StyledTextEntry { runs: runs.to_vec(), para: None },
            );
        }
        node
    }

    pub(crate) fn update_styled_text_impl(&mut self, node: &IosNode, runs: &[runtime_shared::TextRun]) {
        // Theme-cohort re-realization (and any direct caller): keep the
        // stored runs current and rebuild against the last-applied
        // paragraph style so run token colors resolve on the NEW theme.
        let IosNode::Label(label) = node else { return };
        let key = &**label as *const UILabel as *const UIView as usize;
        let para = match self.styled_texts.get_mut(&key) {
            Some(entry) => {
                entry.runs = runs.to_vec();
                entry.para.clone()
            }
            None => None,
        };
        styled_text::realize(label, runs, para.as_deref(), &self.font_registry);
    }

    pub(crate) fn create_button_impl(
        &mut self,
        label: &str,
        on_click: &runtime_shared::Action,
        leading_icon: Option<&runtime_shared::IconData>,
        _trailing_icon: Option<&runtime_shared::IconData>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
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

    pub(crate) fn update_button_label_impl(&mut self, node: &IosNode, label: &str) {
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

    pub(crate) fn create_text_input_impl(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        on_blur: Option<runtime_shared::primitives::text_input::BlurHandler>,
        secure: bool,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
        // IdealystTextField (not a plain UITextField): insets its text by the
        // author `padding_*` and fires StateBits::FOCUSED on first-responder
        // changes — the iOS half of the macOS/web padded-input + focus-ring
        // parity. See `imp::text_inset`.
        let field = text_inset::IdealystTextField::new(self.mtm);
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

        // No native bezel (0 = UITextBorderStyleNone): the framework draws the
        // border via the style's CALayer stroke (so a `focused` variant's
        // border-color change shows as the focus ring), matching macOS/web.
        let _: () = unsafe { msg_send![&field, setBorderStyle: 0isize] };

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
        // UITextField already reports change via target/action above. It ALSO
        // carries `on_blur` (consulted via `textFieldShouldEndEditing:`), so we
        // install it whenever EITHER hook is present.
        if on_key_down.is_some() || on_blur.is_some() {
            let delegate = TextKeyDelegate::new(self.mtm, on_key_down, None, on_blur);
            let _: () = unsafe { msg_send![&field, setDelegate: &*delegate] };
            self.retain_target(&delegate);
        }

        // Single-line height. A UITextField has no children, so without a
        // measure_fn Taffy collapses it to its padding box — leaving no room
        // for the text line, so the text/placeholder render cramped and clipped
        // (the "scuffed placeholder" bug). Report the field's intrinsic height
        // (Taffy then adds the author's `padding_*` around it), matching macOS's
        // intrinsicContentSize measurer. Width falls back to the framework's
        // `DEFAULT_WIDTH_PX`, not the content-hugging intrinsic width — a bare
        // field must render at web's stable default box, not collapse to its
        // text. An author `width` / `width:100%` still wins via
        // `known_dimensions.width`.
        let layout = self.layout_for_view(&field);
        let field_for_measure = field.clone();
        self.layout.set_measure_fn(
            layout,
            std::rc::Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: objc2_foundation::CGSize =
                    unsafe { msg_send![&field_for_measure, intrinsicContentSize] };
                runtime_layout::Size {
                    width: runtime_shared::primitives::text_input::measured_width(
                        known_dimensions.width,
                    ),
                    height: known_dimensions
                        .height
                        .unwrap_or((intrinsic.height as f32).ceil()),
                }
            }),
        );

        let node = IosNode::TextField(Retained::into_super(field));
        // Create-time theme default: even before any `apply_style`, a bare
        // `text_input` resolves its background→color-surface + text→color-text
        // (the `None`/no-explicit path) instead of UIKit's dark-in-dark-mode
        // `systemBackground` + `labelColor`. An authored style overrides in
        // `apply_style`. Mirrors macOS's create-time default.
        backend_ios_core::style::apply_editable_text_control_style(
            node.as_view(),
            &StyleRules::default(),
        );
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn update_text_input_value_impl(&mut self, node: &IosNode, value: &str) {
        if let IosNode::TextField(field) = node {
            let current: Option<Retained<NSString>> = unsafe { msg_send_id![field, text] };
            let current_str = current.map(|ns| ns.to_string()).unwrap_or_default();
            if current_str != value {
                let ns = NSString::from_str(value);
                unsafe { field.setText(Some(&ns)) };
            }
        }
    }

    pub(crate) fn update_text_input_secure_impl(&mut self, node: &IosNode, secure: bool) {
        // Live mask toggle — `isSecureTextEntry` flips in place on the same
        // UITextField, so the controlled value survives (password show/hide).
        if let IosNode::TextField(field) = node {
            unsafe {
                let _: () =
                    msg_send![field, setSecureTextEntry: objc2::runtime::Bool::new(secure)];
            }
        }
    }

    /// iOS has no general hover/press state firing, but an editable field needs
    /// `FOCUSED` to drive its focus ring (the `focused` style variant). Install
    /// the framework's state setter on the `IdealystTextField` so its
    /// become/resignFirstResponder flips FOCUSED; no-op for every other node
    /// (preserving today's behavior where iOS doesn't track interaction state).
    pub(crate) fn attach_states_impl(
        &mut self,
        node: &IosNode,
        setter: Rc<dyn Fn(runtime_shared::StateBits, bool)>,
    ) {
        let focus_setter = setter.clone();
        text_inset::set_text_field_focus_setter(
            node.as_view(),
            Rc::new(move |on: bool| focus_setter(runtime_shared::StateBits::FOCUSED, on)),
        );
    }

    pub(crate) fn create_text_area_impl(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
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
        // on_blur: None — TextArea has no cancelable-blur hook yet (the
        // primitive doesn't expose `on_blur` on the multi-line variant).
        let delegate = TextKeyDelegate::new(self.mtm, on_key_down, Some(on_change), None);
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
                    // Ask for the height the text needs at this width, height
                    // unbounded. `sizeThatFits:` returns content + the text
                    // view's vertical `textContainerInset`.
                    let target = objc2_foundation::CGSize {
                        width: if avail_w.is_finite() { avail_w as f64 } else { f64::MAX },
                        height: f64::MAX,
                    };
                    let fitted: objc2_foundation::CGSize =
                        unsafe { msg_send![&view_for_measure, sizeThatFits: target] };
                    // Strip the vertical inset back out to get the glyph height,
                    // then re-bound by `min_rows`/`max_rows` using the REAL font
                    // line height — the shared cross-backend rows→px contract.
                    // Past the cap, UITextView's default `scrollEnabled` scrolls.
                    let inset: crate::imp::callbacks::UIEdgeInsets =
                        unsafe { msg_send![&view_for_measure, textContainerInset] };
                    let v_pad = ((inset.top + inset.bottom) / 2.0) as f32;
                    let content_h = (fitted.height as f32) - v_pad * 2.0;
                    let line_h: f64 = unsafe {
                        let font: *mut objc2::runtime::AnyObject =
                            msg_send![&view_for_measure, font];
                        if font.is_null() { 0.0 } else { msg_send![font, lineHeight] }
                    };
                    let h = runtime_shared::primitives::text_area::resolve_text_area_height(
                        content_h, line_h as f32, v_pad, min_rows, max_rows,
                    );
                    runtime_layout::Size {
                        width: known_dimensions.width.unwrap_or((fitted.width as f32).ceil()),
                        height: known_dimensions.height.unwrap_or(h.ceil()),
                    }
                }),
            );
        }

        let node = IosNode::TextView(view);
        // Create-time theme default (see `create_text_input`): a bare
        // `text_area` / UITextView gets the theme surface + text color instead
        // of UIKit's dark `systemBackground` in dark mode.
        backend_ios_core::style::apply_editable_text_control_style(
            node.as_view(),
            &StyleRules::default(),
        );
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn update_text_area_value_impl(&mut self, node: &IosNode, value: &str) {
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

    pub(crate) fn create_toggle_impl(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
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

    pub(crate) fn update_toggle_value_impl(&mut self, node: &IosNode, value: bool) {
        if let IosNode::Switch(switch) = node {
            let current: bool = unsafe { msg_send![switch, isOn] };
            if current != value {
                unsafe { switch.setOn_animated(value, true) };
            }
        }
    }

    pub(crate) fn create_scroll_view_impl(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
        // Plain UIScrollView, frame-based. Children are added directly as
        // subviews (no inner UIStackView); their frames come from Taffy via
        // `apply_frames`. We sync `contentSize` to the bounding rect of the Taffy
        // children at the end of every layout pass so scrolling works.
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

        // Deliver touches to content IMMEDIATELY rather than delaying them while
        // UIKit decides whether the gesture is a scroll. The framework's
        // interactive leaves (Pressable / Button / on_touch) are custom
        // `IdealystTouchView`s, NOT `UIControl`s — with the default
        // `delaysContentTouches = YES`, a UIScrollView withholds `touchesBegan`
        // from them, so a button inside a scroll view (e.g. an idea-ui `Modal`'s
        // body, which wraps content in a scroll_view) reads as unpressable while
        // the same button in a non-scrolling overlay (a popover) works. A real
        // scroll drag still cancels the content touch via the pan recognizer's
        // slop, so scrolling-from-a-button is unaffected.
        let _: () = unsafe { msg_send![&scroll, setDelaysContentTouches: false] };

        // Wire `on_scroll` via UIScrollViewDelegate. The delegate
        // forwards `scrollViewDidScroll:` into the Rust closure with
        // (contentOffset.x, contentOffset.y) in UIKit points \u{2014}
        // same units as the web backend's CSS-pixel offset.
        //
        // Deliver the author callback ASYNCHRONOUSLY (next turn),
        // matching web: `scrollViewDidScroll:` fires synchronously from
        // `setContentOffset`, so a handler that scrolls inside a
        // `Ref::with` (holding the refs/arena borrow) while `on_scroll`
        // writes a signal would re-enter the reactive arena and abort
        // with "RefCell already borrowed" — the exact crash the macOS
        // twin hit via its bounds-change notification (website TOC
        // links). Same fix both backends (Rule #7).
        if let Some(cb) = on_scroll {
            let deferred: Rc<dyn Fn(f32, f32)> = Rc::new(move |x, y| {
                let cb = cb.clone();
                runtime_shared::schedule_microtask(move || cb(x, y));
            });
            let delegate = crate::imp::callbacks::ScrollDelegate::new(self.mtm, deferred);
            let _: () = unsafe { msg_send![&scroll, setDelegate: &*delegate] };
            self.retain_target(&delegate);
        }

        let scroll_layout = self.layout_for_view(&scroll);
        // Mark the scroll node `overflow: scroll` on its scroll axis. The
        // scroll *content* is parented as a Taffy CHILD of this node (children
        // are added as direct subviews and the contentSize-sync loop walks
        // `children_of(scroll_layout)`), so the content's height contributes
        // to this node's *automatic minimum size* (a flex item's auto-min is
        // its min-content). For the drawer sidebar — a `flex_grow:1 /
        // flex_basis:0` child of a fixed-height panel — that floor means
        // flexbox can't shrink the scroll node below its tall content: the
        // node grows to its content, the UIScrollView's frame ends up as tall
        // as its contentSize, and there's no overflow to scroll to the bottom
        // ("the content size doesn't match the content, can't scroll all the
        // way down"). `overflow:scroll` suppresses the auto-min floor (CSS
        // rule) so the panel bounds the scroll node to the viewport while the
        // content overflows — which is what makes a UIScrollView scroll its
        // full contentSize. Same reason macOS/Android/terminal call it; iOS
        // parents content under the scroll node just like they do.
        // `set_overflow_scroll` also seeds `flex_grow:1 / flex_basis:0` (a
        // viewport fills available space); `apply_style` still lets authors
        // override with an explicit height. Regression:
        // `regression_scroll_node_bounded_by_overflow_scroll_not_content`
        // (runtime-layout).
        self.layout.set_overflow_scroll(scroll_layout, horizontal);
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

    pub(crate) fn create_slider_impl(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
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

    pub(crate) fn update_slider_value_impl(&mut self, node: &IosNode, value: f32) {
        if let IosNode::Slider(slider) = node {
            unsafe { slider.setValue_animated(value, true) };
        }
    }

    pub(crate) fn create_activity_indicator_impl(
        &mut self,
        size: ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
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

        // Same intrinsic-size measurer as `create_slider`, `create_toggle`
        // and the icon path: `UIActivityIndicatorView` has a real
        // `intrinsicContentSize` (20x20 Medium, 37x37 Large) and Taffy does
        // not know about it. Without this the spinner's box measures 0x0.
        //
        // A zero-size LABEL draws nothing, which is why this went unseen for
        // so long. UIKit paints the indicator's own layer at its natural
        // size regardless of the box, so the glyph lands at the top-left
        // corner of an empty 0x0 box instead of where layout put it. Inside
        // a centring container that reads as "roughly centred, slightly
        // off", not as an obvious break — every loading state in the app was
        // drawing its spinner ~10pt out of place.
        let layout = self.layout_for_view(&indicator);
        let indicator_for_measure = indicator.clone();
        self.layout.set_measure_fn(
            layout,
            std::rc::Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: objc2_foundation::CGSize =
                    unsafe { msg_send![&indicator_for_measure, intrinsicContentSize] };
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(intrinsic.width as f32),
                    height: known_dimensions.height.unwrap_or(intrinsic.height as f32),
                }
            }),
        );

        let node = IosNode::ActivityIndicator(indicator);
        a11y::apply(
            &node,
            a11y,
            Some(runtime_shared::accessibility::Role::Spinner),
        );
        node
    }

    pub(crate) fn create_icon_impl(
        &mut self,
        data: &runtime_shared::primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
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
            Some(runtime_shared::accessibility::Role::Image),
        );
        node
    }

    pub(crate) fn register_asset_impl(
        &mut self,
        id: runtime_shared::AssetId,
        kind: runtime_shared::AssetTag,
        source: &runtime_shared::AssetSource,
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

    pub(crate) fn unregister_asset_impl(
        &mut self,
        id: runtime_shared::AssetId,
        kind: runtime_shared::AssetTag,
    ) {
        self.font_registry.unregister_asset(id, kind);
        if kind == runtime_shared::AssetTag::Image {
            self.image_cache.remove(&id);
        }
    }

    pub(crate) fn register_typeface_impl(
        &mut self,
        id: runtime_shared::assets::TypefaceId,
        family_name: &str,
        faces: &[runtime_shared::assets::TypefaceFace],
        fallback: runtime_shared::assets::SystemFallback,
    ) {
        self.font_registry
            .register_typeface(id, family_name, faces, fallback);
    }

    pub(crate) fn unregister_typeface_impl(&mut self, id: runtime_shared::assets::TypefaceId) {
        self.font_registry.unregister_typeface(id);
    }

    pub(crate) fn create_image_impl(&mut self, src: &str, alt: Option<&str>, a11y: &runtime_shared::accessibility::AccessibilityProps) -> IosNode {
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
            Some(runtime_shared::accessibility::Role::Image),
        );
        node
    }

    pub(crate) fn update_image_src_impl(&mut self, node: &IosNode, src: &str) {
        image::update_image_src(node, &self.image_cache, src);
        if let IosNode::View(view) = node {
            // Image swap can change intrinsicContentSize → re-measure.
            let view_clone = view.clone();
            self.install_image_measure(&view_clone);
        }
    }

    pub(crate) fn install_image_load_handler_impl(
        &mut self,
        node: &IosNode,
        handler: runtime_shared::ImageLoadHandler,
    ) {
        image::install_load_handler(node, handler);
    }

    pub(crate) fn install_image_error_handler_impl(
        &mut self,
        node: &IosNode,
        handler: runtime_shared::ImageErrorHandler,
    ) {
        image::install_error_handler(node, handler);
    }

    pub(crate) fn create_virtualizer_impl(
        &mut self,
        callbacks: runtime_shared::VirtualizerCallbacks<IosNode>,
        overscan: f32,
        virt_layout: runtime_shared::VirtualLayout,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
        let horizontal = virt_layout.axis.is_horizontal();
        // Build the UICollectionView + flow layout + data source.
        // Supports vertical and horizontal single-section lists and
        // uniform grids (`lanes > 1`) with `ItemSize::Known` sizing.
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
            virt_layout,
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
                let avail_w = match available.width {
                    runtime_layout::AvailableSpace::Definite(w) => w,
                    _ => 0.0,
                };
                let avail_h = match available.height {
                    runtime_layout::AvailableSpace::Definite(h) => h,
                    _ => 0.0,
                };
                // Main-axis content extent = sum over grid-rows of the
                // row's max item size + inter-row gaps. For a list
                // (one lane) this collapses to the plain sum. Lanes are
                // resolved against the cross-axis available space so
                // `AutoFit` measures correctly.
                let cross = if horizontal { avail_h } else { avail_w };
                let lanes = virt_layout.lanes.resolve(cross, virt_layout.cross_spacing).max(1);
                let rows = count.div_ceil(lanes);
                let mut total = 0.0_f32;
                for r in 0..rows {
                    let mut row_ext = 0.0_f32;
                    for lane in 0..lanes {
                        let i = r * lanes + lane;
                        if i >= count {
                            break;
                        }
                        row_ext = row_ext.max((item_size)(i));
                    }
                    total += row_ext;
                }
                total += rows.saturating_sub(1) as f32 * virt_layout.main_spacing;
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
            Some(runtime_shared::accessibility::Role::List),
        );
        node
    }

    pub(crate) fn virtualizer_data_changed_impl(&mut self, node: &IosNode) {
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
        runtime_shared::scheduling::schedule_microtask(move || {
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

    pub(crate) fn release_virtualizer_impl(&mut self, node: &IosNode) {
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

    pub(crate) fn update_icon_color_impl(&mut self, node: &IosNode, color: &Color) {
        icon::update_icon_color(node, color)
    }

    pub(crate) fn update_icon_stroke_impl(&mut self, node: &IosNode, progress: f32) {
        icon::update_icon_stroke(node, progress)
    }

    pub(crate) fn animate_icon_stroke_impl(
        &mut self,
        node: &IosNode,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: runtime_shared::Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        icon::animate_icon_stroke(node, from, to, duration_ms, easing, infinite, autoreverses)
    }

    pub(crate) fn make_icon_handle_impl(&self, node: &IosNode) -> runtime_shared::IconHandle {
        icon::make_handle(node)
    }

    pub(crate) fn create_graphics_impl(
        &mut self,
        on_ready: OnReady,
        on_resize: OnResize,
        on_lost: OnLost,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
        let node = graphics::create_graphics(self.mtm, &mut self.callback_targets, on_ready, on_resize, on_lost);
        // Graphics surfaces are GPU-rendered content with no inherent
        // a11y role; authors opt in via props.role / props.label.
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn create_link_impl(&mut self, config: LinkConfig, a11y: &runtime_shared::accessibility::AccessibilityProps) -> IosNode {
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
        let effective_a11y = runtime_shared::accessibility::AccessibilityProps {
            label: Some(resolved_label),
            ..a11y.clone()
        };
        a11y::apply(
            &node,
            &effective_a11y,
            Some(runtime_shared::accessibility::Role::Link),
        );
        node
    }

    pub(crate) fn create_pressable_impl(&mut self, on_click: Rc<dyn Fn()>, a11y: &runtime_shared::accessibility::AccessibilityProps) -> IosNode {
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
        // Do NOT cancel touches in the view (and its subtree) when this tap
        // recognizes. A Pressable can WRAP other interactive content — the idea-ui
        // `Modal` makes its whole card a no-op Pressable (so a tap on the card's
        // empty area is consumed instead of dismissing via the backdrop), with the
        // action buttons (raw `on_touch` `IdealystTouchView`s) nested inside it.
        // With the default `cancelsTouchesInView = YES`, the card's tap recognizer
        // recognizes a tap that lands on a button and CANCELS that button's touch
        // sequence, so the button's `touchesEnded` (where its tap handler fires)
        // never arrives and the button does nothing. `false` lets the recognizer
        // still fire this pressable's own click while delivering the touch through
        // to nested interactive descendants. (The pressable still consumes
        // outside-taps purely by being a hit-test-opaque view over the backdrop.)
        let _: () = unsafe { msg_send![&*tap_gr, setCancelsTouchesInView: false] };
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
            Some(runtime_shared::accessibility::Role::Button),
        );
        node
    }

    pub(crate) fn install_touch_handler_impl(
        &mut self,
        node: &IosNode,
        handler: runtime_shared::TouchHandler,
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

    pub(crate) fn mark_preserves_focus_impl(&mut self, node: &IosNode) {
        // Tag the view with the focus-preservation sentinel; the
        // keyboard-dismiss tap recognizer's `shouldReceiveTouch:` walks the
        // touched view's ancestors and skips `endEditing:` when it finds it
        // (see `callbacks::PRESERVES_FOCUS_TAG` for why `tag`).
        let view = node.as_view();
        let _: () = unsafe { msg_send![view, setTag: callbacks::PRESERVES_FOCUS_TAG] };
    }

    pub(crate) fn claim_touch_impl(
        &mut self,
        node: &IosNode,
        _touch_id: runtime_shared::TouchId,
    ) {
        // Walk up the responder chain looking for any UIScrollView
        // ancestor and force-cancel its in-flight pan. See
        // `imp/touch.rs::claim_touch_internal` for the rationale —
        // toggling `panGestureRecognizer.enabled` is the standard
        // iOS pattern for "stop the parent scroll immediately."
        touch::claim_touch_internal(node.as_view());
    }

    pub(crate) fn insert_impl(&mut self, parent: &mut IosNode, child: IosNode) {
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

            // Anchored portals position their CONTENT child absolutely
            // against the trigger rect and re-pin it each vsync with a
            // CADisplayLink. Composition inserts the children in paint
            // order: backdrop FIRST (it must sit behind), content LAST.
            //
            // The routing decision — and specifically the invariant that
            // we re-track to the LATEST inserted child rather than freeze
            // on the first — lives in `portal_policy::anchored_insert_action`
            // (host-tested). Freezing on the first child pinned the
            // BACKDROP for a `[backdrop, content]` popover and left the
            // content laid out top-left by the container's neutral flex
            // (the "popover renders empty / in the wrong place" bug).
            // Re-tracking to the latest child lands the tracker on the
            // content (inserted last); a single-child anchored portal is
            // unaffected (its one child is both first and last).
            //
            // `entry` definitely exists (we're inside the
            // `portal_instances.contains_key(&parent_key)` arm) but use
            // `if let` rather than `unwrap()` so a future caller can't turn
            // a transient map state into an abort.
            let action = match self.portal_instances.get(&parent_key) {
                Some(entry) => crate::portal_policy::anchored_insert_action(
                    entry.anchor.is_some(),
                    entry.anchor_link.is_some(),
                ),
                None => crate::portal_policy::AnchoredInsertAction::PlainChild,
            };
            use crate::portal_policy::AnchoredInsertAction;
            if matches!(
                action,
                AnchoredInsertAction::StartTracker | AnchoredInsertAction::RetrackToLatest
            ) {
                // Tear down any prior tracker before wiring the new one,
                // so a `[backdrop, content]` portal ends up with exactly
                // ONE live link (on the content). A leaked backdrop link
                // would keep re-pinning a view to anchor coordinates it
                // shouldn't own.
                if matches!(action, AnchoredInsertAction::RetrackToLatest) {
                    if let Some(entry_mut) = self.portal_instances.get_mut(&parent_key) {
                        if let Some(old_link) = entry_mut.anchor_link.take() {
                            let _: () = unsafe { msg_send![&*old_link, invalidate] };
                        }
                    }
                }
                let (target, side, align, offset) = {
                    // Safe: `action` is non-Plain only when `anchor.is_some()`.
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
        // ALWAYS defer. Never lay out from inside an insert.
        //
        // This used to sync on the first insert into a window-attached
        // parent, so a `switch` / `when` toggle painted its new branch
        // in the same frame as its UIKit insert rather than a tick
        // late. The goal was right and the mechanism was wrong: a
        // reactive rebuild is a TEARDOWN followed by N inserts, and
        // `run_layout_pass_global` is global — so firing it on insert
        // #1 laid out and applied frames for a tree in which every
        // OTHER region torn down in the same flush was still empty.
        //
        // What that looked like: switching a project tab tore down the
        // header's three reactive regions (crumbs, title, context bar),
        // and the first insert of the rebuild ran a full pass while
        // they were empty. The header measured 16pt instead of 150,
        // the page under it jumped up 67pt, and the next pass — 16ms
        // later, a display frame — put it back. One frame of the whole
        // screen lurching upward and dropping back, on every tab
        // change.
        //
        // `schedule_layout_pass` is now strictly better at the very
        // job the sync path existed for: its pre-commit runloop
        // observer drains at the END of this turn but BEFORE
        // CoreAnimation commits, so the new branch still paints in the
        // frame that first shows it — laid out from the FINISHED tree
        // instead of a half-built one. See
        // `backend_apple_core::pre_commit`.
        schedule_layout_pass();

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

    /// Opt into the anchorless reactive-region path. This is the root fix
    /// for the latent "`when`-mounted box never appears" bug on iOS — the
    /// exact mirror of Android's. A `create_reactive_anchor` wrapper is a
    /// real UIView that AUTO-sizes to its IN-FLOW children, so a branch
    /// whose only content is `position: Absolute` (the whiteboard's
    /// bottom-right camera box) collapsed the wrapper to 0×0 and the
    /// absolute child — though Taffy gave it a correct frame — never
    /// painted (a 0×0 superview clips a larger subview). Splicing the
    /// active branch DIRECTLY into the real parent (via `remove_child` /
    /// `insert_at`) gives in-flow content normal flow AND absolute content
    /// the real parent as its containing block — both matching web's
    /// `display: contents` anchor, with no per-case wrapper hack. It also
    /// upgrades reactive `for` to keyed reconciliation.
    pub(crate) fn supports_child_splice_impl(&self) -> bool {
        true
    }

    /// Remove a SPECIFIC `child` from `parent` (Backend::remove_child) —
    /// the removal half of an anchorless region's per-toggle rebuild.
    /// Mirrors the teardown `clear_children` does for one child: detach the
    /// native view (`removeFromSuperview`) AND the parallel Taffy child
    /// link, then `mark_dirty` the parent so its cached measured size is
    /// recomputed (Taffy doesn't auto-invalidate on a child-set change —
    /// without this the parent could keep a stale size from when the prior,
    /// taller branch was active).
    pub(crate) fn remove_child_impl(&mut self, parent: &IosNode, child: &IosNode) {
        let parent_view = parent.as_view();
        let child_view = child.as_view();
        let child_key = child_view as *const UIView as usize;

        // Symmetric with `insert` / `insert_at`'s portal + detached-root
        // skips. A portal mounts itself into the host window as an orphan
        // Taffy ROOT (never a child of `parent`); a detached window root
        // (screen_recorder private layer) lives in its own `UIWindow`.
        // Neither is `parent`'s child, so:
        //   - `removeFromSuperview` here would detach the portal container
        //     from the WINDOW out from under `release_portal` (which owns
        //     the deferred, ordered teardown), and
        //   - `layout.remove_child(parent, portal)` asks Taffy to remove a
        //     node that isn't in `parent`'s child list.
        // Their real teardown runs via the scope-tied `release_portal` /
        // `release_private_layer_window`. Skip here.
        //
        // Regression: dismissing a `Modal` by tapping its backdrop aborted
        // the app — `if open { Modal }`'s spliced-`when` unmount calls
        // `remove_child(parent, portal)`, and Taffy's `remove_child`
        // `position(..).unwrap()` panicked on the non-child portal (the
        // panic crosses the objc method boundary → `panic_cannot_unwind` →
        // SIGABRT). `LayoutTree::remove_child` is now also tolerant, but a
        // portal still must not be `removeFromSuperview`'d as if it were a
        // child here.
        if self.portal_instances.contains_key(&child_key)
            || self.detached_window_roots.contains_key(&child_key)
        {
            return;
        }

        let parent_layout = self.layout_for_view(parent_view);
        if let Some(child_layout) = self.layout_of(child_view) {
            self.layout.remove_child(parent_layout, child_layout);
        }
        self.layout.mark_dirty(parent_layout);
        // A clipped view's drop shadow lives on a sibling layer in the PARENT's
        // layer (see `backend_apple_core::shadow`), so `removeFromSuperview`
        // alone would leave it painting where the card used to be.
        backend_ios_core::style::detach_shadow_sibling(child_view);
        // Evict the cached applied frame so the next layout pass re-runs
        // `sync_view_after_frame` for this view unconditionally. The pass skips
        // views whose frame is unchanged, and `sync_shadow_path` is the one step
        // that is not purely size-dependent: it re-parents the sibling shadow
        // layer the line above just detached. Without this, re-inserting the
        // view at a bit-identical frame would leave it permanently shadowless.
        // (The end-of-pass `retain` sweep can't cover it — the view stays
        // registered in `view_to_layout` across a reparent.)
        self.applied_frames.remove(&child_key);
        unsafe { child_view.removeFromSuperview() };

        // Reflow after the removal — SYMMETRIC with `insert_at`. Marking the
        // parent dirty only invalidates its cached size; nothing recomputes
        // layout until a pass runs, and a content-sized ancestor has to
        // *shrink* to fit the now-shorter child set in the same turn (the
        // "Layers popover doesn't shrink on iOS" bug). The pre-commit drain
        // is what makes "the same turn" true; see the note in `insert` for
        // why this is no longer a synchronous pass.
        schedule_layout_pass();
    }

    /// Insert `child` into `parent` at `index` among its current subviews
    /// (Backend::insert_at). Companion to `remove_child`: an anchorless
    /// reactive region splices its single branch node at the region's
    /// stable `base_index` so a region with trailing static siblings
    /// rebuilds in the right place instead of always appending.
    ///
    /// This is `insert` (above) with two differences — it uses UIKit's
    /// `insertSubview:atIndex:` instead of `addSubview:`, and
    /// `layout.add_child_at_index` instead of `add_child` — and it
    /// preserves every special case `insert` has:
    ///
    /// - The `portal_instances` skip (a portal mounts itself into the host
    ///   window; the walker's parent-tree insert is a no-op for it).
    /// - The `detached_window_roots` skip (a private-layer window's content
    ///   view must NOT be reparented out of its excluded `UIWindow`).
    /// - The window-attached layout-pass discriminator (`parent.window !=
    ///   nil`): a child spliced into an already-mounted parent (the `when`
    ///   toggle case) MUST get a layout pass in the same turn, or it renders
    ///   at default 0×0 — reproducing the very bug for a different reason. A
    ///   mid-build splice into a floating parent defers to `finish()`.
    /// - The `promote_pending_sticky_recursive` retry, for parity.
    ///
    /// There is no portal-PARENT branch here: the spliced path only ever
    /// targets the real container the `when`/`each` lives in (see
    /// `walker::view::insert_children` → `build_when_spliced`), never a
    /// portal content holder — portals take the anchored `insert` path.
    pub(crate) fn insert_at_impl(&mut self, parent: &mut IosNode, child: IosNode, index: usize) {
        let parent_view = parent.as_view();
        let child_view = child.as_view();
        let child_key = child_view as *const UIView as usize;

        // Portal containers mount themselves into the host window — skip
        // the parent-tree splice the walker tries for them. (Mirror of
        // `insert`.)
        if self.portal_instances.contains_key(&child_key) {
            return;
        }

        // Detached window root (screen_recorder private layer): the content
        // view already lives in its OWN excluded `UIWindow`. Reparenting it
        // here would pull it back into the recorded tree. Skip the native
        // reparent; its Taffy node stays registered. (Mirror of `insert`.)
        if self.detached_window_roots.contains_key(&child_key) {
            return;
        }

        // Native indexed insert. Clamp `index` to the current subview count
        // — `-[UIView insertSubview:atIndex:]` raises `NSRangeException`
        // when `index > count`. The Taffy side clamps identically in
        // `add_child_at_index`. See `crate::splice_policy`.
        let child_count = parent_view.subviews().len();
        let idx = crate::splice_policy::clamp_insert_index(index, child_count);
        // `-[UIView insertSubview:atIndex:]` takes a signed `NSInteger`
        // (objc type code 'q' = i64); passing a `usize` ('Q' = u64) trips
        // objc2's runtime type-encoding check and aborts. The clamp keeps
        // `idx` in `[0, child_count]`, so the `isize` cast never goes
        // negative.
        let idx_ns = idx as isize;
        let _: () = unsafe { msg_send![parent_view, insertSubview: child_view, atIndex: idx_ns] };

        let p_layout = self.layout_for_view(parent_view);
        let c_layout = self.layout_for_view(child_view);
        self.layout.add_child_at_index(p_layout, c_layout, idx);
        // Same `mark_dirty` rationale as `insert` / `clear_children`: a
        // child-set change doesn't auto-invalidate the parent's cached
        // measured size.
        self.layout.mark_dirty(p_layout);

        // Deferred, like `insert` — the pre-commit drain still paints the
        // spliced branch in the frame that first shows it, and does it from
        // the finished tree. See the note in `insert`.
        schedule_layout_pass();

        // Retry pending sticky registrations for the just-spliced subtree,
        // exactly as `insert` does (the walker fires `apply_style` before
        // the parent insert, so any `Position::Sticky` child deferred its
        // registration to `pending_sticky`).
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

    pub(crate) fn update_text_impl(&mut self, node: &IosNode, content: &str) {
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

    /// Drop every layout-side trace of the subtree rooted at `view`:
    /// the `view_to_layout` registration (which holds a STRONG
    /// `Retained<UIView>`), the Taffy node, and the pointer-keyed side
    /// tables.
    ///
    /// This is the same teardown [`Self::release_portal_impl`] does, and
    /// it exists for the same reason. `clear_children` used to
    /// `layout.remove_child` each child and `removeFromSuperview` it,
    /// and stop there. Both halves leaked:
    ///
    /// - `remove_child` unparents the child's Taffy node, which makes it
    ///   a ROOT. `run_layout_pass_global` computes every root, so each
    ///   discarded subtree was recomputed against the viewport on every
    ///   later pass, forever.
    /// - the registration outlives the view's use, and because it holds
    ///   a `Retained`, the UIView can never deallocate. Every pass then
    ///   walks it in the apply-frames loop.
    ///
    /// Measured on the running app before this fix: a project screen sat
    /// at 10,425 registered views across 478 Taffy roots, climbing by
    /// several hundred with every tab switch and never falling. One
    /// layout pass took 13-19 ms, and a single tab switch ran six of
    /// them.
    ///
    /// Call this while the subtree is still attached — it walks
    /// `subviews()` to find the descendants.
    fn unregister_subtree(&mut self, view: &UIView) {
        let root_key = view as *const UIView as usize;
        let mut descendant_keys: Vec<usize> = Vec::new();
        fn collect(view: &UIView, out: &mut Vec<usize>) {
            for sub in view.subviews().iter() {
                out.push(&*sub as *const UIView as usize);
                collect(&sub, out);
            }
        }
        collect(view, &mut descendant_keys);

        // Same dedup `release_portal_impl` relies on: `remove_node` frees
        // a Taffy slot, and freeing one twice is a corruption bug, so the
        // plan must name each key exactly once.
        let plan = crate::portal_policy::teardown_plan(root_key, &descendant_keys);
        for k in plan {
            // A portal container / detached window root inside this
            // subtree is NOT ours to free: it is an orphan Taffy root
            // whose ordered teardown belongs to `release_portal` /
            // `release_private_layer_window`, exactly as
            // `remove_child_impl` skips them. Freeing it here would pull
            // the slot out from under that path.
            if self.portal_instances.contains_key(&k)
                || self.detached_window_roots.contains_key(&k)
            {
                continue;
            }
            if let Some((_view, layout_node)) = self.view_to_layout.remove(&k) {
                // Frees the Taffy slot AND marks it dropped, so a stray
                // reactive style effect that outlived the scope hits the
                // `set_style` "already-removed node" assert with a clear
                // message instead of corrupting the tree.
                self.layout.remove_node(layout_node);
            }
            // The pointer-keyed side tables, all of which would otherwise
            // be inherited by whatever view the allocator next puts at
            // this address — see `layout_for_view`'s re-registration,
            // which clears the same set for that reason.
            self.applied_frames.remove(&k);
            self.layout_style_keys.remove(&k);
            self.external_content_measures.remove(&k);
            self.styled_texts.remove(&k);
            self.pending_sticky.remove(&k);
            self.impl_drop_animated_state(k);
        }
    }

    /// A screen (or any subtree) the framework has DISCARDED: free
    /// everything the backend keeps for it. See
    /// [`Self::unregister_subtree`] for what leaks without this, and
    /// `runtime_scene::Host::release_subtree` for why this cannot be
    /// folded into `clear_children`.
    pub(crate) fn release_subtree_impl(&mut self, node: &IosNode) {
        self.unregister_subtree(node.as_view());
        // The subtree is gone from the layout tree, so the parent it
        // hung from has a stale cached size until something dirties it.
        schedule_layout_pass();
    }

    pub(crate) fn clear_children_impl(&mut self, node: &IosNode) {
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
            pending: &mut std::collections::HashMap<
                usize,
                runtime_shared::sticky::StickyInsets,
            >,
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
            // Mirror `remove_child`: unparent the sibling shadow layer, which
            // lives in THIS view's layer rather than the child's and would
            // otherwise outlive the child it belongs to, and evict the cached
            // frame so a re-insert at the same geometry still re-attaches it.
            backend_ios_core::style::detach_shadow_sibling(&sub);
            // NOT `unregister_subtree` here, however tempting. The
            // navigator's `show_in_outlet` clears the outlet for BOTH
            // meanings of "this screen is going away": a `LazyDisposing`
            // eviction (really gone) and a `LazyPersistent` switch-away
            // (detached, and the SAME node is re-inserted on return
            // without the route builder re-running). `clear_children`
            // cannot tell them apart, and freeing a retained screen's
            // Taffy nodes leaves it with no layout and no way to get one
            // back. The disposal that IS unambiguous comes from the
            // handler dropping the screen's `Realized` — see
            // `release_subtree`, which is where the teardown lives.
            self.applied_frames.remove(&(&*sub as *const UIView as usize));
            unsafe { sub.removeFromSuperview() };
        }
    }

    pub(crate) fn apply_style_impl(&mut self, node: &IosNode, style: &Rc<StyleRules>) {
        let view = node.as_view();
        apply_style_to_view(view, style);

        // Pointer events: `None` makes the subtree hit-transparent (with
        // explicit-`Auto` descendants opting back in) — the AppShell
        // scrim / overlay click-through pattern web gets from CSS
        // `pointer-events`. Enforced by `IdealystTouchView`'s
        // `hitTest:withEvent:` override; the verdict logic is shared
        // with macOS (Rule #7). `Some` overwrites, unset leaves the
        // prior apply.
        if let Some(p) = style.pointer_events {
            let is_touch_view: bool =
                unsafe { msg_send![view, isKindOfClass: objc2::class!(IdealystTouchView)] };
            if is_touch_view {
                let touch_view: &touch::IdealystTouchView =
                    unsafe { &*(view as *const UIView as *const touch::IdealystTouchView) };
                touch_view.set_pointer_events(Some(p));
            }
        }

        // Image content-fit. `object_fit` maps to the image view's
        // `contentMode`; `None` ⇒ the framework default `Contain`. A no-op on
        // non-image views (the helper guards). Kept out of
        // `apply_style_to_view` because that runs on every UIView and this is
        // image-only.
        image::apply_object_fit(
            view,
            style.object_fit.unwrap_or(runtime_shared::ObjectFit::Contain),
        );

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
        // `overscroll-behavior` → `UIScrollView.bounces`. UIKit
        // scroll views don't chain a gesture to an ancestor scroller,
        // so `Contain` and `Auto` coincide and only `None` is
        // observable: it removes the rubber-band at the content edges.
        // Applied on every style pass (not at construction) so a
        // reactive flip back to `Auto` restores the bounce instead of
        // latching off.
        {
            let is_scroll: bool =
                unsafe { msg_send![view, isKindOfClass: objc2::class!(UIScrollView)] };
            if is_scroll {
                let bounces = !matches!(
                    style.overscroll_behavior,
                    Some(runtime_shared::OverscrollBehavior::None)
                );
                let _: () = unsafe { msg_send![view, setBounces: bounces] };
            }
        }

        let view_key = view as *const UIView as usize;
        match style.position {
            Some(runtime_shared::Position::Sticky) => {
                // Per-axis thresholds (`top` → vertical off
                // `contentOffset.y`, `left` → horizontal off
                // `contentOffset.x`), resolved by the shared helper so
                // every backend reads the same sides the same way.
                let insets = runtime_shared::sticky::StickyInsets::from_style(style);
                let registered = sticky::register(
                    self.mtm,
                    &mut self.sticky_registry,
                    view,
                    insets,
                );
                if !registered {
                    // No enclosing scroll view *yet*. Could be a
                    // first-mount (insert hasn't run) or a
                    // genuinely-not-in-a-scroll-view view.
                    // Record either way; `insert` retries and
                    // `release_*` clears the entry.
                    self.pending_sticky.insert(view_key, insets);
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

        // Decide whether this `apply_style` changes anything Taffy
        // cares about. A reactive re-style frequently flips ONLY paint
        // properties (background on selection, opacity on press, text
        // color) — none of which move a box. For those, both the Taffy
        // `set_style` (which unconditionally marks the node dirty) and
        // the coalesced `schedule_layout_pass` at the end of this
        // method are pure churn: the layout pass walks every registered
        // view and re-runs flex for a result identical to the last
        // pass. Gating on the layout-affecting key removes the
        // "layout runs on every press" cost the user reported.
        //
        // Conservative: the key includes every field that could affect
        // size/placement (see `style_diff::layout_affecting_key`), and
        // a first apply (no cached key) always counts as
        // layout-affecting. A missing layout pass would be a stale-frame
        // bug; an extra one is merely wasteful — so the key errs toward
        // "layout-affecting".
        let view_key_for_style = view as *const UIView as usize;
        let next_layout_key = backend_ios_core::style_diff::layout_affecting_key(style);
        let layout_changed = backend_ios_core::style_diff::is_layout_affecting(
            self.layout_style_keys.get(&view_key_for_style).map(|s| s.as_str()),
            &next_layout_key,
        );
        if layout_changed {
            if matches!(node, IosNode::Label(_)) {
                let mut text_style: StyleRules = (**style).clone();
                text_style.padding_left = None;
                text_style.padding_right = None;
                text_style.padding_top = None;
                text_style.padding_bottom = None;
                self.layout.set_style(layout_node, &text_style);
            } else if let Some(content) = self
                .external_content_measures
                .get(&view_key_for_style)
                .map(|(c, _)| c.clone())
            {
                // External scroller (SDK leaf like `codeblock`): divert the
                // author's `padding_*` to the content label's `textInsets`
                // so the padding scrolls WITH the content — text sits inset
                // at rest and reaches the node's own edge mid-scroll,
                // matching web's `<pre> { padding }` (a UIScrollView always
                // clips at its bounds, so Taffy padding here would instead
                // shrink the scroll viewport and clip the content early).
                // The inset-honoring label folds the insets into
                // `sizeThatFits:`, which the measure_fn reports — so like
                // the Label branch above, Taffy padding must be stripped or
                // the box double-counts it.
                let resolve = |t: &Option<runtime_shared::Tokenized<runtime_shared::Length>>| {
                    t.as_ref()
                        .map(|tok| match tok.resolve() {
                            runtime_shared::Length::Px(px) => px as f64,
                            // Percent/Auto have no defined sizing parent on
                            // a measured leaf; treat as zero (same policy as
                            // `apply_text_insets_if_label`).
                            _ => 0.0,
                        })
                        .unwrap_or(0.0)
                };
                let insets = text_inset::TextInsets {
                    top: resolve(&style.padding_top),
                    left: resolve(&style.padding_left),
                    bottom: resolve(&style.padding_bottom),
                    right: resolve(&style.padding_right),
                };
                let _: () = unsafe { msg_send![&*content, setTextInsets: insets] };
                // Re-fit the label's frame to the new inset-inclusive size so
                // the post-layout `contentSize` sync (subview extents) sees
                // it. Probe at UNBOUNDED size — `sizeToFit` would measure at
                // the label's current bounds, wrapping the text to the
                // previous fitted width minus the new insets (each re-apply
                // then wraps further). A single-axis scroller's content never
                // wraps; its natural extent is the truth.
                unsafe {
                    let probe = objc2_foundation::CGSize {
                        width: f64::MAX,
                        height: f64::MAX,
                    };
                    let fit: objc2_foundation::CGSize =
                        msg_send![&*content, sizeThatFits: probe];
                    let frame = objc2_foundation::CGRect {
                        origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
                        size: fit,
                    };
                    let _: () = msg_send![&*content, setFrame: frame];
                }
                let mut scroller_style: StyleRules = (**style).clone();
                scroller_style.padding_left = None;
                scroller_style.padding_right = None;
                scroller_style.padding_top = None;
                scroller_style.padding_bottom = None;
                self.layout.set_style(layout_node, &scroller_style);
                // The measure_fn's output changed (the label folds the new
                // insets into `sizeThatFits:`), but the padding-STRIPPED
                // Taffy style may be byte-identical to the previous apply —
                // invalidate the measure cache explicitly so the box resizes.
                self.layout.mark_dirty(layout_node);
            } else {
                self.layout.set_style(layout_node, style);
            }
            self.layout_style_keys.insert(view_key_for_style, next_layout_key);
        }

        match node {
            IosNode::Label(_) => {
                apply_text_style(view, style, true, &self.font_registry);
                // Styled-text labels: the paragraph style is the BASE the
                // run deltas layer over, so a (re)style must rebuild the
                // attributed string AFTER the property writes above —
                // never the reverse (`setFont:` after `attributedText`
                // stomps per-range fonts on UIKit).
                let styled_key = view as *const UIView as usize;
                if let Some(entry) = self.styled_texts.get_mut(&styled_key) {
                    entry.para = Some(style.clone());
                    let runs = entry.runs.clone();
                    let para = entry.para.clone();
                    if let IosNode::Label(label) = node {
                        styled_text::realize(
                            label,
                            &runs,
                            para.as_deref(),
                            &self.font_registry,
                        );
                    }
                }
            }
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
                // Editable controls follow the installed THEME, not the OS
                // appearance: resolve background→color-surface, text→color-text
                // when the author didn't set them, so a bare text_input isn't a
                // dark `systemBackground` box in dark mode, and idea-ui's
                // explicit color-surface/color-text actually paints. See
                // `backend_ios_core::style::apply_editable_text_control_style`.
                backend_ios_core::style::apply_editable_text_control_style(view, style);
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
                // Theme-driven background + text color (color-surface /
                // color-text fallback when unset) so the multi-line
                // text_area / idea-ui Textarea isn't a dark box in dark mode.
                backend_ios_core::style::apply_editable_text_control_style(view, style);
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

        // `set_style` above updated the Taffy node, but Taffy only recomputes on
        // a layout pass — and a POST-MOUNT reactive style change (a `style = move
        // || …` closure firing when a signal changes) has no other trigger. Schedule
        // a (coalesced) pass so the new size/position is actually computed AND
        // written to the UIView frame. Without this the visual could update via a
        // sibling path (e.g. the whiteboard camera, whose canvas-composited image
        // tracks `cam_x/cam_y` directly) while the view's own frame — and thus its
        // hit-test rect — stayed stale, so touches missed it intermittently.
        // `schedule_layout_pass` coalesces to one pass per runloop turn, so the
        // many `apply_style` calls during a build collapse into the build's pass.
        //
        // ONLY when the layout-affecting style actually changed. A
        // paint-only re-style (background / opacity / color / shadow /
        // corner radius) leaves every box where it was, so a layout
        // pass would recompute an identical result for the whole tree —
        // that's the "layout on every press" the user reported. The
        // corner-radius paint change in particular is now applied
        // eagerly in `apply_style_to_view` against the view's live
        // bounds, so it no longer depends on a layout pass to survive
        // (see `style_diff::resolve_corner_radius`).
        if layout_changed {
            schedule_layout_pass();
        }
    }

    pub(crate) fn set_animated_f32_impl(
        &mut self,
        node: &IosNode,
        prop: runtime_shared::animation::AnimProp,
        value: f32,
    ) {
        self.impl_set_animated_f32(node, prop, value);
    }

    pub(crate) fn set_animated_color_impl(
        &mut self,
        node: &IosNode,
        prop: runtime_shared::animation::AnimProp,
        value: [f32; 4],
    ) {
        self.impl_set_animated_color(node, prop, value);
    }

    pub(crate) fn apply_presence_impl(
        &mut self,
        node: &IosNode,
        state: runtime_shared::PresenceState,
        transition: Option<(u32, runtime_shared::Easing)>,
    ) {
        self.impl_apply_presence(node, state, transition);
    }

    pub(crate) fn frame_impl(&self, node: &IosNode) -> Option<runtime_shared::primitives::portal::ViewportRect> {
        // UIView.frame is already in superview coordinates — that's
        // the relative-to-parent rect.
        let view = node.as_view();
        let frame: objc2_foundation::CGRect = unsafe { msg_send![view, frame] };
        Some(runtime_shared::primitives::portal::ViewportRect {
            x: frame.origin.x as f32,
            y: frame.origin.y as f32,
            width: frame.size.width as f32,
            height: frame.size.height as f32,
        })
    }

    pub(crate) fn absolute_frame_impl(&self, node: &IosNode) -> Option<runtime_shared::primitives::portal::ViewportRect> {
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
        Some(runtime_shared::primitives::portal::ViewportRect {
            x: frame_in_window.origin.x as f32,
            y: frame_in_window.origin.y as f32,
            width: frame_in_window.size.width as f32,
            height: frame_in_window.size.height as f32,
        })
    }

    pub(crate) fn set_disabled_impl(&mut self, node: &IosNode, disabled: bool) {
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

    pub(crate) fn create_portal_impl(
        &mut self,
        target: runtime_shared::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
        // On iOS-mobile we don't use `presentViewController:` for
        // portals — they're window-level `UIView` subviews. There's
        // no native dismiss event for that path (no swipe-down on
        // raw views, no hardware back). `on_dismiss` is effectively
        // host-signal-driven: the framework flips its open state in
        // response to whatever interaction the composition wires up
        // (backdrop tap, swipe handler on a sheet child, etc.). We
        // accept the callback but never fire it from this backend.
        use runtime_shared::primitives::portal::PortalTarget;

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
                use runtime_shared::primitives::portal::ViewportPlacement;
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

    pub(crate) fn release_portal_impl(&mut self, node: &IosNode) {
        let key = IosBackend::node_key(node);
        let Some(entry) = self.portal_instances.remove(&key) else {
            return;
        };

        // Drop the ENTIRE portal subtree from the layout bookkeeping —
        // not just the container's superview link. The portal container
        // is an orphan Taffy ROOT and its descendants stay registered in
        // `view_to_layout`; if we only `removeFromSuperview` the
        // container (the old behaviour), every later `run_layout_pass_global`
        // still finds the dead root, re-computes it against the viewport,
        // and writes frames into the detached subtree — an unbounded leak
        // plus a stale-layout source that surfaced as flaky teardown.
        //
        // Collect every descendant view key by walking the live UIKit
        // subtree BEFORE detaching anything, then dedup via
        // `portal_policy::teardown_plan` (host-tested) so no Taffy slot is
        // freed twice. `remove_node` panics on a double free, so the dedup
        // is load-bearing.
        let container_key = &*entry.container as *const UIView as usize;
        let mut descendant_keys: Vec<usize> = Vec::new();
        fn collect_descendant_keys(view: &UIView, out: &mut Vec<usize>) {
            for sub in view.subviews().iter() {
                out.push(&*sub as *const UIView as usize);
                collect_descendant_keys(&sub, out);
            }
        }
        collect_descendant_keys(&entry.container, &mut descendant_keys);

        let plan = crate::portal_policy::teardown_plan(container_key, &descendant_keys);
        for k in plan {
            if let Some((_view, layout_node)) = self.view_to_layout.remove(&k) {
                // `remove_node` frees the Taffy slot AND marks it dropped,
                // so a stray reactive style effect that outlived the scope
                // hits the `set_style` "already-removed node" assert with a
                // clear message instead of corrupting the tree.
                self.layout.remove_node(layout_node);
            }
            // Drop the cached "last frame we wrote for this pointer".
            // The allocator recycles freed UIView pointers (see the
            // `layout_for_view` re-registration which clears
            // `layout_style_keys` for the same reason). If a stale
            // `applied_frames` entry survives teardown, the NEXT view to
            // land on that pointer matches its frame_key in the layout
            // pass's short-circuit (`applied_frames.get(key) == Some(&frame_key)`)
            // and the pass SKIPS writing the real frame — the view stays
            // 0×0 / off-screen. That's the "modal re-opens with an empty
            // card" symptom: the card's recycled pointer inherited the
            // prior teardown's frame and never got laid out.
            self.applied_frames.remove(&k);
            // Per-node animation state (opacity/transform caches) keyed by
            // the same pointer — drop it so a recycled pointer doesn't
            // inherit a dead view's transform/alpha (e.g. a leftover
            // opacity 0 from the closing card's fade).
            self.impl_drop_animated_state(k);
        }

        // UIKit + anchor-tracker teardown: invalidates the CADisplayLink
        // (if any) so a final vsync can't fire into the half-torn subtree,
        // then removes the container from its window on the next runloop
        // turn.
        // Mirror `remove_child`: the portal container's own sibling shadow
        // layer belongs to the window's layer, not to the container.
        backend_ios_core::style::detach_shadow_sibling(&entry.container);
        portal::release_portal(entry);
    }

    pub(crate) fn create_external_impl(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _payload: &std::rc::Rc<dyn std::any::Any>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
        // Runtime v2: there is no backend-side External registry any more.
        // Third-party primitives register a payload handler on the scene
        // `Registry` (`runtime_scene::Registry::register`), which dispatches
        // BEFORE reaching a backend cap — so this method is only ever the
        // last-resort placeholder an SDK's own degradation handler asks for
        // on a host it has no leg for. Placeholder body unchanged.
        let node = external_placeholder_node(self, type_name);
        // Third-party externals declare their own role via
        // `props.role` if needed — we don't infer one here.
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn release_external_impl(&mut self, node: &IosNode) {
        // Detached window root (screen_recorder private layer): tear
        // down its separate UIWindow so the overlay stops compositing
        // when the layer unmounts. `release_private_layer_window`
        // returns early for any node that isn't a registered detached
        // root, so this is a cheap no-op for every other external.
        // Future SDK leaves that hold instance state (KVO observers,
        // CADisplayLink, etc.) would also clean up here, keyed by
        // `view_key` like portals do.
        if self.detached_window_roots.contains_key(&node.view_key()) {
            self.release_private_layer_window(node);
        }
        // Drop the external-scroller padding-divert registration (and its
        // retained content view) for content-measured leaves like `codeblock`.
        self.external_content_measures.remove(&node.view_key());
    }

    /// UIScrollView content offset, in points. Non-scroll nodes read (0,0),
    /// matching the trait contract.
    pub(crate) fn node_scroll_impl(&self, node: &IosNode) -> (f32, f32) {
        if let IosNode::ScrollView(scroll) = node {
            let p: objc2_foundation::CGPoint = unsafe { msg_send![&**scroll, contentOffset] };
            (p.x as f32, p.y as f32)
        } else {
            (0.0, 0.0)
        }
    }

    /// Set the UIScrollView content offset (no animation — callers are the
    /// navigator's scroll restore and the robot's `set_scroll` drive, both
    /// of which want the final position, not a tween). Non-scroll nodes are
    /// a no-op, matching the trait contract.
    pub(crate) fn set_node_scroll_impl(&mut self, node: &IosNode, x: f32, y: f32) {
        if let IosNode::ScrollView(scroll) = node {
            let p = objc2_foundation::CGPoint { x: x as f64, y: y as f64 };
            let _: () = unsafe { msg_send![&**scroll, setContentOffset: p] };
        }
    }

    pub(crate) fn apply_safe_area_padding_impl(
        &mut self,
        node: &IosNode,
        sides: runtime_shared::SafeAreaSides,
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
        let top = if sides.contains(runtime_shared::SafeAreaSides::TOP) { insets.top } else { 0.0 };
        let right = if sides.contains(runtime_shared::SafeAreaSides::RIGHT) { insets.right } else { 0.0 };
        let bottom = if sides.contains(runtime_shared::SafeAreaSides::BOTTOM) { insets.bottom } else { 0.0 };
        let left = if sides.contains(runtime_shared::SafeAreaSides::LEFT) { insets.left } else { 0.0 };

        let view = node.as_view();
        let layout_node = self.layout_for_view(view);
        self.layout.set_safe_area_extra(layout_node, top, right, bottom, left);
        schedule_layout_pass();
    }

    pub(crate) fn apply_scroll_view_safe_area_inset_impl(
        &mut self,
        node: &IosNode,
        sides: runtime_shared::SafeAreaSides,
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

    pub(crate) fn make_button_handle_impl(&self, node: &IosNode) -> runtime_shared::ButtonHandle {
        runtime_shared::ButtonHandle::new(Rc::new(node.clone()), &handles::IOS_BUTTON_OPS)
    }

    pub(crate) fn make_pressable_handle_impl(&self, node: &IosNode) -> runtime_shared::PressableHandle {
        runtime_shared::PressableHandle::new(Rc::new(node.clone()), &handles::IOS_PRESSABLE_OPS)
    }

    pub(crate) fn make_view_handle_impl(&self, node: &IosNode) -> runtime_shared::ViewHandle {
        runtime_shared::ViewHandle::new(Rc::new(node.clone()), &handles::IOS_VIEW_OPS)
    }

    pub(crate) fn make_scroll_view_handle_impl(
        &self,
        node: &IosNode,
    ) -> runtime_shared::primitives::scroll_view::ScrollViewHandle {
        runtime_shared::primitives::scroll_view::ScrollViewHandle::new(
            Rc::new(node.clone()),
            &handles::IOS_SCROLL_OPS,
        )
    }

    pub(crate) fn create_virtual_grid_impl(
        &mut self,
        callbacks: runtime_shared::primitives::virtual_grid::GridCallbacks<IosNode>,
        overscan: f32,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> IosNode {
        let view = virtual_grid::create(
            self.mtm,
            &mut self.virtual_grid_registry,
            callbacks,
            overscan,
        );
        // Taffy gives the scroller its OUTER frame; cell frames are
        // owned by the grid engine (content space), so cells are
        // deliberately not Taffy nodes — same split the virtualizer
        // uses for its collection-view cells.
        let _ = self.layout_for_view(&view);
        let node = IosNode::View(view);
        a11y::apply(&node, a11y, Some(runtime_shared::accessibility::Role::List));
        node
    }

    pub(crate) fn virtual_grid_data_changed_impl(&mut self, node: &IosNode) {
        virtual_grid::data_changed(self, node)
    }

    pub(crate) fn release_virtual_grid_impl(&mut self, node: &IosNode) {
        virtual_grid::release(self, node)
    }

    pub(crate) fn make_virtual_grid_handle_impl(
        &self,
        node: &IosNode,
    ) -> runtime_shared::primitives::virtual_grid::VirtualGridHandle {
        virtual_grid::make_handle(node)
    }

    pub(crate) fn make_virtualizer_handle_impl(
        &self,
        node: &IosNode,
    ) -> runtime_shared::primitives::virtualizer::VirtualizerHandle {
        runtime_shared::primitives::virtualizer::VirtualizerHandle::new(
            Rc::new(node.clone()),
            &handles::IOS_VIRTUALIZER_OPS,
        )
    }

    pub(crate) fn make_text_handle_impl(&self, node: &IosNode) -> runtime_shared::TextHandle {
        runtime_shared::TextHandle::new(Rc::new(node.clone()), &handles::IOS_TEXT_OPS)
    }

    pub(crate) fn make_text_input_handle_impl(
        &self,
        node: &IosNode,
    ) -> runtime_shared::primitives::text_input::TextInputHandle {
        if let IosNode::TextField(field) = node {
            runtime_shared::primitives::text_input::TextInputHandle::new(
                Rc::new(field.clone()),
                &handles::IOS_TEXT_INPUT_OPS,
            )
        } else {
            // Shouldn't happen — walker only calls this for TextInput
            // nodes. Fall back to a no-op handle wrapping an empty box.
            runtime_shared::primitives::text_input::TextInputHandle::new(
                Rc::new(()),
                &handles::IOS_TEXT_INPUT_OPS,
            )
        }
    }

    pub(crate) fn make_text_area_handle_impl(
        &self,
        node: &IosNode,
    ) -> runtime_shared::primitives::text_area::TextAreaHandle {
        if let IosNode::TextView(view) = node {
            runtime_shared::primitives::text_area::TextAreaHandle::new(
                Rc::new(view.clone()),
                &handles::IOS_TEXT_AREA_OPS,
            )
        } else {
            runtime_shared::primitives::text_area::TextAreaHandle::new(
                Rc::new(()),
                &handles::IOS_TEXT_AREA_OPS,
            )
        }
    }

    // =================================================================
    // Accessibility
    // =================================================================
    //
    // `dump_accessibility_tree` is intentionally left at its default
    // (returns `None`). UIKit walks each `UIView`'s
    // `accessibilityLabel`/`accessibilityHint`/`accessibilityTraits`
    // directly — there's no parallel semantics tree to dump.

    pub(crate) fn update_accessibility_impl(
        &mut self,
        node: &IosNode,
        a11y_props: &runtime_shared::accessibility::AccessibilityProps,
        inferred_role: Option<runtime_shared::accessibility::Role>,
    ) {
        a11y::apply(node, a11y_props, inferred_role);
    }

    pub(crate) fn announce_for_accessibility_impl(
        &mut self,
        msg: &str,
        priority: runtime_shared::accessibility::LiveRegionPriority,
    ) {
        a11y::announce(msg, priority);
    }

    pub(crate) fn finish_impl(&mut self, root: IosNode) {
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
    pub(crate) fn run_layout_impl(&mut self) {
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
            // Retrace the box shadow's `shadowPath` against the new bounds
            // (and the radius just re-clamped above). MUST run after
            // `sync_corner_radius` so the path follows the final corner
            // curve. Without an explicit path CoreAnimation derives the
            // shadow from the layer's alpha channel through an offscreen
            // render pass on every composite — the dominant GPU cost when
            // scrolling a page of shadowed cards.
            backend_ios_core::style::sync_shadow_path(view);
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
            // Feed any `on_layout` subscribers (a `.container()` view's
            // inline-size signal) with this view's resolved size. Reached
            // only when the frame actually changed (the cache `continue`
            // above skips unchanged views), so the container signal sees a
            // real width change — and its own change-guard absorbs any
            // redundant fire, keeping the container-query loop convergent.
            handles::fire_layout_for_view(*key, frame.width, frame.height);
            applied += 1;
        }
        // Drop cache entries for views that aren't registered
        // anymore. Cheap iteration over a small map (entries only
        // grow with view count; never more than `view_to_layout`).
        self.applied_frames.retain(|k, _| still_present.contains(k));
        // Same lifecycle for the layout-affecting style-key cache: a
        // recycled view-pointer must not inherit a previous view's key
        // (which would make the first `apply_style` on the new view
        // wrongly skip layout). `still_present` is the set of currently
        // registered views, so this drops keys for released views.
        self.layout_style_keys.retain(|k, _| still_present.contains(k));
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
            // Clip-aware content extent — see
            // `LayoutTree::scroll_content_extent`. The SCROLL axis is the
            // bounding box of descendants (deep walk, so a Spacer-pushed
            // footer past a `min_height: 100%` container still drives
            // `contentSize`), but the CROSS axis is clipped to the scroll
            // view's own frame: a vertical scroller can't scroll sideways, so
            // an over-wide child (a non-wrapping button row, a wide table)
            // must NOT inflate `contentSize.width` into a phantom horizontal
            // scroll. Nested scroll views clip their own content out of the
            // extent too. This previously took the bounding box on BOTH axes,
            // which is exactly the phantom-horizontal-scroll bug.
            let (max_x, max_y) = self.layout.scroll_content_extent(scroll_layout);
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

        // Sync contentSize for external content-measured scrollers (SDK
        // leaves like the `codeblock`). These build their OWN UIScrollView +
        // content label in the handler and register via
        // `install_external_content_measure`, so their content is NOT a Taffy
        // child of the scroll node (the node is a measure_fn LEAF). The loop
        // above therefore never sizes them — its extent walk reads Taffy
        // children and finds none — leaving `contentSize` at (0,0) so code
        // wider than the viewport can't scroll horizontally (the reported
        // bug). Drive contentSize from the content's natural size — the
        // widest line + full height, cached from the measure_fn's unbounded
        // `sizeThatFits:` (single-axis scrollers never wrap; the inset label
        // already folds in the diverted `padding_*`). Reading the CACHE, not
        // re-measuring here, keeps this pass O(codeblocks) of cheap frame
        // writes rather than O(codeblocks) of full text layout every pass.
        {
            let entries: Vec<(usize, Retained<UIView>, (f32, f32))> = self
                .external_content_measures
                .iter()
                .map(|(k, (v, sz))| (*k, v.clone(), sz.get()))
                .collect();
            for (scroll_ptr, content, (cw, ch)) in entries {
                let (cw, ch) = (cw as f64, ch as f64);
                let scroll_view: Retained<UIScrollView> = unsafe {
                    Retained::retain(scroll_ptr as *mut UIScrollView).unwrap()
                };
                // Frame the content label to its natural size at the scroll
                // origin so the full width is laid out (UIScrollView clips
                // subviews to its bounds otherwise, hiding the overflow). Skip
                // the write when the frame already matches — the common case
                // once content has settled.
                let cur_frame: objc2_foundation::CGRect = unsafe { msg_send![&content, frame] };
                if (cur_frame.size.width - cw).abs() > 0.5
                    || (cur_frame.size.height - ch).abs() > 0.5
                    || cur_frame.origin.x.abs() > 0.5
                    || cur_frame.origin.y.abs() > 0.5
                {
                    let content_frame = objc2_foundation::CGRect {
                        origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
                        size: objc2_foundation::CGSize { width: cw, height: ch },
                    };
                    let _: () = unsafe { msg_send![&content, setFrame: content_frame] };
                }
                // contentSize at least the viewport (short content still fills
                // edge-to-edge); wider content enables the horizontal scroll.
                let bounds: objc2_foundation::CGRect = unsafe { msg_send![&scroll_view, bounds] };
                let size = objc2_foundation::CGSize {
                    width: cw.max(bounds.size.width),
                    height: ch.max(bounds.size.height),
                };
                let current: objc2_foundation::CGSize =
                    unsafe { msg_send![&scroll_view, contentSize] };
                if (current.width - size.width).abs() > 0.5
                    || (current.height - size.height).abs() > 0.5
                {
                    let _: () = unsafe { msg_send![&scroll_view, setContentSize: size] };
                }
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
        // Re-window every `virtual_grid` now that Taffy has sized its
        // scroller. This is where a grid learns its viewport for the
        // FIRST time — `create_virtual_grid` runs before layout, so
        // without this pass a freshly-mounted grid would sit empty
        // until the user happened to scroll it.
        virtual_grid::sync_all(self);
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
        let (w, h) = self.host_viewport_size();
        // Subtract the soft-keyboard overlap so the layout viewport ends at
        // the top of the keyboard — content reflows above it, and restores
        // to full height when `keyboard_overlap` returns to 0 on dismiss.
        // Width is untouched (the keyboard only ever covers the bottom).
        (w, (h - self.keyboard_overlap).max(0.0))
    }

    /// The raw host viewport (host bounds, falling back to the screen) with
    /// NO keyboard inset applied. Separated from [`Self::viewport_size`] so
    /// the keyboard-overlap math in [`Self::on_keyboard_frame_changed`] can
    /// reason about the full host without the inset feeding back into itself.
    fn host_viewport_size(&self) -> (f32, f32) {
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

    /// Handle a keyboard frame change (show / hide / resize). `kb_frame_screen`
    /// is the keyboard's end frame in window/screen base coordinates (from
    /// `UIKeyboardFrameEndUserInfoKey`). We convert it into the host's
    /// coordinate space, intersect with the host bounds, and store the
    /// covered height as `keyboard_overlap`. On a real change we schedule a
    /// layout pass so the viewport (now inset) re-flows. On dismiss the end
    /// frame sits below the host, the intersection is empty, and the overlap
    /// returns to 0 — making open and close symmetric.
    pub(crate) fn on_keyboard_frame_changed(&mut self, kb_frame_screen: objc2_foundation::CGRect) {
        let overlap = match &self.host_root {
            Some(host) => {
                // `convertRect:fromView:nil` interprets the rect in the
                // window's base coordinate system (where the keyboard frame
                // is reported) and maps it into the host's local space.
                let nil_view: Option<&UIView> = None;
                let kb_in_host: objc2_foundation::CGRect = unsafe {
                    msg_send![&**host, convertRect: kb_frame_screen, fromView: nil_view]
                };
                let host_bounds: objc2_foundation::CGRect =
                    unsafe { msg_send![&**host, bounds] };
                rect_overlap_height(host_bounds, kb_in_host)
            }
            None => 0.0,
        };
        if (self.keyboard_overlap - overlap).abs() > 0.5 {
            self.keyboard_overlap = overlap;
            // Defer to the next main-queue turn (the standard out-of-band
            // relayout path) rather than recomputing synchronously inside the
            // notification dispatch.
            schedule_layout_pass();
        }
    }

    /// Return the host root's current safe-area insets. Used by
    /// `apply_safe_area_padding` to avoid trusting a stale framework
    /// signal value during the build/layout flow — UIKit's value is
    /// the source of truth.
    fn platform_safe_area_insets(&self) -> runtime_shared::EdgeInsets {
        let Some(host) = &self.host_root else {
            return runtime_shared::EdgeInsets::ZERO;
        };
        let insets: callbacks::UIEdgeInsets =
            unsafe { msg_send![&**host, safeAreaInsets] };
        runtime_shared::EdgeInsets {
            top: insets.top as f32,
            right: insets.right as f32,
            bottom: insets.bottom as f32,
            left: insets.left as f32,
        }
    }
}

/// Vertical overlap (points) of two `CGRect`s — the soft keyboard's coverage
/// of the host bottom. Returns 0 unless the rects also overlap horizontally,
/// so a keyboard docked beside a split-screen host (not over it) contributes
/// nothing.
fn rect_overlap_height(a: objc2_foundation::CGRect, b: objc2_foundation::CGRect) -> f32 {
    let (ax0, ax1) = (a.origin.x, a.origin.x + a.size.width);
    let (ay0, ay1) = (a.origin.y, a.origin.y + a.size.height);
    let (bx0, bx1) = (b.origin.x, b.origin.x + b.size.width);
    let (by0, by1) = (b.origin.y, b.origin.y + b.size.height);
    let x_overlap = (ax1.min(bx1) - ax0.max(bx0)).max(0.0);
    if x_overlap <= 0.0 {
        return 0.0;
    }
    (ay1.min(by1) - ay0.max(by0)).max(0.0) as f32
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
    //! which calls `with_backend(|b| b.run_layout_impl())` to size the
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
