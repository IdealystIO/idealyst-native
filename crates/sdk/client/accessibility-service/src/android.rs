//! The JNI transport to `io.idealyst.accessibilityservice.RustAccessibility`.
//!
//! Every call is `RustAccessibility.call(context, method, argsJson, token)`,
//! one static method with one signature, returning a JSON reply (see
//! [`crate::reply`]). The service itself must be a JVM class the system
//! binds by name, so the logic lives in Kotlin and the Rust side is this
//! transport plus the typed surface in `lib.rs`.
//!
//! Screenshots, gestures, searches and clicks reply `{"pending": true}` and
//! later report under the token they were given: a value or a failure
//! through [`nativeResult`] (on the main thread or the SDK's search
//! thread), a screenshot's pixels through [`nativeScreenshot`] (on the
//! SDK's screenshot thread), as a `byte[]` rather than JSON. Enabled-state
//! changes arrive through [`nativeEnabled`], window changes through
//! [`nativeWindowEvent`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use futures_channel::oneshot;
use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong};
use jni::{JNIEnv, JavaVM};
use serde_json::Value;

use crate::reply::{self, Reply};
use crate::{policy, BoxFuture, Error, ImageFormat, Result, Screenshot};

const HELPER: &str = "io.idealyst.accessibilityservice.RustAccessibility";

/// What a pending call resolves with.
enum Outcome {
    Value(Value),
    Screenshot {
        width: u32,
        height: u32,
        rotation: i32,
        data: Vec<u8>,
    },
}

type Pending = HashMap<u64, oneshot::Sender<Result<Outcome>>>;

/// In-flight asynchronous calls: token → the sender awaiting the result. A
/// process-global because the `native*` exports are free functions with no
/// handle back to the caller.
static PENDING: OnceLock<Mutex<Pending>> = OnceLock::new();
/// Token 0 means "no token" to the Kotlin side.
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
/// When the last screenshot request was accepted, for the rate gate.
static LAST_SCREENSHOT: Mutex<Option<Instant>> = Mutex::new(None);

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

/// `RustAccessibility`, resolved through the Context's class loader and
/// cached. `find_class` on a thread the JVM did not start (an executor or
/// socket thread) resolves against the system class loader, which cannot
/// see app classes.
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

/// Invoke `RustAccessibility.call` and return its raw JSON reply.
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

/// Start a call whose result arrives under a token. `Ok(receiver)` when the
/// platform accepted it (replied pending); an immediate reply — an error, or
/// a value — is returned as such.
fn start(method: &str, args: &Value) -> Result<oneshot::Receiver<Result<Outcome>>> {
    let (tx, rx) = oneshot::channel();
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    pending().lock().unwrap().insert(token, tx);
    let immediate = invoke(method, args, token).and_then(|r| reply::parse(&r));
    match immediate {
        Ok(Reply::Pending) => Ok(rx),
        other => {
            pending().lock().unwrap().remove(&token);
            match other {
                Ok(Reply::Ok(v)) => {
                    let (tx, rx) = oneshot::channel();
                    let _ = tx.send(Ok(Outcome::Value(v)));
                    Ok(rx)
                }
                Err(e) => Err(e),
                Ok(Reply::Pending) => unreachable!("matched above"),
            }
        }
    }
}

async fn finish(rx: oneshot::Receiver<Result<Outcome>>) -> Result<Outcome> {
    rx.await
        .unwrap_or_else(|_| Err(Error::Platform("accessibility-service result channel dropped".into())))
}

/// A call whose result arrives through [`nativeResult`].
pub(crate) fn call_async(method: &str, args: &Value) -> BoxFuture<Value> {
    let started = start(method, args);
    Box::pin(async move {
        match finish(started?).await? {
            Outcome::Value(v) => Ok(v),
            Outcome::Screenshot { .. } => Err(Error::Platform(
                "a screenshot arrived for a call that is not one".into(),
            )),
        }
    })
}

/// [`crate::screenshot`] on Android: the rate gate, then the call. The gate's
/// clock starts only when the platform accepts the request, as Android's
/// does, so a refused call (not enabled) does not delay the next one.
pub(crate) fn screenshot(format: ImageFormat) -> BoxFuture<Screenshot> {
    let started = (|| {
        let mut last = LAST_SCREENSHOT.lock().unwrap();
        let now = Instant::now();
        if let Some(retry_after_ms) =
            policy::screenshot_retry_after(last.map(|t| now.duration_since(t)))
        {
            return Err(Error::TooSoon { retry_after_ms });
        }
        let args = match format {
            ImageFormat::Rgba => serde_json::json!({ "format": "rgba" }),
            ImageFormat::Jpeg { quality } => {
                serde_json::json!({ "format": "jpeg", "quality": quality })
            }
        };
        let rx = start("screenshot", &args)?;
        *last = Some(now);
        Ok(rx)
    })();
    Box::pin(async move {
        match finish(started?).await? {
            Outcome::Screenshot {
                width,
                height,
                rotation,
                data,
            } => Ok(Screenshot {
                width,
                height,
                rotation: reply::rotation(rotation)?,
                format,
                data,
            }),
            Outcome::Value(_) => Err(Error::Platform("screenshot replied without pixels".into())),
        }
    })
}

fn resolve(token: jlong, outcome: Result<Outcome>) {
    if let Some(tx) = pending().lock().unwrap().remove(&(token as u64)) {
        let _ = tx.send(outcome);
    }
}

/// `RustAccessibility.nativeResult(token, json)`: an asynchronous call's
/// result, a reply in the same shape `call` returns.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_accessibilityservice_RustAccessibility_nativeResult(
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
            Reply::Ok(v) => Ok(Outcome::Value(v)),
            Reply::Pending => Err(Error::Platform("a result cannot be pending".into())),
        });
        resolve(token, result);
    }));
    if outcome.is_err() {
        eprintln!("accessibility_service: panic in nativeResult; aborting");
        std::process::abort();
    }
}

/// `RustAccessibility.nativeScreenshot(token, width, height, rotation,
/// data)`: a screenshot's pixels, on the SDK's background thread.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_accessibilityservice_RustAccessibility_nativeScreenshot(
    env: JNIEnv,
    _class: JClass,
    token: jlong,
    width: jint,
    height: jint,
    rotation: jint,
    data: JByteArray,
) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = env
            .convert_byte_array(&data)
            .map_err(jni_err)
            .map(|data| Outcome::Screenshot {
                width: width.max(0) as u32,
                height: height.max(0) as u32,
                rotation,
                data,
            });
        resolve(token, result);
    }));
    if outcome.is_err() {
        eprintln!("accessibility_service: panic in nativeScreenshot; aborting");
        std::process::abort();
    }
}

/// `RustAccessibility.nativeEnabled(enabled)`: the service bound or
/// unbound, on the main thread, reported only while a listener is
/// registered.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_accessibilityservice_RustAccessibility_nativeEnabled(
    _env: JNIEnv,
    _class: JClass,
    enabled: jboolean,
) {
    let outcome = std::panic::catch_unwind(|| {
        let enabled = enabled != 0;
        // Through the executor, not called directly: the backend fires its
        // flush after each executor poll and not after an SDK's own JNI
        // callback, so a listener's staged signal writes would otherwise
        // wait for an unrelated tick to commit.
        runtime_shared::driver::spawn_detached(async move { crate::enabled::dispatch(enabled) });
    });
    if outcome.is_err() {
        eprintln!("accessibility_service: panic in nativeEnabled; aborting");
        std::process::abort();
    }
}

/// `RustAccessibility.nativeWindowEvent(json)`: a window change, on the
/// main thread, reported only while a window listener is registered.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_accessibilityservice_RustAccessibility_nativeWindowEvent(
    mut env: JNIEnv,
    _class: JClass,
    json: JString,
) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(text) = env.get_string(&json).map(String::from) else {
            return;
        };
        let event = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| crate::WindowEvent::from_json(&v));
        let Some(event) = event else {
            log::warn!("accessibility_service: unreadable window event {text}");
            return;
        };
        // Through the executor, as `nativeEnabled`: a listener's staged
        // signal writes commit at the poll's flush.
        runtime_shared::driver::spawn_detached(async move { crate::windows::dispatch(event) });
    }));
    if outcome.is_err() {
        eprintln!("accessibility_service: panic in nativeWindowEvent; aborting");
        std::process::abort();
    }
}

/// Pin the exports in the app `cdylib`'s dynamic symbol table: the JVM
/// resolves them by `dlsym`, and a dependency rlib's `#[no_mangle]` export
/// that nothing in Rust references can be dropped by the linker.
#[used]
static KEEP_NATIVE_RESULT: extern "system" fn(JNIEnv, JClass, jlong, JString) =
    Java_io_idealyst_accessibilityservice_RustAccessibility_nativeResult;
#[used]
static KEEP_NATIVE_SCREENSHOT: extern "system" fn(JNIEnv, JClass, jlong, jint, jint, jint, JByteArray) =
    Java_io_idealyst_accessibilityservice_RustAccessibility_nativeScreenshot;
#[used]
static KEEP_NATIVE_ENABLED: extern "system" fn(JNIEnv, JClass, jboolean) =
    Java_io_idealyst_accessibilityservice_RustAccessibility_nativeEnabled;
#[used]
static KEEP_NATIVE_WINDOW_EVENT: extern "system" fn(JNIEnv, JClass, JString) =
    Java_io_idealyst_accessibilityservice_RustAccessibility_nativeWindowEvent;
