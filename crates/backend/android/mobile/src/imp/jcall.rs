//! Instance-method JNI calls with the method id resolved once per call site.
//!
//! `JNIEnv::call_method(obj, name, sig, args)` looks the method up on every
//! call (`GetObjectClass` + `GetMethodID`): 1.5–7 µs on an emulator, against
//! 0.1–0.2 µs for a call through a cached id. The style and mount paths make
//! a dozen or more such calls per view, which was most of their cost.
//!
//! [`jcall!`] keeps `call_method`'s contract (same arguments, same
//! `jni::errors::Result<JValueOwned>`, a Java exception left pending exactly
//! as `call_method` leaves it) and caches the id in a thread-local at each
//! call site, keyed by the class that DECLARES the method. A method id from
//! a superclass is valid on every subclass instance, so the receiver may be
//! any subclass of `$class`; a receiver that is not one returns an error
//! instead of calling through.

use jni::objects::{GlobalRef, JMethodID, JObject, JValueOwned};
use jni::signature::{Primitive, ReturnType};
use jni::sys::jvalue;
use jni::JNIEnv;
use std::cell::RefCell;

/// A resolved instance method: the declaring class pinned by a global
/// reference (which keeps the id valid) and the id, with the return type
/// read off the signature.
pub(crate) struct CachedMethod {
    _class: GlobalRef,
    id: JMethodID,
    ret: ReturnType,
}

/// The `ReturnType` a JNI method signature's return descriptor names.
fn return_type(sig: &str) -> ReturnType {
    match sig.rsplit(')').next().and_then(|r| r.chars().next()) {
        Some('V') => ReturnType::Primitive(Primitive::Void),
        Some('Z') => ReturnType::Primitive(Primitive::Boolean),
        Some('B') => ReturnType::Primitive(Primitive::Byte),
        Some('C') => ReturnType::Primitive(Primitive::Char),
        Some('S') => ReturnType::Primitive(Primitive::Short),
        Some('I') => ReturnType::Primitive(Primitive::Int),
        Some('J') => ReturnType::Primitive(Primitive::Long),
        Some('F') => ReturnType::Primitive(Primitive::Float),
        Some('D') => ReturnType::Primitive(Primitive::Double),
        Some('[') => ReturnType::Array,
        _ => ReturnType::Object,
    }
}

impl CachedMethod {
    /// Call `class.name(sig)` on `obj`, resolving it into `slot` first. A
    /// failed resolve (the method is newer than the running API level)
    /// leaves the `NoSuchMethodError` pending and caches nothing, as a
    /// by-name call would.
    ///
    /// # Safety
    /// `args` must match `sig`, and `obj` must be an instance of `class`.
    pub(crate) unsafe fn call<'local, 'o>(
        slot: &RefCell<Option<CachedMethod>>,
        env: &mut JNIEnv<'local>,
        obj: impl AsRef<JObject<'o>>,
        class: &str,
        name: &str,
        sig: &str,
        args: &[jvalue],
    ) -> jni::errors::Result<JValueOwned<'local>> {
        if slot.borrow().is_none() {
            let local = env.find_class(class)?;
            let id = env.get_method_id(&local, name, sig)?;
            let class = env.new_global_ref(&local)?;
            let _ = env.delete_local_ref(local);
            *slot.borrow_mut() = Some(CachedMethod { _class: class, id, ret: return_type(sig) });
        }
        let (class_ref, id, ret) = {
            let m = slot.borrow();
            let m = m.as_ref().expect("resolved above");
            (m._class.clone(), m.id, m.ret.clone())
        };
        // A by-name call on a receiver that lacks the method fails with
        // `NoSuchMethodError`; a cached id on such a receiver is undefined
        // behaviour. So check the receiver (one cheap JNI call) and fail
        // the same way rather than call through.
        let class_obj = <&jni::objects::JClass>::from(class_ref.as_obj());
        if !env.is_instance_of(obj.as_ref(), class_obj)? {
            return Err(jni::errors::Error::MethodNotFound {
                name: name.to_owned(),
                sig: sig.to_owned(),
            });
        }
        env.call_method_unchecked(obj.as_ref(), id, ret, args)
    }
}

/// `env.call_method(obj, name, sig, &[args])` through a per-call-site cached
/// method id. `$class` is the class that declares the method (a superclass
/// of the receiver is fine). Arguments are `JValue`s, as for `call_method`.
///
/// ```ignore
/// let _ = jcall!(env, &view, "android/view/View", "setAlpha", "(F)V", [JValue::Float(a)]);
/// ```
macro_rules! jcall {
    ($env:expr, $obj:expr, $class:literal, $name:literal, $sig:literal, [$($arg:expr),* $(,)?]) => {{
        runtime_tls::thread_local! {
            static SLOT: std::cell::RefCell<Option<$crate::imp::jcall::CachedMethod>> =
                std::cell::RefCell::new(None);
        }
        SLOT.with(|slot| {
            // SAFETY: each call site pairs `$sig` with arguments of its
            // shape, and a receiver of `$class`, as `call_method` requires.
            unsafe {
                $crate::imp::jcall::CachedMethod::call(
                    slot,
                    $env,
                    $obj,
                    $class,
                    $name,
                    $sig,
                    &[$(($arg).as_jni()),*],
                )
            }
        })
    }};
}

/// [`jcall!`] for a method `RustLabel` shares with `TextView` (same name and
/// signature): on `RustLabel` when `$is_label`, else on `TextView`. The text
/// branch of style application serves both — a `text` node is a
/// `RustLabel`, a `button` / `text_input` a `TextView`.
macro_rules! text_call {
    ($env:expr, $obj:expr, $is_label:expr, $name:literal, $sig:literal, [$($arg:expr),* $(,)?]) => {{
        if $is_label {
            jcall!($env, $obj, "io/idealyst/runtime/RustLabel", $name, $sig, [$($arg),*])
        } else {
            jcall!($env, $obj, "android/widget/TextView", $name, $sig, [$($arg),*])
        }
    }};
}
