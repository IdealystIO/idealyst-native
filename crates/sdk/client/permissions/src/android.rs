//! Android permission backend, via JNI on the host Activity context.
//!
//! ## `status` — fully implementable in JNI
//!
//! `Context.checkSelfPermission(name)` returns `PERMISSION_GRANTED` (0) or
//! `PERMISSION_DENIED` (-1) synchronously. We map our [`Permission`] to the
//! Android permission string and call it on the host context (from
//! `ndk_context`). No callback, no Activity result — a clean JNI read.
//!
//! ## `request` — the dialog and its answer
//!
//! A runtime permission *request* is asynchronous on Android:
//! `Activity.requestPermissions(perms, code)` shows the system dialog and
//! the answer arrives later in the Activity's `onRequestPermissionsResult`
//! override, which pure JNI can't receive. The round trip:
//!
//! 1. [`request`] parks a oneshot under a fresh request code
//!    ([`crate::pending`]) and calls the `RustPermissions.request` Kotlin shim
//!    shipped from this crate (`runtime/kotlin/io/idealyst/permissions/`).
//! 2. The shim registers a handler with the backend's
//!    `io.idealyst.runtime.RustPermissionsResult` registry and calls
//!    `Activity.requestPermissions`.
//! 3. The generated `MainActivity.onRequestPermissionsResult` forwards to
//!    `RustPermissionsResult.dispatch`, which runs the handler, which calls
//!    the [`Java_io_idealyst_permissions_RustPermissions_nativeOnPermissionsResult`]
//!    export below, which resolves the parked oneshot.
//!
//! Android answers every `requestPermissions` call (an interrupted prompt
//! arrives as empty arrays, read as a cancellation), so the future resolves
//! once the user answers. A host Activity that doesn't forward
//! `onRequestPermissionsResult` leaves it parked until app teardown.

use jni::objects::{JClass, JObject, JValue};
use jni::sys::jint;
use jni::{JNIEnv, JavaVM};

use crate::pending::{self, Outcome};
use crate::{Permission, PermissionStatus};

/// The Kotlin shim that launches the dialog (`RustPermissions.kt`).
const HELPER: &str = "io/idealyst/permissions/RustPermissions";

const PERMISSION_GRANTED: i32 = 0; // PackageManager.PERMISSION_GRANTED

/// The Android manifest permission string for a [`Permission`], or `None`
/// where the platform needs no runtime grant for it.
fn android_permission(permission: Permission) -> Option<&'static str> {
    match permission {
        // POST_NOTIFICATIONS is a runtime permission only on API 33+; on
        // older OS versions `checkSelfPermission` reports it granted, so the
        // mapping is still correct to query.
        Permission::Notifications => Some("android.permission.POST_NOTIFICATIONS"),
        Permission::LocationWhenInUse => Some("android.permission.ACCESS_FINE_LOCATION"),
        Permission::LocationAlways => Some("android.permission.ACCESS_BACKGROUND_LOCATION"),
        Permission::Camera => Some("android.permission.CAMERA"),
        Permission::Microphone => Some("android.permission.RECORD_AUDIO"),
    }
}

fn java_vm() -> Option<JavaVM> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }.ok()
}

pub(super) async fn status(permission: Permission) -> PermissionStatus {
    let Some(perm) = android_permission(permission) else {
        return PermissionStatus::Unsupported;
    };
    check_self_permission(perm).unwrap_or(PermissionStatus::Undetermined)
}

/// `context.checkSelfPermission(name)` → Granted / Denied. A JNI failure
/// (no context, detached VM) returns `None`, which the caller maps to
/// `Undetermined` rather than a hard error.
fn check_self_permission(perm: &str) -> Option<PermissionStatus> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().ok()?;
    let ctx = ndk_context::android_context();
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    let perm_j = env.new_string(perm).ok()?;
    let result = env
        .call_method(
            &activity,
            "checkSelfPermission",
            "(Ljava/lang/String;)I",
            &[JValue::Object(&JObject::from(perm_j))],
        )
        .ok()?
        .i()
        .ok()?;
    Some(if result == PERMISSION_GRANTED {
        PermissionStatus::Granted
    } else {
        // `checkSelfPermission` can't distinguish "never asked" from
        // "denied"; both read as `PERMISSION_DENIED`. Report `Undetermined`
        // (the safe assumption that a request may still prompt). A host that
        // tracks "already asked once" can refine this.
        PermissionStatus::Undetermined
    })
}

pub(super) async fn request(permission: Permission) -> PermissionStatus {
    let Some(perm) = android_permission(permission) else {
        return PermissionStatus::Unsupported;
    };

    // Already granted? Don't re-prompt.
    if check_self_permission(perm) == Some(PermissionStatus::Granted) {
        return PermissionStatus::Granted;
    }

    let (code, rx) = pending::park();
    if !launch_request(perm, code) {
        // The dialog never showed, so no answer is coming.
        pending::abandon(code);
        return check_self_permission(perm).unwrap_or(PermissionStatus::Undetermined);
    }
    rx.await
}

/// `RustPermissions.request(activity, perm, code)`: registers the result
/// handler and calls `requestPermissions`. Returns whether the dialog launched.
fn launch_request(perm: &str, code: i32) -> bool {
    (|| -> Option<bool> {
        let vm = java_vm()?;
        let mut env = vm.attach_current_thread().ok()?;
        let ctx = ndk_context::android_context();
        let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
        let perm_j = env.new_string(perm).ok()?;
        let launched = env.call_static_method(
            HELPER,
            "request",
            "(Landroid/content/Context;Ljava/lang/String;I)Z",
            &[
                JValue::Object(&activity),
                JValue::Object(&JObject::from(perm_j)),
                JValue::Int(code),
            ],
        );
        match launched {
            Ok(v) => v.z().ok(),
            Err(_) => {
                // A missing shim class leaves a pending Java exception.
                let _ = env.exception_clear();
                None
            }
        }
    })()
    .unwrap_or(false)
}

/// `RustPermissions.nativeOnPermissionsResult(code, outcome)`: the dialog
/// for `code` was answered. `outcome` is one of the shim's `RESULT_*` codes.
/// Catches any panic so an unwind can't cross the JNI boundary (UB) — logs
/// and aborts.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_permissions_RustPermissions_nativeOnPermissionsResult(
    _env: JNIEnv,
    _class: JClass,
    code: jint,
    outcome: jint,
) {
    let result = std::panic::catch_unwind(|| match Outcome::from_code(outcome) {
        Some(outcome) => pending::complete(code, outcome),
        None => {
            eprintln!("permissions: unknown outcome {outcome} for request {code}");
            pending::complete(code, Outcome::Cancelled);
        }
    });
    if result.is_err() {
        eprintln!("permissions: panic in nativeOnPermissionsResult; aborting");
        std::process::abort();
    }
}

// Pin the export so the linker keeps it in the app `cdylib`'s dynsym for the
// JVM's `dlsym` resolution.
#[used]
static KEEP_ON_RESULT: extern "system" fn(JNIEnv, JClass, jint, jint) =
    Java_io_idealyst_permissions_RustPermissions_nativeOnPermissionsResult;
