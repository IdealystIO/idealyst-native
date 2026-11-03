//! Every `KeyboardKind` on a real `UITextField`, plain and secure.
//!
//! REGRESSION (CrewForge sign-in): a field with any non-`Default` keyboard
//! aborted the app as it mounted — "invalid message send to
//! -[IdealystTextField setKeyboardType:]: method not found". On iOS 26
//! `UITextField` forwards its `UITextInputTraits` setters to an internal
//! traits object, and objc2's debug-build check finds no method on the
//! field's class. The backend now sends each trait to the receiver the
//! runtime itself would use.
//!
//! The traits are read back with a raw `objc_msgSend` — real dispatch,
//! forwarding included, without objc2's check — so the test sees what
//! UIKit holds, not what the backend meant to send.
//!
//! `harness = false`: UIKit is main-thread only, and libtest runs cases on
//! worker threads. The body only exists on iOS; build for the simulator and
//! run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test keyboard_kinds --no-run
//! xcrun simctl spawn booted target/aarch64-apple-ios-sim/debug/deps/keyboard_kinds-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::rc::Rc;

    use backend_ios::{IosBackend, IosNode};
    use objc2::runtime::{AnyObject, Sel};
    use objc2::sel;
    use objc2_foundation::{MainThreadMarker, NSString};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::primitives::text_input::KeyboardKind;
    use runtime_vocabulary::caps::TextInputOps;

    /// `-[obj sel]` returning an `NSInteger`/`BOOL`-sized value, through the
    /// runtime's own dispatch (forwarding included).
    fn get_int(obj: &AnyObject, sel: Sel) -> isize {
        // SAFETY: every getter read here takes no arguments and returns a
        // word-sized integer (`NSInteger` enums; `BOOL` widened).
        unsafe {
            let send: unsafe extern "C" fn(*const AnyObject, Sel) -> isize =
                std::mem::transmute(objc2::ffi::objc_msgSend as *const ());
            send(obj, sel)
        }
    }

    fn get_bool(obj: &AnyObject, sel: Sel) -> bool {
        // SAFETY: as `get_int`, for a `BOOL` getter.
        unsafe {
            let send: unsafe extern "C" fn(*const AnyObject, Sel) -> bool =
                std::mem::transmute(objc2::ffi::objc_msgSend as *const ());
            send(obj, sel)
        }
    }

    fn content_type(obj: &AnyObject) -> Option<String> {
        // SAFETY: `textContentType` returns an `NSString *` or nil.
        unsafe {
            let send: unsafe extern "C" fn(*const AnyObject, Sel) -> *const NSString =
                std::mem::transmute(objc2::ffi::objc_msgSend as *const ());
            send(obj, sel!(textContentType)).as_ref().map(|s| s.to_string())
        }
    }

    fn field_of(node: &IosNode) -> &AnyObject {
        match node {
            IosNode::TextField(f) => f,
            _ => panic!("a text input mounts a UITextField"),
        }
    }

    /// `(kind, UIKeyboardType, textContentType, autocapitalization)`:
    /// UIKit's raw values (`UIKeyboardType`, `UITextAutocapitalizationType`).
    const KINDS: [(KeyboardKind, isize, Option<&str>, isize); 6] = [
        (KeyboardKind::Default, 0, None, 2),
        (KeyboardKind::Email, 7, Some("email"), 0),
        (KeyboardKind::Phone, 5, Some("tel"), 2),
        (KeyboardKind::Number, 4, None, 2),
        (KeyboardKind::Decimal, 8, None, 2),
        (KeyboardKind::Url, 3, Some("url"), 0),
    ];

    fn every_kind_mounts_plain_and_secure(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let a11y = AccessibilityProps::default();
        for secure in [false, true] {
            for (kind, keyboard, content, autocap) in KINDS {
                let node = backend.create_text_input("", Some("hint"), Rc::new(|_| {}), None, None, secure, &a11y);
                backend.update_text_input_keyboard(&node, kind);
                let f = field_of(&node);
                assert_eq!(get_int(f, sel!(keyboardType)), keyboard, "{kind:?} keyboardType");
                assert_eq!(get_int(f, sel!(autocapitalizationType)), autocap, "{kind:?} autocapitalization");
                assert_eq!(content_type(f).as_deref(), content, "{kind:?} textContentType");
                assert_eq!(get_bool(f, sel!(isSecureTextEntry)), secure, "{kind:?} keeps secure = {secure}");
            }
        }
    }

    /// A live kind and a live mask change in place, in either order, and a
    /// kind going back to `Default` restores every trait.
    fn kind_and_mask_change_live(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let node = backend.create_text_input("", None, Rc::new(|_| {}), None, None, false, &AccessibilityProps::default());
        let f = field_of(&node);
        backend.update_text_input_keyboard(&node, KeyboardKind::Email);
        backend.update_text_input_secure(&node, true);
        assert_eq!((get_int(f, sel!(keyboardType)), get_bool(f, sel!(isSecureTextEntry))), (7, true));
        backend.update_text_input_keyboard(&node, KeyboardKind::Default);
        assert_eq!(get_int(f, sel!(keyboardType)), 0);
        assert_eq!(get_int(f, sel!(autocapitalizationType)), 2);
        assert_eq!(content_type(f), None);
        assert!(get_bool(f, sel!(isSecureTextEntry)), "the mask survives a kind change");
        backend.update_text_input_secure(&node, false);
        assert!(!get_bool(f, sel!(isSecureTextEntry)));
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        every_kind_mounts_plain_and_secure(mtm);
        kind_and_mask_change_live(mtm);
        println!("keyboard_kinds: 2 passed");
    }
}
