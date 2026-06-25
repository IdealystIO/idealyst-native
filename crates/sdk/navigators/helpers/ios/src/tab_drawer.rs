//! Tab + drawer navigator iOS engine.
//!
//! Moved verbatim from `backend-ios-mobile::imp::tab_drawer` after the
//! navigator-substrate refactor. The shape changed in two places:
//!   1. Per-instance state lives in this crate's thread-local
//!      `TAB_DRAWER_INSTANCES` registry instead of an `IosBackend` field.
//!   2. The drawer-specific `Open/Close/Toggle` commands now ride
//!      `NavCommand::Custom(Rc<dyn Any>)` carrying a
//!      [`crate::DrawerCmd`] payload; pre-refactor they were
//!      dedicated `NavCommand` variants in core.

use std::any::Any;

use crate::chrome::apply_header_options_with_nav;
use crate::{
    retain_target, DrawerCmd, IosDrawerCallbacks, IosScreenOptions, IosTabCallbacks,
    MountPolicy, TAB_DRAWER_INSTANCES,
};
use backend_ios::{
    pin_to_edges, schedule_layout_pass, with_backend, CallbackTarget, IosNode,
};
use backend_ios_core::style::{animate, color_to_uicolor};
use block2::ConcreteBlock;
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGRect, MainThreadMarker, NSArray, NSObject, NSString};
use objc2_ui_kit::{UIColor, UINavigationController, UIView, UIViewController};
use runtime_core::primitives::navigator::{
    MountResult, NavCommand, NavigatorControl,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A screen tracked across switches. For persistent policies the view
/// stays in the body's subview list and is toggled via `setHidden:`;
/// for `LazyDisposing` the view is removed and the scope released on
/// blur (this entry is dropped from the map at that point). The
/// cached [`IosScreenOptions`] lets re-Selects re-apply the header
/// configuration without re-mounting.
///
/// `effective_policy` is per-screen (the screen's own override if
/// declared via `DrawerScreenOptions::mount_policy`, else the
/// navigator-global default). Stored here so `select_screen` knows
/// what to do with the *previous* screen on the next transition
/// without re-deriving from incoming options each time.
pub(crate) struct MountedScreen {
    pub(crate) view: Retained<UIView>,
    pub(crate) scope_id: u64,
    pub(crate) options: IosScreenOptions,
    pub(crate) effective_policy: MountPolicy,
}

/// Per-instance state for tab and drawer navigators.
pub(crate) struct TabDrawerEntry {
    #[allow(dead_code)]
    pub(crate) outer: Retained<UIView>,
    pub(crate) content_host: Retained<UIView>,
    pub(crate) body: Retained<UIView>,
    pub(crate) control: Rc<NavigatorControl>,
    pub(crate) current_scope: RefCell<Option<u64>>,
    pub(crate) sidebar: Rc<RefCell<Option<Retained<UIView>>>>,
    #[allow(dead_code)]
    pub(crate) is_open: Rc<std::cell::Cell<bool>>,
    pub(crate) mount_policy: MountPolicy,
    pub(crate) mounted: Rc<RefCell<HashMap<&'static str, MountedScreen>>>,
    pub(crate) current_route: Rc<RefCell<Option<&'static str>>>,
    /// The on-screen view of whichever screen is currently active.
    /// Populated for every screen regardless of policy — for
    /// `LazyPersistent` it mirrors the cached `MountedScreen.view`;
    /// for `LazyDisposing` it's the only handle (the screen isn't in
    /// the cache because LazyDisposing screens get released and
    /// rebuilt). `select_screen` reads this to know which subview to
    /// hide vs `removeFromSuperview` on the outgoing transition.
    pub(crate) current_view: Rc<RefCell<Option<Retained<UIView>>>>,
    /// Effective mount policy of the currently active screen (the
    /// screen's own override if `IosScreenOptions::mount_policy` was
    /// set, else the navigator-global default). Used by
    /// `select_screen` to branch on the OUTGOING screen's policy
    /// rather than the navigator default — so a `LazyDisposing` home
    /// screen can drop its scope on blur even when the navigator
    /// global is `LazyPersistent`.
    pub(crate) current_effective_policy: Rc<RefCell<MountPolicy>>,
    pub(crate) active_route_sig: runtime_core::Signal<&'static str>,
    pub(crate) header_root_vc: Option<Retained<UIViewController>>,
    pub(crate) header_nav_ctrl: Option<Retained<NSObject>>,
    #[allow(dead_code)]
    pub(crate) theme_effect: Option<runtime_core::Subscription>,
    #[allow(dead_code)]
    pub(crate) background_effect: Option<runtime_core::Subscription>,
    #[allow(dead_code)]
    pub(crate) menu_callback_target: Option<Retained<NSObject>>,
    /// Header callback targets for the *currently visible* screen. UIKit
    /// holds them weakly via `setTarget:`, so the SDK must own them. Every
    /// header re-apply (screen swap, theme re-run) replaces the contents,
    /// dropping the prior screen's targets — so this holds at most one
    /// screen's worth, released on drawer teardown. Replaces the
    /// `mem::forget(target)` idiom, which leaked a fresh set on every swap.
    pub(crate) header_targets: Rc<RefCell<Vec<Retained<NSObject>>>>,
    /// Configured drawer width (from `DrawerBuilder::drawer_width`).
    /// `drawer_attach_sidebar` reads it to pin the sidebar UIView's
    /// width via Auto Layout — the sidebar's own Taffy node is
    /// orphaned (we addSubview directly instead of going through
    /// `Backend::insert`), so without this pin its frame stays 0×0
    /// and the open animation slides an invisible view.
    pub(crate) drawer_width: f32,
}

// =========================================================================
// Tab Navigator
// =========================================================================

pub(crate) fn create_tab(
    mtm: MainThreadMarker,
    callbacks: IosTabCallbacks,
    control: Rc<NavigatorControl>,
) -> IosNode {
    let body = unsafe { UIView::new(mtm) };
    let outer = body.clone();

    let key = &*outer as *const UIView as usize;
    let mount_policy = callbacks.mount_policy;
    let active_route_sig = callbacks.navigator.nav_state.active_route;
    let mounted: Rc<RefCell<HashMap<&'static str, MountedScreen>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let current_route: Rc<RefCell<Option<&'static str>>> = Rc::new(RefCell::new(None));
    let current_view: Rc<RefCell<Option<Retained<UIView>>>> = Rc::new(RefCell::new(None));
    let current_effective_policy: Rc<RefCell<MountPolicy>> =
        Rc::new(RefCell::new(mount_policy));
    let entry = TabDrawerEntry {
        outer: outer.clone(),
        content_host: outer.clone(),
        body: body.clone(),
        control: control.clone(),
        current_scope: RefCell::new(None),
        sidebar: Rc::new(RefCell::new(None)),
        is_open: Rc::new(std::cell::Cell::new(false)),
        mount_policy,
        mounted: mounted.clone(),
        current_route: current_route.clone(),
        current_view: current_view.clone(),
        current_effective_policy: current_effective_policy.clone(),
        active_route_sig,
        menu_callback_target: None,
        header_targets: Rc::new(RefCell::new(Vec::new())),
        header_root_vc: None,
        header_nav_ctrl: None,
        theme_effect: None,
        background_effect: None,
        // Tab navigators don't have a sidebar; placeholder width is
        // unused. Stored anyway so the struct stays uniform.
        drawer_width: 0.0,
    };
    TAB_DRAWER_INSTANCES.with(|m| {
        m.borrow_mut()
            .insert(key, Rc::new(RefCell::new(entry)));
    });

    let mount = callbacks.navigator.mount_screen.clone();
    let release = callbacks.navigator.release_screen.clone();
    let depth_changed = callbacks.navigator.depth_changed.clone();
    let active_changed = callbacks.active_changed.clone();
    let body_for_dispatch = body.clone();

    let current_scope: Rc<RefCell<Option<u64>>> = Rc::new(RefCell::new(None));
    let cs_for_dispatch = current_scope.clone();
    let mounted_for_dispatch = mounted.clone();
    let current_route_for_dispatch = current_route.clone();
    let current_view_for_dispatch = current_view.clone();
    let current_policy_for_dispatch = current_effective_policy.clone();

    control.install(Box::new(move |cmd| {
        if let NavCommand::Select { name, params, url: _, state: _ } = cmd {
            select_screen(
                mount_policy,
                &body_for_dispatch,
                &mounted_for_dispatch,
                &current_route_for_dispatch,
                &current_view_for_dispatch,
                &current_policy_for_dispatch,
                &cs_for_dispatch,
                &mount,
                &release,
                name,
                params,
            );
            depth_changed(1);
            active_changed(name);
            schedule_layout_pass();
        }
    }));

    IosNode::View(outer)
}

/// Shared screen-switch logic for tab and drawer navigators.
///
/// Honors per-screen `IosScreenOptions::mount_policy`: the OUTGOING
/// screen's effective policy decides whether its view is hidden or
/// fully released; the INCOMING screen's effective policy decides
/// whether it's added to the persistence cache.
///
/// Default behavior when no per-screen override is set matches the
/// navigator-global `navigator_default_policy`.
///
/// Why the per-screen branch matters: a graphics-heavy screen (the
/// website's home + embedded `Simulator`) wants `LazyDisposing` so
/// its wgpu host + descendant UIView tree get released when blurred
/// and rebuilt from scratch when re-focused. Other text-only screens
/// in the same navigator stay `LazyPersistent` so navigating back
/// to them is instant.
fn select_screen(
    navigator_default_policy: MountPolicy,
    body: &Retained<UIView>,
    mounted: &Rc<RefCell<HashMap<&'static str, MountedScreen>>>,
    current_route: &Rc<RefCell<Option<&'static str>>>,
    current_view: &Rc<RefCell<Option<Retained<UIView>>>>,
    current_effective_policy: &Rc<RefCell<MountPolicy>>,
    current_scope: &Rc<RefCell<Option<u64>>>,
    mount_fn: &Rc<dyn Fn(&'static str, Box<dyn std::any::Any>) -> MountResult<IosNode>>,
    release_fn: &Rc<dyn Fn(u64)>,
    name: &'static str,
    params: Box<dyn std::any::Any>,
) -> IosScreenOptions {
    // ---- Outgoing transition ----
    //
    // The outgoing screen's policy decides hide vs release. Only
    // touch the outgoing view — cached `LazyPersistent` screens
    // that aren't the current outgoing stay where they are (still
    // in `body.subviews()`, still hidden).
    let prev_route_name = current_route.borrow().clone();
    let prev_view_opt = current_view.borrow().clone();
    let prev_policy = *current_effective_policy.borrow();
    if let (Some(prev_name), Some(prev_view)) = (prev_route_name, prev_view_opt) {
        match prev_policy {
            MountPolicy::LazyDisposing => {
                // Drop the scope first so descendant cleanups
                // (the embedded wgpu host's `on_lost`, animator
                // tick handles, subscriptions) fire before the
                // UIView leaves the hierarchy. Releasing in the
                // other order is safe in practice — UIKit doesn't
                // require ordering — but firing cleanups first
                // keeps the lifecycle story easy to reason about.
                if let Some(old_scope) = current_scope.borrow_mut().take() {
                    release_fn(old_scope);
                }
                unsafe { prev_view.removeFromSuperview() };
                // `LazyDisposing` screens aren't in the persistence
                // cache by construction, but defensively remove in
                // case a prior policy change left a stale entry.
                mounted.borrow_mut().remove(prev_name);
            }
            MountPolicy::LazyPersistent | MountPolicy::EagerPersistent => {
                let _: () = unsafe { msg_send![prev_view.as_ref(), setHidden: true] };
            }
        }
    }

    // ---- Incoming: cache hit? ----
    //
    // `LazyPersistent` / `EagerPersistent` screens previously
    // visited stay in `mounted` with their view in `body.subviews()`
    // (hidden). Re-focus just unhides.
    let cached = mounted.borrow().get(name).cloned_for_select();
    if let Some(m) = cached {
        let _: () = unsafe { msg_send![m.view.as_ref(), setHidden: false] };
        *current_scope.borrow_mut() = Some(m.scope_id);
        *current_view.borrow_mut() = Some(m.view.clone());
        *current_effective_policy.borrow_mut() = m.effective_policy;
        *current_route.borrow_mut() = Some(name);
        return m.options;
    }

    // ---- Incoming: fresh mount ----
    let result = mount_fn(name, params);
    let view: Retained<UIView> = unsafe {
        Retained::retain(result.node.as_view() as *const UIView as *mut UIView).unwrap()
    };
    attach_screen(body, &view);
    backend_ios::with_backend(|b| b.run_layout());

    let options: IosScreenOptions = result
        .options
        .downcast_ref::<IosScreenOptions>()
        .cloned()
        .unwrap_or_default();
    let effective_policy = options.mount_policy.unwrap_or(navigator_default_policy);

    *current_scope.borrow_mut() = Some(result.scope_id);
    *current_view.borrow_mut() = Some(view.clone());
    *current_effective_policy.borrow_mut() = effective_policy;
    *current_route.borrow_mut() = Some(name);

    // Only cache Persistent screens. `LazyDisposing` ones live only
    // in `current_view` / `current_scope` until they blur.
    if matches!(
        effective_policy,
        MountPolicy::LazyPersistent | MountPolicy::EagerPersistent
    ) {
        mounted.borrow_mut().insert(
            name,
            MountedScreen {
                view,
                scope_id: result.scope_id,
                options: options.clone(),
                effective_policy,
            },
        );
    }

    options
}

/// Trivial helper trait so `Option<&MountedScreen>` can `.cloned()`
/// at a call site where the inner type doesn't implement `Clone`
/// for the whole struct via derive (it doesn't — `Retained<UIView>`
/// is `Clone` but the struct doesn't derive it because some prior
/// version held non-`Clone` fields). Keeps the call site readable
/// without sprinkling `.map(|m| MountedScreen { … })`.
trait CloneForSelect {
    fn cloned_for_select(&self) -> Option<MountedScreen>;
}

impl CloneForSelect for Option<&MountedScreen> {
    fn cloned_for_select(&self) -> Option<MountedScreen> {
        self.map(|m| MountedScreen {
            view: m.view.clone(),
            scope_id: m.scope_id,
            options: m.options.clone(),
            effective_policy: m.effective_policy,
        })
    }
}

pub(crate) fn tab_attach_initial(
    navigator: &IosNode,
    screen: IosNode,
    scope_id: u64,
) {
    let key = navigator.view_key();
    let entry = TAB_DRAWER_INSTANCES.with(|m| m.borrow().get(&key).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();
    let view: Retained<UIView> = unsafe {
        Retained::retain(screen.as_view() as *const UIView as *mut UIView).unwrap()
    };
    pin_to_edges(&entry.body, &view);
    *entry.current_scope.borrow_mut() = Some(scope_id);

    // Track current view + active route regardless of policy. The
    // outgoing-transition path in `select_screen` reads these to
    // decide hide vs release on the FIRST nav-away even when the
    // initial screen is `LazyDisposing` (and therefore not in the
    // persistence cache).
    let initial_name = entry.active_route_sig.get();
    *entry.current_route.borrow_mut() = Some(initial_name);
    *entry.current_view.borrow_mut() = Some(view.clone());
    // Tab navigators don't yet propagate per-screen
    // `IosScreenOptions` for the initial screen — the SDK call site
    // doesn't thread them through `tab_attach_initial`. Fall back to
    // the navigator-global policy; per-screen overrides still take
    // effect on subsequent navigations once `select_screen` reads
    // them from the mount result.
    *entry.current_effective_policy.borrow_mut() = entry.mount_policy;

    if matches!(
        entry.mount_policy,
        MountPolicy::LazyPersistent | MountPolicy::EagerPersistent
    ) {
        entry.mounted.borrow_mut().insert(
            initial_name,
            MountedScreen {
                view,
                scope_id,
                options: IosScreenOptions::default(),
                effective_policy: entry.mount_policy,
            },
        );
    }
}

// =========================================================================
// Drawer Navigator
// =========================================================================

#[repr(C)]
#[derive(Clone, Copy)]
struct CGAffineTransform {
    a: CGFloat,
    b: CGFloat,
    c: CGFloat,
    d: CGFloat,
    tx: CGFloat,
    ty: CGFloat,
}
unsafe impl Encode for CGAffineTransform {
    const ENCODING: Encoding = Encoding::Struct(
        "CGAffineTransform",
        &[
            CGFloat::ENCODING,
            CGFloat::ENCODING,
            CGFloat::ENCODING,
            CGFloat::ENCODING,
            CGFloat::ENCODING,
            CGFloat::ENCODING,
        ],
    );
}
/// Attach a navigator screen as a child of `body`, pinned edge-to-edge
/// via Auto Layout. The drawer's body is a plain UIView (same as the
/// tab navigator), so the screen fills it; the screen owns its own
/// scrolling through an inner `scroll_view` primitive. The navigator
/// never owns scroll.
fn attach_screen(body: &UIView, child: &UIView) {
    pin_to_edges(body, child);
}

const IDENTITY: CGAffineTransform = CGAffineTransform {
    a: 1.0,
    b: 0.0,
    c: 0.0,
    d: 1.0,
    tx: 0.0,
    ty: 0.0,
};

pub(crate) fn create_drawer(
    mtm: MainThreadMarker,
    callbacks: IosDrawerCallbacks,
    control: Rc<NavigatorControl>,
) -> IosNode {
    let outer = unsafe { UIView::new(mtm) };
    unsafe { outer.setClipsToBounds(true) };

    // The drawer's body is a plain UIView, same as the tab navigator.
    // The navigator never owns scroll — the screen pins edge-to-edge
    // and provides its own scrolling via an inner `scroll_view`
    // primitive when its content overflows.
    let body = unsafe { UIView::new(mtm) };

    let nav_ctrl = unsafe { UINavigationController::new(mtm) };
    let nav_view = nav_ctrl.view().expect("UINavigationController.view");
    let white = unsafe { UIColor::colorWithRed_green_blue_alpha(1.0, 1.0, 1.0, 1.0) };
    nav_view.setBackgroundColor(Some(&white));
    unsafe {
        let nav_bar: Retained<NSObject> = msg_send_id![&nav_ctrl, navigationBar];
        let appearance: Retained<NSObject> =
            msg_send_id![objc2::class!(UINavigationBarAppearance), new];
        let _: () = msg_send![&appearance, configureWithOpaqueBackground];
        let _: () = msg_send![&nav_bar, setStandardAppearance: &*appearance];
        let _: () = msg_send![&nav_bar, setScrollEdgeAppearance: &*appearance];
    }
    let root_vc = backend_ios::mount_screen_in_vc(mtm, &body);
    unsafe {
        nav_ctrl.setViewControllers_animated(
            &NSArray::from_vec(vec![root_vc.clone()]),
            false,
        );
    }
    pin_to_edges(&outer, &nav_view);

    // Scrim
    let scrim = unsafe { UIView::new(mtm) };
    let scrim_color = unsafe { UIColor::colorWithRed_green_blue_alpha(0.0, 0.0, 0.0, 0.4) };
    scrim.setBackgroundColor(Some(&scrim_color));
    let _: () = unsafe { msg_send![&scrim, setAlpha: 0.0 as CGFloat] };
    let _: () = unsafe { msg_send![&scrim, setUserInteractionEnabled: false] };
    pin_to_edges(&outer, &scrim);

    let key = &*outer as *const UIView as usize;
    let is_open = Rc::new(std::cell::Cell::new(false));
    let sidebar_cell: Rc<RefCell<Option<Retained<UIView>>>> = Rc::new(RefCell::new(None));
    let mount_policy = callbacks.mount_policy;
    let active_route_sig = callbacks.navigator.nav_state.active_route;
    let mounted: Rc<RefCell<HashMap<&'static str, MountedScreen>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let current_route: Rc<RefCell<Option<&'static str>>> = Rc::new(RefCell::new(None));
    let current_view: Rc<RefCell<Option<Retained<UIView>>>> = Rc::new(RefCell::new(None));
    let current_effective_policy: Rc<RefCell<MountPolicy>> =
        Rc::new(RefCell::new(mount_policy));

    // Install a leading hamburger button on `root_vc.navigationItem`.
    // Tapping dispatches `Custom(DrawerCmd::Open)` on the navigator's
    // control plane — same path `DrawerHandle::open()` uses.
    let menu_target_retain: Retained<NSObject> = unsafe {
        let image: Retained<NSObject> = {
            let name = NSString::from_str("line.3.horizontal");
            msg_send_id![objc2::class!(UIImage), systemImageNamed: &*name]
        };
        let control_for_menu = control.clone();
        let on_press: Rc<dyn Fn()> = Rc::new(move || {
            control_for_menu.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Open)));
        });
        let target = CallbackTarget::new(mtm, on_press);
        let sel = objc2::sel!(invoke);
        let bar_item: Retained<NSObject> =
            msg_send_id![objc2::class!(UIBarButtonItem), new];
        let _: () = msg_send![&bar_item, setImage: &*image];
        let _: () = msg_send![&bar_item, setTarget: &*target];
        let _: () = msg_send![&bar_item, setAction: sel];
        let nav_item: Retained<NSObject> = msg_send_id![&root_vc, navigationItem];
        let _: () = msg_send![&nav_item, setLeftBarButtonItem: &*bar_item];
        Retained::retain(Retained::as_ptr(&target) as *mut NSObject).unwrap()
    };

    // Owns the currently-visible screen's header targets; replaced on
    // every header re-apply below so old targets release on swap.
    let header_targets: Rc<RefCell<Vec<Retained<NSObject>>>> =
        Rc::new(RefCell::new(Vec::new()));
    let entry = TabDrawerEntry {
        outer: outer.clone(),
        content_host: outer.clone(),
        body: body.clone(),
        control: control.clone(),
        current_scope: RefCell::new(None),
        sidebar: sidebar_cell.clone(),
        is_open: is_open.clone(),
        mount_policy,
        mounted: mounted.clone(),
        current_route: current_route.clone(),
        current_view: current_view.clone(),
        current_effective_policy: current_effective_policy.clone(),
        active_route_sig,
        menu_callback_target: Some(menu_target_retain),
        header_targets: header_targets.clone(),
        header_root_vc: Some(root_vc.clone()),
        header_nav_ctrl: Some(unsafe {
            Retained::retain(Retained::as_ptr(&nav_ctrl) as *mut NSObject).unwrap()
        }),
        theme_effect: None,
        background_effect: None,
        drawer_width: callbacks.drawer_width,
    };
    let entry_rc = Rc::new(RefCell::new(entry));
    TAB_DRAWER_INSTANCES.with(|m| {
        m.borrow_mut().insert(key, entry_rc.clone());
    });

    // Install the per-drawer theme-reactivity watch. Caller-owned and
    // stored on the entry, so it's disposed when the entry drops. Re-runs
    // `apply_header_options` for whichever screen is currently visible so
    // token-resolving color closures re-tint on theme swap.
    {
        let mounted_for_theme = mounted.clone();
        let current_route_for_theme = current_route.clone();
        let root_vc_for_theme = root_vc.clone();
        let header_targets_for_theme = header_targets.clone();
        let nav_ctrl_for_theme: Retained<NSObject> = unsafe {
            Retained::retain(Retained::as_ptr(&nav_ctrl) as *mut NSObject).unwrap()
        };
        let theme_effect = runtime_core::watch(move || {
            let route = *current_route_for_theme.borrow();
            let Some(route) = route else { return };
            let map = mounted_for_theme.borrow();
            let Some(m) = map.get(route) else { return };
            // Re-tint the visible screen's header; the new targets replace
            // the previous set (those buttons are gone), releasing them.
            let targets = apply_header_options_with_nav(
                &root_vc_for_theme,
                Some(&nav_ctrl_for_theme),
                &m.options,
                mtm,
            );
            *header_targets_for_theme.borrow_mut() = targets;
        });
        entry_rc.borrow_mut().theme_effect = Some(theme_effect);
    }

    if let Some(bg_closure) = callbacks.background_color.clone() {
        let nav_view_for_bg = nav_view.clone();
        let body_for_bg = body.clone();
        let bg_effect = runtime_core::watch(move || {
            let color = (bg_closure)();
            let ui_color = color_to_uicolor(&color);
            nav_view_for_bg.setBackgroundColor(Some(&ui_color));
            body_for_bg.setBackgroundColor(Some(&ui_color));
        });
        entry_rc.borrow_mut().background_effect = Some(bg_effect);
    }

    let mount = callbacks.navigator.mount_screen.clone();
    let release = callbacks.navigator.release_screen.clone();
    let depth_changed = callbacks.navigator.depth_changed.clone();
    let active_changed = callbacks.active_changed.clone();
    let open_changed = callbacks.open_changed.clone();
    let is_open_signal = callbacks.is_open;
    let body_for_dispatch = body.clone();
    let current_scope: Rc<RefCell<Option<u64>>> = Rc::new(RefCell::new(None));
    let cs_for_dispatch = current_scope.clone();
    let is_open_for_dispatch = is_open.clone();
    let mounted_for_dispatch = mounted.clone();
    let current_route_for_dispatch = current_route.clone();
    let current_view_for_dispatch = current_view.clone();
    let current_policy_for_dispatch = current_effective_policy.clone();
    let scrim_ref = scrim.clone();
    let sidebar_for_anim = sidebar_cell.clone();
    let body_for_anim = nav_view.clone();
    let root_vc_for_dispatch = root_vc.clone();
    let header_targets_for_dispatch = header_targets.clone();
    let nav_ctrl_for_dispatch: Retained<NSObject> = unsafe {
        Retained::retain(Retained::as_ptr(&nav_ctrl) as *mut NSObject).unwrap()
    };

    let drawer_style = callbacks.drawer_type;
    let configured_width = callbacks.drawer_width as CGFloat;
    let reparented = Rc::new(std::cell::Cell::new(false));
    let outer_for_reparent = outer.clone();

    let animate_drawer = move |open: bool| {
        let sidebar = sidebar_for_anim.borrow().clone();

        if !reparented.get() {
            if let Some(ref sb) = sidebar {
                let mut cur: *const UIView = &*outer_for_reparent;
                loop {
                    let parent: *const UIView = unsafe { msg_send![cur, superview] };
                    if parent.is_null() {
                        break;
                    }
                    let subs: Retained<objc2_foundation::NSArray<UIView>> =
                        unsafe { msg_send_id![parent, subviews] };
                    let has_bar = subs.iter().any(|s| {
                        let is_bar: bool = unsafe {
                            msg_send![s, isKindOfClass: objc2::class!(UINavigationBar)]
                        };
                        is_bar
                    });
                    if has_bar {
                        let nav_view: &UIView = unsafe { &*parent };
                        unsafe { scrim_ref.removeFromSuperview() };
                        pin_to_edges(nav_view, &scrim_ref);
                        unsafe { sb.removeFromSuperview() };
                        unsafe { nav_view.addSubview(sb) };
                        reparented.set(true);
                        schedule_layout_pass();
                        break;
                    }
                    cur = parent;
                }
            }
        }

        let sidebar_width: CGFloat = sidebar
            .as_ref()
            .map(|sb| {
                let frame: CGRect = unsafe { msg_send![sb.as_ref(), frame] };
                if frame.size.width > 0.0 {
                    frame.size.width
                } else {
                    configured_width
                }
            })
            .unwrap_or(configured_width);

        if open {
            let _: () =
                unsafe { msg_send![&scrim_ref, setUserInteractionEnabled: true] };
            if let Some(ref sb) = sidebar {
                let _: () = unsafe { msg_send![sb.as_ref(), setHidden: false] };
            }
        }

        let scrim_anim = scrim_ref.clone();
        let sidebar_anim = sidebar.clone();
        let body_anim = body_for_anim.clone();
        let style = drawer_style;

        let trans = runtime_core::Transition::new(300, runtime_core::Easing::EaseOut);
        animate(
            &trans,
            Rc::new(move || {
                let _: () = unsafe {
                    msg_send![
                        &scrim_anim,
                        setAlpha: if open { 1.0 } else { 0.0 } as CGFloat
                    ]
                };

                match style {
                    crate::DrawerType::Slide => {
                        let body_t = if open {
                            CGAffineTransform {
                                tx: sidebar_width,
                                ..IDENTITY
                            }
                        } else {
                            IDENTITY
                        };
                        let _: () =
                            unsafe { msg_send![&body_anim, setTransform: body_t] };
                        if let Some(ref sb) = sidebar_anim {
                            let sb_t = if open {
                                IDENTITY
                            } else {
                                CGAffineTransform {
                                    tx: -sidebar_width,
                                    ..IDENTITY
                                }
                            };
                            let _: () =
                                unsafe { msg_send![sb.as_ref(), setTransform: sb_t] };
                        }
                    }
                    crate::DrawerType::Front => {
                        if let Some(ref sb) = sidebar_anim {
                            let sb_t = if open {
                                IDENTITY
                            } else {
                                CGAffineTransform {
                                    tx: -sidebar_width,
                                    ..IDENTITY
                                }
                            };
                            let _: () =
                                unsafe { msg_send![sb.as_ref(), setTransform: sb_t] };
                        }
                    }
                }
            }),
        );

        if !open {
            let scrim_after = scrim_ref.clone();
            let sidebar_after = sidebar;
            let _: () =
                unsafe { msg_send![&scrim_after, setUserInteractionEnabled: false] };
            if let Some(ref sb) = sidebar_after {
                let sb_ref = unsafe {
                    Retained::retain(sb.as_ref() as *const UIView as *mut UIView).unwrap()
                };
                let timer_block = ConcreteBlock::new(move |_timer: *const NSObject| {
                    let _: () = unsafe { msg_send![&sb_ref, setHidden: true] };
                });
                let timer_block = timer_block.copy();
                let _: Retained<NSObject> = unsafe {
                    msg_send_id![
                        objc2::class!(NSTimer),
                        scheduledTimerWithTimeInterval: 0.4 as f64,
                        repeats: false,
                        block: &*timer_block
                    ]
                };
            }
        }
    };

    let open_fn = animate_drawer.clone();
    let close_fn = animate_drawer.clone();
    let toggle_fn = animate_drawer;

    // Rewrite `Link` activation to `Select`. Without this, the default
    // `NavCommand::Push` falls through to the dispatcher's catch-all
    // and gets ignored (drawer/tab don't honor Push). Same install
    // happens in the tab navigator's helper.
    let select_activator: Rc<
        dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand,
    > = Rc::new(|name, url, params| NavCommand::Select {
        name,
        url,
        params,
        state: None,
    });
    control.install_link_activator(select_activator);

    control.install(Box::new(move |cmd| match cmd {
        NavCommand::Select { name, params, url: _, state: _ } => {
            let options = select_screen(
                mount_policy,
                &body_for_dispatch,
                &mounted_for_dispatch,
                &current_route_for_dispatch,
                &current_view_for_dispatch,
                &current_policy_for_dispatch,
                &cs_for_dispatch,
                &mount,
                &release,
                name,
                params,
            );
            // New screen's header targets replace the outgoing screen's.
            *header_targets_for_dispatch.borrow_mut() = apply_header_options_with_nav(
                &root_vc_for_dispatch,
                Some(&nav_ctrl_for_dispatch),
                &options,
                mtm,
            );
            depth_changed(1);
            active_changed(name);
            schedule_layout_pass();
            if is_open_for_dispatch.get() {
                is_open_for_dispatch.set(false);
                is_open_signal.set(false);
                close_fn(false);
                open_changed(false);
            }
        }
        NavCommand::Custom(payload) => {
            if let Ok(cmd) = payload.downcast::<DrawerCmd>() {
                match *cmd {
                    DrawerCmd::Open => {
                        is_open_for_dispatch.set(true);
                        is_open_signal.set(true);
                        open_fn(true);
                        open_changed(true);
                    }
                    DrawerCmd::Close => {
                        is_open_for_dispatch.set(false);
                        is_open_signal.set(false);
                        close_fn(false);
                        open_changed(false);
                    }
                    DrawerCmd::Toggle => {
                        let new_state = !is_open_for_dispatch.get();
                        is_open_for_dispatch.set(new_state);
                        is_open_signal.set(new_state);
                        toggle_fn(new_state);
                        open_changed(new_state);
                    }
                }
            }
        }
        NavCommand::Push { .. }
        | NavCommand::Pop
        | NavCommand::Replace { .. }
        | NavCommand::Reset { .. } => {}
    }));

    IosNode::View(outer)
}

pub(crate) fn drawer_attach_initial(
    mtm: MainThreadMarker,
    navigator: &IosNode,
    screen: IosNode,
    scope_id: u64,
    options: &IosScreenOptions,
) {
    let key = navigator.view_key();
    let entry = TAB_DRAWER_INSTANCES.with(|m| m.borrow().get(&key).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();

    let subviews = entry.body.subviews();
    for sub in subviews.iter() {
        unsafe { sub.removeFromSuperview() };
    }
    let view: Retained<UIView> = unsafe {
        Retained::retain(screen.as_view() as *const UIView as *mut UIView).unwrap()
    };
    attach_screen(&entry.body, &view);
    // `drawer_attach_initial` runs during navigator init, BEFORE the
    // framework installs the backend's global self-ref. So a direct
    // `with_backend(|b| b.run_layout())` here would return None
    // silently and Taffy wouldn't fire, leaving the pinned screen at
    // 0×0. Defer the layout pass via a microtask so it runs AFTER the
    // framework's initial render kicks the backend's global self into
    // place.
    runtime_core::schedule_microtask(move || {
        backend_ios::with_backend(|b| b.run_layout());
    });
    *entry.current_scope.borrow_mut() = Some(scope_id);

    // Track the active screen regardless of policy so the
    // outgoing-transition path in `select_screen` can read it on
    // the FIRST nav-away. The initial screen IS in the cache when
    // its effective policy is Persistent; otherwise the cache stays
    // empty and `current_view` is the only handle to it.
    let initial_name = entry.active_route_sig.get();
    let effective_policy = options.mount_policy.unwrap_or(entry.mount_policy);
    *entry.current_route.borrow_mut() = Some(initial_name);
    *entry.current_view.borrow_mut() = Some(view.clone());
    *entry.current_effective_policy.borrow_mut() = effective_policy;

    if matches!(
        effective_policy,
        MountPolicy::LazyPersistent | MountPolicy::EagerPersistent
    ) {
        entry.mounted.borrow_mut().insert(
            initial_name,
            MountedScreen {
                view,
                scope_id,
                options: options.clone(),
                effective_policy,
            },
        );
    }

    let header_root_vc = entry.header_root_vc.clone();
    let header_nav_ctrl = entry.header_nav_ctrl.clone();
    let header_targets_for_setup = entry.header_targets.clone();
    let initial_options = options.clone();
    let setup: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(ref vc) = header_root_vc {
            // Initial header for the first screen; owned by the entry so it
            // releases on the next swap / drawer teardown.
            *header_targets_for_setup.borrow_mut() = apply_header_options_with_nav(
                vc,
                header_nav_ctrl.as_ref(),
                &initial_options,
                unsafe { MainThreadMarker::new_unchecked() },
            );
        }
    });
    let setup_target = CallbackTarget::new(mtm, setup);
    let setup_sel = objc2::sel!(invoke);
    let _: () = unsafe {
        msg_send![
            &setup_target,
            performSelector: setup_sel,
            withObject: std::ptr::null::<NSObject>(),
            afterDelay: 0.0 as CGFloat
        ]
    };
    let obj: Retained<NSObject> = unsafe {
        let ptr = Retained::as_ptr(&setup_target) as *mut NSObject;
        Retained::retain(ptr).unwrap()
    };
    retain_target(obj);
}

pub(crate) fn drawer_attach_sidebar(
    mtm: MainThreadMarker,
    navigator: &IosNode,
    sidebar: IosNode,
) {
    let key = navigator.view_key();
    let entry = TAB_DRAWER_INSTANCES.with(|m| m.borrow().get(&key).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();
    let sidebar_view = sidebar.as_view();

    // Pin the sidebar's geometry with Auto Layout, not Taffy. The
    // sidebar's Taffy node has no parent (we attach directly via
    // `addSubview` instead of going through `Backend::insert`, which
    // would `add_child` it into the host's Taffy tree). Without an
    // explicit width constraint the UIView frame stays 0×0 — the
    // open animation then transforms a zero-width view from
    // offscreen to onscreen, so the scrim darkens but no sidebar
    // appears. Pinning top/leading/bottom + a width constraint
    // matches what `pin_to_edges` does for screens, just with the
    // trailing edge replaced by a fixed width.
    let _: () = unsafe {
        msg_send![sidebar_view, setTranslatesAutoresizingMaskIntoConstraints: false]
    };
    unsafe { entry.content_host.addSubview(sidebar_view) };
    unsafe {
        let p_top: Retained<NSObject> =
            msg_send_id![&entry.content_host, topAnchor];
        let p_bot: Retained<NSObject> =
            msg_send_id![&entry.content_host, bottomAnchor];
        let p_lead: Retained<NSObject> =
            msg_send_id![&entry.content_host, leadingAnchor];
        let c_top: Retained<NSObject> = msg_send_id![sidebar_view, topAnchor];
        let c_bot: Retained<NSObject> = msg_send_id![sidebar_view, bottomAnchor];
        let c_lead: Retained<NSObject> = msg_send_id![sidebar_view, leadingAnchor];
        let c_width: Retained<NSObject> = msg_send_id![sidebar_view, widthAnchor];
        for (a, b) in [(&c_top, &p_top), (&c_bot, &p_bot), (&c_lead, &p_lead)] {
            let c: Retained<NSObject> = msg_send_id![a, constraintEqualToAnchor: &**b];
            let _: () = msg_send![&c, setActive: true];
        }
        let w_const: CGFloat = entry.drawer_width as CGFloat;
        let cw: Retained<NSObject> =
            msg_send_id![&c_width, constraintEqualToConstant: w_const];
        let _: () = msg_send![&cw, setActive: true];
    }

    // Start offscreen-left (configured width — not a magic 400) so
    // the open animation translates to identity over the same
    // distance the configured drawer occupies.
    let t = CGAffineTransform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: -(entry.drawer_width as CGFloat),
        ty: 0.0,
    };
    let _: () = unsafe { msg_send![sidebar_view, setTransform: t] };
    let _: () = unsafe { msg_send![sidebar_view, setHidden: true] };

    let ch_subviews = entry.content_host.subviews();
    if ch_subviews.len() >= 2 {
        let scrim = &ch_subviews[1];
        let dismiss_control = entry.control.clone();
        let tap_callback: Rc<dyn Fn()> = Rc::new(move || {
            dismiss_control.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Close)));
        });
        let tap_target = CallbackTarget::new(mtm, tap_callback);
        let tap_sel = objc2::sel!(invoke);
        let tap_gr = unsafe {
            objc2_ui_kit::UITapGestureRecognizer::initWithTarget_action(
                mtm.alloc(),
                Some(&tap_target),
                Some(tap_sel),
            )
        };
        let _: () = unsafe { msg_send![scrim, addGestureRecognizer: &*tap_gr] };
        let obj: Retained<NSObject> = unsafe {
            let ptr = Retained::as_ptr(&tap_target) as *mut NSObject;
            Retained::retain(ptr).unwrap()
        };
        retain_target(obj);
    }

    let sidebar_retained = unsafe {
        Retained::retain(sidebar_view as *const UIView as *mut UIView).unwrap()
    };
    *entry.sidebar.borrow_mut() = Some(sidebar_retained);

    // Run a layout pass so the freshly-attached sidebar gets its
    // Taffy-computed frame applied. The dispatcher's earlier
    // `schedule_layout_pass` calls were for screen swaps; the
    // deferred-attach happens outside that path.
    let _ = with_backend(|b| b.run_layout());
}
