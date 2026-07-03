//! Android save via the **Storage Access Framework** (`ACTION_CREATE_DOCUMENT`),
//! bridged through a Kotlin shim ([`RustFileExport`]) shipped from this crate.
//!
//! SAF needs an Activity-result round-trip (the user picks a destination in
//! the system document creator), which the shim drives via the shared
//! `io.idealyst.runtime.RustActivityResult` registry — the same path
//! `screen-recorder`'s consent intent uses, so no `MainActivity` edits are
//! needed. On the chosen `content://` URI the shim writes the bytes and
//! trampolines the outcome back to the `native*` exports below.
//!
//! VERIFICATION: compile-checked for `aarch64-linux-android`; the SAF /
//! ContentResolver path resolves only at runtime on a device (same posture as
//! `camera`/`media-writer`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use futures_channel::oneshot;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::jlong;
use jni::{JNIEnv, JavaVM};

use crate::{ExportError, SaveOutcome, SaveRequest, Source};

const HELPER: &str = "io/idealyst/fileexport/RustFileExport";

type Sender = oneshot::Sender<Result<SaveOutcome, ExportError>>;

/// token → the sender awaiting the save outcome.
static PENDING: OnceLock<Mutex<HashMap<u64, Sender>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn pending() -> &'static Mutex<HashMap<u64, Sender>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) async fn save(request: SaveRequest) -> Result<SaveOutcome, ExportError> {
    // SAF writes through a ContentResolver stream, so the bytes cross to
    // Kotlin (a `content://` URI has no filesystem path to hand off).
    let bytes = match request.source {
        Source::Bytes(b) => b,
        Source::Path(p) => std::fs::read(&p).map_err(|e| ExportError::Io(e.to_string()))?,
    };

    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let activity = android_context();

    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    pending().lock().unwrap().insert(token, tx);

    let name = env.new_string(&request.suggested_name).map_err(jni_err)?;
    let mime = env.new_string(&request.mime).map_err(jni_err)?;
    let arr = env.byte_array_from_slice(&bytes).map_err(jni_err)?;

    let launch = env.call_static_method(
        HELPER,
        "save",
        "(Landroid/content/Context;JLjava/lang/String;Ljava/lang/String;[B)V",
        &[
            JValue::Object(&activity),
            JValue::Long(token as jlong),
            JValue::Object(&JObject::from(name)),
            JValue::Object(&JObject::from(mime)),
            JValue::Object(&JObject::from(arr)),
        ],
    );
    if let Err(e) = launch {
        pending().lock().unwrap().remove(&token);
        return Err(jni_err(e));
    }

    rx.await
        .unwrap_or_else(|_| Err(ExportError::Backend("save channel dropped".into())))
}

// ---------------------------------------------------------------------------
// JNI helpers.
// ---------------------------------------------------------------------------

fn java_vm() -> Result<JavaVM, ExportError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| ExportError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn jni_err(e: jni::errors::Error) -> ExportError {
    ExportError::Backend(format!("JNI: {e}"))
}

fn android_context<'a>() -> JObject<'a> {
    let ctx = ndk_context::android_context();
    unsafe { JObject::from_raw(ctx.context().cast()) }
}

fn resolve(token: u64, outcome: Result<SaveOutcome, ExportError>) {
    if let Some(tx) = pending().lock().unwrap().remove(&token) {
        let _ = tx.send(outcome);
    }
}

// ---------------------------------------------------------------------------
// JNI exports — the Kotlin shim's trampolines.
// ---------------------------------------------------------------------------

/// `RustFileExport.nativeSaved` — the file was written to the chosen URI.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`; `location` is a `String?`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_fileexport_RustFileExport_nativeSaved(
    mut env: JNIEnv,
    _class: JClass,
    token: jlong,
    location: JString,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let location = if location.is_null() {
            None
        } else {
            env.get_string(&location).ok().map(|s| s.into())
        };
        resolve(token as u64, Ok(SaveOutcome::Saved { location }));
    }));
    if result.is_err() {
        eprintln!("file-export: panic in nativeSaved; aborting");
        std::process::abort();
    }
}

/// `RustFileExport.nativeCancelled` — the user dismissed the document creator.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_fileexport_RustFileExport_nativeCancelled(
    _env: JNIEnv,
    _class: JClass,
    token: jlong,
) {
    let result = std::panic::catch_unwind(|| {
        resolve(token as u64, Ok(SaveOutcome::Cancelled));
    });
    if result.is_err() {
        eprintln!("file-export: panic in nativeCancelled; aborting");
        std::process::abort();
    }
}

/// `RustFileExport.nativeError` — the picker or write failed.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`; `message` is a `String?`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_fileexport_RustFileExport_nativeError(
    mut env: JNIEnv,
    _class: JClass,
    token: jlong,
    message: JString,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let message = if message.is_null() {
            "save failed".to_string()
        } else {
            env.get_string(&message)
                .ok()
                .map(|s| s.into())
                .unwrap_or_else(|| "save failed".to_string())
        };
        resolve(token as u64, Err(ExportError::Backend(message)));
    }));
    if result.is_err() {
        eprintln!("file-export: panic in nativeError; aborting");
        std::process::abort();
    }
}

// Pin the exports so the linker keeps them in the app `cdylib`'s dynsym for
// the JVM's `dlsym` resolution.
#[used]
static KEEP_SAVED: extern "system" fn(JNIEnv, JClass, jlong, JString) =
    Java_io_idealyst_fileexport_RustFileExport_nativeSaved;
#[used]
static KEEP_CANCELLED: extern "system" fn(JNIEnv, JClass, jlong) =
    Java_io_idealyst_fileexport_RustFileExport_nativeCancelled;
#[used]
static KEEP_ERROR: extern "system" fn(JNIEnv, JClass, jlong, JString) =
    Java_io_idealyst_fileexport_RustFileExport_nativeError;
