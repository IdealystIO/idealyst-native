pub(crate) mod anchored_overlay;
pub(crate) mod callbacks;
pub(crate) mod graphics;
pub(crate) mod handles;
pub(crate) mod icon;
pub(crate) mod navigator;
pub(crate) mod overlay;
pub(crate) mod overlay_shared;
pub(crate) mod tab_drawer;

/// Platform log with format. Forwards to `backend_ios_core::ios_log`
/// which wraps NSLog.
#[allow(dead_code)]
macro_rules! ios_log {
    ($($arg:tt)*) => {
        backend_ios_core::ios_log(&format!($($arg)*))
    };
}

use framework_core::primitives::activity_indicator::ActivityIndicatorSize;
use framework_core::primitives::graphics::{OnLost, OnReady, OnResize};
use framework_core::primitives::link::LinkConfig;
use framework_core::primitives::navigator::{
    DrawerHandle, DrawerNavigatorCallbacks, NavigatorCallbacks,
    NavigatorControl, NavigatorHandle, TabNavigatorCallbacks, TabsHandle,
};
use framework_core::{Backend, Color, StyleRules};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::{
    UIActivityIndicatorView, UIActivityIndicatorViewStyle, UIButton, UIButtonType,
    UILabel, UIScrollView, UISlider, UISwitch,
    UITextField, UIView, UIViewController,
};
use std::collections::HashMap;
use std::rc::Rc;

use callbacks::{
    BoolCallbackTarget, CallbackTarget, FloatCallbackTarget, StringCallbackTarget,
};
use navigator::NavigatorEntry;
use backend_ios_core::style::{
    animate, apply_style_to_view, apply_text_style, color_to_uicolor, font_weight_to_uikit,
    length_to_px,
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
    /// Holds the per-backend theme-transition Effect installed in
    /// `set_host_root`. Kept on the struct so the subscription's
    /// lifetime matches the backend's. None until the host root is
    /// attached.
    theme_transition_effect: Option<framework_core::Effect>,
    /// Set of view pointers that are UIScrollViews. Used in the
    /// post-layout pass to sync `contentSize` from Taffy children.
    scroll_views: std::collections::HashSet<usize>,
    /// Cache of rasterized icon UIImages keyed by (icon identity, size).
    /// Icon identity = pointer address of the `paths` static slice.
    /// Size = point size as u16 (half-point granularity is enough).
    /// Only used by `render_to_uiimage` — the standalone `create_icon`
    /// uses CAShapeLayer (vector, no raster needed).
    icon_image_cache: HashMap<(usize, u16), Retained<NSObject>>,
    /// Active viewport-anchored overlays keyed by container view
    /// pointer. Element-anchored ones live in
    /// `anchored_overlay_instances` (separate map so the two
    /// teardown paths don't have to discriminate at runtime).
    overlay_instances: overlay::OverlayInstances,
    /// Active element-anchored overlays keyed by container view
    /// pointer.
    anchored_overlay_instances: anchored_overlay::AnchoredOverlayInstances,
    /// Taffy-backed flex layout tree, parallel to the UIView tree.
    /// `view_to_layout` maps a view pointer to its layout node so the
    /// `apply_style` / `insert` / `finish` paths can update it. After
    /// build, `finish` calls `layout.compute(...)` and walks the
    /// UIView tree to set each subview's `frame`.
    pub(crate) layout: native_layout::LayoutTree,
    /// Map from view pointer (as key) to (retained reference, layout node).
    /// We hold a `Retained<UIView>` so the layout pass can iterate every
    /// registered view directly — recursing through `UIView.subviews`
    /// misses subtrees that aren't yet attached to the host (e.g. a
    /// `UINavigationController`'s top VC view, which only gets added
    /// on UIKit's first layout pass, after our `finish()` returns).
    pub(crate) view_to_layout: HashMap<usize, (Retained<UIView>, native_layout::LayoutNode)>,
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
            overlay_instances: HashMap::new(),
            anchored_overlay_instances: HashMap::new(),
            layout: native_layout::LayoutTree::new(),
            view_to_layout: HashMap::new(),
            theme_transition_effect: None,
        }
    }

    /// Get or create a layout node for a UIView. Called from every
    /// `create_*` method so each native view has a corresponding
    /// node in the layout tree.
    pub(crate) fn layout_for_view(&mut self, view: &UIView) -> native_layout::LayoutNode {
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
    pub(crate) fn layout_of(&self, view: &UIView) -> Option<native_layout::LayoutNode> {
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
        // Theme-transition Effect is installed lazily in `finish()`,
        // not here. `set_host_root` runs BEFORE the app's
        // `install_theme(...)` (which lives inside the user's
        // `app()` and is invoked by `render`), so subscribing now
        // would panic in `active_theme()` with "no theme installed".
    }

    /// Install the per-host theme-transition Effect. Subscribes to
    /// `active_theme()`; on every fire after the initial one, flips
    /// `backend_ios_core::style::THEME_TRANSITION_ACTIVE` so any
    /// color setter run during the cohort re-apply wraps itself in
    /// a 200ms `UIView.animate`. A `performSelector:afterDelay:0.0`
    /// resets the flag on the next run-loop tick.
    ///
    /// Called from `finish()` (which runs after the user's `app()`
    /// has invoked `install_theme`). Once-only — re-renders re-use
    /// the existing subscription.
    fn install_theme_transition_effect(&mut self) {
        if self.theme_transition_effect.is_some() {
            return;
        }
        let mtm = self.mtm;
        let initial = Rc::new(std::cell::Cell::new(true));
        let initial_for_effect = initial.clone();
        let theme_effect = framework_core::Effect::new(move || {
            // Subscribe to the active theme signal. Safe to call
            // unconditionally now — `finish()` runs after the user's
            // `install_theme(...)`.
            let _ = framework_core::active_theme();
            // Skip the initial run — Effect::new() always fires
            // once at install; we only want to animate genuine
            // subsequent theme swaps.
            if initial_for_effect.get() {
                initial_for_effect.set(false);
                return;
            }
            backend_ios_core::style::THEME_TRANSITION_ACTIVE.with(|c| c.set(true));
            // Schedule the flag reset for the next run loop pass.
            // By then the cohort driver's synchronous reapply loop
            // has finished and the UIView.animate blocks have all
            // been opened — clearing the flag afterward keeps
            // subsequent (non-theme) setters snappy.
            let reset_target = callbacks::CallbackTarget::new(
                mtm,
                Rc::new(|| {
                    backend_ios_core::style::THEME_TRANSITION_ACTIVE.with(|c| c.set(false));
                }),
            );
            let reset_sel = objc2::sel!(invoke);
            let _: () = unsafe {
                msg_send![
                    &reset_target,
                    performSelector: reset_sel,
                    withObject: std::ptr::null::<NSObject>(),
                    afterDelay: 0.0 as objc2_foundation::CGFloat
                ]
            };
            // `forget` so the target outlives the perform delay.
            let target_obj: Retained<NSObject> = unsafe {
                let ptr = Retained::as_ptr(&reset_target) as *mut NSObject;
                Retained::retain(ptr).unwrap()
            };
            std::mem::forget(target_obj);
        });
        self.theme_transition_effect = Some(theme_effect);
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
    options: &framework_core::ScreenOptions,
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
    options: &framework_core::ScreenOptions,
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

    fn color_scheme(&self) -> framework_core::ColorScheme {
        // UITraitCollection.currentTraitCollection.userInterfaceStyle
        // 0 = Unspecified, 1 = Light, 2 = Dark (UIUserInterfaceStyle).
        let tc: Retained<NSObject> =
            unsafe { msg_send_id![objc2::class!(UITraitCollection), currentTraitCollection] };
        let style: isize = unsafe { msg_send![&tc, userInterfaceStyle] };
        match style {
            1 => framework_core::ColorScheme::Light,
            2 => framework_core::ColorScheme::Dark,
            _ => framework_core::ColorScheme::Auto,
        }
    }

    fn create_view(&mut self) -> Self::Node {
        // Plain UIView. Children are positioned via Taffy-computed
        // frames in `finish`. We no longer use UIStackView — its
        // arranged-subview model fights with flex semantics (no
        // flex-grow/shrink, no wrap, zero-intrinsic-collapsing
        // children, opaque constraint conflicts).
        let view = unsafe { UIView::new(self.mtm) };
        // `translatesAutoresizingMaskIntoConstraints` defaults to
        // YES on `[UIView new]`, which is what we want — frame
        // assignment becomes authoritative.
        let _ = self.layout_for_view(&view);
        IosNode::View(view)
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
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
                        native_layout::AvailableSpace::Definite(w) => w,
                        native_layout::AvailableSpace::MaxContent => f32::INFINITY,
                        native_layout::AvailableSpace::MinContent => 0.0,
                    });
                let avail_h = known_dimensions
                    .height
                    .unwrap_or(match available_space.height {
                        native_layout::AvailableSpace::Definite(h) => h,
                        native_layout::AvailableSpace::MaxContent => f32::INFINITY,
                        native_layout::AvailableSpace::MinContent => 0.0,
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
                let result = native_layout::Size {
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

        IosNode::Label(label)
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &framework_core::Action,
        leading_icon: Option<&framework_core::IconData>,
        _trailing_icon: Option<&framework_core::IconData>,
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
                        native_layout::AvailableSpace::Definite(w) => w,
                        native_layout::AvailableSpace::MaxContent => f32::INFINITY,
                        native_layout::AvailableSpace::MinContent => 0.0,
                    });
                let avail_h = known_dimensions
                    .height
                    .unwrap_or(match available_space.height {
                        native_layout::AvailableSpace::Definite(h) => h,
                        native_layout::AvailableSpace::MaxContent => f32::INFINITY,
                        native_layout::AvailableSpace::MinContent => 0.0,
                    });
                let target = objc2_foundation::CGSize {
                    width: if avail_w.is_finite() { avail_w as f64 } else { f64::MAX },
                    height: if avail_h.is_finite() { avail_h as f64 } else { f64::MAX },
                };
                let fitted: objc2_foundation::CGSize =
                    unsafe { msg_send![&button_for_measure, sizeThatFits: target] };
                native_layout::Size {
                    width: known_dimensions.width.unwrap_or(fitted.width as f32),
                    height: known_dimensions.height.unwrap_or(fitted.height as f32),
                }
            }),
        );

        IosNode::Button(button)
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

        IosNode::TextField(field)
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

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
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
                native_layout::Size {
                    width: known_dimensions.width.unwrap_or(intrinsic.width as f32),
                    height: known_dimensions.height.unwrap_or(intrinsic.height as f32),
                }
            }),
        );

        IosNode::Switch(switch)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        if let IosNode::Switch(switch) = node {
            let current: bool = unsafe { msg_send![switch, isOn] };
            if current != value {
                unsafe { switch.setOn_animated(value, true) };
            }
        }
    }

    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
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
        IosNode::ScrollView(scroll)
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
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
                native_layout::Size {
                    width: known_dimensions.width.unwrap_or(intrinsic.width as f32),
                    height: known_dimensions.height.unwrap_or(intrinsic.height as f32),
                }
            }),
        );

        IosNode::Slider(slider)
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

        IosNode::ActivityIndicator(indicator)
    }

    fn create_icon(
        &mut self,
        data: &framework_core::primitives::icon::IconData,
        color: Option<&Color>,
    ) -> Self::Node {
        icon::create_icon(self.mtm, data, color)
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
        easing: framework_core::Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        icon::animate_icon_stroke(node, from, to, duration_ms, easing, infinite, autoreverses)
    }

    fn make_icon_handle(&self, node: &Self::Node) -> framework_core::IconHandle {
        icon::make_handle(node)
    }

    fn create_graphics(
        &mut self,
        on_ready: OnReady,
        on_resize: OnResize,
        on_lost: OnLost,
    ) -> Self::Node {
        graphics::create_graphics(self.mtm, &mut self.callback_targets, on_ready, on_resize, on_lost)
    }

    fn create_link(&mut self, config: LinkConfig) -> Self::Node {
        // Plain UIView (was UIStackView). UIStackView injected internal
        // UISV-canvas-connection constraints that fought Taffy's
        // frame-based positioning — manifested as sibling links in the
        // drawer sidebar overlapping with gap=0 instead of honoring
        // the parent's `gap`, and the Link's own height collapsing.
        // Children now render via the normal addSubview + Taffy frame
        // path, identical to `create_view`.
        let view = unsafe { UIView::new(self.mtm) };
        let _: () = unsafe { msg_send![&view, setUserInteractionEnabled: true] };

        let ns_route = NSString::from_str(config.route);
        let _: () = unsafe { msg_send![&view, setAccessibilityLabel: &*ns_route] };

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
        IosNode::View(view)
    }

    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>) -> Self::Node {
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
        IosNode::View(view)
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let parent_view = parent.as_view();
        let parent_key = parent_view as *const UIView as usize;
        let child_view = child.as_view();
        let child_key = child_view as *const UIView as usize;

        // Overlay containers (both kinds) mount themselves into the
        // host window — skip the parent-tree insert the walker tries
        // for them.
        if self.overlay_instances.contains_key(&child_key)
            || self.anchored_overlay_instances.contains_key(&child_key)
        {
            return;
        }

        // Viewport-anchored overlay parent: addSubview + Taffy
        // add_child. The container's flex style (set in
        // `create_overlay`) places the child via justify/align.
        if self.overlay_instances.contains_key(&parent_key) {
            unsafe { parent_view.addSubview(child_view) };
            let p_layout = self.layout_for_view(parent_view);
            let c_layout = self.layout_for_view(child_view);
            self.layout.add_child(p_layout, c_layout);
            // Overlays mount dynamically (when their open signal
            // flips) so the framework's `finish()` hook can't size
            // them — kick a layout pass now.
            schedule_layout_pass();
            return;
        }

        // Element-anchored overlay parent: addSubview + Taffy
        // add_child + apply the absolute-position child style + start
        // the per-vsync anchor tracker.
        if self.anchored_overlay_instances.contains_key(&parent_key) {
            unsafe { parent_view.addSubview(child_view) };
            let p_layout = self.layout_for_view(parent_view);
            let c_layout = self.layout_for_view(child_view);
            self.layout.add_child(p_layout, c_layout);

            // Snapshot what we need from the entry before we take a
            // mutable borrow on `self.layout` (which the entry's
            // tracker setup also needs).
            let (target, side, align, offset) = {
                let entry = self.anchored_overlay_instances.get(&parent_key).unwrap();
                (entry.target.clone(), entry.side, entry.align, entry.offset)
            };
            let child_rules = anchored_overlay::child_style_for_anchor(&target, side, align, offset);
            self.layout.set_style(c_layout, &child_rules);
            schedule_layout_pass();

            let popover: Retained<UIView> = unsafe {
                Retained::retain(child_view as *const UIView as *mut UIView)
                    .expect("retain popover view")
            };
            let link = anchored_overlay::start_anchor_tracker(
                self.mtm, popover, target, side, align, offset,
            );
            if let Some(entry_mut) = self.anchored_overlay_instances.get_mut(&parent_key) {
                entry_mut.anchor_link = Some(link);
            }
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
        let child_layouts: Vec<native_layout::LayoutNode> = parent
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

        // Mirror the resolved style into the Taffy node so flex
        // properties (width/height/flex-direction/padding/gap/…) take
        // effect during the layout pass.
        let layout_node = self.layout_for_view(view);
        self.layout.set_style(layout_node, style);

        match node {
            IosNode::Label(_) => apply_text_style(view, style, true),
            IosNode::Button(button) => {
                if let Some(color) = &style.color {
                    let c = color_to_uicolor(color.value());
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
                if let Some(fs) = &style.font_size {
                    let size = length_to_px(fs.value());
                    if size > 0.0 {
                        let weight = style.font_weight.as_ref().copied().unwrap_or(framework_core::FontWeight::Normal);
                        let ui_weight = font_weight_to_uikit(weight);
                        let font: Retained<NSObject> = unsafe {
                            msg_send_id![
                                objc2::class!(UIFont),
                                systemFontOfSize: size,
                                weight: ui_weight
                            ]
                        };
                        let title_label: Option<Retained<UILabel>> = unsafe { msg_send_id![button, titleLabel] };
                        if let Some(tl) = title_label {
                            let _: () = unsafe { msg_send![&tl, setFont: &*font] };
                        }
                    }
                }
            }
            IosNode::TextField(_) => apply_text_style(view, style, false),
            _ => {}
        }
    }

    fn frame(&self, node: &Self::Node) -> Option<framework_core::primitives::overlay::ViewportRect> {
        // UIView.frame is already in superview coordinates — that's
        // the relative-to-parent rect.
        let view = node.as_view();
        let frame: objc2_foundation::CGRect = unsafe { msg_send![view, frame] };
        Some(framework_core::primitives::overlay::ViewportRect {
            x: frame.origin.x as f32,
            y: frame.origin.y as f32,
            width: frame.size.width as f32,
            height: frame.size.height as f32,
        })
    }

    fn absolute_frame(&self, node: &Self::Node) -> Option<framework_core::primitives::overlay::ViewportRect> {
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
        Some(framework_core::primitives::overlay::ViewportRect {
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
    ) -> Self::Node {
        navigator::create_navigator(self.mtm, &mut self.navigator_instances, callbacks, control)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::ScreenOptions,
    ) {
        navigator::navigator_attach_initial(self.mtm, &self.navigator_instances, navigator, screen, scope_id, options)
    }

    fn apply_navigator_header_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<framework_core::StyleRules>,
    ) {
        let key = navigator.view_key();
        if let Some(entry) = self.navigator_instances.get(&key) {
            navigator::apply_nav_header_style(&entry.controller, navigator.as_view(), style);
        }
    }

    fn apply_navigator_title_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<framework_core::StyleRules>,
    ) {
        let key = navigator.view_key();
        if let Some(entry) = self.navigator_instances.get(&key) {
            navigator::apply_nav_title_style(&entry.controller, style);
        }
    }

    fn apply_navigator_button_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<framework_core::StyleRules>,
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
    // Overlay
    // =================================================================

    fn create_overlay(
        &mut self,
        placement: framework_core::primitives::overlay::ViewportPlacement,
        backdrop: framework_core::primitives::overlay::BackdropMode,
        on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
    ) -> Self::Node {
        let (content_view, entry) = overlay::create_overlay(
            self.mtm,
            self.host_root.as_ref(),
            placement,
            backdrop,
            on_dismiss,
        );
        let key = &*content_view as *const UIView as usize;
        self.overlay_instances.insert(key, entry);

        // Register the container in the layout tree as a Taffy root.
        // It's orphan (no parent in Taffy because `insert` skips its
        // own insertion), so `compute()`'s viewport auto-fill resizes
        // it to the full viewport on every layout pass — including
        // orientation flips. The placement-derived flex style places
        // the overlay's content child within that frame.
        let layout_node = self.layout_for_view(&content_view);
        let rules = overlay::container_style_for_placement(placement);
        self.layout.set_style(layout_node, &rules);

        IosNode::View(content_view)
    }

    fn release_overlay(&mut self, node: &Self::Node) {
        let key = IosBackend::node_key(node);
        if let Some(entry) = self.overlay_instances.remove(&key) {
            overlay::release_overlay(entry);
        }
    }

    fn create_anchored_overlay(
        &mut self,
        target: framework_core::primitives::overlay::AnchorTarget,
        side: framework_core::primitives::overlay::ElementSide,
        align: framework_core::primitives::overlay::ElementAlign,
        offset: f32,
        backdrop: framework_core::primitives::overlay::BackdropMode,
        on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
    ) -> Self::Node {
        let (content_view, entry) = anchored_overlay::create_anchored_overlay(
            self.mtm,
            self.host_root.as_ref(),
            target,
            side,
            align,
            offset,
            backdrop,
            on_dismiss,
        );
        let key = &*content_view as *const UIView as usize;
        self.anchored_overlay_instances.insert(key, entry);

        // Same Taffy-root treatment as viewport overlays — neutral
        // flex container that fills the viewport so the popover
        // child's absolute coordinates line up with window space.
        let layout_node = self.layout_for_view(&content_view);
        let rules = anchored_overlay::container_style();
        self.layout.set_style(layout_node, &rules);

        IosNode::View(content_view)
    }

    fn release_anchored_overlay(&mut self, node: &Self::Node) {
        let key = IosBackend::node_key(node);
        if let Some(entry) = self.anchored_overlay_instances.remove(&key) {
            anchored_overlay::release_anchored_overlay(entry);
        }
    }

    fn apply_safe_area_padding(
        &mut self,
        node: &Self::Node,
        sides: framework_core::SafeAreaSides,
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
        let top = if sides.contains(framework_core::SafeAreaSides::TOP) { insets.top } else { 0.0 };
        let right = if sides.contains(framework_core::SafeAreaSides::RIGHT) { insets.right } else { 0.0 };
        let bottom = if sides.contains(framework_core::SafeAreaSides::BOTTOM) { insets.bottom } else { 0.0 };
        let left = if sides.contains(framework_core::SafeAreaSides::LEFT) { insets.left } else { 0.0 };

        let view = node.as_view();
        let layout_node = self.layout_for_view(view);
        self.layout.set_safe_area_extra(layout_node, top, right, bottom, left);
        schedule_layout_pass();
    }

    fn apply_scroll_view_safe_area_inset(
        &mut self,
        node: &Self::Node,
        sides: framework_core::SafeAreaSides,
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

    fn make_button_handle(&self, node: &Self::Node) -> framework_core::ButtonHandle {
        framework_core::ButtonHandle::new(Rc::new(node.clone()), &handles::IOS_BUTTON_OPS)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> framework_core::PressableHandle {
        framework_core::PressableHandle::new(Rc::new(node.clone()), &handles::IOS_PRESSABLE_OPS)
    }

    fn make_view_handle(&self, node: &Self::Node) -> framework_core::ViewHandle {
        framework_core::ViewHandle::new(Rc::new(node.clone()), &handles::IOS_VIEW_OPS)
    }

    // =================================================================
    // Tab Navigator
    // =================================================================

    fn create_tab_navigator(
        &mut self,
        callbacks: TabNavigatorCallbacks<Self::Node>,
        control: Rc<NavigatorControl>,
    ) -> Self::Node {
        tab_drawer::create_tab_navigator(self.mtm, &mut self.tab_drawer_instances, callbacks, control)
    }

    fn tab_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::ScreenOptions,
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
    ) -> Self::Node {
        tab_drawer::create_drawer_navigator(self.mtm, &mut self.tab_drawer_instances, callbacks, control)
    }

    fn drawer_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::ScreenOptions,
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
        style: &Rc<framework_core::StyleRules>,
    ) {
        let key = navigator.view_key();
        if let Some(entry) = self.tab_drawer_instances.get(&key) {
            if let Some(ref sidebar) = *entry.sidebar.borrow() {
                if let Some(ref bg) = style.background {
                    let c = backend_ios_core::style::color_to_uicolor(bg.value());
                    sidebar.setBackgroundColor(Some(&c));
                }
            }
        }
    }

    fn finish(&mut self, root: Self::Node) {
        if let Some(host) = &self.host_root {
            pin_to_edges(host, root.as_view());
        }
        self.run_layout_pass(&root);
        // Theme is guaranteed installed by now — `app()` ran during
        // the walker pass that produced `root`, and any theme-using
        // app calls `install_theme` at the top of `app()`. Subscribe
        // for theme-change animations.
        self.install_theme_transition_effect();
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
    /// [`schedule_layout_pass`] / `IOS_BACKEND_SELF`. The AAS iOS
    /// client uses this after each command batch: in AAS mode the
    /// `IosBackend` is moved into the `AasClient` by value, so
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
        let roots: Vec<native_layout::LayoutNode> = self
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
            let is_scroll_view = self.scroll_views.contains(key);
            let origin = if is_scroll_view {
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
    fn platform_safe_area_insets(&self) -> framework_core::EdgeInsets {
        let Some(host) = &self.host_root else {
            return framework_core::EdgeInsets::ZERO;
        };
        let insets: callbacks::UIEdgeInsets =
            unsafe { msg_send![&**host, safeAreaInsets] };
        framework_core::EdgeInsets {
            top: insets.top as f32,
            right: insets.right as f32,
            bottom: insets.bottom as f32,
            left: insets.left as f32,
        }
    }
}
