//! iOS: `UIApplication.isIdleTimerDisabled`. App-wide, honoured only while
//! the app is frontmost, and main-thread only, which the guard's thread is.

use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;

pub(crate) fn set(on: bool) {
    let Some(class) = AnyClass::get("UIApplication") else { return };
    unsafe {
        let app: *mut AnyObject = msg_send![class, sharedApplication];
        if !app.is_null() {
            let _: () = msg_send![app, setIdleTimerDisabled: on];
        }
    }
}

pub(crate) fn is_supported() -> bool {
    true
}
