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
//! - **Splash dismissed** (`runtime_shared::app_state::report_splash_dismissed`,
//!   not `app_state::report`): `false` at boot, then `true` from
//!   [`watch_first_frame`]. iOS has no public callback for the launch
//!   screen going away: it stays until the app's window has committed its
//!   first frame. The tree mounts from `viewDidLoad` inside
//!   `didFinishLaunching`, before the window is on screen. The host view
//!   controller belongs to the generated Swift shell, and its
//!   `viewDidAppear:` marks the view joining the window, not the frame
//!   reaching the screen. So a one-shot `CADisplayLink` watches the host
//!   view: one tick after it
//!   first finds it in a visible window, the commit that drew the app has
//!   run ([`policy::first_frame_tick`]), and the watch reports and stops.
//! - **Color scheme** (`runtime_shared::host::report_color_scheme`, not
//!   `app_state`): the trait collection's `userInterfaceStyle`
//!   ([`read_color_scheme`]), re-read on `traitCollectionDidChange:` of a
//!   zero-size view in the host ([`observe_appearance`]) and on
//!   `DidBecomeActive`. Trait changes in the background are ignored
//!   ([`policy::trait_change_counts`]): iOS renders the app's switcher
//!   snapshots in both styles there. A view, not the generated view
//!   controller, so no app template has to change; `traitCollectionDidChange:`
//!   still reaches every view on iOS 17+, where it is deprecated in favour
//!   of trait registration.
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

fn application_state() -> isize {
    unsafe {
        let app: *mut AnyObject = msg_send![objc2::class!(UIApplication), sharedApplication];
        if app.is_null() {
            0
        } else {
            msg_send![app, applicationState]
        }
    }
}

/// The current trait collection's light / dark style.
pub(crate) fn read_color_scheme() -> runtime_shared::ColorScheme {
    let tc: Retained<NSObject> = unsafe { msg_send_id![objc2::class!(UITraitCollection), currentTraitCollection] };
    let style: isize = unsafe { msg_send![&tc, userInterfaceStyle] };
    policy::color_scheme(style)
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
                // The appearance may have changed while trait changes were
                // ignored in the background.
                runtime_shared::backend::report_color_scheme(read_color_scheme());
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

declare_class!(
    /// Hears the host's trait changes for [`observe_appearance`].
    pub(crate) struct IdealystAppearanceObserverView;

    unsafe impl ClassType for IdealystAppearanceObserverView {
        type Super = objc2_ui_kit::UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystAppearanceObserverView";
    }

    impl DeclaredClass for IdealystAppearanceObserverView {
        type Ivars = AppStateObserverIvars;
    }

    unsafe impl IdealystAppearanceObserverView {
        #[method(traitCollectionDidChange:)]
        fn trait_collection_did_change(&self, previous: *mut AnyObject) {
            let _: () = unsafe { msg_send![super(self), traitCollectionDidChange: previous] };
            crate::imp::ffi_guard::guard_ffi("IdealystAppearanceObserverView::traitCollectionDidChange", || {
                if !policy::trait_change_counts(application_state()) {
                    return;
                }
                let tc: Retained<NSObject> = unsafe { msg_send_id![self, traitCollection] };
                let style: isize = unsafe { msg_send![&tc, userInterfaceStyle] };
                let after = self.ivars().after_report.clone();
                runtime_shared::schedule_microtask(move || {
                    runtime_shared::backend::report_color_scheme(policy::color_scheme(style));
                    after();
                });
            });
        }
    }
);

/// Follow the light / dark appearance: a zero-size, hit-test-transparent
/// view added to `host`, whose trait changes report the color scheme, then
/// `after_report`. The caller keeps the view and removes it to stop.
pub(crate) fn observe_appearance(
    mtm: MainThreadMarker,
    host: &objc2_ui_kit::UIView,
    after_report: Rc<dyn Fn()>,
) -> Retained<objc2_ui_kit::UIView> {
    let this = mtm.alloc::<IdealystAppearanceObserverView>();
    let this = this.set_ivars(AppStateObserverIvars { after_report });
    let view: Retained<IdealystAppearanceObserverView> = unsafe { msg_send_id![super(this), init] };
    let _: () = unsafe { msg_send![&view, setUserInteractionEnabled: false] };
    let view: Retained<objc2_ui_kit::UIView> = Retained::into_super(view);
    unsafe { host.addSubview(&view) };
    view
}

/// Report the boot reading and install the observer. `after_report` runs
/// after every live report. The returned observer must outlive the app;
/// dropping it unregisters it (`NSNotificationCenter` drops deallocated
/// selector-based observers on iOS 9+), and [`uninstall`] does so
/// explicitly.
pub(crate) fn install(mtm: MainThreadMarker, after_report: Rc<dyn Fn()>) -> Retained<NSObject> {
    let (focused, visible) = policy::focus_and_visibility(application_state());
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

pub(crate) struct FirstFrameWatchIvars {
    host: Retained<objc2_ui_kit::UIView>,
    armed: std::cell::Cell<bool>,
    after_report: Rc<dyn Fn()>,
}

declare_class!(
    /// The display-link target of [`watch_first_frame`].
    pub(crate) struct IdealystFirstFrameWatch;

    unsafe impl ClassType for IdealystFirstFrameWatch {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystFirstFrameWatch";
    }

    impl DeclaredClass for IdealystFirstFrameWatch {
        type Ivars = FirstFrameWatchIvars;
    }

    unsafe impl NSObjectProtocol for IdealystFirstFrameWatch {}

    unsafe impl IdealystFirstFrameWatch {
        #[method(tick:)]
        fn tick(&self, link: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystFirstFrameWatch::tick", || {
                let ivars = self.ivars();
                let window: *mut AnyObject = unsafe { msg_send![&*ivars.host, window] };
                let visible = !window.is_null() && {
                    let hidden: bool = unsafe { msg_send![window, isHidden] };
                    !hidden
                };
                let (armed, report_now) = policy::first_frame_tick(ivars.armed.get(), visible);
                ivars.armed.set(armed);
                if !report_now {
                    return;
                }
                // The run loop retains a scheduled link and the link its
                // target (this watch); invalidating releases both.
                let _: () = unsafe { msg_send![link, invalidate] };
                runtime_shared::app_state::report_splash_dismissed(true);
                (ivars.after_report)();
            });
        }
    }
);

/// Report the launch screen as up and watch for the app's first frame in
/// `host` (see the module docs); `after_report` runs after the report.
/// Nothing to watch once it was reported (a second boot on this thread).
/// The returned display link can be invalidated to stop the watch early.
pub(crate) fn watch_first_frame(
    mtm: MainThreadMarker,
    host: &objc2_ui_kit::UIView,
    after_report: Rc<dyn Fn()>,
) -> Option<Retained<NSObject>> {
    if !runtime_shared::app_state::begin_splash_watch() {
        return None;
    }
    let this = mtm.alloc::<IdealystFirstFrameWatch>();
    let this = this.set_ivars(FirstFrameWatchIvars {
        host: unsafe { Retained::retain(host as *const objc2_ui_kit::UIView as *mut objc2_ui_kit::UIView) }
            .expect("a host view reference is non-null"),
        armed: std::cell::Cell::new(false),
        after_report,
    });
    let watch: Retained<IdealystFirstFrameWatch> = unsafe { msg_send_id![super(this), init] };
    let link: Retained<NSObject> = unsafe {
        msg_send_id![objc2::class!(CADisplayLink), displayLinkWithTarget: &*watch, selector: objc2::sel!(tick:)]
    };
    extern "C" {
        static NSRunLoopCommonModes: &'static NSString;
    }
    unsafe {
        let run_loop: *mut AnyObject = msg_send![objc2::class!(NSRunLoop), mainRunLoop];
        let _: () = msg_send![&link, addToRunLoop: run_loop, forMode: NSRunLoopCommonModes];
    }
    Some(link)
}

/// Unregister an observer from [`install`].
pub(crate) fn uninstall(observer: &NSObject) {
    unsafe {
        let center: *mut AnyObject = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
        let _: () = msg_send![center, removeObserver: observer];
    }
}
