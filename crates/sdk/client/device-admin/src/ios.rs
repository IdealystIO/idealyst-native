//! iOS: Autonomous Single App Mode, Guided Access state, and the MDM's
//! managed app configuration.
//!
//! iOS has no device-owner app; a supervised device is managed by its MDM
//! server. What an app itself can do is ask to lock itself
//! (`UIAccessibilityRequestGuidedAccessSession`, granted only when the MDM
//! profile allow-lists the app for Autonomous Single App Mode) and exchange
//! a dictionary with the MDM through `NSUserDefaults`.

use std::ffi::{c_void, CString};

use block2::RcBlock;
use futures_channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, msg_send_id};
use serde_json::Value;

use crate::{BoxFuture, Error, LockTaskState, Result};

const MANAGED_CONFIG_KEY: &str = "com.apple.configuration.managed";
const MANAGED_FEEDBACK_KEY: &str = "com.apple.feedback.managed";

#[link(name = "UIKit", kind = "framework")]
extern "C" {
    fn UIAccessibilityIsGuidedAccessEnabled() -> Bool;
    fn UIAccessibilityRequestGuidedAccessSession(
        enable: Bool,
        completion: &block2::Block<dyn Fn(Bool)>,
    );
}

/// Request (or end) Single App Mode. Resolves with whether iOS granted it;
/// `false` on an unsupervised device or one whose profile does not
/// allow-list the app. Call from the main thread.
pub(crate) fn set_single_app_mode(enable: bool) -> BoxFuture<bool> {
    let (tx, rx) = oneshot::channel::<bool>();
    let tx = std::cell::Cell::new(Some(tx));
    let block = RcBlock::new(move |granted: Bool| {
        if let Some(tx) = tx.take() {
            let _ = tx.send(granted.as_bool());
        }
    });
    // SAFETY: a UIKit C function taking a BOOL and a completion block, which
    // UIKit copies before returning.
    unsafe { UIAccessibilityRequestGuidedAccessSession(Bool::new(enable), &block) };
    Box::pin(async move {
        rx.await
            .map_err(|_| Error::Platform("Single App Mode request never completed".into()))
    })
}

pub(crate) fn lock_task_state() -> LockTaskState {
    // SAFETY: a UIKit C predicate with no arguments.
    if unsafe { UIAccessibilityIsGuidedAccessEnabled() }.as_bool() {
        LockTaskState::Locked
    } else {
        LockTaskState::None
    }
}

fn ns_string(s: &str) -> Result<Retained<AnyObject>> {
    let c = CString::new(s).map_err(|_| Error::Platform(format!("{s:?} contains a NUL")))?;
    // SAFETY: `stringWithUTF8String:` copies the NUL-terminated buffer.
    let string: Option<Retained<AnyObject>> =
        unsafe { msg_send_id![class!(NSString), stringWithUTF8String: c.as_ptr()] };
    string.ok_or_else(|| Error::Platform(format!("NSString from {s:?}")))
}

fn standard_defaults() -> Retained<AnyObject> {
    // SAFETY: `+standardUserDefaults` returns the process-wide instance.
    let defaults: Retained<AnyObject> =
        unsafe { msg_send_id![class!(NSUserDefaults), standardUserDefaults] };
    defaults
}

/// `NSJSONSerialization` round trip: Foundation object → JSON bytes.
fn to_json(object: &AnyObject) -> Result<Value> {
    // SAFETY: `dataWithJSONObject:options:error:` with a nil error out
    // pointer returns nil on failure; `bytes`/`length` read the NSData.
    unsafe {
        let data: Option<Retained<AnyObject>> = msg_send_id![
            class!(NSJSONSerialization),
            dataWithJSONObject: object,
            options: 0usize,
            error: std::ptr::null_mut::<*mut AnyObject>()
        ];
        let data = data.ok_or_else(|| {
            Error::Platform("the managed configuration is not JSON-representable".into())
        })?;
        let bytes: *const c_void = msg_send![&*data, bytes];
        let len: usize = msg_send![&*data, length];
        let slice = if len == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(bytes.cast::<u8>(), len)
        };
        serde_json::from_slice(slice).map_err(|e| Error::Platform(format!("managed configuration: {e}")))
    }
}

/// JSON → a Foundation object via `NSJSONSerialization`.
fn from_json(value: &Value) -> Result<Retained<AnyObject>> {
    let bytes = serde_json::to_vec(value).map_err(|e| Error::Platform(e.to_string()))?;
    // SAFETY: `dataWithBytes:length:` copies the buffer;
    // `JSONObjectWithData:options:error:` returns nil on failure.
    unsafe {
        let data: Retained<AnyObject> = msg_send_id![
            class!(NSData),
            dataWithBytes: bytes.as_ptr().cast::<c_void>(),
            length: bytes.len()
        ];
        let object: Option<Retained<AnyObject>> = msg_send_id![
            class!(NSJSONSerialization),
            JSONObjectWithData: &*data,
            options: 0usize,
            error: std::ptr::null_mut::<*mut AnyObject>()
        ];
        object.ok_or_else(|| Error::Platform("could not convert the feedback to a dictionary".into()))
    }
}

pub(crate) fn managed_config() -> Result<Option<Value>> {
    let key = ns_string(MANAGED_CONFIG_KEY)?;
    let defaults = standard_defaults();
    // SAFETY: `dictionaryForKey:` returns nil when absent or not a dictionary.
    let dict: Option<Retained<AnyObject>> =
        unsafe { msg_send_id![&*defaults, dictionaryForKey: &*key] };
    dict.map(|d| to_json(&d)).transpose()
}

pub(crate) fn set_managed_feedback(feedback: &Value) -> Result<()> {
    if !feedback.is_object() {
        return Err(Error::Platform("managed feedback must be a JSON object".into()));
    }
    let object = from_json(feedback)?;
    let key = ns_string(MANAGED_FEEDBACK_KEY)?;
    let defaults = standard_defaults();
    // SAFETY: `setObject:forKey:` with a property-list dictionary and a key.
    unsafe {
        let _: () = msg_send![&*defaults, setObject: &*object, forKey: &*key];
    }
    Ok(())
}
