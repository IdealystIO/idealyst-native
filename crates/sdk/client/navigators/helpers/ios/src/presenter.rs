//! `UINavigationController` presenter for the outlet-model stack
//! navigator — the implementation behind
//! `runtime_vocabulary::handlers::nav_native_push`.
//!
//! # Why a presenter
//!
//! Without one, every stack reveal is one direction-blind
//! `clear_children` + `insert` on the outlet: correct but inert — no
//! push/pop animation and, beyond cosmetics, no interactive swipe-back
//! and no system Back integration. This seats a real
//! `UINavigationController` inside the outlet and routes the five
//! direction-tagged reveals at it.
//!
//! # Division of labour with the handler
//!
//! The handler still owns everything logical: mounting screens,
//! retention, the `Vec<StackEntry>`, the reactive nav state. This owns
//! only *placement and transition* — it receives already-realized screen
//! nodes and decides how they appear. That split is why the presenter
//! never mounts or releases a screen scope.
//!
//! # Every move is a whole stack, shown when UIKit can take it
//!
//! UIKit drops a move made at the wrong time, silently: one made in the turn
//! of an unanimated `setViewControllers:` (a `reset`, the first seat), and a
//! push made while another slide plays. The container then shows one screen
//! and the handler believes another. So a reveal edits the stack the
//! container SHOULD hold and [`Container::show`] hands UIKit the whole of it:
//! at once when nothing is moving, else once the slide in flight is over. A
//! stack UIKit did not take is handed over again. Several moves in one turn
//! land as one transition to the final stack.
//!
//! # Chrome stays author layout
//!
//! The native bar is hidden (`setNavigationBarHidden`). Per CLAUDE.md §7
//! the observable output must be uniform across backends, and every
//! other backend renders the author's `StackHeader`. Only the transition
//! *mechanics* are platform-idiomatic here.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use backend_ios::{mount_screen_in_vc, schedule_layout_pass, IosNode};
use objc2::rc::{Retained, WeakId};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSArray};
use objc2_ui_kit::{
    UIGestureRecognizer, UIGestureRecognizerDelegate, UINavigationController,
    UINavigationControllerDelegate, UIView, UIViewController,
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
    /// Whether the container held `vcs` when last looked at. Until it does,
    /// a shorter container is a move of ours UIKit has yet to make, and no
    /// user pop.
    in_step: Rc<Cell<bool>>,
    /// Weak: the controller does not retain its delegate, and the closures
    /// that own this delegate own the controller too.
    nav: WeakId<UINavigationController>,
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
            if actual >= believed || !ivars.in_step.get() {
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
            // A raw UIKit delegate callback, outside every framework-
            // wrapped handler: nothing else commits the writes the
            // handler just staged (depth, active route, header chrome).
            backend_ios::newcore::schedule_flush();
            schedule_layout_pass();
        }
    }

    unsafe impl UIGestureRecognizerDelegate for PresenterDelegate {
        /// The swipe-back begins whenever there is a screen to go back to.
        /// UIKit's own delegate refuses it while the navigation bar is
        /// hidden, which here is always. Not at the root, nor during a slide:
        /// a gesture begun there leaves the controller stuck.
        #[method(gestureRecognizerShouldBegin:)]
        fn should_begin(&self, _recognizer: &UIGestureRecognizer) -> bool {
            self.ivars()
                .nav
                .load()
                .is_some_and(|nav| unsafe { nav.viewControllers().count() } > 1 && !is_moving(&nav))
        }
    }
);

impl PresenterDelegate {
    fn new(
        mtm: MainThreadMarker,
        nav: &Retained<UINavigationController>,
        vcs: Rc<RefCell<Vec<Retained<UIViewController>>>>,
        in_step: Rc<Cell<bool>>,
        user_pop: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let nav = WeakId::from_retained(nav);
        let this = this.set_ivars(PresenterDelegateIvars { vcs, in_step, nav, user_pop });
        unsafe { msg_send_id![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// The container — what UIKit should show, and getting it to
// ---------------------------------------------------------------------------

/// A slide is playing, or the user is swiping.
fn is_moving(nav: &UINavigationController) -> bool {
    let coordinator: *mut AnyObject = unsafe { msg_send![nav, transitionCoordinator] };
    !coordinator.is_null()
}

/// How often a move that could not be made yet is tried again.
const RETRY_MS: i32 = 32;

/// How many times a stack UIKit does not take is handed over before giving
/// up, so a container that can't move (torn out of its window mid-move)
/// doesn't keep the app awake.
const MAX_TRIES: u32 = 30;

struct Container {
    nav: Retained<UINavigationController>,
    /// `setDelegate:` does NOT retain, so this keeps the delegate alive as
    /// long as the closures of the `NativePushHandle`, which the handler
    /// holds for the navigator's lifetime.
    _delegate: Retained<PresenterDelegate>,
    /// The stack to show (the delegate's `vcs`).
    wanted: Rc<RefCell<Vec<Retained<UIViewController>>>>,
    in_step: Rc<Cell<bool>>,
    /// A move UIKit has not been given yet, and whether it animates.
    held: Cell<Option<bool>>,
    /// UIKit was given a move in this turn of the run loop.
    moved: Cell<bool>,
    checking: Cell<bool>,
    tries: Cell<u32>,
}

impl Container {
    /// Show `wanted`, which the caller has just changed.
    fn show(self: &Rc<Self>, animated: bool) {
        let held = self.held.get();
        if held.is_some() || self.moved.get() || is_moving(&self.nav) {
            self.held.set(Some(held.unwrap_or(false) || animated));
            self.in_step.set(false);
        } else {
            self.hand_over(animated);
        }
        self.check_later();
        schedule_layout_pass();
    }

    fn hand_over(&self, animated: bool) {
        self.moved.set(true);
        // Before the call: an unanimated move can report `didShow` inside it.
        self.in_step.set(false);
        let stack = self.wanted.borrow().clone();
        unsafe { self.nav.setViewControllers_animated(&NSArray::from_vec(stack), animated) };
        self.in_step.set(self.shows_wanted());
    }

    fn shows_wanted(&self) -> bool {
        let shown = unsafe { self.nav.viewControllers() };
        let wanted = self.wanted.borrow();
        shown.count() == wanted.len() && shown.iter().zip(wanted.iter()).all(|(a, b)| std::ptr::eq(a, &**b))
    }

    /// Look again on a later turn. A timer, not a microtask: during the boot
    /// microtasks run inside the turn that queued them.
    fn check_later(self: &Rc<Self>) {
        if self.checking.replace(true) {
            return;
        }
        let this: Weak<Self> = Rc::downgrade(self);
        runtime_shared::scheduling::after_ms_detached(RETRY_MS, move || {
            if let Some(this) = this.upgrade() {
                this.check();
            }
        });
    }

    fn check(self: &Rc<Self>) {
        self.checking.set(false);
        self.moved.set(false);
        if is_moving(&self.nav) {
            // Nothing to look at until the slide is over, and nothing to do
            // after it when UIKit took the move and no other waits.
            if self.held.get().is_some() || !self.in_step.get() {
                self.check_later();
            }
            return;
        }
        let shows = self.shows_wanted();
        self.in_step.set(shows);
        if shows {
            self.held.set(None);
            self.tries.set(0);
            return;
        }
        let tries = self.tries.get() + 1;
        self.tries.set(tries);
        if tries > MAX_TRIES {
            backend_ios_core::ios_log("[stack] UIKit did not take the stack; giving up until the next move");
            self.held.set(None);
            self.tries.set(0);
            return;
        }
        self.hand_over(self.held.take().unwrap_or(true));
        self.check_later();
        schedule_layout_pass();
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

        let in_step = Rc::new(Cell::new(true));
        let delegate = PresenterDelegate::new(mtm, &nav, vcs.clone(), in_step.clone(), user_pop.clone());
        unsafe {
            nav.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            let recognizer: *mut AnyObject = msg_send![&*nav, interactivePopGestureRecognizer];
            if !recognizer.is_null() {
                let _: () = msg_send![recognizer, setDelegate: &*delegate];
            }
            // iOS 26's swipe from anywhere in the content stays off, whatever
            // UIKit thinks of a hidden bar: it would take the app's own
            // horizontal drags (a row, a canvas), which are raw touches no
            // recognizer stands for.
            let has: bool =
                msg_send![&*nav, respondsToSelector: objc2::sel!(interactiveContentPopGestureRecognizer)];
            if has {
                let recognizer: *mut AnyObject = msg_send![&*nav, interactiveContentPopGestureRecognizer];
                if !recognizer.is_null() {
                    let _: () = msg_send![recognizer, setEnabled: false];
                }
            }
        }
        let container = Rc::new(Container {
            nav: nav.clone(),
            _delegate: delegate,
            wanted: vcs.clone(),
            in_step,
            held: Cell::new(None),
            moved: Cell::new(false),
            checking: Cell::new(false),
            tries: Cell::new(0),
        });

        // `seat` and `reset` are the same UIKit move — collapse the
        // container to one screen, unanimated — and differ only in the
        // handler's intent, so they share a builder.
        let collapse_to_one = || {
            let container = container.clone();
            Rc::new(move |node: Rc<dyn Any>| {
                let Some(view) = screen_view(&node) else { return };
                *container.wanted.borrow_mut() = vec![mount_screen_in_vc(mtm, &view)];
                container.show(false);
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };
        let seat = collapse_to_one();
        let reset = collapse_to_one();

        let push = {
            let container = container.clone();
            Rc::new(move |node: Rc<dyn Any>| {
                let Some(view) = screen_view(&node) else { return };
                container.wanted.borrow_mut().push(mount_screen_in_vc(mtm, &view));
                container.show(true);
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };

        let pop = {
            let container = container.clone();
            // The revealed screen's VC is already in the container (a
            // native stack retains what it covers — which is why the
            // handler tightens retention to `Retain` on attach), so the
            // node argument is not needed. Taking it anyway keeps all
            // five reveals one shape.
            Rc::new(move |_node: Rc<dyn Any>| {
                // Never pop the root: the handler guards this too, but a
                // presenter that emptied the container would leave a
                // blank screen no command could recover.
                if container.wanted.borrow().len() <= 1 {
                    return;
                }
                container.wanted.borrow_mut().pop();
                container.show(true);
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };

        let replace = {
            let container = container.clone();
            Rc::new(move |node: Rc<dyn Any>| {
                let Some(view) = screen_view(&node) else { return };
                let vc = mount_screen_in_vc(mtm, &view);
                let animated = replace_move(&mut container.wanted.borrow_mut(), vc);
                container.show(animated);
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };

        let set_user_pop = {
            let user_pop = user_pop.clone();
            Rc::new(move |cb: Rc<dyn Fn()>| {
                *user_pop.borrow_mut() = Some(cb);
            }) as Rc<dyn Fn(Rc<dyn Fn()>)>
        };

        // The back lock. The swipe-back recognizer belongs to the
        // CONTROLLER, not to a view controller, so it must describe
        // whichever screen is on top — the handler calls this after
        // every navigation with that screen's flag. The native back
        // chevron needs no lock: the bar is hidden.
        let set_back_enabled = {
            let container = container.clone();
            Rc::new(move |enabled: bool| unsafe {
                let recognizer: *mut AnyObject = msg_send![&*container.nav, interactivePopGestureRecognizer];
                if !recognizer.is_null() {
                    let _: () = msg_send![recognizer, setEnabled: enabled];
                }
            }) as Rc<dyn Fn(bool)>
        };

        Some(NativePushHandle {
            host: Rc::new(IosNode::View(nav_view)),
            seat,
            push,
            pop,
            replace,
            reset,
            set_user_pop,
            set_back_enabled,
        })
    }
}

/// Swap the top of `wanted` for `incoming` and say whether to show it
/// animated.
///
/// Animated, because replacing the top is a navigation on iOS: with a new
/// top that was not in the old stack, `setViewControllers:animated:YES`
/// plays the standard push transition and leaves the screens below as
/// given, the native move for linear flows (sign-in steps, checkout
/// steps). A replace that keeps the same screen never reaches the
/// presenter: the handler absorbs a query-only `replace_with_state`
/// before revealing anything.
///
/// The outgoing screen keeps its content for the whole slide. The handler
/// releases it before this reveal, but `release_subtree` only unregisters
/// it from layout; the view stays inside its view controller, which UIKit
/// retains until the transition ends (the same as an app-initiated pop).
fn replace_move<T>(wanted: &mut Vec<T>, incoming: T) -> bool {
    wanted.pop();
    wanted.push(incoming);
    true
}

/// Install the iOS stack presenter. Idempotent; safe to call more than
/// once (the seam replaces).
pub fn install() {
    runtime_vocabulary::handlers::nav_native_push::install_stack_presenter(Rc::new(
        IosStackPresenter,
    ));
}

// The moves themselves run on a real `UINavigationController` in
// `stack-navigator`'s `tests/ios_stack.rs`. These pin the decision a replace
// makes. The crate is iOS-only, so they run on the simulator: `cargo test -p
// ios-navigator-helpers --target aarch64-apple-ios-sim --no-run`, then
// `xcrun simctl spawn booted <bin>`.
#[cfg(test)]
mod tests {
    use super::replace_move;

    /// Regression: `replace` from a pushed step cut to the next step with
    /// no slide, while push and pop animated.
    #[test]
    fn regression_replace_to_a_different_screen_animates() {
        let mut wanted = vec!["pin_pad", "picker"];
        assert!(replace_move(&mut wanted, "employee_id"), "a replace that changes the top screen is a navigation");
        assert_eq!(wanted, ["pin_pad", "employee_id"], "only the top is swapped");
    }

    #[test]
    fn replace_at_the_root_keeps_a_single_screen() {
        let mut wanted = vec!["pin_pad"];
        assert!(replace_move(&mut wanted, "form"));
        assert_eq!(wanted, ["form"]);
    }
}
