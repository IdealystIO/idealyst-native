//! NSObject subclasses that bridge AppKit `target/action` selectors
//! to Rust closures. Mirrors the iOS backend's
//! `callbacks.rs` shape — different subclass per parameter shape so
//! each method's signature matches what the AppKit sender expects.
//!
//! Each control's `setTarget:` / `setAction:` keeps the target as a
//! **weak** reference. The owning [`MacosBackend`] stashes the
//! `Retained<...>` in `callback_targets` for the backend's lifetime
//! so the target survives long enough for every dispatched action
//! to find a live receiver.

use objc2::rc::Retained;
use objc2::runtime::{NSObject as NSObjectRuntime, NSObjectProtocol};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_app_kit::NSControl;
use objc2_foundation::{MainThreadMarker, NSObject};
use std::cell::RefCell;
use std::rc::Rc;

// =========================================================================
// CallbackTarget — fires a `Fn()` from `-(IBAction)invoke:(id)sender`.
// Used for NSButton clicks.
// =========================================================================

pub(crate) struct CallbackTargetIvars {
    pub(crate) callback: RefCell<Option<Rc<dyn Fn()>>>,
}

declare_class!(
    pub(crate) struct CallbackTarget;

    unsafe impl ClassType for CallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystCallbackTarget";
    }

    impl DeclaredClass for CallbackTarget {
        type Ivars = CallbackTargetIvars;
    }

    unsafe impl NSObjectProtocol for CallbackTarget {}

    unsafe impl CallbackTarget {
        #[method(invoke:)]
        fn invoke(&self, _sender: &NSObjectRuntime) {
            if let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() {
                cb();
            }
        }
    }
);

impl CallbackTarget {
    pub(crate) fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn()>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(CallbackTargetIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// BoolCallbackTarget — fires a `Fn(bool)` from an NSSwitch / NSButton
// in checkbox mode. Reads the sender's `state` (NSControlStateValueOn
// = 1, NSControlStateValueOff = 0) and forwards.
// =========================================================================

pub(crate) struct BoolCallbackTargetIvars {
    pub(crate) callback: RefCell<Option<Rc<dyn Fn(bool)>>>,
}

declare_class!(
    pub(crate) struct BoolCallbackTarget;

    unsafe impl ClassType for BoolCallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystBoolCallbackTarget";
    }

    impl DeclaredClass for BoolCallbackTarget {
        type Ivars = BoolCallbackTargetIvars;
    }

    unsafe impl NSObjectProtocol for BoolCallbackTarget {}

    unsafe impl BoolCallbackTarget {
        #[method(invoke:)]
        fn invoke(&self, sender: &NSObjectRuntime) {
            if let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() {
                // `state` is an NSInteger; 1 = on, 0 = off. NSSwitch
                // uses 1/0 directly; NSButton in checkbox/toggle style
                // uses NSControlStateValueOn/Off which are also 1/0.
                let state: isize = unsafe { msg_send![sender, state] };
                cb(state == 1);
            }
        }
    }
);

impl BoolCallbackTarget {
    pub(crate) fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn(bool)>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(BoolCallbackTargetIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// FloatCallbackTarget — fires a `Fn(f32)` from an NSSlider / NSControl
// whose `value` is a `double`. Reads `doubleValue` and forwards as f32.
// =========================================================================

pub(crate) struct FloatCallbackTargetIvars {
    pub(crate) callback: RefCell<Option<Rc<dyn Fn(f32)>>>,
}

declare_class!(
    pub(crate) struct FloatCallbackTarget;

    unsafe impl ClassType for FloatCallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystFloatCallbackTarget";
    }

    impl DeclaredClass for FloatCallbackTarget {
        type Ivars = FloatCallbackTargetIvars;
    }

    unsafe impl NSObjectProtocol for FloatCallbackTarget {}

    unsafe impl FloatCallbackTarget {
        #[method(invoke:)]
        fn invoke(&self, sender: &NSObjectRuntime) {
            if let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() {
                let value: f64 = unsafe { msg_send![sender, doubleValue] };
                cb(value as f32);
            }
        }
    }
);

impl FloatCallbackTarget {
    pub(crate) fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn(f32)>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(FloatCallbackTargetIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// StringCallbackTarget — fires a `Fn(String)` from an NSTextField's
// `controlTextDidChange:` notification (NotificationCenter-driven,
// not target/action).
//
// The receiver method is `controlTextDidChange:` — NSTextField fires
// this on every keystroke when its delegate or notification observer
// implements it. We register the same selector against
// NSNotificationCenter via `addObserver:selector:name:object:`.
// =========================================================================

pub(crate) struct StringCallbackTargetIvars {
    pub(crate) callback: RefCell<Option<Rc<dyn Fn(String)>>>,
}

declare_class!(
    pub(crate) struct StringCallbackTarget;

    unsafe impl ClassType for StringCallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystStringCallbackTarget";
    }

    impl DeclaredClass for StringCallbackTarget {
        type Ivars = StringCallbackTargetIvars;
    }

    unsafe impl NSObjectProtocol for StringCallbackTarget {}

    unsafe impl StringCallbackTarget {
        #[method(controlTextDidChange:)]
        fn control_text_did_change(&self, notification: &NSObjectRuntime) {
            if let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() {
                // `notification.object` is the NSTextField; ask it for
                // its `stringValue`. NSTextField is an NSControl.
                let sender: *mut NSControl = unsafe { msg_send![notification, object] };
                if sender.is_null() {
                    return;
                }
                let ns: *mut objc2_foundation::NSString =
                    unsafe { msg_send![sender, stringValue] };
                if ns.is_null() {
                    return;
                }
                let ns_ref: &objc2_foundation::NSString = unsafe { &*ns };
                cb(ns_ref.to_string());
            }
        }

        /// `NSTextDidChangeNotification` — fires for `NSTextView`
        /// (multi-line editor backing our `TextArea` primitive)
        /// on every edit. NSTextView is NOT an NSControl, so we
        /// can't reach its content through `stringValue`; the
        /// canonical accessor is `string` (returns `NSString*`).
        /// Same callback Rc carries the value back to the
        /// framework.
        #[method(textDidChange:)]
        fn text_did_change(&self, notification: &NSObjectRuntime) {
            if let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() {
                let sender: *mut NSObjectRuntime =
                    unsafe { msg_send![notification, object] };
                if sender.is_null() {
                    return;
                }
                let ns: *mut objc2_foundation::NSString =
                    unsafe { msg_send![sender, string] };
                if ns.is_null() {
                    return;
                }
                let ns_ref: &objc2_foundation::NSString = unsafe { &*ns };
                cb(ns_ref.to_string());
            }
        }
    }
);

impl StringCallbackTarget {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        callback: Rc<dyn Fn(String)>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(StringCallbackTargetIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// ScrollObserverTarget — bridges `NSViewBoundsDidChangeNotification`
// (fired on an NSScrollView's clipView whenever the user scrolls) to
// a Rust `Fn(f32, f32)` closure.
//
// NSScrollView doesn't expose a delegate \u{2014} the canonical macOS
// pattern is to flip the contentView's `postsBoundsChangedNotifications`
// on, then observe `NSView.boundsDidChangeNotification` keyed on that
// clipView. The observer reads the clipView's `documentVisibleRect`
// origin to recover the current scroll offset.
// =========================================================================

pub(crate) struct ScrollObserverTargetIvars {
    pub(crate) callback: RefCell<Option<Rc<dyn Fn(f32, f32)>>>,
}

declare_class!(
    pub(crate) struct ScrollObserverTarget;

    unsafe impl ClassType for ScrollObserverTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystScrollObserverTarget";
    }

    impl DeclaredClass for ScrollObserverTarget {
        type Ivars = ScrollObserverTargetIvars;
    }

    unsafe impl NSObjectProtocol for ScrollObserverTarget {}

    unsafe impl ScrollObserverTarget {
        #[method(boundsDidChange:)]
        fn bounds_did_change(&self, notification: &NSObjectRuntime) {
            let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() else {
                return;
            };
            // `notification.object` is the NSClipView whose bounds
            // changed. Its `bounds.origin` is the current scroll
            // offset in the clip view's coordinate space \u{2014}
            // identical units to web/iOS (CSS pixels / points).
            let clip: *mut objc2::runtime::AnyObject =
                unsafe { msg_send![notification, object] };
            if clip.is_null() {
                return;
            }
            let bounds: objc2_foundation::CGRect = unsafe { msg_send![clip, bounds] };
            cb(bounds.origin.x as f32, bounds.origin.y as f32);
        }
    }
);

impl ScrollObserverTarget {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        callback: Rc<dyn Fn(f32, f32)>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(ScrollObserverTargetIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}
