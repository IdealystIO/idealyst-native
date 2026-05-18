use block2::ConcreteBlock;
use framework_core::primitives::navigator::{
    DrawerHandle, DrawerNavigatorCallbacks, DrawerType, NavCommand, NavigatorControl,
    NavigatorHandle, TabNavigatorCallbacks, TabsHandle,
};
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGRect, MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::{UIColor, UIView, UIViewController};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::callbacks::CallbackTarget;
use super::navigator::IosNavigatorOps;
use super::style::animate;
use super::{pin_to_edges, IosNode};

#[cfg(feature = "debug-stats")]
fn dump_debug_stats(label: &str) {
    let events = framework_core::debug::take_events();
    let summary = framework_core::debug::component_summary(&events);
    let counters = framework_core::debug::take_phase_counters();
    super::ios_log(&format!("[profiler] {} — {} events", label, events.len()));
    for (name, s) in &summary {
        super::ios_log(&format!("[profiler]   {} — calls: {}, total: {}µs, max: {}µs",
            name, s.call_count, s.total_inclusive_us, s.max_inclusive_us));
    }
    for (phase, c) in &counters {
        super::ios_log(&format!("[profiler]   phase {} — calls: {}, total: {}µs, max: {}µs",
            phase, c.call_count, c.total_us, c.max_us));
    }
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
    let entry = TabDrawerEntry {
        outer: outer.clone(),
        content_host: outer.clone(),
        body: body.clone(),
        control: control.clone(),
        current_scope: RefCell::new(None),
        sidebar: Rc::new(RefCell::new(None)),
        is_open: Rc::new(std::cell::Cell::new(false)),
    };
    tab_drawer_instances.insert(key, entry);

    let mount = callbacks.navigator.mount_screen.clone();
    let release = callbacks.navigator.release_screen.clone();
    let depth_changed = callbacks.navigator.depth_changed.clone();
    let active_changed = callbacks.active_changed.clone();
    let body_for_dispatch = body.clone();

    let current_scope: Rc<RefCell<Option<u64>>> = Rc::new(RefCell::new(None));
    let cs_for_dispatch = current_scope.clone();

    control.install(Box::new(move |cmd| {
        match cmd {
            NavCommand::Select { name, params, url: _ } => {
                if let Some(old_scope) = cs_for_dispatch.borrow_mut().take() {
                    release(old_scope);
                }
                let subviews = body_for_dispatch.subviews();
                for sub in subviews.iter() {
                    unsafe { sub.removeFromSuperview() };
                }
                let result = mount(name, params);
                pin_to_edges(&body_for_dispatch, result.node.as_view());
                *cs_for_dispatch.borrow_mut() = Some(result.scope_id);
                depth_changed(1);
                active_changed(name);
            }
            _ => {}
        }
    }));

    IosNode::View(outer)
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
    pin_to_edges(&entry.body, screen.as_view());
    *entry.current_scope.borrow_mut() = Some(scope_id);
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
    pin_to_edges(&outer, &body);

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
    let entry = TabDrawerEntry {
        outer: outer.clone(),
        content_host: outer.clone(),
        body: body.clone(),
        control: control.clone(),
        current_scope: RefCell::new(None),
        sidebar: sidebar_cell.clone(),
        is_open: is_open.clone(),
    };
    tab_drawer_instances.insert(key, entry);

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
    let scrim_ref = scrim.clone();
    let sidebar_for_anim = sidebar_cell.clone();
    let body_for_anim = body.clone();

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
                        unsafe { scrim_ref.removeFromSuperview() };
                        pin_to_edges(nav_view, &scrim_ref);
                        unsafe { sb.removeFromSuperview() };
                        let _: () = unsafe {
                            msg_send![sb.as_ref(), setTranslatesAutoresizingMaskIntoConstraints: false]
                        };
                        unsafe { nav_view.addSubview(sb) };
                        let p_top: Retained<NSObject> = unsafe { msg_send_id![nav_view, topAnchor] };
                        let p_bot: Retained<NSObject> = unsafe { msg_send_id![nav_view, bottomAnchor] };
                        let p_lead: Retained<NSObject> = unsafe { msg_send_id![nav_view, leadingAnchor] };
                        let s_top: Retained<NSObject> = unsafe { msg_send_id![sb.as_ref(), topAnchor] };
                        let s_bot: Retained<NSObject> = unsafe { msg_send_id![sb.as_ref(), bottomAnchor] };
                        let s_lead: Retained<NSObject> = unsafe { msg_send_id![sb.as_ref(), leadingAnchor] };
                        for (a, b) in [(&s_top, &p_top), (&s_bot, &p_bot), (&s_lead, &p_lead)] {
                            let c: Retained<NSObject> = unsafe {
                                msg_send_id![a, constraintEqualToAnchor: &**b]
                            };
                            let _: () = unsafe { msg_send![&c, setActive: true] };
                        }
                        reparented.set(true);
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

        let trans = framework_core::Transition::new(200, framework_core::Easing::EaseOut);
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
                super::ios_log(&format!("[drawer] Select: {}", name));
                if let Some(old_scope) = cs_for_dispatch.borrow_mut().take() {
                    release(old_scope);
                }
                let subviews = body_for_dispatch.subviews();
                for sub in subviews.iter() {
                    unsafe { sub.removeFromSuperview() };
                }
                let result = mount(name, params);
                pin_to_edges(&body_for_dispatch, result.node.as_view());
                *cs_for_dispatch.borrow_mut() = Some(result.scope_id);
                depth_changed(1);
                active_changed(name);
                super::ios_log(&format!("[drawer] Select done: {}", name));
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
    pin_to_edges(&entry.body, screen.as_view());
    *entry.current_scope.borrow_mut() = Some(scope_id);

    // Defer header setup until the next run loop pass — by then
    // the stack navigator will have wrapped the drawer's view in a
    // VC with a navigationItem we can configure via the options.
    let outer_ref = entry.outer.clone();
    let setup: Rc<dyn Fn()> = Rc::new(move || {
        // Walk up the responder chain to find the parent VC
        let mut resp: *const NSObject = &*outer_ref as *const UIView as *const NSObject;
        loop {
            let next: *const NSObject = unsafe { msg_send![resp, nextResponder] };
            if next.is_null() { break; }
            let is_vc: bool = unsafe {
                msg_send![next, isKindOfClass: objc2::class!(UIViewController)]
            };
            if is_vc {
                let vc: &UIViewController = unsafe { &*(next as *const UIViewController) };
                // Ensure the nav bar is visible — the stack navigator
                // may have hidden it if the Home screen set
                // header_shown(false). The drawer's options override.
                let nav_ctrl: *const NSObject = unsafe { msg_send![vc, navigationController] };
                if !nav_ctrl.is_null() {
                    let _: () = unsafe { msg_send![nav_ctrl, setNavigationBarHidden: false, animated: false] };
                }
                for target in super::apply_header_options(
                    vc,
                    &options,
                    unsafe { MainThreadMarker::new_unchecked() },
                ) {
                    std::mem::forget(target);
                }
                break;
            }
            resp = next;
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
    let _: () = unsafe {
        msg_send![sidebar_view, setTranslatesAutoresizingMaskIntoConstraints: false]
    };
    unsafe { entry.content_host.addSubview(sidebar_view) };

    let o_top: Retained<NSObject> = unsafe { msg_send_id![&entry.content_host, topAnchor] };
    let o_bot: Retained<NSObject> = unsafe { msg_send_id![&entry.content_host, bottomAnchor] };
    let o_lead: Retained<NSObject> = unsafe { msg_send_id![&entry.content_host, leadingAnchor] };
    let s_top: Retained<NSObject> = unsafe { msg_send_id![sidebar_view, topAnchor] };
    let s_bot: Retained<NSObject> = unsafe { msg_send_id![sidebar_view, bottomAnchor] };
    let s_lead: Retained<NSObject> = unsafe { msg_send_id![sidebar_view, leadingAnchor] };
    for (a, b) in [(&s_top, &o_top), (&s_bot, &o_bot), (&s_lead, &o_lead)] {
        let c: Retained<NSObject> = unsafe { msg_send_id![a, constraintEqualToAnchor: &**b] };
        let _: () = unsafe { msg_send![&c, setActive: true] };
    }

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
