//! `view` / `pressable` `on_focus` (`InputOps::install_focus_handler`) on iOS.
//!
//! Keyboard focus on iOS is first responder on a text input: a pressable is
//! not a focus target here (Tab moves among text inputs — see
//! `focus_traversal` — and UIKit moves among buttons only under the
//! system's Full Keyboard Access, which an app does not observe as
//! first responder). So a view's `on_focus` hears the text inputs inside
//! it, and a pressable's never fires.
//!
//! Driven by UIKit's editing notifications (`UITextField…DidBegin/
//! DidEndEditing`, `UITextView…`), posted for every text input — the
//! framework's and one a third-party view embeds — without touching their
//! classes. Each installed handler keeps its view WEAKLY and is pruned once
//! the view is gone; on each notification every live handler whose view
//! contains the input reports. A move between two inputs of one wrapper
//! reports `false` then `true`; the vocabulary dedups to transitions.
//!
//! Delivery is deferred one main-queue turn in a reactive `cycle`: UIKit
//! posts these from inside `becomeFirstResponder`, which a reactive flush
//! can trigger (autofocus, a programmatic `focus()`), and the author's
//! handler writes signals.

use std::cell::RefCell;

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::UIView;
use runtime_shared::FocusWithinHandler;

extern "C" {
    static UITextFieldTextDidBeginEditingNotification: &'static NSString;
    static UITextFieldTextDidEndEditingNotification: &'static NSString;
    static UITextViewTextDidBeginEditingNotification: &'static NSString;
    static UITextViewTextDidEndEditingNotification: &'static NSString;
}

declare_class!(
    pub(crate) struct IdealystFocusWithinObserver;

    unsafe impl ClassType for IdealystFocusWithinObserver {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystFocusWithinObserver";
    }

    impl DeclaredClass for IdealystFocusWithinObserver {
        type Ivars = ();
    }

    unsafe impl IdealystFocusWithinObserver {
        #[method(didBeginEditing:)]
        fn did_begin_editing(&self, note: &NSObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystFocusWithinObserver::didBeginEditing", || {
                report(note, true)
            });
        }

        #[method(didEndEditing:)]
        fn did_end_editing(&self, note: &NSObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystFocusWithinObserver::didEndEditing", || {
                report(note, false)
            });
        }
    }
);

thread_local! {
    /// Installed handlers with their (weak) views.
    static HANDLERS: RefCell<Vec<(Weak<UIView>, FocusWithinHandler)>> = const { RefCell::new(Vec::new()) };
    /// The notification observer, made on the first install. The center
    /// does not retain it; this does, for the process lifetime.
    static OBSERVER: RefCell<Option<Retained<IdealystFocusWithinObserver>>> = const { RefCell::new(None) };
}

/// Register `handler` for focus entering / leaving `view`'s subtree.
pub(crate) fn install(view: &UIView, handler: FocusWithinHandler) {
    ensure_observer();
    HANDLERS.with(|h| h.borrow_mut().push((Weak::from(view), handler)));
}

fn ensure_observer() {
    if OBSERVER.with(|o| o.borrow().is_some()) {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else { return };
    let this = mtm.alloc::<IdealystFocusWithinObserver>().set_ivars(());
    let observer: Retained<IdealystFocusWithinObserver> = unsafe { msg_send_id![super(this), init] };
    unsafe {
        let center: *mut AnyObject = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let add = |sel: objc2::runtime::Sel, name: &NSString| {
            let _: () = msg_send![center, addObserver: &*observer, selector: sel, name: name, object: nil];
        };
        add(objc2::sel!(didBeginEditing:), UITextFieldTextDidBeginEditingNotification);
        add(objc2::sel!(didBeginEditing:), UITextViewTextDidBeginEditingNotification);
        add(objc2::sel!(didEndEditing:), UITextFieldTextDidEndEditingNotification);
        add(objc2::sel!(didEndEditing:), UITextViewTextDidEndEditingNotification);
    }
    OBSERVER.with(|o| *o.borrow_mut() = Some(observer));
}

/// A text input (the notification's object) began / ended editing: every
/// live handler whose view contains it reports, deferred (module docs).
fn report(note: &NSObject, focused: bool) {
    let input: *mut AnyObject = unsafe { msg_send![note, object] };
    if input.is_null() {
        return;
    }
    let input = input as *const UIView;
    let hit: Vec<FocusWithinHandler> = HANDLERS.with(|h| {
        let mut h = h.borrow_mut();
        h.retain(|(v, _)| v.load().is_some());
        h.iter()
            .filter_map(|(v, f)| {
                let v = v.load()?;
                let inside: bool = unsafe { msg_send![input, isDescendantOfView: &*v] };
                inside.then(|| f.clone())
            })
            .collect()
    });
    if hit.is_empty() {
        return;
    }
    runtime_shared::after_ms_detached(0, move || {
        runtime_shared::cycle(|| {
            for f in &hit {
                f(focused);
            }
        });
    });
}

impl super::IosBackend {
    /// `InputOps::install_focus_handler` (module docs).
    pub(crate) fn install_focus_handler_impl(&mut self, node: &super::IosNode, handler: FocusWithinHandler) {
        install(node.as_view(), handler);
    }
}
