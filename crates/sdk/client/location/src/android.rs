//! Android geolocation via the framework `android.location.LocationManager`.
//!
//! The framework `LocationManager` rather than Google Play Services'
//! `FusedLocationProviderClient`, so this SDK pulls in **no Play Services
//! dependency** and works on de-Googled devices.
//!
//! The work is in the Kotlin shim (`runtime/kotlin/io/idealyst/location/`):
//! both a fresh fix and a stream of them arrive through a `LocationListener`,
//! a Java interface pure JNI cannot implement. This file is the transport:
//! a call parks a sender under a token, and the shim answers on the main
//! thread through the exports below.
//!
//! - [`current_fix`] asks the best provider that is on for a fix newer than
//!   the call, and after [`FIX_TIMEOUT_MS`] settles for the newest one the
//!   device already has. Dropping the future stops the request.
//! - [`start_watch`] streams updates (one a second at most) until its handle
//!   is dropped.
//!
//! Everything runs on the application context, so neither needs an Activity:
//! an agent kept alive in the background by a foreground service reads its
//! position this way (it needs `ACCESS_BACKGROUND_LOCATION`, see the README).
//!
//! The permission GRANT is the `permissions` SDK's job — `crate::current`
//! awaited it before reaching here. We only read position data.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jdouble, jlong};
use jni::{JNIEnv, JavaVM};

use crate::android_fix::{self, RawFix};
use crate::oneshot;
use crate::{BoxedCallback, LocationError, Position};

const HELPER: &str = "io.idealyst.location.RustLocation";

/// How long [`current_fix`] waits for a fresh fix: 30 s, like the other
/// backends.
const FIX_TIMEOUT_MS: i64 = 30_000;

type FixSender = oneshot::Sender<Result<Position, LocationError>>;

/// Requests waiting for their fix, by token.
static PENDING: Mutex<Option<HashMap<u64, FixSender>>> = Mutex::new(None);
/// Live watches, by id.
static WATCHES: Mutex<Option<HashMap<u64, Arc<Mutex<BoxedCallback>>>>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn unavailable(what: &str, e: impl std::fmt::Display) -> LocationError {
    LocationError::Unavailable(format!("{what}: {e}"))
}

fn java_vm() -> Result<JavaVM, LocationError> {
    let ctx = ndk_context::android_context();
    unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(|e| unavailable("no JavaVM", e))
}

/// `RustLocation`, resolved through the Context's class loader and cached:
/// `find_class` on a thread the JVM did not start resolves against the system
/// class loader, which cannot see app classes.
static HELPER_CLASS: OnceLock<GlobalRef> = OnceLock::new();

fn helper_class(env: &mut JNIEnv<'_>, ctx: &JObject<'_>) -> Result<&'static GlobalRef, LocationError> {
    if let Some(c) = HELPER_CLASS.get() {
        return Ok(c);
    }
    let class = (|| -> jni::errors::Result<GlobalRef> {
        let loader = env.call_method(ctx, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
        let name = env.new_string(HELPER)?;
        let class = env
            .call_method(
                &loader,
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        env.new_global_ref(class)
    })();
    let class = class.map_err(|e| {
        let _ = env.exception_clear();
        unavailable(&format!("{HELPER} is not in the APK; was the app built by the idealyst CLI?"), e)
    })?;
    Ok(HELPER_CLASS.get_or_init(|| class))
}

/// Call one of the shim's static methods.
fn call<R>(
    f: impl FnOnce(&mut JNIEnv<'_>, &JClass<'_>, &JObject<'_>) -> jni::errors::Result<R>,
) -> Result<R, LocationError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(|e| unavailable("JNI attach", e))?;
    let ptr = ndk_context::android_context().context();
    if ptr.is_null() {
        return Err(LocationError::Unavailable("no Android context (the app has not attached yet)".into()));
    }
    // SAFETY: `ndk_context` holds a process-lifetime global ref to the
    // Context; this is a non-owning wrapper that never deletes it.
    let ctx = unsafe { JObject::from_raw(ptr.cast()) };
    let class = helper_class(&mut env, &ctx)?;
    let class: &JClass = class.as_obj().into();
    f(&mut env, class, &ctx).map_err(|e| {
        // Left pending, the exception would surface in whatever runs next.
        let _ = env.exception_clear();
        unavailable("RustLocation", e)
    })
}

/// A parked request. Dropped (answered, or its future was), it stops.
struct Request(u64);

impl Drop for Request {
    fn drop(&mut self) {
        if PENDING.lock().unwrap().as_mut().and_then(|p| p.remove(&self.0)).is_none() {
            return; // answered
        }
        let token = self.0 as jlong;
        let _ = call(|env, class, _| {
            env.call_static_method(class, "cancel", "(J)V", &[JValue::Long(token)]).map(drop)
        });
    }
}

/// Whether Location is off for the device.
pub(crate) fn services_off() -> bool {
    call(|env, class, ctx| {
        env.call_static_method(
            class,
            "servicesEnabled",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(ctx)],
        )?
        .z()
    })
    .is_ok_and(|on| !on)
}

pub(crate) async fn current_fix() -> Result<Position, LocationError> {
    let token = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel(Err(LocationError::Unavailable("the location request was dropped".into())));
    PENDING.lock().unwrap().get_or_insert_with(HashMap::new).insert(token, tx);
    let _request = Request(token);
    let code = call(|env, class, ctx| {
        env.call_static_method(
            class,
            "current",
            "(Landroid/content/Context;JJ)I",
            &[JValue::Object(ctx), JValue::Long(token as jlong), JValue::Long(FIX_TIMEOUT_MS)],
        )?
        .i()
    })?;
    android_fix::started(code)?;
    rx.await
}

/// Stops updates on drop.
pub(crate) struct WatchHandle {
    /// `None` when the watch could not start.
    id: Option<u64>,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        let Some(id) = self.id else { return };
        if let Some(watches) = WATCHES.lock().unwrap().as_mut() {
            watches.remove(&id);
        }
        let _ = call(|env, class, _| {
            env.call_static_method(class, "stopWatch", "(J)V", &[JValue::Long(id as jlong)]).map(drop)
        });
    }
}

pub(crate) fn start_watch(callback: BoxedCallback) -> WatchHandle {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    // Before the shim starts: its first update must find the callback.
    WATCHES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(id, Arc::new(Mutex::new(callback)));
    let started = call(|env, class, ctx| {
        env.call_static_method(
            class,
            "startWatch",
            "(Landroid/content/Context;J)I",
            &[JValue::Object(ctx), JValue::Long(id as jlong)],
        )?
        .i()
    })
    .and_then(android_fix::started);
    match started {
        Ok(()) => WatchHandle { id: Some(id) },
        Err(e) => {
            // `watch` is synchronous and has no error to return.
            log::warn!("location::watch: {e}");
            if let Some(watches) = WATCHES.lock().unwrap().as_mut() {
                watches.remove(&id);
            }
            WatchHandle { id: None }
        }
    }
}

/// Run an export's body: never unwind into the JVM.
fn guard(what: &str, f: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
        log::error!("location: panic in {what}; aborting");
        std::process::abort();
    }
}

fn answer(token: jlong, result: Result<Position, LocationError>) {
    // Taken out first: waking the future may run it here.
    let tx = PENDING.lock().unwrap().as_mut().and_then(|p| p.remove(&(token as u64)));
    if let Some(tx) = tx {
        tx.send(result);
    }
}

/// `RustLocation.nativeOnFix`, on the main thread.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_location_RustLocation_nativeOnFix(
    _env: JNIEnv,
    _class: JClass,
    token: jlong,
    latitude: jdouble,
    longitude: jdouble,
    accuracy: jdouble,
    altitude: jdouble,
    heading: jdouble,
    speed: jdouble,
    time_ms: jlong,
) {
    guard("nativeOnFix", || {
        let fix = RawFix { latitude, longitude, accuracy, altitude, heading, speed, time_ms };
        answer(token, Ok(fix.into()));
    });
}

/// `RustLocation.nativeOnNoFix`, on the main thread.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_location_RustLocation_nativeOnNoFix(
    mut env: JNIEnv,
    _class: JClass,
    token: jlong,
    reason: JString,
) {
    guard("nativeOnNoFix", || {
        let reason = env.get_string(&reason).map(String::from).unwrap_or_default();
        answer(token, Err(LocationError::Unavailable(reason)));
    });
}

/// `RustLocation.nativeOnUpdate`, on the main thread.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_location_RustLocation_nativeOnUpdate(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
    latitude: jdouble,
    longitude: jdouble,
    accuracy: jdouble,
    altitude: jdouble,
    heading: jdouble,
    speed: jdouble,
    time_ms: jlong,
) {
    guard("nativeOnUpdate", || {
        // Cloned out: the callback may drop its own watch.
        let callback = WATCHES.lock().unwrap().as_ref().and_then(|w| w.get(&(id as u64)).cloned());
        let Some(callback) = callback else { return };
        let fix = RawFix { latitude, longitude, accuracy, altitude, heading, speed, time_ms };
        (callback.lock().unwrap())(fix.into());
    });
}

type FixExport = extern "system" fn(
    JNIEnv,
    JClass,
    jlong,
    jdouble,
    jdouble,
    jdouble,
    jdouble,
    jdouble,
    jdouble,
    jlong,
);

/// Pin the exports in the app `cdylib`'s dynamic symbol table: the JVM
/// resolves them by `dlsym`, and a dependency rlib's `#[no_mangle]` export
/// that nothing in Rust references can be dropped by the linker.
#[used]
static KEEP_ON_FIX: FixExport = Java_io_idealyst_location_RustLocation_nativeOnFix;
#[used]
static KEEP_ON_UPDATE: FixExport = Java_io_idealyst_location_RustLocation_nativeOnUpdate;
#[used]
static KEEP_ON_NO_FIX: extern "system" fn(JNIEnv, JClass, jlong, JString) =
    Java_io_idealyst_location_RustLocation_nativeOnNoFix;
