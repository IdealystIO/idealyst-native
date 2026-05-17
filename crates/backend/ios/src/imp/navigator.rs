use framework_core::primitives::navigator::{
    NavCommand, NavigatorCallbacks, NavigatorControl, NavigatorHandle, NavigatorOps,
};
use framework_core::StyleRules;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass,
};
use objc2_foundation::{CGFloat, MainThreadMarker};
use objc2_ui_kit::{
    UIColor, UINavigationController, UINavigationControllerDelegate, UIView,
    UIViewController,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::{apply_header_options, mount_screen_in_vc, IosNode};
use super::style::{color_to_uicolor, font_weight_to_uikit};

pub(crate) struct NavigatorEntry {
    pub(crate) controller: Retained<UINavigationController>,
    pub(crate) control: Rc<NavigatorControl>,
    pub(crate) stack: Rc<RefCell<Vec<ScreenEntry>>>,
    /// Keep the delegate alive for the navigator's lifetime —
    /// `setDelegate:` doesn't retain, so dropping this would leave a
    /// dangling pointer in UIKit and the interactive-pop observer
    /// would silently stop firing.
    #[allow(dead_code)]
    pub(crate) delegate: Retained<NavigatorDelegate>,
}

pub(crate) struct ScreenEntry {
    pub(crate) vc: Retained<UIViewController>,
    pub(crate) scope_id: u64,
}

pub(crate) struct IosNavigatorOps;
impl NavigatorOps for IosNavigatorOps {}

// ---------------------------------------------------------------------------
// UINavigationControllerDelegate — observe interactive pops
// ---------------------------------------------------------------------------
//
// UIKit pops view controllers in three ways that the dispatcher
// doesn't see: swipe-back (`interactivePopGestureRecognizer`), the
// system back-chevron in the navigation bar, and any external
// `popViewController` call. None of those route through our
// `NavCommand::Pop` branch, so the rust-side `stack` would diverge
// from the UI and `release_screen` (which in AAS mode ships a
// `ScreenReleased` event back over the wire) would never fire.
//
// Hooking the delegate's `didShow` callback is the simplest reliable
// signal: it fires after any transition, including cancelled swipes,
// so we can reconcile the rust stack against the controller's actual
// `viewControllers` count.

pub(crate) struct NavigatorDelegateIvars {
    /// Shared with the dispatch closure so we observe (and trim) the
    /// same stack the dispatcher pushes onto.
    stack: Rc<RefCell<Vec<ScreenEntry>>>,
    /// The navigator callbacks' `release_screen` — usually the AAS
    /// stub that emits `AppToDev::ScreenReleased`.
    release: Rc<dyn Fn(u64)>,
    /// The navigator callbacks' `depth_changed`.
    depth_changed: Rc<dyn Fn(usize)>,
}

declare_class!(
    pub(crate) struct NavigatorDelegate;

    unsafe impl ClassType for NavigatorDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystNavigatorDelegate";
    }

    impl DeclaredClass for NavigatorDelegate {
        type Ivars = NavigatorDelegateIvars;
    }

    unsafe impl NSObjectProtocol for NavigatorDelegate {}

    unsafe impl UINavigationControllerDelegate for NavigatorDelegate {
        #[method(navigationController:didShowViewController:animated:)]
        fn did_show(
            &self,
            nav: &UINavigationController,
            _vc: &UIViewController,
            _animated: bool,
        ) {
            // Trim the rust stack until it matches the controller's
            // actual depth. Stack navigators only pop from the top
            // and only one or two at a time in practice, so the
            // simple loop is fine. Releases fire in pop-order.
            let visible_depth =
                unsafe { nav.viewControllers().count() };
            let ivars = self.ivars();
            let rust_depth = ivars.stack.borrow().len();
            eprintln!(
                "[ios-nav-delegate] didShow: rust_stack={}, visible={}",
                rust_depth, visible_depth
            );
            let mut popped_scopes: Vec<u64> = Vec::new();
            {
                let mut stack = ivars.stack.borrow_mut();
                while stack.len() > visible_depth {
                    if let Some(entry) = stack.pop() {
                        popped_scopes.push(entry.scope_id);
                    } else {
                        break;
                    }
                }
            }
            for scope_id in popped_scopes {
                eprintln!("  -> releasing scope {}", scope_id);
                (ivars.release)(scope_id);
            }
            (ivars.depth_changed)(visible_depth);
        }
    }
);

impl NavigatorDelegate {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        stack: Rc<RefCell<Vec<ScreenEntry>>>,
        release: Rc<dyn Fn(u64)>,
        depth_changed: Rc<dyn Fn(usize)>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(NavigatorDelegateIvars {
            stack,
            release,
            depth_changed,
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

pub(crate) fn create_navigator(
    mtm: MainThreadMarker,
    navigator_instances: &mut HashMap<usize, NavigatorEntry>,
    callbacks: NavigatorCallbacks<IosNode>,
    control: Rc<NavigatorControl>,
) -> IosNode {
    let nav = unsafe { UINavigationController::new(mtm) };
    let nav_view = nav.view().expect("UINavigationController.view");
    // Set a fallback white background. The header_style slot's first
    // apply will immediately update this to the correct theme color.
    let white = unsafe { objc2_ui_kit::UIColor::colorWithRed_green_blue_alpha(1.0, 1.0, 1.0, 1.0) };
    nav_view.setBackgroundColor(Some(&white));

    unsafe {
        let nav_bar: Retained<objc2_foundation::NSObject> = msg_send_id![&nav, navigationBar];
        let appearance: Retained<objc2_foundation::NSObject> = msg_send_id![objc2::class!(UINavigationBarAppearance), new];
        let _: () = msg_send![&appearance, configureWithOpaqueBackground];
        let _: () = msg_send![&nav_bar, setStandardAppearance: &*appearance];
        let _: () = msg_send![&nav_bar, setScrollEdgeAppearance: &*appearance];
    }

    let stack_rc: Rc<RefCell<Vec<ScreenEntry>>> = Rc::new(RefCell::new(Vec::new()));

    let nav_for_dispatch = nav.clone();
    let mount_for_dispatch = callbacks.mount_screen.clone();
    let release_for_dispatch = callbacks.release_screen.clone();
    let depth_for_dispatch = callbacks.depth_changed.clone();
    let stack_ref = stack_rc.clone();

    // Delegate observes interactive pops (swipe-back, system
    // back-chevron). Reconciles `stack_rc` against the controller's
    // actual depth and fires `release_for_dispatch` for any vc UIKit
    // removed without our dispatcher knowing.
    let delegate = NavigatorDelegate::new(
        mtm,
        stack_rc.clone(),
        callbacks.release_screen.clone(),
        callbacks.depth_changed.clone(),
    );
    unsafe {
        let delegate_proto = ProtocolObject::from_ref(&*delegate);
        nav.setDelegate(Some(delegate_proto));
    }
    eprintln!("[ios-nav] create_navigator: delegate set on UINavigationController");

    let entry = NavigatorEntry {
        controller: nav.clone(),
        control: control.clone(),
        stack: stack_rc.clone(),
        delegate: delegate.clone(),
    };
    let key = &*nav_view as *const UIView as usize;
    navigator_instances.insert(key, entry);

    control.install(Box::new(move |cmd| {
        eprintln!("[ios-nav-dispatcher] cmd received");
        let mut stack = stack_ref.borrow_mut();
        eprintln!("  -> stack_len before = {}", stack.len());
        match cmd {
            NavCommand::Push { name, params, url: _ } => {
                eprintln!("  -> Push: calling mount_for_dispatch");
                let result = mount_for_dispatch(name, params);
                let vc = mount_screen_in_vc(mtm, result.node.as_view());
                let scope_id = result.scope_id;
                unsafe { nav_for_dispatch.pushViewController_animated(&vc, true) };
                // Apply header options (title, buttons, style)
                for target in apply_header_options(&vc, &result.options, mtm) {
                    std::mem::forget(target); // keep alive for VC lifetime
                }
                stack.push(ScreenEntry { vc, scope_id });
                eprintln!("  -> Push: stack_len after = {}", stack.len());
                depth_for_dispatch(stack.len());
                super::schedule_layout_pass();
            }
            NavCommand::Pop => {
                if stack.len() <= 1 {
                    return;
                }
                let _ = unsafe { nav_for_dispatch.popViewControllerAnimated(true) };
                if let Some(popped) = stack.pop() {
                    release_for_dispatch(popped.scope_id);
                }
                depth_for_dispatch(stack.len());
                super::schedule_layout_pass();
            }
            NavCommand::Replace { name, params, url: _ } => {
                let result = mount_for_dispatch(name, params);
                let vc = mount_screen_in_vc(mtm, result.node.as_view());
                let scope_id = result.scope_id;
                for target in apply_header_options(&vc, &result.options, mtm) {
                    std::mem::forget(target);
                }
                if let Some(old) = stack.pop() {
                    release_for_dispatch(old.scope_id);
                }
                stack.push(ScreenEntry { vc, scope_id });
                let vcs: Vec<Retained<UIViewController>> =
                    stack.iter().map(|e| e.vc.clone()).collect();
                unsafe {
                    nav_for_dispatch.setViewControllers_animated(
                        &objc2_foundation::NSArray::from_vec(vcs),
                        false,
                    );
                }
                depth_for_dispatch(stack.len());
                super::schedule_layout_pass();
            }
            NavCommand::Reset { name, params, url: _ } => {
                let result = mount_for_dispatch(name, params);
                let vc = mount_screen_in_vc(mtm, result.node.as_view());
                let scope_id = result.scope_id;
                for target in apply_header_options(&vc, &result.options, mtm) {
                    std::mem::forget(target);
                }
                while let Some(prev) = stack.pop() {
                    release_for_dispatch(prev.scope_id);
                }
                stack.push(ScreenEntry { vc: vc.clone(), scope_id });
                unsafe {
                    nav_for_dispatch.setViewControllers_animated(
                        &objc2_foundation::NSArray::from_vec(vec![vc]),
                        false,
                    );
                }
                depth_for_dispatch(stack.len());
                super::schedule_layout_pass();
            }
            NavCommand::Select { .. }
            | NavCommand::OpenDrawer
            | NavCommand::CloseDrawer
            | NavCommand::ToggleDrawer => {
                panic!(
                    "stack Navigator received a non-stack NavCommand -- \
                     check that the dispatched command's shape matches \
                     the navigator kind (stack: Push/Pop/Replace/Reset)"
                );
            }
        }
    }));

    IosNode::View(nav_view)
}

pub(crate) fn navigator_attach_initial(
    mtm: MainThreadMarker,
    navigator_instances: &HashMap<usize, NavigatorEntry>,
    navigator: &IosNode,
    screen: IosNode,
    scope_id: u64,
    options: framework_core::ScreenOptions,
) {
    let key = navigator.view_key();
    let Some(entry) = navigator_instances.get(&key) else {
        return;
    };
    let root_vc = mount_screen_in_vc(mtm, screen.as_view());
    unsafe {
        entry.controller.setViewControllers_animated(
            &objc2_foundation::NSArray::from_vec(vec![root_vc.clone()]),
            false,
        );
    }
    for target in apply_header_options(&root_vc, &options, mtm) {
        std::mem::forget(target);
    }
    entry
        .stack
        .borrow_mut()
        .push(ScreenEntry { vc: root_vc, scope_id });
}

pub(crate) fn release_navigator(
    navigator_instances: &mut HashMap<usize, NavigatorEntry>,
    node: &IosNode,
) {
    let key = node.view_key();
    if let Some(entry) = navigator_instances.remove(&key) {
        drop(entry);
    }
}

pub(crate) fn make_navigator_handle(
    navigator_instances: &HashMap<usize, NavigatorEntry>,
    node: &IosNode,
) -> NavigatorHandle {
    let key = node.view_key();
    let Some(entry) = navigator_instances.get(&key) else {
        return NavigatorHandle::new(Rc::new(()), &IosNavigatorOps);
    };
    NavigatorHandle::with_control(Rc::new(()), &IosNavigatorOps, entry.control.clone())
}

// ---------------------------------------------------------------------------
// Style slot implementations
// ---------------------------------------------------------------------------

pub(crate) fn apply_nav_header_style(
    controller: &UINavigationController,
    nav_view: &UIView,
    style: &Rc<StyleRules>,
) {
    unsafe {
        let nav_bar: Retained<objc2_foundation::NSObject> = msg_send_id![controller, navigationBar];
        let appearance: Retained<objc2_foundation::NSObject> =
            msg_send_id![objc2::class!(UINavigationBarAppearance), new];

        if let Some(ref bg) = style.background {
            let _: () = msg_send![&appearance, configureWithOpaqueBackground];
            let c = color_to_uicolor(bg.value());
            let _: () = msg_send![&appearance, setBackgroundColor: &*c];
            // Set the navigator's view AND the top VC's view background
            // so the themed color fills behind the nav bar, status bar,
            // and home indicator areas.
            nav_view.setBackgroundColor(Some(&c));
            let top_vc: Option<Retained<UIViewController>> =
                msg_send_id![controller, topViewController];
            if let Some(vc) = top_vc {
                if let Some(vc_view) = vc.view() {
                    vc_view.setBackgroundColor(Some(&c));
                }
            }
        } else {
            let _: () = msg_send![&appearance, configureWithTransparentBackground];
        }

        // Shadow — check border_bottom_width == 0 or explicit "no shadow"
        // For now: if no shadow-related property, clear shadow
        let clear = UIColor::clearColor();
        let _: () = msg_send![&appearance, setShadowColor: &*clear];

        let _: () = msg_send![&nav_bar, setStandardAppearance: &*appearance];
        let _: () = msg_send![&nav_bar, setScrollEdgeAppearance: &*appearance];
    }
}

pub(crate) fn apply_nav_title_style(
    controller: &UINavigationController,
    style: &Rc<StyleRules>,
) {
    unsafe {
        let nav_bar: Retained<objc2_foundation::NSObject> = msg_send_id![controller, navigationBar];
        let appearance: Retained<objc2_foundation::NSObject> = msg_send_id![&nav_bar, standardAppearance];
        // Clone the appearance so we don't mutate the shared one
        let appearance: Retained<objc2_foundation::NSObject> = msg_send_id![&appearance, copy];

        let dict: Retained<objc2_foundation::NSObject> =
            msg_send_id![objc2::class!(NSMutableDictionary), new];

        if let Some(ref color) = style.color {
            let c = color_to_uicolor(color.value());
            let key: Retained<objc2_foundation::NSObject> =
                msg_send_id![objc2::class!(NSString), stringWithUTF8String: b"NSColor\0".as_ptr()];
            let _: () = msg_send![&dict, setObject: &*c, forKey: &*key];
        }

        let size: CGFloat = style.font_size.as_ref()
            .map(|t| match t.value() {
                framework_core::Length::Px(v) => *v as CGFloat,
                _ => 17.0,
            })
            .unwrap_or(17.0);
        let weight = style.font_weight.unwrap_or(framework_core::FontWeight::SemiBold);
        let ui_weight = font_weight_to_uikit(weight);
        let font: Retained<objc2_foundation::NSObject> =
            msg_send_id![
                objc2::class!(UIFont),
                systemFontOfSize: size,
                weight: ui_weight
            ];
        let key: Retained<objc2_foundation::NSObject> =
            msg_send_id![objc2::class!(NSString), stringWithUTF8String: b"NSFont\0".as_ptr()];
        let _: () = msg_send![&dict, setObject: &*font, forKey: &*key];

        let _: () = msg_send![&appearance, setTitleTextAttributes: &*dict];
        let _: () = msg_send![&nav_bar, setStandardAppearance: &*appearance];
        let _: () = msg_send![&nav_bar, setScrollEdgeAppearance: &*appearance];
    }
}

pub(crate) fn apply_nav_button_style(
    controller: &UINavigationController,
    style: &Rc<StyleRules>,
) {
    unsafe {
        let nav_bar: Retained<objc2_foundation::NSObject> = msg_send_id![controller, navigationBar];
        if let Some(ref color) = style.color {
            let c = color_to_uicolor(color.value());
            let _: () = msg_send![&nav_bar, setTintColor: &*c];
        }
    }
}
