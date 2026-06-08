//! Android open via `ACTION_OPEN_DOCUMENT` (documents) and the **Photo Picker**
//! (`ACTION_PICK_IMAGES`, media), bridged through a Kotlin shim
//! ([`RustFilePicker`]) shipped from this crate.
//!
//! Both need an Activity-result round-trip, driven via the shared
//! `io.idealyst.runtime.RustActivityResult` registry — the same path
//! `file-export`'s SAF save uses, so no `MainActivity` edits are needed. The
//! picker hands back `content://` URIs (which have **no** filesystem path), so
//! [`PickedFile::path`](crate::PickedFile::path) is `None` here; reads stream
//! over a **detached file descriptor**: `openFd` calls
//! `ContentResolver.openFileDescriptor(uri,"r").detachFd()`, hands the raw fd to
//! Rust, and Rust reads it in 1-MiB chunks. No bytes are copied across JNI and
//! nothing is buffered whole.
//!
//! VERIFICATION: compile-checked for `aarch64-linux-android`; the
//! ContentResolver / Photo Picker path resolves only at runtime on a device
//! (same posture `file-export`/`camera` take).

use std::collections::HashMap;
use std::os::fd::FromRawFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use futures_channel::oneshot;
use jni::objects::{JClass, JLongArray, JObject, JObjectArray, JString, JValue};
use jni::sys::{jint, jlong};
use jni::{JNIEnv, JavaVM};

use crate::{PickError, PickKind, PickRequest};

const HELPER: &str = "io/idealyst/filepicker/RustFilePicker";

type Outcome = Result<Option<Vec<PickedFile>>, PickError>;
type Sender = oneshot::Sender<Outcome>;

/// token → the sender awaiting the pick outcome.
static PENDING: OnceLock<Mutex<HashMap<u64, Sender>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn pending() -> &'static Mutex<HashMap<u64, Sender>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A file the user picked on Android: a `content://` URI plus metadata. The
/// bytes are read on demand over a detached fd (see [`open`](Self::open)).
pub(crate) struct PickedFile {
    name: String,
    mime: String,
    size: Option<u64>,
    uri: String,
}

impl PickedFile {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn mime(&self) -> &str {
        &self.mime
    }
    pub(crate) fn size(&self) -> Option<u64> {
        self.size
    }
    pub(crate) fn path(&self) -> Option<&std::path::Path> {
        // `content://` URIs have no filesystem path.
        None
    }
    pub(crate) async fn open(&self) -> Result<FileStream, PickError> {
        let fd = open_fd(&self.uri)?;
        Ok(FileStream::from_fd(fd))
    }
}

/// Streams the picked file over a file descriptor detached from a
/// `ParcelFileDescriptor` on the Kotlin side.
pub(crate) struct FileStream {
    file: std::fs::File,
}

impl FileStream {
    fn from_fd(fd: i32) -> Self {
        // SAFETY: `fd` was just produced by `ParcelFileDescriptor.detachFd()` on
        // the Kotlin side, which relinquishes ownership — Rust now solely owns
        // it and `File` closes it on drop. Kotlin must NOT also close it
        // (double-close); the shim's `openFd` is written to detach, not close.
        Self {
            file: unsafe { std::fs::File::from_raw_fd(fd) },
        }
    }

    pub(crate) async fn chunk(&mut self) -> Result<Option<Vec<u8>>, PickError> {
        use std::io::Read;
        let mut buf = vec![0u8; crate::READ_CHUNK];
        let n = self
            .file
            .read(&mut buf)
            .map_err(|e| PickError::Io(e.to_string()))?;
        if n == 0 {
            return Ok(None);
        }
        buf.truncate(n);
        Ok(Some(buf))
    }
}

pub(crate) async fn pick(request: &PickRequest) -> Outcome {
    let (mimes, is_media): (Vec<String>, bool) = match &request.kind {
        PickKind::Documents(m) => (m.clone(), false),
        PickKind::Media(k) => (
            crate::mime::media_mimes(*k)
                .iter()
                .map(|s| s.to_string())
                .collect(),
            true,
        ),
    };

    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let activity = android_context();

    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    pending().lock().unwrap().insert(token, tx);

    let mime_arr = match new_string_array(&mut env, &mimes) {
        Ok(a) => a,
        Err(e) => {
            pending().lock().unwrap().remove(&token);
            return Err(e);
        }
    };

    let launch = env.call_static_method(
        HELPER,
        "pick",
        "(Landroid/content/Context;J[Ljava/lang/String;ZZ)V",
        &[
            JValue::Object(&activity),
            JValue::Long(token as jlong),
            JValue::Object(&JObject::from(mime_arr)),
            JValue::Bool(request.allow_multiple as u8),
            JValue::Bool(is_media as u8),
        ],
    );
    if let Err(e) = launch {
        pending().lock().unwrap().remove(&token);
        return Err(jni_err(e));
    }

    rx.await
        .unwrap_or_else(|_| Err(PickError::Backend("pick channel dropped".into())))
}

/// Call back into Kotlin to open a read fd for a picked `content://` URI.
fn open_fd(uri: &str) -> Result<i32, PickError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let activity = android_context();
    let juri = env.new_string(uri).map_err(jni_err)?;
    let ret = env
        .call_static_method(
            HELPER,
            "openFd",
            "(Landroid/content/Context;Ljava/lang/String;)I",
            &[JValue::Object(&activity), JValue::Object(&JObject::from(juri))],
        )
        .map_err(jni_err)?;
    let fd = ret.i().map_err(jni_err)?;
    if fd < 0 {
        return Err(PickError::Io(format!("could not open fd for {uri}")));
    }
    Ok(fd)
}

// ---------------------------------------------------------------------------
// JNI helpers.
// ---------------------------------------------------------------------------

fn java_vm() -> Result<JavaVM, PickError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| PickError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn jni_err(e: jni::errors::Error) -> PickError {
    PickError::Backend(format!("JNI: {e}"))
}

fn android_context<'a>() -> JObject<'a> {
    let ctx = ndk_context::android_context();
    unsafe { JObject::from_raw(ctx.context().cast()) }
}

fn new_string_array<'a>(
    env: &mut JNIEnv<'a>,
    items: &[String],
) -> Result<JObjectArray<'a>, PickError> {
    let class = env.find_class("java/lang/String").map_err(jni_err)?;
    let empty = env.new_string("").map_err(jni_err)?;
    let arr = env
        .new_object_array(items.len() as jint, &class, &empty)
        .map_err(jni_err)?;
    for (i, s) in items.iter().enumerate() {
        let js = env.new_string(s).map_err(jni_err)?;
        env.set_object_array_element(&arr, i as jint, &js)
            .map_err(jni_err)?;
    }
    Ok(arr)
}

fn resolve(token: u64, outcome: Outcome) {
    if let Some(tx) = pending().lock().unwrap().remove(&token) {
        let _ = tx.send(outcome);
    }
}

/// Read element `i` of a `String[]` as a Rust `String` (empty on error).
fn array_string(env: &mut JNIEnv, arr: &JObjectArray, i: jint) -> String {
    let Ok(obj) = env.get_object_array_element(arr, i) else {
        return String::new();
    };
    let js = JString::from(obj);
    env.get_string(&js).map(Into::into).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// JNI exports — the Kotlin shim's trampolines.
// ---------------------------------------------------------------------------

/// `RustFilePicker.nativeFilesPicked` — the user chose `count` files, given as
/// parallel `uris`/`names`/`mimes` `String[]`s plus a `long[]` of sizes
/// (`-1` = unknown).
///
/// # Safety
/// Called by the JVM with a valid `env`/`class` and parallel arrays of length
/// `count`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_filepicker_RustFilePicker_nativeFilesPicked(
    mut env: JNIEnv,
    _class: JClass,
    token: jlong,
    count: jint,
    uris: JObjectArray,
    names: JObjectArray,
    mimes: JObjectArray,
    sizes: JLongArray,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let n = count.max(0) as usize;
        let mut size_buf = vec![0i64; n];
        if n > 0 {
            let _ = env.get_long_array_region(&sizes, 0, &mut size_buf);
        }
        let mut files = Vec::with_capacity(n);
        for i in 0..count.max(0) {
            let uri = array_string(&mut env, &uris, i);
            if uri.is_empty() {
                continue;
            }
            let name = array_string(&mut env, &names, i);
            let mime = array_string(&mut env, &mimes, i);
            let raw = size_buf[i as usize];
            files.push(PickedFile {
                name,
                mime,
                size: if raw >= 0 { Some(raw as u64) } else { None },
                uri,
            });
        }
        resolve(token as u64, Ok(Some(files)));
    }));
    if result.is_err() {
        eprintln!("file-picker: panic in nativeFilesPicked; aborting");
        std::process::abort();
    }
}

/// `RustFilePicker.nativeCancelled` — the user dismissed the picker.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_filepicker_RustFilePicker_nativeCancelled(
    _env: JNIEnv,
    _class: JClass,
    token: jlong,
) {
    let result = std::panic::catch_unwind(|| {
        resolve(token as u64, Ok(None));
    });
    if result.is_err() {
        eprintln!("file-picker: panic in nativeCancelled; aborting");
        std::process::abort();
    }
}

/// `RustFilePicker.nativeError` — the picker failed.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`; `message` is a `String?`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_filepicker_RustFilePicker_nativeError(
    mut env: JNIEnv,
    _class: JClass,
    token: jlong,
    message: JString,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let message = if message.is_null() {
            "file pick failed".to_string()
        } else {
            env.get_string(&message)
                .ok()
                .map(Into::into)
                .unwrap_or_else(|| "file pick failed".to_string())
        };
        resolve(token as u64, Err(PickError::Backend(message)));
    }));
    if result.is_err() {
        eprintln!("file-picker: panic in nativeError; aborting");
        std::process::abort();
    }
}

// Pin the exports so the linker keeps them in the app `cdylib`'s dynsym for the
// JVM's `dlsym` resolution.
#[used]
static KEEP_PICKED: extern "system" fn(
    JNIEnv,
    JClass,
    jlong,
    jint,
    JObjectArray,
    JObjectArray,
    JObjectArray,
    JLongArray,
) = Java_io_idealyst_filepicker_RustFilePicker_nativeFilesPicked;
#[used]
static KEEP_CANCELLED: extern "system" fn(JNIEnv, JClass, jlong) =
    Java_io_idealyst_filepicker_RustFilePicker_nativeCancelled;
#[used]
static KEEP_ERROR: extern "system" fn(JNIEnv, JClass, jlong, JString) =
    Java_io_idealyst_filepicker_RustFilePicker_nativeError;
