//! App situation facts for the iOS backend (`runtime_shared::app_state`):
//! focus, visibility, reduce motion and the primary pointer, read at boot
//! and followed live. Installed once per boot by `newcore::start_with`.
//!
//! - **Focused / Visible**: `UIApplication.applicationState` at boot
//!   (`pointer_input_policy::focus_and_visibility`); then
//!   `DidBecomeActive` → focused + visible, `WillResignActive` → not
//!   focused (app switcher, Control Center, a call), `DidEnterBackground` →
//!   not visible, `WillEnterForeground` → visible. Each notification sets
//!   its own value instead of re-reading `applicationState`, which still
//!   reads "active" while `WillResignActive` is being posted.
//! - **ReduceMotion**: `UIAccessibilityIsReduceMotionEnabled()`, re-read on
//!   `UIAccessibilityReduceMotionStatusDidChangeNotification`.
//! - **PrimaryPointer**: `Coarse` (touch), `Fine` while a `GCMouse` is
//!   connected (an iPad with a mouse / trackpad), following
//!   `GCMouseDidConnect` / `DidDisconnect`.
//! - **UserActivated**: not reported (native apps may play audio at once).
//!
//! Every live report runs on the next main-queue turn (UIKit posts these
//! synchronously inside its own work) and is followed by the backend's
//! flush, so world signals commit.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use runtime_shared::app_state::{report, AppStateChange};

use crate::pointer_input_policy as policy;

#[link(name = "GameController", kind = "framework")]
extern "C" {
    static GCMouseDidConnectNotification: &'static NSString;
    static GCMouseDidDisconnectNotification: &'static NSString;
}

extern "C" {
    static UIAccessibilityReduceMotionStatusDidChangeNotification: &'static NSString;
    fn UIAccessibilityIsReduceMotionEnabled() -> objc2::runtime::Bool;
}

fn read_reduce_motion() -> bool {
    unsafe { UIAccessibilityIsReduceMotionEnabled() }.as_bool()
}

fn read_primary_pointer() -> runtime_shared::app_state::PointerKind {
    let count: usize = unsafe {
        let mice: *mut AnyObject = msg_send![objc2::class!(GCMouse), mice];
        if mice.is_null() {
            0
        } else {
            msg_send![mice, count]
        }
    };
    policy::primary_pointer(count)
}

pub(crate) struct AppStateObserverIvars {
    after_report: Rc<dyn Fn()>,
}

impl AppStateObserverIvars {
    /// Report on the next main-queue turn, then flush.
    fn later(&self, change: impl FnOnce() -> Vec<AppStateChange> + 'static) {
        let after = self.after_report.clone();
        runtime_shared::schedule_microtask(move || {
            for c in change() {
                report(c);
            }
            after();
        });
    }
}

declare_class!(
    pub(crate) struct IdealystAppStateObserver;

    unsafe impl ClassType for IdealystAppStateObserver {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystAppStateObserver";
    }

    impl DeclaredClass for IdealystAppStateObserver {
        type Ivars = AppStateObserverIvars;
    }

    unsafe impl NSObjectProtocol for IdealystAppStateObserver {}

    unsafe impl IdealystAppStateObserver {
        #[method(didBecomeActive:)]
        fn did_become_active(&self, _n: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystAppStateObserver::didBecomeActive", || {
                self.ivars().later(|| vec![AppStateChange::Focused(true), AppStateChange::Visible(true)]);
            });
        }

        #[method(willResignActive:)]
        fn will_resign_active(&self, _n: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystAppStateObserver::willResignActive", || {
                self.ivars().later(|| vec![AppStateChange::Focused(false)]);
            });
        }

        #[method(didEnterBackground:)]
        fn did_enter_background(&self, _n: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystAppStateObserver::didEnterBackground", || {
                self.ivars().later(|| vec![AppStateChange::Focused(false), AppStateChange::Visible(false)]);
            });
        }

        #[method(willEnterForeground:)]
        fn will_enter_foreground(&self, _n: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystAppStateObserver::willEnterForeground", || {
                self.ivars().later(|| vec![AppStateChange::Visible(true)]);
            });
        }

        #[method(reduceMotionChanged:)]
        fn reduce_motion_changed(&self, _n: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystAppStateObserver::reduceMotionChanged", || {
                self.ivars().later(|| vec![AppStateChange::ReduceMotion(read_reduce_motion())]);
            });
        }

        #[method(miceChanged:)]
        fn mice_changed(&self, _n: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystAppStateObserver::miceChanged", || {
                // Re-read on the next turn: `GCMouse.mice` reflects the
                // change once the notification has been delivered.
                self.ivars().later(|| vec![AppStateChange::PrimaryPointer(read_primary_pointer())]);
            });
        }
    }
);

/// Report the boot reading and install the observer. `after_report` runs
/// after every live report. The returned observer must outlive the app;
/// dropping it unregisters it (`NSNotificationCenter` drops deallocated
/// selector-based observers on iOS 9+), and [`uninstall`] does so
/// explicitly.
pub(crate) fn install(mtm: MainThreadMarker, after_report: Rc<dyn Fn()>) -> Retained<NSObject> {
    let state: isize = unsafe {
        let app: *mut AnyObject = msg_send![objc2::class!(UIApplication), sharedApplication];
        if app.is_null() {
            0
        } else {
            msg_send![app, applicationState]
        }
    };
    let (focused, visible) = policy::focus_and_visibility(state);
    report(AppStateChange::Focused(focused));
    report(AppStateChange::Visible(visible));
    report(AppStateChange::ReduceMotion(read_reduce_motion()));
    report(AppStateChange::PrimaryPointer(read_primary_pointer()));

    let this = mtm.alloc::<IdealystAppStateObserver>();
    let this = this.set_ivars(AppStateObserverIvars { after_report });
    let observer: Retained<IdealystAppStateObserver> = unsafe { msg_send_id![super(this), init] };
    unsafe {
        let center: *mut AnyObject = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let add = |sel: objc2::runtime::Sel, name: &NSString| {
            let _: () = msg_send![center, addObserver: &*observer, selector: sel, name: name, object: nil];
        };
        add(objc2::sel!(didBecomeActive:), &NSString::from_str("UIApplicationDidBecomeActiveNotification"));
        add(objc2::sel!(willResignActive:), &NSString::from_str("UIApplicationWillResignActiveNotification"));
        add(objc2::sel!(didEnterBackground:), &NSString::from_str("UIApplicationDidEnterBackgroundNotification"));
        add(objc2::sel!(willEnterForeground:), &NSString::from_str("UIApplicationWillEnterForegroundNotification"));
        add(objc2::sel!(reduceMotionChanged:), UIAccessibilityReduceMotionStatusDidChangeNotification);
        add(objc2::sel!(miceChanged:), GCMouseDidConnectNotification);
        add(objc2::sel!(miceChanged:), GCMouseDidDisconnectNotification);
    }
    unsafe { Retained::cast::<NSObject>(observer) }
}

/// Unregister an observer from [`install`].
pub(crate) fn uninstall(observer: &NSObject) {
    unsafe {
        let center: *mut AnyObject = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
        let _: () = msg_send![center, removeObserver: observer];
    }
}
