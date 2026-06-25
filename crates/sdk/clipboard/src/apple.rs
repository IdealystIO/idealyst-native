//! Apple clipboard backend — iOS `UIPasteboard`, macOS `NSPasteboard`.
//!
//! Both are reached through the Obj-C runtime (`objc2` `msg_send`), no
//! typed framework crate. The general pasteboard is a process-wide
//! singleton, so we hold a raw pointer to it without retaining (the same
//! approach the `storage` SDK uses for `NSUserDefaults`).
//!
//! - **iOS / tvOS**: `[UIPasteboard generalPasteboard]`, read `.string`
//!   (`string` selector → `NSString*`), write `setString:`.
//! - **macOS**: `[NSPasteboard generalPasteboard]`, read
//!   `stringForType:NSPasteboardTypeString`, write by `clearContents`
//!   then `setString:forType:NSPasteboardTypeString`. macOS requires the
//!   clear+declare dance because a pasteboard write replaces *all*
//!   representations and must first reset the change count.
//!
//! Compile-checked only ⚠️ — not hardware-verified on an Apple device.

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSString;
use std::ffi::CStr;
use std::os::raw::c_char;

use crate::ClipboardError;

/// Convert an `NSString*` (id) to a Rust `String` via `UTF8String`.
/// `None` for a null pointer (e.g. an empty pasteboard) — matching the
/// crate's "no text → `None`" contract.
unsafe fn nsstring_to_string(s: *mut AnyObject) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let utf8: *const c_char = msg_send![s, UTF8String];
    if utf8.is_null() {
        return None;
    }
    Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
}

// --- iOS / tvOS: UIPasteboard --------------------------------------------

#[cfg(any(target_os = "ios", target_os = "tvos"))]
pub(crate) async fn set_text(text: &str) -> Result<(), ClipboardError> {
    let text = text.to_string();
    unsafe {
        let pasteboard: *mut AnyObject =
            msg_send![class!(UIPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            return Err(ClipboardError::Backend(
                "UIPasteboard generalPasteboard was nil".into(),
            ));
        }
        let value = NSString::from_str(&text);
        let _: () = msg_send![pasteboard, setString: &*value];
    }
    Ok(())
}

#[cfg(any(target_os = "ios", target_os = "tvos"))]
pub(crate) async fn text() -> Result<Option<String>, ClipboardError> {
    unsafe {
        let pasteboard: *mut AnyObject =
            msg_send![class!(UIPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            return Err(ClipboardError::Backend(
                "UIPasteboard generalPasteboard was nil".into(),
            ));
        }
        // `string` returns nil when there's no string representation.
        let value: *mut AnyObject = msg_send![pasteboard, string];
        Ok(nsstring_to_string(value))
    }
}

// --- macOS: NSPasteboard --------------------------------------------------

/// `NSPasteboardTypeString` — the UTI for plain-text pasteboard items
/// (`"public.utf8-plain-text"`). We pass the type as an `NSString` to the
/// `forType:` selectors. Using the literal UTI avoids linking the
/// `NSPasteboardTypeString` symbol.
#[cfg(target_os = "macos")]
const NS_PASTEBOARD_TYPE_STRING: &str = "public.utf8-plain-text";

#[cfg(target_os = "macos")]
pub(crate) async fn set_text(text: &str) -> Result<(), ClipboardError> {
    let text = text.to_string();
    unsafe {
        let pasteboard: *mut AnyObject =
            msg_send![class!(NSPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            return Err(ClipboardError::Backend(
                "NSPasteboard generalPasteboard was nil".into(),
            ));
        }
        // A write must first reset the pasteboard (bumps the change count
        // and drops the prior owner's representations) or `setString:` is
        // a no-op. This is the documented NSPasteboard contract.
        let _: i64 = msg_send![pasteboard, clearContents];
        let value = NSString::from_str(&text);
        let ty = NSString::from_str(NS_PASTEBOARD_TYPE_STRING);
        let ok: bool = msg_send![pasteboard, setString: &*value, forType: &*ty];
        if !ok {
            return Err(ClipboardError::Backend(
                "NSPasteboard setString:forType: returned NO".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) async fn text() -> Result<Option<String>, ClipboardError> {
    unsafe {
        let pasteboard: *mut AnyObject =
            msg_send![class!(NSPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            return Err(ClipboardError::Backend(
                "NSPasteboard generalPasteboard was nil".into(),
            ));
        }
        let ty = NSString::from_str(NS_PASTEBOARD_TYPE_STRING);
        // `stringForType:` returns nil when there's no string of that type.
        let value: *mut AnyObject = msg_send![pasteboard, stringForType: &*ty];
        Ok(nsstring_to_string(value))
    }
}
