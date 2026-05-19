use block2::ConcreteBlock;
use framework_core::primitives::navigator::{
    DrawerHandle, DrawerNavigatorCallbacks, DrawerType, MountPolicy, NavCommand, NavigatorControl,
    NavigatorHandle, TabNavigatorCallbacks, TabsHandle,
};
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGRect, MainThreadMarker, NSArray, NSObject, NSString};
use objc2_ui_kit::{UIColor, UINavigationController, UIView, UIViewController};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::callbacks::CallbackTarget;
use super::navigator::IosNavigatorOps;
use backend_ios_core::style::{animate, color_to_uicolor};
use super::{pin_to_edges, IosNode};

#[cfg(feature = "debug-stats")]
fn dump_debug_stats(label: &str) {
    let events = framework_core::debug::take_events();
    let summary = framework_core::debug::component_summary(&events);
    let counters = framework_core::debug::take_phase_counters();
    backend_ios_core::ios_log(&format!("[profiler] {} — {} events", label, events.len()));
    for (name, s) in &summary {
        backend_ios_core::ios_log(&format!("[profiler]   {} — calls: {}, total: {}µs, max: {}µs",
            name, s.call_count, s.total_inclusive_us, s.max_inclusive_us));
    }
    for (phase, c) in &counters {
        backend_ios_core::ios_log(&format!("[profiler]   phase {} — calls: {}, total: {}µs, max: {}µs",
            phase, c.call_count, c.total_us, c.max_us));
    }
}

/// A screen retained across switches under a persistent
/// `MountPolicy`. The view stays in the body's subview list and is
/// toggled visible via `setHidden:` rather than torn down. The
/// cached `ScreenOptions` lets re-Selects re-apply the header
/// configuration (title, bar buttons) without re-mounting.
pub(crate) struct MountedScreen {
    pub(crate) view: Retained<UIView>,
    pub(crate) scope_id: u64,
    pub(crate) options: framework_core::ScreenOptions,
}

/// Per-instance state for tab and drawer navigators.
pub(crate) struct TabDrawerEntry {
    pub(crate) outer: Retained<UIView>,
    pub(crate) content_host: Retained<UIView>,
    pub(crate) body: Retained<UIView>,
    pub(crate) control: Rc<NavigatorControl>,
    pub(crate) current_scope: RefCell<Option<u64>>,
    pub(crate) sidebar: Rc<RefCell<Option<Retained<UIView>>>>,
    pub(crate) is_open: Rc<std::cell::Cell<bool>>,
    pub(crate) mount_policy: MountPolicy,
    /// Mounted screens keyed by route name. Populated lazily by
    /// `Select` (and by `*_attach_initial` for the boot route);
    /// drained only on full navigator teardown when the policy is
    /// persistent. Empty when the policy is `LazyDisposing`.
    pub(crate) mounted: Rc<RefCell<HashMap<&'static str, MountedScreen>>>,
    /// Name of the currently-visible route. Used to find which
    /// cached view to hide on the next `Select`.
    pub(crate) current_route: Rc<RefCell<Option<&'static str>>>,
    /// Mirror of the nav state's active-route signal, so
    /// `*_attach_initial` (which receives no route name) can ask the
    /// framework what the initial route is.
    pub(crate) active_route_sig: framework_core::Signal<&'static str>,
    /// Embedded `UINavigationController`'s root `UIViewController`.
    /// Populated for drawer navigators (so they own a native header
    /// bar without depending on a parent stack) and left `None` for
    /// tab navigators (which have no header). `apply_header_options`
    /// targets this VC when present — its `navigationItem` populates
    /// the drawer's own `UINavigationBar`.
    pub(crate) header_root_vc: Option<Retained<UIViewController>>,
    /// The embedded `UINavigationController` itself, kept alongside
    /// `header_root_vc`. We pass this into `apply_header_options`
    /// directly because `rootVc.navigationController` returns nil
    /// for our setup even after `setViewControllers:` (UIKit only
    /// wires the responder-chain link once the nav controller is
    /// added as a child of an outer VC, which we don't have — the
    /// drawer's outer view is parented straight onto the host view).
    pub(crate) header_nav_ctrl: Option<Retained<NSObject>>,
    /// Effect that re-fires `apply_header_options` on the active
    /// screen whenever the global `active_theme()` signal changes.
    /// Kept alive on the entry so the subscription survives for as
    /// long as the drawer exists. `None` until installed in
    /// `create_drawer_navigator`; never set for tab navigators
    /// (they have no header bar to re-tint).
    pub(crate) theme_effect: Option<framework_core::Effect>,
    /// Effect that re-applies the drawer's body background color
    /// when the theme swaps. Same lifetime as `theme_effect`. `None`
    /// when the author didn't pass `.background_color(...)`.
    pub(crate) background_effect: Option<framework_core::Effect>,
}

// =========================================================================
// Tab Navigator
// =========================================================================

pub(crate) fn create_tab_navigator(
    mtm: MainThreadMarker,
    tab_drawer_instances: &mut HashMap<usize, TabDrawerEntry>,
    callbacks: TabNavigatorCallbacks<IosNode>,
    control: Rc<NavigatorControl>,
) -> IosNode {
    let body = unsafe { UIView::new(mtm) };
    let outer = body.clone();

    let key = &*outer as *const UIView as usize;
    let mount_policy = callbacks.mount_policy;
    let active_route_sig = callbacks.navigator.nav_state.active_route;
    let mounted: Rc<RefCell<HashMap<&'static str, MountedScreen>>> = Rc::new(RefCell::new(HashMap::new()));
    let current_route: Rc<RefCell<Option<&'static str>>> = Rc::new(RefCell::new(None));
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
        active_route_sig,
        // Tabs have no header bar; the drawer constructor populates
        // this with its embedded UINavigationController's rootVC.
        header_root_vc: None,
        header_nav_ctrl: None,
        theme_effect: None,
        background_effect: None,
    };
    tab_drawer_instances.insert(key, entry);

    let mount = callbacks.navigator.mount_screen.clone();
    let release = callbacks.navigator.release_screen.clone();
    let depth_changed = callbacks.navigator.depth_changed.clone();
    let active_changed = callbacks.active_changed.clone();
    let body_for_dispatch = body.clone();

    let current_scope: Rc<RefCell<Option<u64>>> = Rc::new(RefCell::new(None));
    let cs_for_dispatch = current_scope.clone();
    let mounted_for_dispatch = mounted.clone();
    let current_route_for_dispatch = current_route.clone();

    control.install(Box::new(move |cmd| {
        match cmd {
            NavCommand::Select { name, params, url: _ } => {
                select_screen(
                    mount_policy,
                    &body_for_dispatch,
                    &mounted_for_dispatch,
                    &current_route_for_dispatch,
                    &cs_for_dispatch,
                    &mount,
                    &release,
                    name,
                    params,
                );
                depth_changed(1);
                active_changed(name);
                super::schedule_layout_pass();
            }
            _ => {}
        }
    }));

    IosNode::View(outer)
}

/// Shared screen-switch logic for tab and drawer navigators. Honors
/// `MountPolicy`:
///
/// - `LazyDisposing`: tear down the previous screen entirely (release
///   scope + remove view), then mount the new one fresh.
/// - `LazyPersistent` / `EagerPersistent`: keep the previous screen
///   in the subview tree but hide it; mount the new screen on first
///   visit and cache it; subsequent visits just unhide.
fn select_screen(
    policy: MountPolicy,
    body: &Retained<UIView>,
    mounted: &Rc<RefCell<HashMap<&'static str, MountedScreen>>>,
    current_route: &Rc<RefCell<Option<&'static str>>>,
    current_scope: &Rc<RefCell<Option<u64>>>,
    mount_fn: &Rc<dyn Fn(&'static str, Box<dyn std::any::Any>) -> framework_core::primitives::navigator::MountResult<IosNode>>,
    release_fn: &Rc<dyn Fn(u64)>,
    name: &'static str,
    params: Box<dyn std::any::Any>,
) -> framework_core::ScreenOptions {
    match policy {
        MountPolicy::LazyDisposing => {
            if let Some(old_scope) = current_scope.borrow_mut().take() {
                release_fn(old_scope);
            }
            for sub in body.subviews().iter() {
                unsafe { sub.removeFromSuperview() };
            }
            let result = mount_fn(name, params);
            pin_to_edges(body, result.node.as_view());
            *current_scope.borrow_mut() = Some(result.scope_id);
            *current_route.borrow_mut() = Some(name);
            result.options
        }
        MountPolicy::LazyPersistent | MountPolicy::EagerPersistent => {
            // Hide the previous screen, if any. We deliberately do
            // NOT release its scope — the framework's contract is
            // that persistent screens keep their reactive state
            // (signals, effects) alive across switches.
            if let Some(prev) = *current_route.borrow() {
                if let Some(m) = mounted.borrow().get(prev) {
                    let _: () = unsafe { msg_send![m.view.as_ref(), setHidden: true] };
                }
            }
            // Reveal the cached view, or build it on first visit.
            let mut map = mounted.borrow_mut();
            let options = if let Some(m) = map.get(name) {
                let _: () = unsafe { msg_send![m.view.as_ref(), setHidden: false] };
                *current_scope.borrow_mut() = Some(m.scope_id);
                m.options.clone()
            } else {
                let result = mount_fn(name, params);
                let view: Retained<UIView> = unsafe {
                    Retained::retain(result.node.as_view() as *const UIView as *mut UIView).unwrap()
                };
                pin_to_edges(body, &view);
                *current_scope.borrow_mut() = Some(result.scope_id);
                let options = result.options.clone();
                map.insert(
                    name,
                    MountedScreen { view, scope_id: result.scope_id, options },
                );
                result.options
            };
            *current_route.borrow_mut() = Some(name);
            options
        }
    }
}

pub(crate) fn tab_navigator_attach_initial(
    tab_drawer_instances: &HashMap<usize, TabDrawerEntry>,
    navigator: &IosNode,
    screen: IosNode,
    scope_id: u64,
    _options: framework_core::ScreenOptions,
) {
    let key = navigator.view_key();
    let Some(entry) = tab_drawer_instances.get(&key) else {
        return;
    };
    let view: Retained<UIView> = unsafe {
        Retained::retain(screen.as_view() as *const UIView as *mut UIView).unwrap()
    };
    pin_to_edges(&entry.body, &view);
    *entry.current_scope.borrow_mut() = Some(scope_id);

    // Seed the persistent-mount cache with the initial screen so the
    // first `Select` away from it hides this one (instead of leaking
    // a duplicate). For `LazyDisposing` the cache stays empty — the
    // first `Select` removes this view and mounts a new one.
    if matches!(
        entry.mount_policy,
        MountPolicy::LazyPersistent | MountPolicy::EagerPersistent
    ) {
        let initial_name = entry.active_route_sig.get();
        *entry.current_route.borrow_mut() = Some(initial_name);
        entry.mounted.borrow_mut().insert(
            initial_name,
            MountedScreen {
                view,
                scope_id,
                // Tabs don't render a header; defaulted ScreenOptions
                // is fine here. (`_options` is ignored above.)
                options: framework_core::ScreenOptions::default(),
            },
        );
    }
}

pub(crate) fn release_tab_navigator(
    tab_drawer_instances: &mut HashMap<usize, TabDrawerEntry>,
    node: &IosNode,
) {
    let key = node.view_key();
    tab_drawer_instances.remove(&key);
}

pub(crate) fn make_tab_navigator_handle(
    tab_drawer_instances: &HashMap<usize, TabDrawerEntry>,
    node: &IosNode,
) -> TabsHandle {
    let key = node.view_key();
    let Some(entry) = tab_drawer_instances.get(&key) else {
        return TabsHandle::from_inner(
            NavigatorHandle::new(Rc::new(()), &IosNavigatorOps),
        );
    };
    TabsHandle::from_inner(
        NavigatorHandle::with_control(Rc::new(()), &IosNavigatorOps, entry.control.clone()),
    )
}

// =========================================================================
// Drawer Navigator
// =========================================================================

#[repr(C)]
#[derive(Clone, Copy)]
struct CGAffineTransform {
    a: CGFloat, b: CGFloat,
    c: CGFloat, d: CGFloat,
    tx: CGFloat, ty: CGFloat,
}
unsafe impl Encode for CGAffineTransform {
    const ENCODING: Encoding = Encoding::Struct(
        "CGAffineTransform",
        &[CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING,
          CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING],
    );
}
const IDENTITY: CGAffineTransform = CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };


pub(crate) fn create_drawer_navigator(
    mtm: MainThreadMarker,
    tab_drawer_instances: &mut HashMap<usize, TabDrawerEntry>,
    callbacks: DrawerNavigatorCallbacks<IosNode>,
    control: Rc<NavigatorControl>,
) -> IosNode {
    let outer = unsafe { UIView::new(mtm) };
    unsafe { outer.setClipsToBounds(true) };

    let body = unsafe { UIView::new(mtm) };

    // Embed the drawer's body inside a self-owned UINavigationController.
    // That way the drawer has a real native header bar regardless of
    // whether it's the root of the app or nested inside a parent
    // stack — the old code assumed a parent stack provided the nav
    // controller and the header silently disappeared when it didn't.
    //
    // `mount_screen_in_vc` wraps body in a UIVC whose root view pins
    // body to `safeAreaLayoutGuide`, so screen content sits below
    // the nav bar without taffy having to know about safe areas.
    let nav_ctrl = unsafe { UINavigationController::new(mtm) };
    let nav_view = nav_ctrl.view().expect("UINavigationController.view");
    let white = unsafe { UIColor::colorWithRed_green_blue_alpha(1.0, 1.0, 1.0, 1.0) };
    nav_view.setBackgroundColor(Some(&white));
    // Opaque appearance so content reliably sits below the bar — the
    // same configuration the stack navigator applies in navigator.rs.
    unsafe {
        let nav_bar: Retained<NSObject> = msg_send_id![&nav_ctrl, navigationBar];
        let appearance: Retained<NSObject> =
            msg_send_id![objc2::class!(UINavigationBarAppearance), new];
        let _: () = msg_send![&appearance, configureWithOpaqueBackground];
        let _: () = msg_send![&nav_bar, setStandardAppearance: &*appearance];
        let _: () = msg_send![&nav_bar, setScrollEdgeAppearance: &*appearance];
    }
    let root_vc = super::mount_screen_in_vc(mtm, &body);
    unsafe {
        nav_ctrl.setViewControllers_animated(
            &NSArray::from_vec(vec![root_vc.clone()]),
            false,
        );
    }
    // Outer holds the nav controller's view (header bar + body), then
    // the scrim and sidebar are added on top later. Order matters:
    // later subviews paint on top, so scrim/sidebar overlay the bar.
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
    let mounted: Rc<RefCell<HashMap<&'static str, MountedScreen>>> = Rc::new(RefCell::new(HashMap::new()));
    let current_route: Rc<RefCell<Option<&'static str>>> = Rc::new(RefCell::new(None));
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
        active_route_sig,
        header_root_vc: Some(root_vc.clone()),
        header_nav_ctrl: Some(unsafe {
            Retained::retain(Retained::as_ptr(&nav_ctrl) as *mut NSObject).unwrap()
        }),
        theme_effect: None,
        background_effect: None,
    };
    tab_drawer_instances.insert(key, entry);

    // Install the per-drawer theme-reactivity Effect. Subscribes to
    // the global `active_theme()` signal and re-runs
    // `apply_header_options` for whichever screen is currently
    // visible. Color fields in `ScreenOptions` are closures, so each
    // re-run resolves them against the new theme — the header bar's
    // background / title / tint re-tint in place without remounting
    // the screen or its primitive subtree. Stored on the entry so
    // the subscription's lifetime matches the drawer's.
    {
        let mounted_for_theme = mounted.clone();
        let current_route_for_theme = current_route.clone();
        let root_vc_for_theme = root_vc.clone();
        let nav_ctrl_for_theme: Retained<NSObject> = unsafe {
            Retained::retain(Retained::as_ptr(&nav_ctrl) as *mut NSObject).unwrap()
        };
        let theme_effect = framework_core::Effect::new(move || {
            // Subscribe to the active-theme signal so this effect
            // re-fires on every `set_theme(...)`. We want a single
            // "theme swapped" wake, not per-token wakes — the
            // closure body re-resolves *all* header colors below.
            // TODO: narrow subscription — the header reads a small
            // known subset of tokens (header_bg / header_title /
            // header_tint). Subscribing to just those via
            // `Tokenized::<Color>::resolve` would avoid waking on
            // unrelated token swaps, but the closures here read
            // through user-defined `ScreenOptions` so the names
            // aren't statically known from this site.
            let _ = framework_theme::active_theme();
            let route = current_route_for_theme.borrow().clone();
            let Some(route) = route else { return };
            let map = mounted_for_theme.borrow();
            let Some(m) = map.get(route) else { return };
            for target in super::apply_header_options_with_nav(
                &root_vc_for_theme,
                Some(&nav_ctrl_for_theme),
                &m.options,
                mtm,
            ) {
                std::mem::forget(target);
            }
        });
        if let Some(entry) = tab_drawer_instances.get_mut(&key) {
            entry.theme_effect = Some(theme_effect);
        }
    }

    // Background-color reactivity. Mirrors the header `theme_effect`
    // shape — re-fires on `active_theme()` change and re-tints the
    // nav controller's root view (which shows through any
    // transparent regions of the mounted screen, i.e. the area
    // between cards in the docs site). `None` ⇒ keep the hardcoded
    // white set above.
    if let Some(bg_closure) = callbacks.background_color.clone() {
        let nav_view_for_bg = nav_view.clone();
        let body_for_bg = body.clone();
        let bg_effect = framework_core::Effect::new(move || {
            // Subscribe to the active-theme signal — same shape as
            // the header `theme_effect` above. The `bg_closure` is a
            // user-supplied callback; we can't introspect which
            // token(s) it reads, so we wake on every theme swap.
            // TODO: narrow subscription — if the closure reads a
            // single `Tokenized<Color>::resolve()`, the natural
            // subscription would do this for us with finer grain.
            let _ = framework_theme::active_theme();
            let color = (bg_closure)();
            let ui_color = color_to_uicolor(&color);
            // Interpolate the body+nav background when a theme
            // transition is in flight; snap on initial mount. Same
            // 200ms ease-out as the rest of the iOS theme-fade
            // path (driven by `THEME_TRANSITION_ACTIVE` set by the
            // backend's per-host Effect).
            if backend_ios_core::style::THEME_TRANSITION_ACTIVE.with(|c| c.get()) {
                let trans = backend_ios_core::style::theme_transition_default();
                let nv = nav_view_for_bg.clone();
                let bd = body_for_bg.clone();
                let c = ui_color.clone();
                backend_ios_core::style::animate(&trans, Rc::new(move || {
                    nv.setBackgroundColor(Some(&c));
                    bd.setBackgroundColor(Some(&c));
                }));
            } else {
                nav_view_for_bg.setBackgroundColor(Some(&ui_color));
                // Also paint the rootVC's view (body) so the surface
                // behind any transparent gaps inside the screen also
                // re-tints — without this the body shows through as
                // the hardcoded white from `mount_screen_in_vc`.
                body_for_bg.setBackgroundColor(Some(&ui_color));
            }
        });
        if let Some(entry) = tab_drawer_instances.get_mut(&key) {
            entry.background_effect = Some(bg_effect);
        }
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
    let scrim_ref = scrim.clone();
    let sidebar_for_anim = sidebar_cell.clone();
    // Translate the nav controller's view (header bar + body
    // together) rather than just `body` — sliding the body alone
    // would leave the nav bar pinned and look wrong.
    let body_for_anim = nav_view.clone();
    let root_vc_for_dispatch = root_vc.clone();
    let nav_ctrl_for_dispatch: Retained<NSObject> = unsafe {
        Retained::retain(Retained::as_ptr(&nav_ctrl) as *mut NSObject).unwrap()
    };

    let drawer_style = callbacks.drawer_type;
    let configured_width = callbacks.drawer_width as CGFloat;
    let reparented = Rc::new(std::cell::Cell::new(false));
    let outer_for_reparent = outer.clone();

    let animate_drawer = move |open: bool| {
        let sidebar = sidebar_for_anim.borrow().clone();

        // Reparent scrim + sidebar to the nav controller's view on first open
        if !reparented.get() {
            if let Some(ref sb) = sidebar {
                let mut cur: *const UIView = &*outer_for_reparent;
                loop {
                    let parent: *const UIView = unsafe { msg_send![cur, superview] };
                    if parent.is_null() { break; }
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
                        // Move the scrim into the nav controller's view
                        // so it covers the entire navigator (including
                        // the nav bar). pin_to_edges puts it in Auto
                        // Layout mode — fine for the scrim because it's
                        // not in the framework's layout tree (no Taffy
                        // node) and just needs to fill.
                        unsafe { scrim_ref.removeFromSuperview() };
                        pin_to_edges(nav_view, &scrim_ref);
                        // Move the sidebar but KEEP IT FRAME-BASED.
                        // The earlier version put it in Auto Layout mode
                        // (top/bot/leading pins, no width) — without a
                        // width constraint the sidebar collapsed to 0pt
                        // wide and its background went invisible. Taffy
                        // already gives the sidebar its frame
                        // (320×852); the same bounds/center will be
                        // re-applied to it in its new parent on the
                        // next layout pass.
                        unsafe { sb.removeFromSuperview() };
                        unsafe { nav_view.addSubview(sb) };
                        reparented.set(true);
                        // Reapply frames after reparenting — UIView's
                        // frame is interpreted relative to its
                        // superview, and moving the sidebar to
                        // nav_view changes that coordinate space.
                        crate::imp::schedule_layout_pass();
                        break;
                    }
                    cur = parent;
                }
            }
        }

        let sidebar_width: CGFloat = sidebar.as_ref().map(|sb| {
            let frame: CGRect = unsafe { msg_send![sb.as_ref(), frame] };
            if frame.size.width > 0.0 { frame.size.width } else { configured_width }
        }).unwrap_or(configured_width);

        if open {
            let _: () = unsafe { msg_send![&scrim_ref, setUserInteractionEnabled: true] };
            if let Some(ref sb) = sidebar {
                let _: () = unsafe { msg_send![sb.as_ref(), setHidden: false] };
            }
        }

        let scrim_anim = scrim_ref.clone();
        let sidebar_anim = sidebar.clone();
        let body_anim = body_for_anim.clone();
        let style = drawer_style;

        let trans = framework_core::Transition::new(300, framework_core::Easing::EaseOut);
        animate(&trans, Rc::new(move || {
            let _: () = unsafe {
                msg_send![&scrim_anim, setAlpha: if open { 1.0 } else { 0.0 } as CGFloat]
            };

            match style {
                DrawerType::Slide => {
                    let body_t = if open {
                        CGAffineTransform { tx: sidebar_width, ..IDENTITY }
                    } else {
                        IDENTITY
                    };
                    let _: () = unsafe { msg_send![&body_anim, setTransform: body_t] };
                    if let Some(ref sb) = sidebar_anim {
                        let sb_t = if open {
                            IDENTITY
                        } else {
                            CGAffineTransform { tx: -sidebar_width, ..IDENTITY }
                        };
                        let _: () = unsafe { msg_send![sb.as_ref(), setTransform: sb_t] };
                    }
                }
                DrawerType::Front => {
                    if let Some(ref sb) = sidebar_anim {
                        let sb_t = if open {
                            IDENTITY
                        } else {
                            CGAffineTransform { tx: -sidebar_width, ..IDENTITY }
                        };
                        let _: () = unsafe { msg_send![sb.as_ref(), setTransform: sb_t] };
                    }
                }
            }
        }));

        if !open {
            let scrim_after = scrim_ref.clone();
            let sidebar_after = sidebar;
            let _: () = unsafe {
                msg_send![&scrim_after, setUserInteractionEnabled: false]
            };
            if let Some(ref sb) = sidebar_after {
                let sb_ref = unsafe { Retained::retain(sb.as_ref() as *const UIView as *mut UIView).unwrap() };
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

    control.install(Box::new(move |cmd| {
        match cmd {
            NavCommand::Select { name, params, url: _ } => {
                backend_ios_core::ios_log(&format!("[drawer] Select: {}", name));
                let options = select_screen(
                    mount_policy,
                    &body_for_dispatch,
                    &mounted_for_dispatch,
                    &current_route_for_dispatch,
                    &cs_for_dispatch,
                    &mount,
                    &release,
                    name,
                    params,
                );
                // Re-apply the new screen's header options to the
                // drawer's embedded root VC so the title and bar
                // buttons update when the user switches drawer entries.
                // Pass the nav controller explicitly — see the
                // `header_nav_ctrl` field comment on `TabDrawerEntry`
                // for why `rootVc.navigationController` is nil here.
                // `forget` is the standard pattern for these
                // callback target retains — the targets need to
                // outlive the VC for taps to keep firing.
                for target in super::apply_header_options_with_nav(
                    &root_vc_for_dispatch,
                    Some(&nav_ctrl_for_dispatch),
                    &options,
                    mtm,
                ) {
                    std::mem::forget(target);
                }
                depth_changed(1);
                active_changed(name);
                super::schedule_layout_pass();
                // Auto-close on navigation when the drawer is open
                // (standard mobile drawer behavior — the user has
                // navigated to a new screen, they don't want the
                // drawer covering it). Cheap when already closed:
                // the `is_open` check skips the animation entirely.
                if is_open_for_dispatch.get() {
                    is_open_for_dispatch.set(false);
                    is_open_signal.set(false);
                    close_fn(false);
                    open_changed(false);
                }
                backend_ios_core::ios_log(&format!("[drawer] Select done: {}", name));
            }
            NavCommand::OpenDrawer => {
                #[cfg(feature = "debug-stats")]
                framework_core::debug::clear_events();
                is_open_for_dispatch.set(true);
                is_open_signal.set(true);
                open_fn(true);
                open_changed(true);
                #[cfg(feature = "debug-stats")]
                dump_debug_stats("OpenDrawer");
            }
            NavCommand::CloseDrawer => {
                #[cfg(feature = "debug-stats")]
                framework_core::debug::clear_events();
                is_open_for_dispatch.set(false);
                is_open_signal.set(false);
                close_fn(false);
                open_changed(false);
                #[cfg(feature = "debug-stats")]
                dump_debug_stats("CloseDrawer");
            }
            NavCommand::ToggleDrawer => {
                #[cfg(feature = "debug-stats")]
                framework_core::debug::clear_events();
                let new_state = !is_open_for_dispatch.get();
                is_open_for_dispatch.set(new_state);
                is_open_signal.set(new_state);
                toggle_fn(new_state);
                open_changed(new_state);
                #[cfg(feature = "debug-stats")]
                dump_debug_stats(if new_state { "ToggleDrawer(open)" } else { "ToggleDrawer(close)" });
            }
            _ => {}
        }
    }));

    IosNode::View(outer)
}

pub(crate) fn drawer_navigator_attach_initial(
    mtm: MainThreadMarker,
    tab_drawer_instances: &HashMap<usize, TabDrawerEntry>,
    callback_targets: &mut Vec<Retained<NSObject>>,
    navigator: &IosNode,
    screen: IosNode,
    scope_id: u64,
    options: framework_core::ScreenOptions,
) {
    let key = navigator.view_key();
    let Some(entry) = tab_drawer_instances.get(&key) else {
        return;
    };
    let subviews = entry.body.subviews();
    for sub in subviews.iter() {
        unsafe { sub.removeFromSuperview() };
    }
    let view: Retained<UIView> = unsafe {
        Retained::retain(screen.as_view() as *const UIView as *mut UIView).unwrap()
    };
    pin_to_edges(&entry.body, &view);
    *entry.current_scope.borrow_mut() = Some(scope_id);

    // Seed the persistent-mount cache so subsequent `Select`s hide
    // this view instead of orphaning it. The cached options let
    // re-Select switches re-apply the header without remounting.
    let initial_options = options.clone();
    if matches!(
        entry.mount_policy,
        MountPolicy::LazyPersistent | MountPolicy::EagerPersistent
    ) {
        let initial_name = entry.active_route_sig.get();
        *entry.current_route.borrow_mut() = Some(initial_name);
        entry.mounted.borrow_mut().insert(
            initial_name,
            MountedScreen {
                view,
                scope_id,
                options: initial_options.clone(),
            },
        );
    }

    // The drawer owns its own UINavigationController + rootVC (set
    // up in `create_drawer_navigator`), so we apply the header
    // options directly to that nav controller — no responder-chain
    // walk, no dependence on a parent stack. Defer to the next run
    // loop pass so the rootVC has a chance to finish viewDidLoad.
    let header_root_vc = entry.header_root_vc.clone();
    let header_nav_ctrl = entry.header_nav_ctrl.clone();
    let setup: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(ref vc) = header_root_vc {
            for target in super::apply_header_options_with_nav(
                vc,
                header_nav_ctrl.as_ref(),
                &initial_options,
                unsafe { MainThreadMarker::new_unchecked() },
            ) {
                std::mem::forget(target);
            }
        }
    });
    let setup_target = CallbackTarget::new(mtm, setup);
    let setup_sel = objc2::sel!(invoke);
    let _: () = unsafe {
        msg_send![&setup_target, performSelector: setup_sel, withObject: std::ptr::null::<NSObject>(), afterDelay: 0.0 as CGFloat]
    };
    // Retain the target
    let obj: Retained<NSObject> = unsafe {
        let ptr = Retained::as_ptr(&setup_target) as *mut NSObject;
        Retained::retain(ptr).unwrap()
    };
    callback_targets.push(obj);
}

pub(crate) fn drawer_navigator_attach_sidebar(
    mtm: MainThreadMarker,
    tab_drawer_instances: &HashMap<usize, TabDrawerEntry>,
    callback_targets: &mut Vec<Retained<NSObject>>,
    navigator: &IosNode,
    sidebar: IosNode,
) {
    let key = navigator.view_key();
    let mut deferred_retain: Option<Retained<NSObject>> = None;
    let Some(entry) = tab_drawer_instances.get(&key) else {
        return;
    };
    let sidebar_view = sidebar.as_view();
    // Frame-based: leave `translatesAutoresizingMaskIntoConstraints`
    // YES (the default) so Taffy's `apply_frames` can write
    // `view.frame` directly. The sidebar is a Taffy root (it's
    // attached here, not through `Backend::insert`), so `compute()`
    // will give it a frame from its own style (explicit width=320
    // preserved, height filled to viewport on the `Auto` axis).
    unsafe { entry.content_host.addSubview(sidebar_view) };

    // Start off-screen and hidden
    let t = CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: -400.0, ty: 0.0 };
    let _: () = unsafe { msg_send![sidebar_view, setTransform: t] };
    let _: () = unsafe { msg_send![sidebar_view, setHidden: true] };

    // Tap-to-dismiss on scrim
    let ch_subviews = entry.content_host.subviews();
    if ch_subviews.len() >= 2 {
        let scrim = &ch_subviews[1];
        let dismiss_control = entry.control.clone();
        let tap_callback: Rc<dyn Fn()> = Rc::new(move || {
            dismiss_control.dispatch(NavCommand::CloseDrawer);
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
        deferred_retain = Some(unsafe {
            let ptr = Retained::as_ptr(&tap_target) as *mut NSObject;
            Retained::retain(ptr).unwrap()
        });
    }

    if let Some(obj) = deferred_retain {
        callback_targets.push(obj);
    }

    let sidebar_retained = unsafe {
        Retained::retain(sidebar_view as *const UIView as *mut UIView).unwrap()
    };
    *entry.sidebar.borrow_mut() = Some(sidebar_retained);
}

pub(crate) fn release_drawer_navigator(
    tab_drawer_instances: &mut HashMap<usize, TabDrawerEntry>,
    node: &IosNode,
) {
    let key = node.view_key();
    tab_drawer_instances.remove(&key);
}

pub(crate) fn make_drawer_navigator_handle(
    tab_drawer_instances: &HashMap<usize, TabDrawerEntry>,
    node: &IosNode,
) -> DrawerHandle {
    let key = node.view_key();
    let Some(entry) = tab_drawer_instances.get(&key) else {
        return DrawerHandle::from_inner(
            NavigatorHandle::new(Rc::new(()), &IosNavigatorOps),
            Rc::new(std::cell::Cell::new(false)),
        );
    };
    DrawerHandle::from_inner(
        NavigatorHandle::with_control(Rc::new(()), &IosNavigatorOps, entry.control.clone()),
        entry.is_open.clone(),
    )
}
