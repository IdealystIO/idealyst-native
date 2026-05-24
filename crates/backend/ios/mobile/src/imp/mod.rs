pub(crate) mod a11y;
pub(crate) mod animated;
pub(crate) mod callbacks;
pub(crate) mod graphics;
pub(crate) mod handles;
pub(crate) mod icon;
pub(crate) mod image;
pub(crate) mod navigator;
pub(crate) mod portal;
pub(crate) mod tab_drawer;
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
use runtime_core::primitives::navigator::{
    DrawerHandle, DrawerNavigatorCallbacks, NavigatorCallbacks,
    NavigatorControl, NavigatorHandle, TabNavigatorCallbacks, TabsHandle,
};
use runtime_core::{Backend, Color, StyleRules};
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
use navigator::NavigatorEntry;
use backend_ios_core::style::{
    animate, apply_style_to_view, apply_text_style, color_to_uicolor,
};
use tab_drawer::TabDrawerEntry;

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
    navigator_instances: HashMap<usize, NavigatorEntry>,
    tab_drawer_instances: HashMap<usize, TabDrawerEntry>,
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
    /// Per-view cached animation state. Mirrors the web backend's
    /// `animated_states` map; see [`animated`] for the routing
    /// from [`AnimProp`](runtime_core::animation::AnimProp) to
    /// UIKit setters and the rationale for caching the transform
    /// components.
    pub(crate) animated_states: animated::AnimatedStateMap,
    /// Registry of third-party `Primitive::External` handlers,
    /// populated by `register_external::<T>(...)` calls from
    /// per-platform leaf crates (e.g. `webview-ios::register`).
    /// `create_external` looks the handler up by payload TypeId;
    /// unregistered kinds fall through to a "not supported" placeholder
    /// UILabel.
    pub(crate) external_handlers:
        runtime_core::ExternalRegistry<IosBackend>,
    /// Registry of `Primitive::NavigatorExt` handler factories.
    /// SDK leaf crates (`stack_navigator::register`, etc.) install
    /// factories keyed by their presentation TypeId.
    pub(crate) navigator_handlers:
        runtime_core::NavigatorRegistry<IosBackend>,
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
    pub(crate) fn as_view(&self) -> &UIView {
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

    pub(crate) fn view_key(&self) -> usize {
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
pub(crate) fn schedule_layout_pass() {
    extern "C" {
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }

    extern "C" fn trampoline(_ctx: *mut std::ffi::c_void) {
        // Absorb panics — libdispatch is C and a Rust panic unwinding
        // back into it is undefined behavior. The layout pass touches
        // user reactive state via apply effects; if any of those
        // panic, log + bail rather than abort the app.
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
            eprintln!("[backend-ios] layout-pass trampoline panic absorbed: {msg}");
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

// =========================================================================
// Helpers
// =========================================================================

impl IosBackend {
    pub fn new(mtm: MainThreadMarker) -> Self {
        Self {
            mtm,
            host_root: None,
            navigator_instances: HashMap::new(),
            tab_drawer_instances: HashMap::new(),
            callback_targets: Vec::new(),
            scroll_views: std::collections::HashSet::new(),
            icon_image_cache: HashMap::new(),
            image_cache: HashMap::new(),
            font_registry: backend_ios_core::font::FontRegistry::new(),
            portal_instances: HashMap::new(),
            layout: runtime_layout::LayoutTree::new(),
            view_to_layout: HashMap::new(),
            animated_states: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
            navigator_handlers: runtime_core::NavigatorRegistry::new(),
            virtualizer_instances: HashMap::new(),
            collection_views: std::collections::HashSet::new(),
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
    /// `Primitive::NavigatorExt`. SDK leaf crates
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
pub(crate) fn pin_to_edges(parent: &UIView, child: &UIView) {
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
pub(crate) fn mount_screen_in_vc(mtm: MainThreadMarker, screen: &UIView) -> Retained<UIViewController> {
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

/// Configure a UIViewController's navigationItem and the parent
/// UINavigationBar from `ScreenOptions`. Called after mounting a
/// screen in a stack or drawer navigator.
/// Configure a UIViewController's navigationItem and the parent
/// UINavigationBar from `ScreenOptions`. Returns retained callback
/// targets that must be kept alive (caller stores or forgets them).
pub(crate) fn apply_header_options(
    vc: &UIViewController,
    options: &runtime_core::ScreenOptions,
    mtm: MainThreadMarker,
) -> Vec<Retained<NSObject>> {
    apply_header_options_with_nav(vc, None, options, mtm)
}

/// Variant of [`apply_header_options`] that takes the parent
/// `UINavigationController` explicitly. The drawer navigator owns
/// its embedded nav controller and the rootVC's
/// `navigationController` property unexpectedly returns nil (even
/// after `setViewControllers:`) — so the drawer passes the nav
/// controller through directly. Stack navigators use the no-arg form
/// and fall back to `vc.navigationController` lookup.
pub(crate) fn apply_header_options_with_nav(
    vc: &UIViewController,
    explicit_nav_ctrl: Option<&Retained<NSObject>>,
    options: &runtime_core::ScreenOptions,
    mtm: MainThreadMarker,
) -> Vec<Retained<NSObject>> {
    let mut retained = Vec::new();

    // Resolve the nav controller pointer once. Prefer the
    // caller-supplied one; fall back to the responder-chain lookup.
    let nav_ctrl_obj: Option<Retained<NSObject>> = match explicit_nav_ctrl {
        Some(n) => Some(n.clone()),
        None => unsafe {
            let p: *const NSObject = msg_send![vc, navigationController];
            if p.is_null() {
                None
            } else {
                Retained::retain(p as *mut NSObject)
            }
        },
    };

    // Hide/show header
    if let Some(false) = options.header_shown {
        if let Some(ref nav_ctrl) = nav_ctrl_obj {
            let _: () = unsafe { msg_send![&**nav_ctrl, setNavigationBarHidden: true, animated: false] };
        }
        return vec![];
    }

    // Title
    if let Some(ref title) = options.title {
        let ns = NSString::from_str(title);
        let _: () = unsafe { msg_send![vc, setTitle: &*ns] };
    }

    // Header bar style — background, title color, and tint for back
    // chevron / bar buttons. Resolve a `UINavigationBarAppearance`
    // and assign it both as `standardAppearance` and
    // `scrollEdgeAppearance` so it stays correct whether or not the
    // top of the screen scrolls under the bar. Set it on the *nav
    // controller's* bar (not just the navItem) so the same bar is
    // re-styled per active screen.
    // Resolve color closures once. The closures are `Fn`, so calling
    // them is cheap; they typically read `active_theme()` which both
    // returns the current theme and subscribes the surrounding Effect
    // to future theme changes (when this is being called from inside
    // an Effect — see the per-VC reapply Effect set up in
    // `tab_drawer::create_drawer_navigator`).
    let header_bg = options.header_background.as_ref().map(|f| f());
    let title_color = options.title_color.as_ref().map(|f| f());
    let header_tint = options.header_tint.as_ref().map(|f| f());
    let has_bar_style = header_bg.is_some() || title_color.is_some() || header_tint.is_some();
    if has_bar_style {
        if let Some(ref nav_ctrl) = nav_ctrl_obj {
            let nav_bar: Retained<NSObject> = unsafe { msg_send_id![&**nav_ctrl, navigationBar] };
            let appearance: Retained<NSObject> = unsafe {
                msg_send_id![objc2::class!(UINavigationBarAppearance), new]
            };
            let _: () = unsafe { msg_send![&appearance, configureWithOpaqueBackground] };
            if let Some(ref bg) = header_bg {
                let c = color_to_uicolor(bg);
                let _: () = unsafe { msg_send![&appearance, setBackgroundColor: &*c] };
            }
            if let Some(ref tc) = title_color {
                // titleTextAttributes is an NSDictionary keyed by
                // NSForegroundColorAttributeName ("NSColor").
                let c = color_to_uicolor(tc);
                let key = NSString::from_str("NSColor");
                let dict: Retained<NSObject> = unsafe {
                    msg_send_id![
                        objc2::class!(NSDictionary),
                        dictionaryWithObject: &*c,
                        forKey: &*key
                    ]
                };
                let _: () = unsafe { msg_send![&appearance, setTitleTextAttributes: &*dict] };
            }
            // Cover all three appearance slots — UIKit picks among
            // them based on scroll state and compact size class, and
            // leaving any slot on a stale value lets the wrong
            // appearance flash through on rotation / scroll.
            let _: () = unsafe { msg_send![&nav_bar, setStandardAppearance: &*appearance] };
            let _: () = unsafe { msg_send![&nav_bar, setScrollEdgeAppearance: &*appearance] };
            let _: () = unsafe { msg_send![&nav_bar, setCompactAppearance: &*appearance] };
            // Per-VC appearance via `navigationItem`. UIKit 15+
            // prefers VC-level over bar-level when both are set.
            let nav_item: Retained<NSObject> = unsafe { msg_send_id![vc, navigationItem] };
            let _: () = unsafe { msg_send![&nav_item, setStandardAppearance: &*appearance] };
            let _: () = unsafe { msg_send![&nav_item, setScrollEdgeAppearance: &*appearance] };
            let _: () = unsafe { msg_send![&nav_item, setCompactAppearance: &*appearance] };
            if let Some(ref tint) = header_tint {
                let c = color_to_uicolor(tint);
                let _: () = unsafe { msg_send![&nav_bar, setTintColor: &*c] };
            }
        }
    }

    // Left bar button
    if let Some(ref btn) = options.header_left {
        let image: Retained<NSObject> = unsafe {
            let name = NSString::from_str(&btn.icon);
            msg_send_id![objc2::class!(UIImage), systemImageNamed: &*name]
        };
        let on_press = btn.on_press.clone();
        let target = CallbackTarget::new(mtm, on_press);
        let sel = objc2::sel!(invoke);
        let bar_item: Retained<NSObject> = unsafe {
            msg_send_id![objc2::class!(UIBarButtonItem), new]
        };
        let _: () = unsafe { msg_send![&bar_item, setImage: &*image] };
        let _: () = unsafe { msg_send![&bar_item, setTarget: &*target] };
        let _: () = unsafe { msg_send![&bar_item, setAction: sel] };
        // Per-button tint overrides screen-level header_tint, which
        // in turn overrides UIKit's default (systemBlue). Apply
        // directly on the bar item rather than relying on the bar's
        // tintColor inheritance — UIKit 15+ broke inheritance from
        // `navigationBar.tintColor` for items added after the
        // appearance is configured.
        let tint = btn.tint.clone().or_else(|| header_tint.clone());
        if let Some(t) = tint {
            let c = color_to_uicolor(&t);
            let _: () = unsafe { msg_send![&bar_item, setTintColor: &*c] };
        }
        let nav_item: Retained<NSObject> = unsafe { msg_send_id![vc, navigationItem] };
        let _: () = unsafe { msg_send![&nav_item, setLeftBarButtonItem: &*bar_item] };
        let obj: Retained<NSObject> = unsafe {
            Retained::retain(Retained::as_ptr(&target) as *mut NSObject).unwrap()
        };
        retained.push(obj);
    }

    // Right bar button
    if let Some(ref btn) = options.header_right {
        let image: Retained<NSObject> = unsafe {
            let name = NSString::from_str(&btn.icon);
            msg_send_id![objc2::class!(UIImage), systemImageNamed: &*name]
        };
        let on_press = btn.on_press.clone();
        let target = CallbackTarget::new(mtm, on_press);
        let sel = objc2::sel!(invoke);
        let bar_item: Retained<NSObject> = unsafe {
            msg_send_id![objc2::class!(UIBarButtonItem), new]
        };
        let _: () = unsafe { msg_send![&bar_item, setImage: &*image] };
        let _: () = unsafe { msg_send![&bar_item, setTarget: &*target] };
        let _: () = unsafe { msg_send![&bar_item, setAction: sel] };
        let tint = btn.tint.clone().or_else(|| header_tint.clone());
        if let Some(t) = tint {
            let c = color_to_uicolor(&t);
            let _: () = unsafe { msg_send![&bar_item, setTintColor: &*c] };
        }
        let nav_item: Retained<NSObject> = unsafe { msg_send_id![vc, navigationItem] };
        let _: () = unsafe { msg_send![&nav_item, setRightBarButtonItem: &*bar_item] };
        let obj: Retained<NSObject> = unsafe {
            Retained::retain(Retained::as_ptr(&target) as *mut NSObject).unwrap()
        };
        retained.push(obj);
    }

    retained
}

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
        let label = unsafe { UILabel::new(self.mtm) };
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
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let field = unsafe { UITextField::new(self.mtm) };
        let ns_val = NSString::from_str(initial_value);
        unsafe { field.setText(Some(&ns_val)) };

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
        // text rendering on iOS yet — flagged on Primitive::TextArea).
        let view: Retained<UITextView> = unsafe { UITextView::new(self.mtm) };
        let ns_val = NSString::from_str(initial_value);
        unsafe { view.setText(Some(&ns_val)) };

        // One delegate carries BOTH on_change (via textViewDidChange:)
        // and on_key_down (via shouldChangeTextInRange:). UITextView
        // has no target/action editing-changed event; the delegate is
        // the only canonical change-notification path.
        let delegate = TextKeyDelegate::new(self.mtm, on_key_down, Some(on_change));
        let _: () = unsafe { msg_send![&view, setDelegate: &*delegate] };
        self.retain_target(&delegate);

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

    fn create_scroll_view(&mut self, horizontal: bool, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
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
        // Phase-1 MVP: vertical-scrolling, single-column, Known sizing.
        // Phase-2 gaps (documented in `imp/virtualizer.rs`):
        //   - horizontal scrolling (the `horizontal` param is parked).
        //   - Measured sizing via `preferredLayoutAttributesFitting`.
        //   - Sections + sticky headers.
        //   - `performBatchUpdates` instead of `reloadData` on data
        //     changes, for animated row mutations.
        //   - Overscan tuning (UIKit's prefetch surface).
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
        let _ = self.layout_for_view(&view);
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
        virtualizer::data_changed(view);
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
        let _: () = unsafe { msg_send![&view, addGestureRecognizer: &*tap_gr] };
        self.retain_target(&target);

        let _ = self.layout_for_view(&view);
        let node = IosNode::View(view);
        // Default Link label = the route, if no author label was given.
        // `a11y::apply` clears the label when `props.label.is_none()`;
        // we re-set the route afterwards so reactive prop changes that
        // explicitly clear the label fall back to the route rather
        // than leaving the link unlabelled. Author overrides still win.
        let resolved_label = a11y.label.clone()
            .unwrap_or_else(|| config.route.to_string());
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
        if !parent_window.is_null() {
            self.run_layout_pass_global();
        } else {
            schedule_layout_pass();
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
        // effect during the layout pass.
        let layout_node = self.layout_for_view(view);
        self.layout.set_style(layout_node, style);

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

    fn create_navigator(
        &mut self,
        callbacks: NavigatorCallbacks<Self::Node>,
        control: Rc<NavigatorControl>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = navigator::create_navigator(self.mtm, &mut self.navigator_instances, callbacks, control);
        // Navigator chrome is transparent in the AX tree; per-screen
        // views inside still carry their own labels. apply() still
        // writes author-set label/hint/identifier when present.
        a11y::apply(&node, a11y, None);
        node
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: runtime_core::ScreenOptions,
    ) {
        navigator::navigator_attach_initial(self.mtm, &self.navigator_instances, navigator, screen, scope_id, options)
    }

    fn apply_navigator_header_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let key = navigator.view_key();
        if let Some(entry) = self.navigator_instances.get(&key) {
            navigator::apply_nav_header_style(&entry.controller, navigator.as_view(), style);
        }
    }

    fn apply_navigator_title_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let key = navigator.view_key();
        if let Some(entry) = self.navigator_instances.get(&key) {
            navigator::apply_nav_title_style(&entry.controller, style);
        }
    }

    fn apply_navigator_button_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let key = navigator.view_key();
        if let Some(entry) = self.navigator_instances.get(&key) {
            navigator::apply_nav_button_style(&entry.controller, style);
        }
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        navigator::release_navigator(&mut self.navigator_instances, node)
    }

    fn make_navigator_handle(&self, node: &Self::Node) -> NavigatorHandle {
        navigator::make_navigator_handle(&self.navigator_instances, node)
    }

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

    fn create_tab_navigator(
        &mut self,
        callbacks: TabNavigatorCallbacks<Self::Node>,
        control: Rc<NavigatorControl>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = tab_drawer::create_tab_navigator(self.mtm, &mut self.tab_drawer_instances, callbacks, control);
        a11y::apply(&node, a11y, None);
        node
    }

    fn tab_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: runtime_core::ScreenOptions,
    ) {
        tab_drawer::tab_navigator_attach_initial(&self.tab_drawer_instances, navigator, screen, scope_id, options)
    }

    fn release_tab_navigator(&mut self, node: &Self::Node) {
        tab_drawer::release_tab_navigator(&mut self.tab_drawer_instances, node)
    }

    fn make_tab_navigator_handle(&self, node: &Self::Node) -> TabsHandle {
        tab_drawer::make_tab_navigator_handle(&self.tab_drawer_instances, node)
    }

    // =================================================================
    // Drawer Navigator
    // =================================================================

    fn create_drawer_navigator(
        &mut self,
        callbacks: DrawerNavigatorCallbacks<Self::Node>,
        control: Rc<NavigatorControl>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = tab_drawer::create_drawer_navigator(self.mtm, &mut self.tab_drawer_instances, callbacks, control);
        a11y::apply(&node, a11y, None);
        node
    }

    fn drawer_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: runtime_core::ScreenOptions,
    ) {
        tab_drawer::drawer_navigator_attach_initial(
            self.mtm, &self.tab_drawer_instances, &mut self.callback_targets,
            navigator, screen, scope_id, options,
        )
    }

    fn drawer_navigator_attach_sidebar(
        &mut self,
        navigator: &Self::Node,
        sidebar: Self::Node,
    ) {
        tab_drawer::drawer_navigator_attach_sidebar(
            self.mtm, &self.tab_drawer_instances, &mut self.callback_targets,
            navigator, sidebar,
        )
    }

    fn release_drawer_navigator(&mut self, node: &Self::Node) {
        tab_drawer::release_drawer_navigator(&mut self.tab_drawer_instances, node)
    }

    fn make_drawer_navigator_handle(&self, node: &Self::Node) -> DrawerHandle {
        tab_drawer::make_drawer_navigator_handle(&self.tab_drawer_instances, node)
    }

    fn apply_drawer_sidebar_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let key = navigator.view_key();
        if let Some(entry) = self.tab_drawer_instances.get(&key) {
            if let Some(ref sidebar) = *entry.sidebar.borrow() {
                if let Some(ref bg) = style.background {
                    let bg_val = bg.resolve();
                    let c = backend_ios_core::style::color_to_uicolor(&bg_val);
                    sidebar.setBackgroundColor(Some(&c));
                }
            }
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
        let roots: Vec<runtime_layout::LayoutNode> = self
            .view_to_layout
            .values()
            .map(|(_, n)| *n)
            .filter(|n| self.layout.is_root(*n))
            .collect();

        backend_ios_core::ios_log(&format!("[layout] {} taffy roots to compute", roots.len()));
        for root_node in &roots {
            self.layout.compute(*root_node, vw, vh);
            let f = self.layout.frame_of(*root_node);
            backend_ios_core::ios_log(&format!(
                "[layout] root {:?} → frame ({:.1},{:.1}) {:.1}×{:.1}  style: {}",
                root_node, f.x, f.y, f.width, f.height,
                self.layout.debug_style(*root_node),
            ));
            if f.height > 500.0 {
                let children = self.layout.children_of(*root_node);
                for (i, c) in children.iter().enumerate() {
                    let cf = self.layout.frame_of(*c);
                    backend_ios_core::ios_log(&format!(
                        "[layout]    child[{}] → ({:.1},{:.1}) {:.1}×{:.1}  style: {}",
                        i, cf.x, cf.y, cf.width, cf.height,
                        self.layout.debug_style(*c),
                    ));
                }
            }
        }

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
        for (key, (view, layout_node)) in self.view_to_layout.iter() {
            let frame = self.layout.frame_of(*layout_node);
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
            applied += 1;
        }
        backend_ios_core::ios_log(&format!("[layout] apply_frames done: applied={}", applied));

        // Sync UIScrollView contentSize: walk each scroll view's
        // Taffy children, compute the bounding box, set
        // `scrollView.contentSize` to that size. Without this the
        // scroll view doesn't know how tall its content is and
        // gestures don't scroll (or only bounce, when
        // `alwaysBounceVertical` is on).
        for view_ptr in self.scroll_views.iter().copied() {
            let Some((_view_ref, scroll_layout)) = self.view_to_layout.values()
                .find(|(v, _)| (&**v as *const UIView as usize) == view_ptr)
                .cloned()
            else {
                continue;
            };
            let _ = scroll_layout; // currently not used; reserved for future per-axis adjustments

            // Bounding box of all direct Taffy children of the scroll view.
            let children = self.layout.children_of(scroll_layout);
            let mut max_x = 0.0_f32;
            let mut max_y = 0.0_f32;
            for c in children {
                let f = self.layout.frame_of(c);
                max_x = max_x.max(f.x + f.width);
                max_y = max_y.max(f.y + f.height);
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
