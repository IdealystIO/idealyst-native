//! `NSUserDefaults`-backed plaintext store for iOS / macOS / tvOS.
//!
//! Uses the `standardUserDefaults` singleton with a key prefix so several
//! stores coexist and `clear()` only removes its own keys. UserDefaults is
//! a plaintext preferences plist — never put secrets here (see crate docs).
//!
//! We message the singleton through the Obj-C runtime (`objc2`); the
//! shared instance lives for the process lifetime, so we hold a raw
//! pointer without retaining it (same approach the `microphone` SDK uses
//! for `AVAudioSession`).

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSString;
use std::ffi::CStr;
use std::os::raw::c_char;

use crate::{Storage, StorageFuture};

/// A [`Storage`] over `NSUserDefaults`, namespaced by a key prefix.
pub struct UserDefaultsStorage {
    prefix: String,
}

impl UserDefaultsStorage {
    pub fn new(namespace: &str) -> Self {
        Self {
            prefix: format!("{namespace}."),
        }
    }
}

/// `[NSUserDefaults standardUserDefaults]` — a process-wide singleton, so
/// the raw pointer is valid for the program's lifetime without retaining.
unsafe fn standard_defaults() -> *mut AnyObject {
    msg_send![class!(NSUserDefaults), standardUserDefaults]
}

/// Convert an `NSString*` (id) to a Rust `String` via `UTF8String`. `None`
/// for a null pointer (e.g. `stringForKey:` on a missing key).
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

// All ops are synchronous Obj-C messages with no `.await`; the `!Send`
// NSString temporaries never cross a suspension, so the futures are `Send`.
impl Storage for UserDefaultsStorage {
    fn get(&self, key: &str) -> StorageFuture<'_, Option<String>> {
        let full = format!("{}{key}", self.prefix);
        Box::pin(async move {
            unsafe {
                let defaults = standard_defaults();
                let key_ns = NSString::from_str(&full);
                let value: *mut AnyObject = msg_send![defaults, stringForKey: &*key_ns];
                Ok(nsstring_to_string(value))
            }
        })
    }

    fn set(&self, key: &str, value: &str) -> StorageFuture<'_, ()> {
        let full = format!("{}{key}", self.prefix);
        let value = value.to_string();
        Box::pin(async move {
            unsafe {
                let defaults = standard_defaults();
                let key_ns = NSString::from_str(&full);
                let value_ns = NSString::from_str(&value);
                let _: () = msg_send![defaults, setObject: &*value_ns, forKey: &*key_ns];
            }
            Ok(())
        })
    }

    fn remove(&self, key: &str) -> StorageFuture<'_, ()> {
        let full = format!("{}{key}", self.prefix);
        Box::pin(async move {
            unsafe {
                let defaults = standard_defaults();
                let key_ns = NSString::from_str(&full);
                let _: () = msg_send![defaults, removeObjectForKey: &*key_ns];
            }
            Ok(())
        })
    }

    fn clear(&self) -> StorageFuture<'_, ()> {
        let prefix = self.prefix.clone();
        Box::pin(async move {
            unsafe {
                let defaults = standard_defaults();
                // Enumerate every defaults key and remove the ones owned by
                // this store. `dictionaryRepresentation` returns an
                // NSDictionary of all entries; we filter by prefix so we
                // never touch other stores' or the system's keys.
                let dict: *mut AnyObject = msg_send![defaults, dictionaryRepresentation];
                let keys: *mut AnyObject = msg_send![dict, allKeys];
                let count: usize = msg_send![keys, count];
                for i in 0..count {
                    let key_obj: *mut AnyObject = msg_send![keys, objectAtIndex: i];
                    if let Some(k) = nsstring_to_string(key_obj) {
                        if k.starts_with(&prefix) {
                            let _: () = msg_send![defaults, removeObjectForKey: key_obj];
                        }
                    }
                }
            }
            Ok(())
        })
    }
}
