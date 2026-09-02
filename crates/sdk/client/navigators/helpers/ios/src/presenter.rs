//! `UINavigationController` presenter for the outlet-model stack
//! navigator — the implementation behind
//! `runtime_vocabulary::handlers::nav_native_push`.
//!
//! # What this restores
//!
//! The outlet-model rewrite left every stack reveal as one
//! direction-blind `clear_children` + `insert` on the outlet. That is
//! correct but inert: no push/pop animation, and — the part that is not
//! cosmetic — no interactive swipe-back and no system Back integration.
//! This seats a real `UINavigationController` inside the outlet and
//! routes the five direction-tagged reveals at it.
//!
//! # Division of labour with the handler
//!
//! The handler still owns everything logical: mounting screens,
//! retention, the `Vec<StackEntry>`, the reactive nav state. This owns
//! only *placement and transition* — it receives already-realized screen
//! nodes and decides how they appear. That split is why the presenter
//! never mounts or releases a screen scope.
//!
//! # Chrome stays author layout
//!
//! The native bar is hidden (`setNavigationBarHidden`). Per CLAUDE.md §7
//! the observable output must be uniform across backends, and every
//! other backend renders the author's `StackHeader`. Only the transition
//! *mechanics* are platform-idiomatic here.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use backend_ios::{mount_screen_in_vc, schedule_layout_pass, IosNode};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSArray};
use objc2_ui_kit::{
    UINavigationController, UINavigationControllerDelegate, UIView, UIViewController,
};
use runtime_vocabulary::handlers::nav_native_push::{NativePushHandle, StackPresenter};

// ---------------------------------------------------------------------------
// Delegate — the reverse channel for user-initiated pops
// ---------------------------------------------------------------------------

pub(crate) struct PresenterDelegateIvars {
    /// The view controllers this presenter believes the container holds,
    /// newest last — our own copy rather than a read-back of
    /// `nav.viewControllers()`, so the retain/lifetime story is plain
    /// Rust and there is no bridging of an `NSArray` on a hot path.
    ///
    /// Every op we drive updates it BEFORE handing UIKit the command, so
    /// the `didShow` that follows sees `actual == believed` and stays
    /// quiet. A user-driven pop moves the container without going
    /// through us, so there `actual < believed` — which is the signal.
    vcs: Rc<RefCell<Vec<Retained<UIViewController>>>>,
    /// The handler's logical-only pop, installed via `set_user_pop`.
    /// `RefCell` because the handle is built (and handed to the handler)
    /// before the handler can give us this.
    user_pop: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
}

declare_class!(
    pub(crate) struct PresenterDelegate;

    unsafe impl ClassType for PresenterDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        // Distinct from the legacy engine's delegate class: ObjC class
        // names are process-global, and registering the same name twice
        // aborts.
        const NAME: &'static str = "IdealystStackPresenterDelegate";
    }

    impl DeclaredClass for PresenterDelegate {
        type Ivars = PresenterDelegateIvars;
    }

    unsafe impl NSObjectProtocol for PresenterDelegate {}

    unsafe impl UINavigationControllerDelegate for PresenterDelegate {
        #[method(navigationController:didShowViewController:animated:)]
        fn did_show(
            &self,
            nav: &UINavigationController,
            _vc: &UIViewController,
            _animated: bool,
        ) {
            let ivars = self.ivars();
            let actual = unsafe { nav.viewControllers().count() };
            let believed = ivars.vcs.borrow().len();
            if actual >= believed {
                // We drove this transition (or nothing moved).
                return;
            }
            // The user popped — swipe-back completed, or the system back
            // chevron. UIKit has already moved; tell the handler to
            // reconcile its logical stack. Once per popped level: a fast
            // swipe can collapse more than one.
            //
            // `tracked` is updated BEFORE the callbacks so a re-entrant
            // `didShow` (UIKit can nest these during a fast gesture)
            // cannot double-report the same level.
            let popped = believed - actual;
            ivars.vcs.borrow_mut().truncate(actual);
            let cb = ivars.user_pop.borrow().clone();
            if let Some(cb) = cb {
                for _ in 0..popped {
                    cb();
                }
            }
            schedule_layout_pass();
        }
    }
);

impl PresenterDelegate {
    fn new(
        mtm: MainThreadMarker,
        vcs: Rc<RefCell<Vec<Retained<UIViewController>>>>,
        user_pop: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(PresenterDelegateIvars { vcs, user_pop });
        unsafe { msg_send_id![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// The presenter
// ---------------------------------------------------------------------------

/// Installed once at boot; consulted by every stack navigator that mounts.
pub struct IosStackPresenter;

/// Pull the screen `UIView` out of the handler's erased node.
///
/// Returns `None` for a foreign node type rather than panicking: one
/// presenter is installed per process, but a process can host more than
/// one backend (a runtime-server sidecar beside the real one), and a
/// screen we cannot place is not a crash — it is a navigator we should
/// have declined.
fn screen_view(node: &Rc<dyn Any>) -> Option<Retained<UIView>> {
    node.downcast_ref::<IosNode>()
        .map(|n| n.as_view().retain())
}

impl StackPresenter for IosStackPresenter {
    fn attach(&self, outlet: Rc<dyn Any>) -> Option<NativePushHandle> {
        // Decline anything that is not this backend's node — see
        // `screen_view` for why declining beats panicking.
        let outlet_node = outlet.downcast_ref::<IosNode>()?;
        let mtm = MainThreadMarker::new()?;
        let outlet_view = outlet_node.as_view().retain();

        let nav = unsafe { UINavigationController::new(mtm) };
        // Chrome is the author's `StackHeader` on every backend; the
        // native bar would be a second one.
        unsafe { nav.setNavigationBarHidden_animated(true, false) };

        let nav_view = nav.view()?;

        // Autoresizing, NOT Auto Layout, to sit inside the outlet.
        //
        // The outlet is a framework node: Taffy computes its rect and
        // sets `frame` on it directly. A child pinned with Auto Layout
        // constraints against a directly-framed parent resolves through
        // the parent's implicit `translatesAutoresizingMaskIntoConstraints`
        // constraints, which Taffy then fights on the next layout pass.
        // An autoresizing mask has no such conflict: UIKit reapplies it
        // from the superview's bounds whenever Taffy moves them, which
        // is precisely the behavior wanted here.
        unsafe {
            nav_view.setFrame(outlet_view.bounds());
            let _: () = msg_send![&*nav_view, setTranslatesAutoresizingMaskIntoConstraints: true];
            // UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight
            let _: () = msg_send![&*nav_view, setAutoresizingMask: 2usize | 16usize];
            outlet_view.addSubview(&nav_view);
        }

        let vcs: Rc<RefCell<Vec<Retained<UIViewController>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let user_pop: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));

        let delegate = PresenterDelegate::new(mtm, vcs.clone(), user_pop.clone());
        unsafe {
            nav.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }

        // Every closure below captures `nav` and `delegate`. That is the
        // ownership model: `setDelegate:` does NOT retain, so dropping
        // the delegate would leave UIKit holding a dangling pointer and
        // the swipe-back observer would silently stop firing. Both live
        // exactly as long as the `NativePushHandle`, which the handler
        // holds for the navigator's lifetime.
        let keep_delegate = delegate.clone();

        // `seat` and `reset` are the same UIKit move — collapse the
        // container to one screen, unanimated — and differ only in the
        // handler's intent, so they share a builder.
        let collapse_to_one = |nav: Retained<UINavigationController>,
                               vcs: Rc<RefCell<Vec<Retained<UIViewController>>>>,
                               _d: Retained<PresenterDelegate>| {
            Rc::new(move |node: Rc<dyn Any>| {
                let Some(view) = screen_view(&node) else { return };
                let vc = mount_screen_in_vc(mtm, &view);
                *vcs.borrow_mut() = vec![vc.clone()];
                unsafe {
                    nav.setViewControllers_animated(&NSArray::from_vec(vec![vc]), false);
                }
                schedule_layout_pass();
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };

        let seat = collapse_to_one(nav.clone(), vcs.clone(), keep_delegate.clone());
        let reset = collapse_to_one(nav.clone(), vcs.clone(), keep_delegate.clone());

        let push = {
            let nav = nav.clone();
            let vcs = vcs.clone();
            let _d = keep_delegate.clone();
            Rc::new(move |node: Rc<dyn Any>| {
                let Some(view) = screen_view(&node) else { return };
                let vc = mount_screen_in_vc(mtm, &view);
                vcs.borrow_mut().push(vc.clone());
                unsafe { nav.pushViewController_animated(&vc, true) };
                schedule_layout_pass();
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };

        let pop = {
            let nav = nav.clone();
            let vcs = vcs.clone();
            let _d = keep_delegate.clone();
            // The revealed screen's VC is already in the container (a
            // native stack retains what it covers — which is why the
            // handler tightens retention to `Retain` on attach), so the
            // node argument is not needed. Taking it anyway keeps all
            // five reveals one shape.
            Rc::new(move |_node: Rc<dyn Any>| {
                // Never pop the root: the handler guards this too, but a
                // presenter that emptied the container would leave a
                // blank screen no command could recover.
                if vcs.borrow().len() <= 1 {
                    return;
                }
                vcs.borrow_mut().pop();
                let _ = unsafe { nav.popViewControllerAnimated(true) };
                schedule_layout_pass();
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };

        let replace = {
            let nav = nav.clone();
            let vcs = vcs.clone();
            let _d = keep_delegate.clone();
            Rc::new(move |node: Rc<dyn Any>| {
                let Some(view) = screen_view(&node) else { return };
                let vc = mount_screen_in_vc(mtm, &view);
                // Swap the top in place, unanimated: Replace is not a
                // navigation, it is the same position holding something
                // else.
                let snapshot = {
                    let mut v = vcs.borrow_mut();
                    v.pop();
                    v.push(vc);
                    v.clone()
                };
                unsafe {
                    nav.setViewControllers_animated(&NSArray::from_vec(snapshot), false);
                }
                schedule_layout_pass();
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };

        let set_user_pop = {
            let user_pop = user_pop.clone();
            Rc::new(move |cb: Rc<dyn Fn()>| {
                *user_pop.borrow_mut() = Some(cb);
            }) as Rc<dyn Fn(Rc<dyn Fn()>)>
        };

        Some(NativePushHandle {
            host: Rc::new(IosNode::View(nav_view)),
            seat,
            push,
            pop,
            replace,
            reset,
            set_user_pop,
        })
    }
}

/// Install the iOS stack presenter. Idempotent; safe to call more than
/// once (the seam replaces).
pub fn install() {
    runtime_vocabulary::handlers::nav_native_push::install_stack_presenter(Rc::new(
        IosStackPresenter,
    ));
}
