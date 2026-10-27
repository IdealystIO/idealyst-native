//! The JNI transport to `io.idealyst.deviceadmin.RustDeviceAdmin`.
//!
//! Every call is `RustDeviceAdmin.call(context, method, argsJson, token)`,
//! one static method with one signature, returning a JSON reply (see
//! [`crate::reply`]). The policy logic lives in Kotlin, next to the
//! `DeviceAdminReceiver` and activities it needs — which pure JNI cannot
//! define — so the Rust side is only this transport and the typed surface
//! in `lib.rs`.
//!
//! Asynchronous calls (lock task, installs) reply `{"pending": true}` and
//! later call [`nativeResult`] with the token they were given; events arrive
//! through [`nativeEvent`], which hands each to the async executor. Both
//! exports run on the main thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use futures_channel::oneshot;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::jlong;
use jni::{JNIEnv, JavaVM};
use serde_json::Value;

use crate::reply::{self, Reply};
use crate::{BoxFuture, Error, Result};

const HELPER: &str = "io.idealyst.deviceadmin.RustDeviceAdmin";

type Pending = HashMap<u64, oneshot::Sender<Result<Value>>>;

/// In-flight asynchronous calls: token → the sender awaiting the result. A
/// process-global because [`nativeResult`] is a free function with no handle
/// back to the caller.
static PENDING: OnceLock<Mutex<Pending>> = OnceLock::new();
/// Token 0 means "no token" to the Kotlin side.
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn pending() -> &'static Mutex<Pending> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

fn jni_err(e: jni::errors::Error) -> Error {
    Error::Platform(format!("JNI: {e}"))
}

fn java_vm() -> Result<JavaVM> {
    let ctx = ndk_context::android_context();
    unsafe { JavaVM::from_raw(ctx.vm().cast()) }
        .map_err(|e| Error::Platform(format!("no JavaVM (the app has not attached yet?): {e}")))
}

fn android_context<'a>() -> Result<JObject<'a>> {
    let ptr = ndk_context::android_context().context();
    if ptr.is_null() {
        return Err(Error::Platform("no Android context (the app has not attached yet)".into()));
    }
    // SAFETY: `ndk_context` holds a process-lifetime global ref to the
    // Context; this is a non-owning wrapper that never deletes it.
    Ok(unsafe { JObject::from_raw(ptr.cast()) })
}

/// `RustDeviceAdmin`, resolved through the Context's class loader and cached.
/// `find_class` on a thread the JVM did not start (an executor thread)
/// resolves against the system class loader, which cannot see app classes.
static HELPER_CLASS: OnceLock<GlobalRef> = OnceLock::new();

fn helper_class(env: &mut JNIEnv<'_>, ctx: &JObject<'_>) -> Result<&'static GlobalRef> {
    if let Some(c) = HELPER_CLASS.get() {
        return Ok(c);
    }
    let loader = env
        .call_method(ctx, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(|v| v.l())
        .map_err(jni_err)?;
    let name = env.new_string(HELPER).map_err(jni_err)?;
    let class = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )
        .and_then(|v| v.l());
    let class = match class {
        Ok(c) => c,
        Err(e) => {
            let _ = env.exception_clear();
            return Err(Error::Platform(format!(
                "{HELPER} is not in the APK ({e}); was the app built by the idealyst CLI?"
            )));
        }
    };
    let global = env.new_global_ref(class).map_err(jni_err)?;
    let _ = HELPER_CLASS.set(global);
    Ok(HELPER_CLASS.get().expect("just set"))
}

/// Invoke `RustDeviceAdmin.call` and return its raw JSON reply.
fn invoke(method: &str, args: &Value, token: u64) -> Result<String> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let ctx = android_context()?;
    let class = helper_class(&mut env, &ctx)?;
    let method_j = env.new_string(method).map_err(jni_err)?;
    let args_j = env.new_string(args.to_string()).map_err(jni_err)?;
    let class_obj: &JClass = class.as_obj().into();
    let reply = env
        .call_static_method(
            class_obj,
            "call",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;J)Ljava/lang/String;",
            &[
                JValue::Object(&ctx),
                JValue::Object(&method_j),
                JValue::Object(&args_j),
                JValue::Long(token as jlong),
            ],
        )
        .and_then(|v| v.l());
    let reply = match reply {
        Ok(r) => r,
        Err(e) => {
            // `call` catches every Throwable itself; an exception here is a
            // JNI-level failure (signature mismatch), cleared so the thread
            // stays usable.
            let _ = env.exception_clear();
            return Err(jni_err(e));
        }
    };
    let reply = JString::from(reply);
    let text: String = env.get_string(&reply).map_err(jni_err)?.into();
    Ok(text)
}

/// A synchronous call.
pub(crate) fn call(method: &str, args: &Value) -> Result<Value> {
    match reply::parse(&invoke(method, args, 0)?)? {
        Reply::Ok(v) => Ok(v),
        Reply::Pending => Err(Error::Platform(format!(
            "{method} replied pending to a synchronous call"
        ))),
    }
}

/// A call whose result arrives through [`nativeResult`].
pub(crate) fn call_async(method: &str, args: &Value) -> BoxFuture<Value> {
    let (tx, rx) = oneshot::channel();
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    pending().lock().unwrap().insert(token, tx);
    let immediate = invoke(method, args, token).and_then(|r| reply::parse(&r));
    match immediate {
        Ok(Reply::Pending) => {}
        // An immediate answer (an error, or a value) never reaches
        // `nativeResult`; resolve the parked sender here.
        other => {
            if let Some(tx) = pending().lock().unwrap().remove(&token) {
                let _ = tx.send(other.map(|r| match r {
                    Reply::Ok(v) => v,
                    Reply::Pending => unreachable!("matched above"),
                }));
            }
        }
    }
    Box::pin(async move {
        rx.await.unwrap_or_else(|_| {
            Err(Error::Platform("device-admin result channel dropped".into()))
        })
    })
}

/// `RustDeviceAdmin.nativeResult(token, json)`: an asynchronous call's
/// result, a reply in the same shape `call` returns.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_deviceadmin_RustDeviceAdmin_nativeResult(
    mut env: JNIEnv,
    _class: JClass,
    token: jlong,
    json: JString,
) {
    // FFI boundary: never unwind into the JVM.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(text) = env.get_string(&json).map(String::from) else {
            return;
        };
        let result = reply::parse(&text).and_then(|r| match r {
            Reply::Ok(v) => Ok(v),
            Reply::Pending => Err(Error::Platform("a result cannot be pending".into())),
        });
        if let Some(tx) = pending().lock().unwrap().remove(&(token as u64)) {
            let _ = tx.send(result);
        }
    }));
    if outcome.is_err() {
        eprintln!("device_admin: panic in nativeResult; aborting");
        std::process::abort();
    }
}

/// `RustDeviceAdmin.nativeEvent(json)`: an OS event, on the main thread.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_deviceadmin_RustDeviceAdmin_nativeEvent(
    mut env: JNIEnv,
    _class: JClass,
    json: JString,
) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Ok(text) = env.get_string(&json).map(String::from) {
            // Through the executor, not called directly: the backend fires
            // its flush after each executor poll and not after an SDK's own
            // JNI callback, so a listener's staged signal writes would
            // otherwise wait for an unrelated tick to commit.
            runtime_shared::driver::spawn_async(async move { crate::events::dispatch(&text) });
        }
    }));
    if outcome.is_err() {
        eprintln!("device_admin: panic in an event listener; aborting");
        std::process::abort();
    }
}

/// Pin both exports in the app `cdylib`'s dynamic symbol table: the JVM
/// resolves them by `dlsym`, and a dependency rlib's `#[no_mangle]` export
/// that nothing in Rust references can be dropped by the linker.
#[used]
static KEEP_NATIVE_RESULT: extern "system" fn(JNIEnv, JClass, jlong, JString) =
    Java_io_idealyst_deviceadmin_RustDeviceAdmin_nativeResult;
#[used]
static KEEP_NATIVE_EVENT: extern "system" fn(JNIEnv, JClass, JString) =
    Java_io_idealyst_deviceadmin_RustDeviceAdmin_nativeEvent;
