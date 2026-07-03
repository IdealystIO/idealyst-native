//! Android reachability via **`ConnectivityManager`** (JNI).
//!
//! ⚠️ **Compile-checked only — not device-verified.**
//!
//! - [`current`] is **fully implemented**: `getActiveNetwork` +
//!   `getNetworkCapabilities`, reading `NET_CAPABILITY_VALIDATED` for
//!   online-ness and `hasTransport(TRANSPORT_*)` for the transport. Needs the
//!   `ACCESS_NETWORK_STATE` permission.
//! - [`watch`] is **structured around** `registerDefaultNetworkCallback`, but
//!   there is a real seam: see below.
//!
//! ## The `watch` seam — `NetworkCallback` needs a Java subclass
//!
//! `registerDefaultNetworkCallback` takes a
//! `ConnectivityManager.NetworkCallback`, an **abstract Java class** whose
//! `onAvailable` / `onLost` / `onCapabilitiesChanged` methods the platform
//! calls back. Pure JNI cannot define a new Java class at runtime (no
//! `DefineClass` of arbitrary bytecode without a dex loader), so we cannot
//! synthesize the required subclass here. Delivering change events therefore
//! needs a tiny host-provided Java/Kotlin shim that subclasses
//! `NetworkCallback` and forwards each event back across JNI to a registered
//! Rust trampoline — the same host-shim pattern the camera SDK uses for frame
//! delivery.
//!
//! Until that shim is wired by the host, [`watch`] here registers nothing and
//! returns an inert subscription (it does NOT leak a partial registration).
//! [`current`] is authoritative regardless — a host that needs change
//! notifications today can poll [`current`]. The seam is documented so a
//! native driver can slot in behind the unchanged public API later.

use jni::objects::{JObject, JValue};
use jni::JavaVM;

use crate::{Connectivity, Transport, WatchCallback};

// NetworkCapabilities transport constants (android.net.NetworkCapabilities).
const TRANSPORT_CELLULAR: i32 = 0;
const TRANSPORT_WIFI: i32 = 1;
const TRANSPORT_ETHERNET: i32 = 3;
// NET_CAPABILITY_VALIDATED — the network has verified internet access.
const NET_CAPABILITY_VALIDATED: i32 = 16;

fn java_vm() -> Option<JavaVM> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }.ok()
}

/// The host Activity/Context as a `JObject`.
fn android_context<'a>() -> JObject<'a> {
    let ctx = ndk_context::android_context();
    unsafe { JObject::from_raw(ctx.context().cast()) }
}

/// Query the active network's reachability + transport. Best-effort: any JNI
/// failure (missing context, permission, JVM detach) yields the SDK's
/// `ASSUME_ONLINE` fallback rather than a panic, matching the contract that
/// `current()` never throws.
pub(crate) fn current() -> Connectivity {
    match query_current() {
        Some(c) => c,
        None => Connectivity::ASSUME_ONLINE,
    }
}

fn query_current() -> Option<Connectivity> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().ok()?;
    let ctx = android_context();

    // ConnectivityManager cm = ctx.getSystemService(Context.CONNECTIVITY_SERVICE);
    let service_name = env.new_string("connectivity").ok()?;
    let cm = env
        .call_method(
            &ctx,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[(&service_name).into()],
        )
        .ok()?
        .l()
        .ok()?;
    if cm.is_null() {
        return None;
    }

    // Network active = cm.getActiveNetwork();
    let network = env
        .call_method(&cm, "getActiveNetwork", "()Landroid/net/Network;", &[])
        .ok()?
        .l()
        .ok()?;
    if network.is_null() {
        // No active network at all → offline.
        return Some(Connectivity::OFFLINE);
    }

    // NetworkCapabilities caps = cm.getNetworkCapabilities(active);
    let caps = env
        .call_method(
            &cm,
            "getNetworkCapabilities",
            "(Landroid/net/Network;)Landroid/net/NetworkCapabilities;",
            &[JValue::Object(&network)],
        )
        .ok()?
        .l()
        .ok()?;
    if caps.is_null() {
        return Some(Connectivity::OFFLINE);
    }

    let has_cap = |env: &mut jni::JNIEnv, cap: i32| -> bool {
        env.call_method(&caps, "hasCapability", "(I)Z", &[JValue::Int(cap)])
            .and_then(|v| v.z())
            .unwrap_or(false)
    };
    let has_transport = |env: &mut jni::JNIEnv, t: i32| -> bool {
        env.call_method(&caps, "hasTransport", "(I)Z", &[JValue::Int(t)])
            .and_then(|v| v.z())
            .unwrap_or(false)
    };

    // Online iff the network reports verified internet access.
    let online = has_cap(&mut env, NET_CAPABILITY_VALIDATED);
    if !online {
        return Some(Connectivity::OFFLINE);
    }

    let transport = if has_transport(&mut env, TRANSPORT_WIFI) {
        Transport::Wifi
    } else if has_transport(&mut env, TRANSPORT_CELLULAR) {
        Transport::Cellular
    } else if has_transport(&mut env, TRANSPORT_ETHERNET) {
        Transport::Ethernet
    } else {
        Transport::Other
    };

    Some(Connectivity {
        online: true,
        transport,
    })
}

/// Register a change watcher. See the module-level seam docs: delivering
/// `NetworkCallback` events requires a host Java shim that pure JNI can't
/// synthesize, so this returns an inert subscription today. It deliberately
/// registers nothing (rather than half-registering and leaking) and owns the
/// callback so the lifecycle matches the other backends.
pub(crate) fn watch(callback: WatchCallback) -> Subscription {
    // SEAM: when a host `NetworkCallback` shim exists, this is where we'd
    // `registerDefaultNetworkCallback` with a Rust trampoline forwarding
    // onAvailable/onLost/onCapabilitiesChanged → `callback(current())`, and
    // store the registered callback object + ConnectivityManager here so
    // `Drop` can `unregisterNetworkCallback`.
    Subscription { _callback: callback }
}

/// Android subscription. Today it only owns the callback (the seam means no
/// native registration yet); its `Drop` would `unregisterNetworkCallback`
/// once the host shim lands.
pub(crate) struct Subscription {
    _callback: WatchCallback,
}
