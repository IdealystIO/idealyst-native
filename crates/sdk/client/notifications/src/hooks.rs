//! Per-platform hooks: the escape hatch for what the shared
//! [`Notification`](crate::Notification) fields do not cover.
//!
//! Each platform's notification can do things the others cannot — an
//! Android progress bar or messaging style, an iOS relevance score or
//! summary argument, a Windows toast progress bar or hero image layout, a
//! web vibration pattern, a Linux daemon-specific hint. A hook receives the
//! platform's own object after the SDK has filled in the shared fields and
//! before the notification is posted, and may change anything on it.
//!
//! Each setter exists only on its own target (`#[cfg]`-gated), so code that
//! uses one is per-target by construction. The hook types are the platform
//! bindings' own (objc2, jni, windows, zbus, serde_json), so whatever those
//! bindings can express, a hook can.

use std::rc::Rc;

#[cfg(any(target_os = "ios", target_os = "macos", target_os = "tvos"))]
pub(crate) type AppleHook = Rc<dyn Fn(&objc2::runtime::AnyObject)>;

#[cfg(target_os = "android")]
pub(crate) type AndroidHook =
    Rc<dyn for<'a> Fn(&mut jni::JNIEnv<'a>, &jni::objects::JObject<'a>) -> jni::errors::Result<()>>;

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
pub(crate) type WindowsHook = Rc<dyn Fn(&windows::Data::Xml::Dom::XmlDocument) -> windows::core::Result<()>>;

#[cfg(target_arch = "wasm32")]
pub(crate) type WebHook =
    Rc<dyn Fn(&mut runtime_core::__serde_json::Map<String, runtime_core::__serde_json::Value>)>;

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
pub(crate) type LinuxHook =
    Rc<dyn Fn(&mut std::collections::HashMap<String, zbus::zvariant::Value<'static>>)>;

/// The hooks set on one notification. Only the current target's field
/// exists.
#[derive(Clone, Default)]
pub(crate) struct PlatformHooks {
    #[cfg(any(target_os = "ios", target_os = "macos", target_os = "tvos"))]
    pub(crate) apple: Option<AppleHook>,
    #[cfg(target_os = "android")]
    pub(crate) android: Option<AndroidHook>,
    #[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
    pub(crate) windows: Option<WindowsHook>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) web: Option<WebHook>,
    #[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
    pub(crate) linux: Option<LinuxHook>,
}

impl PlatformHooks {
    /// Whether a hook is set for the current target.
    pub(crate) fn is_set(&self) -> bool {
        #[allow(unused_mut)]
        let mut set = false;
        #[cfg(any(target_os = "ios", target_os = "macos", target_os = "tvos"))]
        {
            set |= self.apple.is_some();
        }
        #[cfg(target_os = "android")]
        {
            set |= self.android.is_some();
        }
        #[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
        {
            set |= self.windows.is_some();
        }
        #[cfg(target_arch = "wasm32")]
        {
            set |= self.web.is_some();
        }
        #[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
        {
            set |= self.linux.is_some();
        }
        set
    }
}

/// Two hook sets are equal when they hold the same hook closures (pointer
/// identity): a closure has no value to compare.
impl PartialEq for PlatformHooks {
    fn eq(&self, _other: &Self) -> bool {
        #[allow(unused_mut)]
        let mut eq = true;
        #[cfg(any(target_os = "ios", target_os = "macos", target_os = "tvos"))]
        {
            eq &= same(&self.apple, &_other.apple);
        }
        #[cfg(target_os = "android")]
        {
            eq &= same(&self.android, &_other.android);
        }
        #[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
        {
            eq &= same(&self.windows, &_other.windows);
        }
        #[cfg(target_arch = "wasm32")]
        {
            eq &= same(&self.web, &_other.web);
        }
        #[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
        {
            eq &= same(&self.linux, &_other.linux);
        }
        eq
    }
}

#[allow(dead_code)]
fn same<T: ?Sized>(a: &Option<Rc<T>>, b: &Option<Rc<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

impl std::fmt::Debug for PlatformHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_set() { "PlatformHooks(set)" } else { "PlatformHooks(none)" })
    }
}
