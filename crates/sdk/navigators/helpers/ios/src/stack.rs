//! Stack navigator iOS engine.
//!
//! Moved verbatim from `backend-ios-mobile::imp::navigator` after the
//! navigator-substrate refactor. The shape changed in two places:
//!   1. The per-instance state is now stored in this crate's thread-local
//!      `STACK_INSTANCES` registry instead of an `IosBackend` field.
//!   2. The `IosNavCallbacks` (defined in this crate's `lib.rs`)
//!      replaces the deleted `runtime_core::NavigatorCallbacks<N>`.
//!   3. `attach_initial`'s opaque options come through as the helper's
//!      `IosScreenOptions` reference instead of the deleted
//!      `runtime_core::ScreenOptions`.

use crate::chrome::apply_header_options;
use crate::{IosNavCallbacks, IosScreenOptions, IOS_NAV_OPS, STACK_INSTANCES};
use backend_ios::{mount_screen_in_vc, schedule_layout_pass, IosNode};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass,
};
use objc2_foundation::MainThreadMarker;
use objc2_ui_kit::{
    UINavigationController, UINavigationControllerDelegate, UIView, UIViewController,
};
use runtime_core::primitives::navigator::{
    MountResult, NavCommand, NavState, NavigatorControl,
};
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) struct StackEntry {
    pub(crate) controller: Retained<UINavigationController>,
    pub(crate) control: Rc<NavigatorControl>,
    pub(crate) stack: Rc<RefCell<Vec<ScreenEntry>>>,
    /// Keep the delegate alive for the navigator's lifetime —
    /// `setDelegate:` doesn't retain, so dropping this would leave a
    /// dangling pointer in UIKit and the interactive-pop observer
    /// would silently stop firing.
    #[allow(dead_code)]
    pub(crate) delegate: Retained<NavigatorDelegate>,
    /// Configured initial route + screen builder + nav-state + depth
    /// callback, retained so `attach_initial` can reconstruct the back
    /// stack on a cold-start deep link (see `attach_initial`).
    pub(crate) initial_route: &'static str,
    pub(crate) mount_screen:
        Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<IosNode>>,
    pub(crate) nav_state: NavState,
    pub(crate) depth_changed: Rc<dyn Fn(usize)>,
}

pub(crate) struct ScreenEntry {
    #[allow(dead_code)]
    pub(crate) vc: Retained<UIViewController>,
    pub(crate) scope_id: u64,
    /// Header callback targets (nav-bar button action handlers). UIKit
    /// holds these weakly via `setTarget:`, so the SDK must own them for
    /// the life of the screen. Storing them here releases them when the
    /// screen pops — the correct lifetime, instead of leaking them for the
    /// whole app via `mem::forget`.
    #[allow(dead_code)]
    pub(crate) header_targets: Vec<Retained<NSObject>>,
    /// Whether the system back affordance may pop THIS screen
    /// (`IosScreenOptions::back_enabled`, defaulting to `true`). The
    /// nav controller's `interactivePopGestureRecognizer` is global, so
    /// it's re-synced to the *top* entry's value after every transition
    /// (see [`sync_back_gesture`]); the back chevron is per-VC and set
    /// once at mount.
    pub(crate) back_enabled: bool,
    /// Whether THIS screen wants full-screen while active
    /// (`IosScreenOptions::fullscreen`, defaulting to `false`). The
    /// app-global full-screen state is re-applied to the *top* entry's
    /// value after every transition (see [`sync_active_screen`]).
    pub(crate) fullscreen: bool,
}

/// Resolve the back-lock flag from a mounted screen's options. Missing
/// or non-`IosScreenOptions` options mean "back works normally" (`true`).
fn back_enabled_of(options: &dyn Any) -> bool {
    options
        .downcast_ref::<IosScreenOptions>()
        .and_then(|o| o.back_enabled)
        .unwrap_or(true)
}

/// Resolve the full-screen flag from a mounted screen's options. Missing
/// or non-`IosScreenOptions` options mean "windowed" (`false`).
fn fullscreen_of(options: &dyn Any) -> bool {
    options
        .downcast_ref::<IosScreenOptions>()
        .and_then(|o| o.fullscreen)
        .unwrap_or(false)
}

/// Hide / show the nav-bar back chevron for one screen. UIKit's
/// `interactivePopGestureRecognizer` is a separate, controller-global
/// affordance — toggling the chevron alone leaves the swipe live — so
/// this pairs with [`sync_back_gesture`] for a full lock.
fn set_back_chevron_hidden(vc: &UIViewController, hidden: bool) {
    // Raw msg_send avoids pulling the UINavigationItem binding (and its
    // objc2-ui-kit feature gate) in just for one setter. `navigationItem`
    // is non-null on every UIViewController.
    unsafe {
        let item: Retained<NSObject> = msg_send_id![vc, navigationItem];
        let _: () = msg_send![&item, setHidesBackButton: hidden];
    }
}

/// Re-sync the controller-global swipe-back recognizer to the TOP
/// screen's `back_enabled`. Called after every push/pop/replace/reset
/// and on the delegate's `didShow` (which fires after an interactive or
/// programmatic pop reveals a new top). An empty stack leaves the swipe
/// enabled — there's nothing to lock.
fn sync_back_gesture(nav: &UINavigationController, stack: &[ScreenEntry]) {
    let enabled = stack.last().map(|e| e.back_enabled).unwrap_or(true);
    unsafe {
        // `interactivePopGestureRecognizer` is nullable (nil before the
        // controller has a navigation bar), so receive it as Option.
        let gr: Option<Retained<NSObject>> = msg_send_id![nav, interactivePopGestureRecognizer];
        if let Some(gr) = gr {
            let _: () = msg_send![&gr, setEnabled: enabled];
        }
    }
}

/// Re-apply everything that tracks the TOP screen after a transition:
/// the controller-global swipe-back recognizer AND the app-global
/// full-screen state. `set_fullscreen` routes to the backend's installed
/// setter (iOS hides the status bar + home indicator), defaulting to
/// `false` (windowed) when the stack is empty or the top screen didn't
/// opt in. Called after every push/pop/replace/reset and on `didShow`.
fn sync_active_screen(nav: &UINavigationController, stack: &[ScreenEntry]) {
    sync_back_gesture(nav, stack);
    let fullscreen = stack.last().map(|e| e.fullscreen).unwrap_or(false);
    runtime_core::set_fullscreen(fullscreen);
}

// ---------------------------------------------------------------------------
// UINavigationControllerDelegate — observe interactive pops
// ---------------------------------------------------------------------------
//
// UIKit pops view controllers in three ways that the dispatcher
// doesn't see: swipe-back, the system back-chevron, and any external
// `popViewController` call. Hooking `didShow` reconciles the rust
// stack against the controller's actual `viewControllers` count.

pub(crate) struct NavigatorDelegateIvars {
    stack: Rc<RefCell<Vec<ScreenEntry>>>,
    release: Rc<dyn Fn(u64)>,
    depth_changed: Rc<dyn Fn(usize)>,
}

declare_class!(
    pub(crate) struct NavigatorDelegate;

    unsafe impl ClassType for NavigatorDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystHelpersNavigatorDelegate";
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
            let visible_depth = unsafe { nav.viewControllers().count() };
            let ivars = self.ivars();
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
                (ivars.release)(scope_id);
            }
            // A pop just revealed a (possibly different) top screen — the
            // swipe recognizer is controller-global, so re-point it at the
            // newly-revealed top's back-lock state.
            sync_active_screen(nav, &ivars.stack.borrow());
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

// ---------------------------------------------------------------------------
// create / attach_initial
// ---------------------------------------------------------------------------

pub(crate) fn create(
    mtm: MainThreadMarker,
    callbacks: IosNavCallbacks,
    control: Rc<NavigatorControl>,
) -> IosNode {
    let nav = unsafe { UINavigationController::new(mtm) };
    let nav_view = nav.view().expect("UINavigationController.view");
    let white = unsafe {
        objc2_ui_kit::UIColor::colorWithRed_green_blue_alpha(1.0, 1.0, 1.0, 1.0)
    };
    nav_view.setBackgroundColor(Some(&white));

    unsafe {
        let nav_bar: Retained<objc2_foundation::NSObject> =
            msg_send_id![&nav, navigationBar];
        let appearance: Retained<objc2_foundation::NSObject> =
            msg_send_id![objc2::class!(UINavigationBarAppearance), new];
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

    let entry = StackEntry {
        controller: nav.clone(),
        control: control.clone(),
        stack: stack_rc.clone(),
        delegate: delegate.clone(),
        initial_route: callbacks.initial_route,
        mount_screen: callbacks.mount_screen.clone(),
        nav_state: callbacks.nav_state.clone(),
        depth_changed: callbacks.depth_changed.clone(),
    };
    let key = &*nav_view as *const UIView as usize;
    STACK_INSTANCES.with(|m| {
        m.borrow_mut()
            .insert(key, Rc::new(RefCell::new(entry)));
    });

    control.install(Box::new(move |cmd| {
        let mut stack = stack_ref.borrow_mut();
        match cmd {
            NavCommand::Push { name, params, url: _, state: _ } => {
                let result = mount_for_dispatch(name, params);
                let vc = mount_screen_in_vc(mtm, result.node.as_view());
                let scope_id = result.scope_id;
                unsafe { nav_for_dispatch.pushViewController_animated(&vc, true) };
                // Downcast options to IosScreenOptions; if it doesn't match
                // (no header options attached), there are no targets to own.
                let header_targets = result
                    .options
                    .downcast_ref::<IosScreenOptions>()
                    .map(|opts| apply_header_options(&vc, opts, mtm))
                    .unwrap_or_default();
                let back_enabled = back_enabled_of(&*result.options);
                let fullscreen = fullscreen_of(&*result.options);
                set_back_chevron_hidden(&vc, !back_enabled);
                stack.push(ScreenEntry { vc, scope_id, header_targets, back_enabled, fullscreen });
                sync_active_screen(&nav_for_dispatch, &stack);
                depth_for_dispatch(stack.len());
                schedule_layout_pass();
            }
            NavCommand::Pop => {
                if stack.len() <= 1 {
                    return;
                }
                let _ = unsafe { nav_for_dispatch.popViewControllerAnimated(true) };
                if let Some(popped) = stack.pop() {
                    release_for_dispatch(popped.scope_id);
                }
                // Revealed the screen beneath — re-sync the swipe to it.
                sync_active_screen(&nav_for_dispatch, &stack);
                depth_for_dispatch(stack.len());
                schedule_layout_pass();
            }
            NavCommand::Replace { name, params, url: _, state: _ } => {
                let result = mount_for_dispatch(name, params);
                let vc = mount_screen_in_vc(mtm, result.node.as_view());
                let scope_id = result.scope_id;
                let header_targets = result
                    .options
                    .downcast_ref::<IosScreenOptions>()
                    .map(|opts| apply_header_options(&vc, opts, mtm))
                    .unwrap_or_default();
                let back_enabled = back_enabled_of(&*result.options);
                let fullscreen = fullscreen_of(&*result.options);
                set_back_chevron_hidden(&vc, !back_enabled);
                if let Some(old) = stack.pop() {
                    release_for_dispatch(old.scope_id);
                }
                stack.push(ScreenEntry { vc, scope_id, header_targets, back_enabled, fullscreen });
                sync_active_screen(&nav_for_dispatch, &stack);
                let vcs: Vec<Retained<UIViewController>> =
                    stack.iter().map(|e| e.vc.clone()).collect();
                unsafe {
                    nav_for_dispatch.setViewControllers_animated(
                        &objc2_foundation::NSArray::from_vec(vcs),
                        false,
                    );
                }
                depth_for_dispatch(stack.len());
                schedule_layout_pass();
            }
            NavCommand::Reset { name, params, url: _, state: _ } => {
                let result = mount_for_dispatch(name, params);
                let vc = mount_screen_in_vc(mtm, result.node.as_view());
                let scope_id = result.scope_id;
                let header_targets = result
                    .options
                    .downcast_ref::<IosScreenOptions>()
                    .map(|opts| apply_header_options(&vc, opts, mtm))
                    .unwrap_or_default();
                let back_enabled = back_enabled_of(&*result.options);
                let fullscreen = fullscreen_of(&*result.options);
                set_back_chevron_hidden(&vc, !back_enabled);
                while let Some(prev) = stack.pop() {
                    release_for_dispatch(prev.scope_id);
                }
                stack.push(ScreenEntry { vc: vc.clone(), scope_id, header_targets, back_enabled, fullscreen });
                sync_active_screen(&nav_for_dispatch, &stack);
                unsafe {
                    nav_for_dispatch.setViewControllers_animated(
                        &objc2_foundation::NSArray::from_vec(vec![vc]),
                        false,
                    );
                }
                depth_for_dispatch(stack.len());
                schedule_layout_pass();
            }
            NavCommand::Select { .. } | NavCommand::Custom(_) => {
                // Pre-fix this panicked, which would unwind into UIKit's
                // event loop (UB on the FFI boundary). A mismatched
                // dispatch is a programmer error, not a fatal app
                // condition — log and drop.
                eprintln!(
                    "[ios-nav-helpers::stack] stack Navigator received a non-stack \
                     NavCommand; ignoring."
                );
            }
        }
    }));

    // Pulled `IosNode::View(...)` access into a small local so the
    // borrow on `nav_view` ends before we move it.
    IosNode::View(nav_view)
}

pub(crate) fn attach_initial(
    mtm: MainThreadMarker,
    navigator: &IosNode,
    screen: IosNode,
    scope_id: u64,
    options: &IosScreenOptions,
) {
    let key = navigator.view_key();
    let entry = STACK_INSTANCES.with(|m| m.borrow().get(&key).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();

    // Seat the framework-resolved screen as the navigation root. On a
    // cold-start deep link the walker resolves the launch URL and mounts the
    // RESOLVED (detail) screen here, so the correct screen is on-screen
    // immediately. Back-stack reconstruction (seating the configured `initial`
    // BELOW the detail so the system back-chevron returns to the index) is
    // deferred — `mount_screen` cannot run synchronously here because the
    // framework still holds `backend.borrow_mut()` across `attach_initial`, and
    // mounting re-enters the build walker (double borrow). See
    // [`reconstruct_back_stack_if_deep_link`].
    let root_vc = mount_screen_in_vc(mtm, screen.as_view());
    unsafe {
        entry.controller.setViewControllers_animated(
            &objc2_foundation::NSArray::from_vec(vec![root_vc.clone()]),
            false,
        );
    }
    let header_targets = apply_header_options(&root_vc, options, mtm);
    let back_enabled = options.back_enabled.unwrap_or(true);
    let fullscreen = options.fullscreen.unwrap_or(false);
    set_back_chevron_hidden(&root_vc, !back_enabled);
    entry
        .stack
        .borrow_mut()
        .push(ScreenEntry { vc: root_vc, scope_id, header_targets, back_enabled, fullscreen });
    sync_active_screen(&entry.controller, &entry.stack.borrow());

    // If this was a deep link (resolved route != configured initial), insert
    // the index UNDER the detail once the walker's borrow releases.
    let is_deep_link = entry.nav_state.active_route.get() != entry.initial_route;
    if is_deep_link {
        let navigator_key = key;
        let initial_route = entry.initial_route;
        // Drop the `entry` borrow before scheduling so the deferred closure can
        // re-borrow the registry entry.
        drop(entry);
        // Off-scope, single-shot reconstruction that must survive the
        // `after_ms(0)` window — the runtime owns it and sweeps it after it
        // fires. The closure only touches the per-navigator registry entry
        // (cleaned up on `release`), so this is safe.
        runtime_core::after_ms_detached(0, move || {
            reconstruct_back_stack(mtm, navigator_key, initial_route);
        });
    }
}

/// Deferred half of [`attach_initial`]'s deep-link back-stack reconstruction.
/// Runs AFTER the walker's `backend.borrow_mut()` releases (queued via
/// `after_ms(0)`), so `mount_screen` can safely re-enter the build walker.
/// Mounts the configured `initial` index and re-seats the controller's stack
/// as [index, detail] — preserving the already-mounted detail on top so Back
/// returns to the index.
fn reconstruct_back_stack(
    mtm: MainThreadMarker,
    navigator_key: usize,
    initial_route: &'static str,
) {
    let entry = STACK_INSTANCES.with(|m| m.borrow().get(&navigator_key).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();

    // Build the index screen now that the borrow is released.
    let index = (entry.mount_screen)(initial_route, Box::new(()));
    let index_vc = mount_screen_in_vc(mtm, index.node.as_view());

    // Current detail VC (the deep-linked root we seated synchronously).
    let detail_vc = {
        let stack = entry.stack.borrow();
        stack.last().map(|e| e.vc.clone())
    };
    let Some(detail_vc) = detail_vc else { return };

    unsafe {
        entry.controller.setViewControllers_animated(
            &objc2_foundation::NSArray::from_vec(vec![index_vc.clone(), detail_vc]),
            false,
        );
    }
    let header_targets = apply_header_options(&index_vc, &IosScreenOptions::default(), mtm);

    // Insert the index UNDER the existing detail entry in the rust stack mirror.
    // The reconstructed index is the configured `initial` route, which carries
    // no per-screen options here — back works normally on it.
    {
        let mut stack = entry.stack.borrow_mut();
        let detail = stack.pop();
        stack.push(ScreenEntry {
            vc: index_vc,
            scope_id: index.scope_id,
            header_targets,
            back_enabled: true,
            // Configured `initial` index carries no per-screen options
            // here; it's windowed. (The detail re-pushed on top drives the
            // active full-screen state via `sync_active_screen` below.)
            fullscreen: false,
        });
        if let Some(detail) = detail {
            stack.push(detail);
        }
    }
    // Detail is back on top after re-seating — re-sync the swipe to it.
    sync_active_screen(&entry.controller, &entry.stack.borrow());
    (entry.depth_changed)(entry.stack.borrow().len());
}

// Anchor so the unused-import lint on `IOS_NAV_OPS` doesn't trip when
// only the public-API funcs in `lib.rs` reach for it.
#[allow(dead_code)]
fn _ops_anchor() -> &'static dyn runtime_core::primitives::navigator::NavigatorOps {
    &IOS_NAV_OPS
}
