//! The iOS `user_activity()` source (`runtime_shared::user_activity`).
//!
//! A touch reaches a view's `on_touch` only if every view under the finger
//! passes it up — buttons, text fields and scroll views don't — and portals
//! (modals, sheets) are added to the window outside the app's root view.
//! So the source observes at the WINDOW:
//!
//! - **Touches**: one [`IdealystActivityRecognizer`] per app window. UIKit
//!   delivers `touchesBegan:` to every recognizer attached to the hit
//!   view's ancestors before the touch reaches the view, so a recognizer on
//!   the window sees every touch in it — portals, scroll views, buttons and
//!   text fields included. It reports, then puts itself in `Failed` at
//!   once: a failed recognizer takes no further part in the touch, so it
//!   can never delay `touchesEnded:` to the views (`delaysTouchesEnded`
//!   holds touches only while a recognizer is still `Possible`), cancel
//!   them, or block another recognizer. `cancelsTouchesInView` /
//!   `delaysTouchesBegan` / `delaysTouchesEnded` are cleared as well, so
//!   nothing depends on the order UIKit applies them in.
//! - **Windows**: every app window that is already visible at install, then
//!   each one that becomes visible later
//!   (`UIWindowDidBecomeVisibleNotification` — a second scene, the screen
//!   recorder's private layer). UIKit's private keyboard windows are
//!   skipped (`user_activity_policy::observes_window`). Attaching is
//!   idempotent: a window that already carries the recognizer is left as is.
//! - **Typing**: soft-keyboard key taps happen in the keyboard's own
//!   process, so they are observed as text changes —
//!   `UITextFieldTextDidChangeNotification` /
//!   `UITextViewTextDidChangeNotification` with a nil object, i.e. every
//!   field in the app, whoever owns it.
//! - **Hardware keys and iPad mouse buttons / wheel** tick through the app
//!   key and pointer sources (`runtime_shared::key_input` /
//!   `pointer_input` count them) while those are installed; a hardware key
//!   typed into a text field is also a text change.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSObject, NSSet, NSString};
use objc2_ui_kit::{UIEvent, UIGestureRecognizer, UITouch};

extern "C" {
    static UIWindowDidBecomeVisibleNotification: &'static NSString;
    static UITextFieldTextDidChangeNotification: &'static NSString;
    static UITextViewTextDidChangeNotification: &'static NSString;
}

/// `UIGestureRecognizerStateFailed`.
const GESTURE_STATE_FAILED: isize = 5;

/// Tick `user_activity()` and, when it ticked, queue the flush that
/// commits the world signal. Every caller is a raw UIKit callback.
fn report() {
    if runtime_shared::user_activity::report() {
        crate::newcore::schedule_flush();
    }
}

declare_class!(
    pub(crate) struct IdealystActivityRecognizer;

    unsafe impl ClassType for IdealystActivityRecognizer {
        type Super = UIGestureRecognizer;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystActivityRecognizer";
    }

    impl DeclaredClass for IdealystActivityRecognizer {
        type Ivars = ();
    }

    unsafe impl IdealystActivityRecognizer {
        #[method(touchesBegan:withEvent:)]
        fn touches_began(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            crate::imp::ffi_guard::guard_ffi("IdealystActivityRecognizer::touchesBegan", report);
            // UIGestureRecognizerSubclass.h: overrides must call super.
            let _: () = unsafe { msg_send![super(self), touchesBegan: touches, withEvent: event] };
            // Fail at once — see the module docs for why this is what keeps
            // the recognizer from ever affecting the touch.
            let _: () = unsafe { msg_send![self, setState: GESTURE_STATE_FAILED] };
        }
    }
);

impl IdealystActivityRecognizer {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(());
        let nil: *mut AnyObject = std::ptr::null_mut();
        let r: Retained<Self> =
            unsafe { msg_send_id![super(this), initWithTarget: nil, action: None::<objc2::runtime::Sel>] };
        unsafe {
            let _: () = msg_send![&r, setCancelsTouchesInView: false];
            let _: () = msg_send![&r, setDelaysTouchesBegan: false];
            let _: () = msg_send![&r, setDelaysTouchesEnded: false];
        }
        r
    }
}

declare_class!(
    pub(crate) struct IdealystActivityObserver;

    unsafe impl ClassType for IdealystActivityObserver {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystActivityObserver";
    }

    impl DeclaredClass for IdealystActivityObserver {
        type Ivars = ();
    }

    unsafe impl NSObjectProtocol for IdealystActivityObserver {}

    unsafe impl IdealystActivityObserver {
        #[method(windowDidBecomeVisible:)]
        fn window_did_become_visible(&self, note: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystActivityObserver::windowDidBecomeVisible", || {
                let window: *mut AnyObject = unsafe { msg_send![note, object] };
                attach(window);
            });
        }

        #[method(textDidChange:)]
        fn text_did_change(&self, _note: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystActivityObserver::textDidChange", report);
        }
    }
);

/// Give `window` the activity recognizer unless it is a UIKit-private
/// window or already has one.
fn attach(window: *mut AnyObject) {
    if window.is_null() {
        return;
    }
    let class_name = unsafe { (*window).class().name() };
    if !crate::user_activity_policy::observes_window(class_name) {
        return;
    }
    let recognizer_class = IdealystActivityRecognizer::class();
    unsafe {
        let existing: *mut AnyObject = msg_send![window, gestureRecognizers];
        if !existing.is_null() {
            let n: usize = msg_send![existing, count];
            for i in 0..n {
                let r: *mut AnyObject = msg_send![existing, objectAtIndex: i];
                let ours: bool = msg_send![r, isKindOfClass: recognizer_class];
                if ours {
                    return;
                }
            }
        }
    }
    let Some(mtm) = MainThreadMarker::new() else { return };
    let recognizer = IdealystActivityRecognizer::new(mtm);
    // The window retains its recognizers.
    let _: () = unsafe { msg_send![window, addGestureRecognizer: &*recognizer] };
}

/// Every window of every connected `UIWindowScene`.
fn attach_to_existing_windows() {
    unsafe {
        let app: *mut AnyObject = msg_send![objc2::class!(UIApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let scenes: *mut AnyObject = msg_send![app, connectedScenes];
        if scenes.is_null() {
            return;
        }
        let all: *mut AnyObject = msg_send![scenes, allObjects];
        let n: usize = msg_send![all, count];
        let window_scene_class = objc2::class!(UIWindowScene);
        for i in 0..n {
            let scene: *mut AnyObject = msg_send![all, objectAtIndex: i];
            let is_window_scene: bool = msg_send![scene, isKindOfClass: window_scene_class];
            if !is_window_scene {
                continue;
            }
            let windows: *mut AnyObject = msg_send![scene, windows];
            let wn: usize = msg_send![windows, count];
            for j in 0..wn {
                attach(msg_send![windows, objectAtIndex: j]);
            }
        }
    }
}

thread_local! {
    /// The live notification observer; replaced (and the old one
    /// unregistered) by a re-boot's [`install`].
    static OBSERVER: std::cell::RefCell<Option<Retained<IdealystActivityObserver>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the source: recognizers on the current windows, and the
/// observer for later windows and text changes. Called once per boot by
/// `newcore::start_with`; a second call replaces the observer, and
/// recognizers already attached stay (it finds them and adds none).
pub(crate) fn install(mtm: MainThreadMarker) {
    attach_to_existing_windows();
    let this = mtm.alloc::<IdealystActivityObserver>().set_ivars(());
    let observer: Retained<IdealystActivityObserver> = unsafe { msg_send_id![super(this), init] };
    unsafe {
        let center: *mut AnyObject = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let add = |sel: objc2::runtime::Sel, name: &NSString| {
            let _: () = msg_send![center, addObserver: &*observer, selector: sel, name: name, object: nil];
        };
        add(objc2::sel!(windowDidBecomeVisible:), UIWindowDidBecomeVisibleNotification);
        add(objc2::sel!(textDidChange:), UITextFieldTextDidChangeNotification);
        add(objc2::sel!(textDidChange:), UITextViewTextDidChangeNotification);
    }
    if let Some(old) = OBSERVER.with(|o| o.borrow_mut().replace(observer)) {
        unsafe {
            let center: *mut AnyObject = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![center, removeObserver: &*old];
        }
    }
}
