//! Android permission backend, via JNI on the host Activity context.
//!
//! ## `status` — fully implementable in JNI
//!
//! `Context.checkSelfPermission(name)` returns `PERMISSION_GRANTED` (0) or
//! `PERMISSION_DENIED` (-1) synchronously. We map our [`Permission`] to the
//! Android permission string and call it on the host context (from
//! `ndk_context`). No callback, no Activity result — a clean JNI read.
//!
//! ## `request` — needs a host result-forwarding seam
//!
//! A runtime permission *request* is inherently asynchronous and
//! callback-delivered on Android: `Activity.requestPermissions(perms, code)`
//! shows the system dialog and the answer arrives later in the Activity's
//! `onRequestPermissionsResult(code, perms, grantResults)`. **Pure JNI
//! cannot receive that callback** — it's an override on the host Activity
//! (or a registered `ActivityResultLauncher`), which lives in the app's
//! Kotlin/Java, not in this crate.
//!
//! So `request` here:
//! 1. Calls `Activity.requestPermissions(...)` with a request code, surfacing
//!    the dialog, and parks a oneshot keyed by that code.
//! 2. Relies on the host to forward `onRequestPermissionsResult` back into
//!    this crate via [`complete_request`] — the documented integration seam.
//!    The host adds one line to its `onRequestPermissionsResult` override
//!    (or its `ActivityResultCallback`) that calls our
//!    `Java_..._nativeOnPermissionsResult` JNI export, or — for a pure-Rust
//!    host with no override — calls [`complete_request`] directly.
//!
//! Without that hook wired, `request` resolves to the *re-read* status once
//! the future is polled again, which for an unanswered prompt is still
//! `Undetermined` — it never hangs, it just can't observe the grant until
//! the host forwards the result. This is the honest seam, not a fake grant.
//!
//! VERIFICATION: compile-checked only — exercising it needs a device/emulator
//! plus a host that forwards the result callback.

use std::collections::HashMap;
use std::sync::Mutex;

use jni::objects::{JObject, JValue};
use jni::JavaVM;

use crate::oneshot;
use crate::{Permission, PermissionStatus};

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

    // Park a oneshot under a fresh request code; the host forwards the
    // result to `complete_request(code, granted)`.
    let code = next_request_code();
    let (tx, rx) = oneshot::channel(PermissionStatus::Undetermined);
    PENDING.lock().unwrap().insert(code, tx);

    if !invoke_request_permissions(perm, code) {
        // Couldn't even launch the dialog — drop the pending entry so it
        // doesn't leak, and fall back to a status re-read.
        PENDING.lock().unwrap().remove(&code);
        return check_self_permission(perm).unwrap_or(PermissionStatus::Undetermined);
    }

    // Resolves when the host forwards `onRequestPermissionsResult` via
    // `complete_request`. If the host never wires the seam the oneshot's
    // sender is eventually dropped (app teardown) and this falls back to the
    // re-read status — it never hangs the caller indefinitely.
    rx.await
}

/// `activity.requestPermissions(new String[]{perm}, code)`. Returns whether
/// the call dispatched (true) or a JNI failure prevented it (false).
fn invoke_request_permissions(perm: &str, code: i32) -> bool {
    (|| -> Option<()> {
        let vm = java_vm()?;
        let mut env = vm.attach_current_thread().ok()?;
        let ctx = ndk_context::android_context();
        let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

        // String[]{ perm }
        let string_class = env.find_class("java/lang/String").ok()?;
        let perm_j = env.new_string(perm).ok()?;
        let arr = env
            .new_object_array(1, &string_class, &perm_j)
            .ok()?;

        env.call_method(
            &activity,
            "requestPermissions",
            "([Ljava/lang/String;I)V",
            &[JValue::Object(&JObject::from(arr)), JValue::Int(code)],
        )
        .ok()?;
        Some(())
    })()
    .is_some()
}

// =========================================================================
// Request-completion seam.
//
// The host's `onRequestPermissionsResult` (or its ActivityResultCallback)
// forwards each settled request here, by request code. Two ways in:
//   - `complete_request(code, granted)` from Rust host glue, OR
//   - the `Java_..._nativeOnPermissionsResult` JNI export below, called from
//     a Kotlin/Java `onRequestPermissionsResult` override.
// =========================================================================

static PENDING: Mutex<Option<HashMap<i32, oneshot::Sender<PermissionStatus>>>> = Mutex::new(None);

// Monotonic request codes. `requestPermissions` codes are 16-bit (the high
// bits are reserved by the framework), so we wrap within that range.
static NEXT_CODE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);

trait PendingMapExt {
    fn insert(&mut self, code: i32, tx: oneshot::Sender<PermissionStatus>);
    fn remove(&mut self, code: &i32);
    fn take(&mut self, code: i32) -> Option<oneshot::Sender<PermissionStatus>>;
}

impl PendingMapExt for std::sync::MutexGuard<'_, Option<HashMap<i32, oneshot::Sender<PermissionStatus>>>> {
    fn insert(&mut self, code: i32, tx: oneshot::Sender<PermissionStatus>) {
        self.get_or_insert_with(HashMap::new).insert(code, tx);
    }
    fn remove(&mut self, code: &i32) {
        if let Some(map) = self.as_mut() {
            map.remove(code);
        }
    }
    fn take(&mut self, code: i32) -> Option<oneshot::Sender<PermissionStatus>> {
        self.as_mut().and_then(|m| m.remove(&code))
    }
}

fn next_request_code() -> i32 {
    use std::sync::atomic::Ordering;
    // Keep within the framework's 16-bit request-code space.
    let v = NEXT_CODE.fetch_add(1, Ordering::Relaxed) & 0xFFFF;
    if v == 0 {
        1
    } else {
        v
    }
}

/// Complete a pending [`request`] from the host's
/// `onRequestPermissionsResult`. `granted` is whether every requested
/// permission in the group was granted. A no-op for an unknown code (e.g. a
/// request this crate didn't originate).
///
/// Pure-Rust hosts call this directly from their result callback; hosts with
/// a Kotlin/Java Activity use the [`nativeOnPermissionsResult`] JNI export,
/// which calls through to this.
pub fn complete_request(request_code: i32, granted: bool) {
    let status = if granted {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    };
    let tx = PENDING.lock().unwrap().take(request_code);
    if let Some(tx) = tx {
        tx.send(status);
    }
}

/// JNI export the host's Kotlin/Java `onRequestPermissionsResult` override
/// trampolines into:
///
/// ```kotlin
/// override fun onRequestPermissionsResult(
///     code: Int, perms: Array<String>, results: IntArray
/// ) {
///     val granted = results.isNotEmpty() &&
///         results.all { it == PackageManager.PERMISSION_GRANTED }
///     nativeOnPermissionsResult(code, granted)
/// }
/// external fun nativeOnPermissionsResult(code: Int, granted: Boolean)
/// ```
///
/// The Kotlin `external fun` must live in a class whose package/name matches
/// this symbol; adjust the host glue accordingly. Catches any panic so an
/// unwind can't cross the JNI boundary (UB) — logs + aborts.
///
/// # Safety
/// Called by the JVM with valid `env` / `class`; standard JNI export contract.
#[no_mangle]
pub extern "system" fn Java_com_idealyst_permissions_Permissions_nativeOnPermissionsResult(
    _env: jni::JNIEnv,
    _class: JObject,
    code: i32,
    granted: jni::sys::jboolean,
) {
    let result = std::panic::catch_unwind(|| {
        complete_request(code, granted != 0);
    });
    if result.is_err() {
        eprintln!("permissions: panic in nativeOnPermissionsResult; aborting");
        std::process::abort();
    }
}
