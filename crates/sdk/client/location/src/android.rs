//! Android geolocation via the framework `android.location.LocationManager`.
//!
//! We use the framework `LocationManager` (`Context.getSystemService(
//! LOCATION_SERVICE)`) rather than Google Play Services' `FusedLocation
//! ProviderClient`, so this SDK pulls in **no Play Services dependency** and
//! works on de-Googled devices.
//!
//! - [`current_fix`] reads `getLastKnownLocation(provider)` — a synchronous,
//!   cached position that's a fast, dependency-free one-shot. This path is
//!   implemented fully over JNI: pick the best available provider, read the
//!   `Location`, and map its getters into a [`Position`].
//! - [`start_watch`] needs `requestLocationUpdates(provider, minTime,
//!   minDist, listener)`, and the `listener` is an `android.location.
//!   LocationListener` — a Java **interface** the framework calls back. Pure
//!   JNI cannot subclass / implement a Java interface at runtime without a
//!   compiled stub `.dex` or a `Proxy`, so the listener is a **host-shim
//!   seam**: the app's Android host must supply a small `LocationListener`
//!   that forwards `onLocationChanged(Location)` back to this crate. Until
//!   that shim lands, `start_watch` installs nothing and the callback never
//!   fires — documented honestly, not faked. (`getLastKnownLocation`, the
//!   common "where am I now" case, is fully functional.)
//!
//! The permission GRANT is the `permissions` SDK's job — `crate::current`
//! awaited `permissions::request(LocationWhenInUse)` (→ `ACCESS_FINE_LOCATION`
//! `checkSelfPermission`) before reaching here. We only read position data.
//!
//! VERIFICATION: compile-checked only — exercising it needs a device/emulator
//! with the `ACCESS_FINE_LOCATION` / `ACCESS_COARSE_LOCATION` manifest entries
//! and a granted runtime permission. The JNI structure mirrors the verified
//! `net` / `microphone` Android backends.

use jni::objects::{JObject, JValue};
use jni::JavaVM;

use crate::{BoxedCallback, LocationError, Position};

/// `Context.LOCATION_SERVICE`.
const LOCATION_SERVICE: &str = "location";
/// `LocationManager.GPS_PROVIDER` — most accurate when a fix is fresh.
const GPS_PROVIDER: &str = "gps";
/// `LocationManager.NETWORK_PROVIDER` — coarse, but usually has a cached fix.
const NETWORK_PROVIDER: &str = "network";

/// Attach to the host `JavaVM`. A bad pointer is a bootstrap bug surfaced as
/// [`LocationError::Unavailable`].
fn java_vm() -> Result<JavaVM, LocationError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| LocationError::Unavailable(format!("invalid JavaVM pointer: {e}")))
}

pub(crate) async fn current_fix() -> Result<Position, LocationError> {
    // All JNI work happens on the calling thread, attached to the VM. Reads
    // are synchronous (`getLastKnownLocation` returns immediately), so no
    // worker thread is needed — unlike the microphone's blocking read loop.
    let vm = java_vm()?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| LocationError::Unavailable(format!("JNI attach failed: {e}")))?;

    // The host Activity/Context, from ndk_context.
    let ctx = ndk_context::android_context();
    let context = unsafe { JObject::from_raw(ctx.context() as jni::sys::jobject) };

    // LocationManager lm = (LocationManager) context.getSystemService("location");
    let service_name = env
        .new_string(LOCATION_SERVICE)
        .map_err(|e| LocationError::Unavailable(format!("new_string: {e}")))?;
    let manager = env
        .call_method(
            &context,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[JValue::Object(&service_name)],
        )
        .and_then(|v| v.l())
        .map_err(|e| LocationError::Unavailable(format!("getSystemService: {e}")))?;
    if manager.is_null() {
        return Err(LocationError::Unavailable(
            "LOCATION_SERVICE unavailable".into(),
        ));
    }

    // Try GPS first (most accurate), then network (most likely to have a
    // cached fix). The first provider with a non-null last-known location wins.
    for provider in [GPS_PROVIDER, NETWORK_PROVIDER] {
        let provider_str = env
            .new_string(provider)
            .map_err(|e| LocationError::Unavailable(format!("new_string: {e}")))?;
        // `getLastKnownLocation` throws SecurityException without permission;
        // the grant is the caller's responsibility (via `permissions`), so a
        // thrown exception here means "not authorized".
        let location = match env.call_method(
            &manager,
            "getLastKnownLocation",
            "(Ljava/lang/String;)Landroid/location/Location;",
            &[JValue::Object(&provider_str)],
        ) {
            Ok(v) => v.l().unwrap_or(JObject::null()),
            Err(_) => {
                // Clear the pending Java exception so the next call is clean.
                let _ = env.exception_clear();
                return Err(LocationError::NotAuthorized);
            }
        };
        if location.is_null() {
            continue;
        }
        return read_location(&mut env, &location);
    }

    Err(LocationError::Unavailable(
        "no last-known location from any provider".into(),
    ))
}

/// Map an `android.location.Location` to a [`Position`] via its getters.
///
/// `getAltitude` / `getBearing` / `getSpeed` always return a value, but a
/// `has*()` companion reports whether it's meaningful — we gate on those so an
/// absent field maps to `None` rather than a spurious `0.0`.
fn read_location(env: &mut jni::JNIEnv, location: &JObject) -> Result<Position, LocationError> {
    let f64_getter = |env: &mut jni::JNIEnv, name: &str| -> Result<f64, LocationError> {
        env.call_method(location, name, "()D", &[])
            .and_then(|v| v.d())
            .map_err(|e| LocationError::Unavailable(format!("{name}: {e}")))
    };
    let f32_getter = |env: &mut jni::JNIEnv, name: &str| -> Result<f32, LocationError> {
        env.call_method(location, name, "()F", &[])
            .and_then(|v| v.f())
            .map_err(|e| LocationError::Unavailable(format!("{name}: {e}")))
    };
    let bool_has = |env: &mut jni::JNIEnv, name: &str| -> bool {
        env.call_method(location, name, "()Z", &[])
            .and_then(|v| v.z())
            .unwrap_or(false)
    };
    let i64_getter = |env: &mut jni::JNIEnv, name: &str| -> i64 {
        env.call_method(location, name, "()J", &[])
            .and_then(|v| v.j())
            .unwrap_or(0)
    };

    let latitude = f64_getter(env, "getLatitude")?;
    let longitude = f64_getter(env, "getLongitude")?;
    // `getAccuracy` is a float (metres). `hasAccuracy` gates validity; absent
    // → a large sentinel so callers treat the fix as low-confidence rather
    // than perfectly precise.
    let accuracy_m = if bool_has(env, "hasAccuracy") {
        f32_getter(env, "getAccuracy")? as f64
    } else {
        f64::INFINITY
    };
    let altitude = if bool_has(env, "hasAltitude") {
        Some(f64_getter(env, "getAltitude")?)
    } else {
        None
    };
    let heading = if bool_has(env, "hasBearing") {
        Some(f32_getter(env, "getBearing")? as f64)
    } else {
        None
    };
    let speed = if bool_has(env, "hasSpeed") {
        Some(f32_getter(env, "getSpeed")? as f64)
    } else {
        None
    };
    // `getTime()` → ms since the Unix epoch (UTC).
    let timestamp_ms = i64_getter(env, "getTime") as f64;

    Ok(Position {
        latitude,
        longitude,
        accuracy_m,
        altitude,
        heading,
        speed,
        timestamp_ms,
    })
}

/// Stops updates on drop. Today this is inert (see the module docs) — the
/// continuous-update path needs a Java `LocationListener` shim supplied by the
/// host. The handle still implements the RAII contract so swapping in the shim
/// later is source-compatible: its `Drop` will call `removeUpdates(listener)`.
pub(crate) struct WatchHandle {
    // Reserved for the listener global-ref the host shim will register; kept
    // as a field so the public type and Drop shape don't change when the seam
    // is filled.
    _seam: (),
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        // No-op until the LocationListener seam is wired (see module docs).
        // Once it is: `locationManager.removeUpdates(listener)` here.
    }
}

pub(crate) fn start_watch(_callback: BoxedCallback) -> WatchHandle {
    // SEAM: continuous updates require `requestLocationUpdates(provider,
    // minTimeMs, minDistanceM, LocationListener)`. `LocationListener` is a
    // Java interface; pure JNI can't implement it at runtime without a host
    // stub `.dex`. The Android host must supply a `LocationListener` that
    // forwards `onLocationChanged` into `_callback`. Until then this installs
    // nothing and the callback never fires (documented, not faked).
    //
    // The one-shot `current()` path (`getLastKnownLocation`) is fully
    // functional and covers the common "where am I now" case.
    WatchHandle { _seam: () }
}
