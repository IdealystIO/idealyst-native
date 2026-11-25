//! Android permission backend, via JNI on the host Activity context.
//!
//! ## `status` — fully implementable in JNI
//!
//! `Context.checkSelfPermission(name)` returns `PERMISSION_GRANTED` (0) or
//! `PERMISSION_DENIED` (-1) synchronously. [`crate::android_policy`] maps our
//! [`Permission`] to the Android permissions it stands for, and we call it on
//! the host context (from `ndk_context`). No callback, no Activity result — a
//! clean JNI read.
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

use crate::android_policy::{self, Stage};
use crate::pending::{self, Outcome};
use crate::{Permission, PermissionStatus};

/// The Kotlin shim that launches the dialog (`RustPermissions.kt`).
const HELPER: &str = "io/idealyst/permissions/RustPermissions";

const PERMISSION_GRANTED: i32 = 0; // PackageManager.PERMISSION_GRANTED

fn java_vm() -> Option<JavaVM> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }.ok()
}

/// `Build.VERSION.SDK_INT`.
fn sdk_int(env: &mut JNIEnv) -> Option<i32> {
    env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I").ok()?.i().ok()
}

/// The first grant `permission` still lacks; `None` inside when it has them
/// all. `None` outside on a JNI failure (no context, detached VM).
fn missing(permission: Permission) -> Option<Option<Stage>> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().ok()?;
    let stages = android_policy::stages(permission, sdk_int(&mut env)?);
    let env = std::cell::RefCell::new(env);
    let failed = std::cell::Cell::new(false);
    let missing = android_policy::first_missing(stages, |p| {
        check_self_permission(&mut env.borrow_mut(), p).unwrap_or_else(|| {
            failed.set(true);
            false
        })
    });
    (!failed.get()).then_some(missing)
}

pub(super) async fn status(permission: Permission) -> PermissionStatus {
    match missing(permission) {
        Some(None) => PermissionStatus::Granted,
        // `checkSelfPermission` can't distinguish "never asked" from
        // "denied"; both read as `PERMISSION_DENIED`. Report `Undetermined`
        // (the safe assumption that a request may still prompt). A host that
        // tracks "already asked once" can refine this.
        _ => PermissionStatus::Undetermined,
    }
}

/// `context.checkSelfPermission(name)`.
fn check_self_permission(env: &mut JNIEnv, perm: &str) -> Option<bool> {
    let ctx = ndk_context::android_context();
    let context = unsafe { JObject::from_raw(ctx.context().cast()) };
    let perm_j = env.new_string(perm).ok()?;
    let result = env.call_method(
        &context,
        "checkSelfPermission",
        "(Ljava/lang/String;)I",
        &[JValue::Object(&JObject::from(perm_j))],
    );
    match result {
        Ok(v) => Some(v.i().ok()? == PERMISSION_GRANTED),
        Err(_) => {
            // Left pending, it would surface in whatever runs next.
            let _ = env.exception_clear();
            None
        }
    }
}

pub(super) async fn request(permission: Permission) -> PermissionStatus {
    let mut asked: Option<Stage> = None;
    loop {
        let stage = match missing(permission) {
            Some(None) => return PermissionStatus::Granted,
            Some(Some(stage)) => stage,
            None => return PermissionStatus::Undetermined,
        };
        if asked.is_some_and(|a| std::ptr::eq(a, stage)) {
            // Answered as granted and still not held: asking again would
            // loop.
            return PermissionStatus::Undetermined;
        }
        asked = Some(stage);
        let (code, rx) = pending::park();
        if !launch_request(stage, code) {
            // The dialog never showed, so no answer is coming.
            pending::abandon(code);
            return PermissionStatus::Undetermined;
        }
        match rx.await {
            // The next stage, if there is one.
            PermissionStatus::Granted => {}
            other => return other,
        }
    }
}

/// `RustPermissions.request(activity, perms, code)`: registers the result
/// handler and calls `requestPermissions`. Returns whether the dialog launched.
fn launch_request(perms: Stage, code: i32) -> bool {
    (|| -> Option<bool> {
        let vm = java_vm()?;
        let mut env = vm.attach_current_thread().ok()?;
        let ctx = ndk_context::android_context();
        let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
        let array = env
            .new_object_array(perms.len() as i32, "java/lang/String", JObject::null())
            .ok()?;
        for (i, perm) in perms.iter().enumerate() {
            let perm_j = env.new_string(perm).ok()?;
            env.set_object_array_element(&array, i as i32, &perm_j).ok()?;
        }
        let launched = env.call_static_method(
            HELPER,
            "request",
            "(Landroid/content/Context;[Ljava/lang/String;I)Z",
            &[
                JValue::Object(&activity),
                JValue::Object(&array),
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
