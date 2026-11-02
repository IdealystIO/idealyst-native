//! Android backend: `NfcAdapter` reader mode through the `RustNfc` Kotlin
//! shim (`runtime/kotlin/io/idealyst/nfc/RustNfc.kt`, shipped via
//! `[package.metadata.idealyst.android].runtime_kotlin`).
//!
//! ## Why a Kotlin shim
//!
//! Reader mode delivers tags to an `NfcAdapter.ReaderCallback` interface,
//! and tag I/O (`Ndef.writeNdefMessage`, `IsoDep.transceive`, …) blocks, so
//! it must run off the UI thread. The shim implements the callback, keeps
//! discovered `Tag` objects in a handle map, and runs every operation on
//! one background thread (which also serializes the per-tag connected
//! technology — see the shim's doc for why a connection is kept open
//! between operations).
//!
//! ## Bridge
//!
//! - Tags: `begin` registers the session's [`Mailbox`] under a session token
//!   in [`SESSIONS`]; the shim's `nativeTagDiscovered(session, handle, uid,
//!   techs)` (NFC binder thread) pushes into it.
//! - Operations: each mints an op token, parks a oneshot sender in
//!   [`PENDING`], and calls the shim; `nativeOpResult` (the shim's I/O
//!   thread) completes it.
//!
//! Both callbacks only complete a channel. Waking the awaiting future from
//! those threads is safe: the backend's async executor waker posts the
//! re-poll to the main looper, so the continuation runs on the main thread.
//!
//! Calls INTO the shim (`begin`, `end`, the ops) happen while a future is
//! being polled, which is on the main thread; there `find_class` sees the
//! app's class loader. [`TagHandle`]'s `Drop` may run elsewhere, so its
//! release is best-effort (the session's end clears every tag anyway).
//!
//! ## Activity lifecycle
//!
//! Reader mode belongs to the Activity published through `ndk_context`. It
//! is only active while that Activity is resumed: the framework suspends it
//! on pause and re-applies it on resume, so a session simply sees no tags
//! while the app is in the background.
//!
//! ## VERIFICATION
//!
//! Compile-checked for `aarch64-linux-android` and the Kotlin against
//! `android.jar`; NOT device-verified. The Android emulator has no NFC
//! controller, so exercising this needs a physical NFC phone and a tag.
//! JNI signatures and the exported symbols resolve only at runtime.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use futures_channel::oneshot;
use jni::objects::{JByteArray, JClass, JObject, JObjectArray, JString, JValue};
use jni::sys::{jint, jlong};
use jni::{JNIEnv, JavaVM};

use crate::mailbox::Mailbox;
use crate::{Availability, NdefMessage, NdefRecord, NfcError, SessionOptions, Tag, Technology};

const SHIM: &str = "io/idealyst/nfc/RustNfc";

// `RustNfc.begin` / `availability` results.
const BEGUN: jint = 0;
const UNSUPPORTED: jint = 1;
const DISABLED: jint = 2;

// `nativeOpResult` status codes.
const OK: jint = 0;
const TAG_LOST: jint = 1;
const NOT_NDEF: jint = 2;
const READ_ONLY: jint = 3;
const TOO_LARGE: jint = 4;
const ERROR: jint = 5;
const NO_RAW_TECH: jint = 6;

/// A tag as reported by the shim, before it becomes a [`Tag`].
struct Discovered {
    id: Vec<u8>,
    technologies: Vec<Technology>,
    handle: i64,
}

/// What `nativeOpResult` delivers.
struct OpReply {
    status: jint,
    data: Option<Vec<u8>>,
    message: Option<String>,
    extra: jint,
}

/// Open sessions: token → the mailbox `nativeTagDiscovered` pushes into.
static SESSIONS: OnceLock<Mutex<HashMap<u64, Mailbox<Discovered>>>> = OnceLock::new();
/// In-flight tag operations: op token → the sender awaiting its reply.
static PENDING: OnceLock<Mutex<HashMap<u64, oneshot::Sender<OpReply>>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn sessions() -> &'static Mutex<HashMap<u64, Mailbox<Discovered>>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pending() -> &'static Mutex<HashMap<u64, oneshot::Sender<OpReply>>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

fn java_vm() -> Result<JavaVM, NfcError> {
    let ctx = ndk_context::android_context();
    unsafe { JavaVM::from_raw(ctx.vm().cast()) }
        .map_err(|e| NfcError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn jni_err(e: jni::errors::Error) -> NfcError {
    NfcError::Backend(format!("JNI: {e}"))
}

/// The Activity the backend published through `ndk_context`.
fn activity<'a>() -> JObject<'a> {
    unsafe { JObject::from_raw(ndk_context::android_context().context().cast()) }
}

pub(crate) fn availability() -> Availability {
    let status = (|| {
        let vm = java_vm()?;
        let mut env = vm.attach_current_thread().map_err(jni_err)?;
        let ctx = activity();
        env.call_static_method(
            SHIM,
            "availability",
            "(Landroid/content/Context;)I",
            &[(&ctx).into()],
        )
        .and_then(|v| v.i())
        .map_err(jni_err)
    })();
    match status {
        Ok(BEGUN) => Availability::Available,
        Ok(DISABLED) => Availability::Disabled,
        _ => Availability::Unsupported,
    }
}

pub(crate) struct Session {
    token: u64,
    mailbox: Mailbox<Discovered>,
    closed: AtomicBool,
}

impl Session {
    pub(crate) fn begin(_options: &SessionOptions) -> Result<Session, NfcError> {
        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        let mailbox = Mailbox::new();
        {
            let mut open = sessions().lock().unwrap();
            if !open.is_empty() {
                return Err(NfcError::Busy);
            }
            // Registered before reader mode starts, so the first tag has
            // somewhere to go.
            open.insert(token, mailbox.clone());
        }
        let status = (|| {
            let vm = java_vm()?;
            let mut env = vm.attach_current_thread().map_err(jni_err)?;
            let act = activity();
            env.call_static_method(
                SHIM,
                "begin",
                "(Landroid/app/Activity;J)I",
                &[(&act).into(), JValue::Long(token as jlong)],
            )
            .and_then(|v| v.i())
            .map_err(jni_err)
        })();
        let err = match status {
            Ok(BEGUN) => None,
            Ok(UNSUPPORTED) => Some(NfcError::Unsupported("this device has no NFC controller".into())),
            Ok(DISABLED) => Some(NfcError::Disabled),
            Ok(other) => Some(NfcError::Backend(format!("RustNfc.begin returned {other}"))),
            Err(e) => Some(e),
        };
        if let Some(err) = err {
            sessions().lock().unwrap().remove(&token);
            return Err(err);
        }
        Ok(Session { token, mailbox, closed: AtomicBool::new(false) })
    }

    pub(crate) async fn next_tag(&self) -> Result<Tag, NfcError> {
        let d = self.mailbox.recv().await?;
        Ok(Tag::new(d.id, d.technologies, TagHandle { handle: d.handle }))
    }

    pub(crate) fn set_prompt(&self, _text: &str) {}

    pub(crate) fn close(&self, _error_message: Option<&str>) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        sessions().lock().unwrap().remove(&self.token);
        self.mailbox.close(NfcError::Cancelled);
        let ended = (|| {
            let vm = java_vm()?;
            let mut env = vm.attach_current_thread().map_err(jni_err)?;
            let act = activity();
            env.call_static_method(
                SHIM,
                "end",
                "(Landroid/app/Activity;J)V",
                &[(&act).into(), JValue::Long(self.token as jlong)],
            )
            .map_err(jni_err)?;
            Ok::<_, NfcError>(())
        })();
        if let Err(e) = ended {
            log::error!("nfc: leaving reader mode failed: {e}");
        }
    }
}

pub(crate) struct TagHandle {
    handle: i64,
}

impl TagHandle {
    pub(crate) async fn read_ndef(&self) -> Result<Option<NdefMessage>, NfcError> {
        let reply = self.op("readNdef", None).await?;
        match reply.status {
            OK => match reply.data {
                Some(bytes) => Ok(Some(NdefMessage::from_bytes(&bytes)?)),
                None => Ok(Some(NdefMessage::default())),
            },
            NOT_NDEF => Ok(None),
            _ => Err(op_error(reply, 0)),
        }
    }

    pub(crate) async fn write_ndef(&self, message: &NdefMessage) -> Result<(), NfcError> {
        // `android.nfc.NdefMessage` needs at least one record; the Empty
        // record is the NDEF way to say "nothing".
        let bytes = if message.records.is_empty() {
            NdefMessage::new(vec![NdefRecord::empty()]).to_bytes()?
        } else {
            message.to_bytes()?
        };
        let needed = bytes.len();
        let reply = self.op("writeNdef", Some(&bytes)).await?;
        match reply.status {
            OK => Ok(()),
            _ => Err(op_error(reply, needed)),
        }
    }

    pub(crate) async fn transceive(&self, command: &[u8]) -> Result<Vec<u8>, NfcError> {
        let reply = self.op("transceive", Some(command)).await?;
        match reply.status {
            OK => Ok(reply.data.unwrap_or_default()),
            _ => Err(op_error(reply, 0)),
        }
    }

    /// Run one shim operation (`readNdef(handle, op)` or `method(handle,
    /// bytes, op)`) and await its `nativeOpResult`.
    async fn op(&self, method: &str, bytes: Option<&[u8]>) -> Result<OpReply, NfcError> {
        let (tx, rx) = oneshot::channel();
        let op = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        pending().lock().unwrap().insert(op, tx);
        let launched = (|| {
            let vm = java_vm()?;
            let mut env = vm.attach_current_thread().map_err(jni_err)?;
            match bytes {
                None => env.call_static_method(
                    SHIM,
                    method,
                    "(JJ)V",
                    &[JValue::Long(self.handle), JValue::Long(op as jlong)],
                ),
                Some(b) => {
                    let array = env.byte_array_from_slice(b).map_err(jni_err)?;
                    env.call_static_method(
                        SHIM,
                        method,
                        "(J[BJ)V",
                        &[JValue::Long(self.handle), (&array).into(), JValue::Long(op as jlong)],
                    )
                }
            }
            .map_err(jni_err)?;
            Ok::<_, NfcError>(())
        })();
        if let Err(e) = launched {
            pending().lock().unwrap().remove(&op);
            return Err(e);
        }
        rx.await.map_err(|_| NfcError::Backend("NFC operation reply channel dropped".into()))
    }
}

impl Drop for TagHandle {
    fn drop(&mut self) {
        // Best-effort: off the main thread the shim class may not resolve,
        // and the session's end clears the tag regardless.
        let Ok(vm) = java_vm() else { return };
        let Ok(mut env) = vm.attach_current_thread() else { return };
        if env.call_static_method(SHIM, "release", "(J)V", &[JValue::Long(self.handle)]).is_err() {
            // Leaves a pending ClassNotFoundException; don't hand it to
            // whatever JNI call this thread makes next.
            let _ = env.exception_clear();
        }
    }
}

/// The error for a non-OK reply. `needed` is the encoded message size for
/// a write (used by `TOO_LARGE`).
fn op_error(reply: OpReply, needed: usize) -> NfcError {
    match reply.status {
        TAG_LOST => NfcError::TagLost,
        NOT_NDEF => NfcError::NotNdef,
        READ_ONLY => NfcError::ReadOnly,
        TOO_LARGE => NfcError::TooLarge { needed, capacity: reply.extra.max(0) as usize },
        NO_RAW_TECH => NfcError::NotAvailable("raw commands for this tag's technology"),
        ERROR => NfcError::Backend(reply.message.unwrap_or_else(|| "tag I/O failed".into())),
        other => NfcError::Backend(format!("unknown RustNfc status {other}")),
    }
}

/// Map an `android.nfc.tech.*` class name to a [`Technology`].
fn technology(class: &str) -> Option<Technology> {
    Some(match class.rsplit('.').next()? {
        "NfcA" => Technology::NfcA,
        "NfcB" => Technology::NfcB,
        "NfcF" => Technology::NfcF,
        "NfcV" => Technology::NfcV,
        "IsoDep" => Technology::IsoDep,
        "MifareClassic" => Technology::MifareClassic,
        "MifareUltralight" => Technology::MifareUltralight,
        "Ndef" => Technology::Ndef,
        "NdefFormatable" => Technology::NdefFormatable,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// JNI exports — called by the Kotlin shim.
// ---------------------------------------------------------------------------

/// `RustNfc.nativeTagDiscovered` — a tag entered the field (NFC binder
/// thread).
#[no_mangle]
pub extern "system" fn Java_io_idealyst_nfc_RustNfc_nativeTagDiscovered(
    mut env: JNIEnv,
    _class: JClass,
    session: jlong,
    handle: jlong,
    uid: JByteArray,
    techs: JObjectArray,
) {
    // Never unwind into the JVM.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let id = env.convert_byte_array(&uid).unwrap_or_default();
        let mut technologies = Vec::new();
        let count = env.get_array_length(&techs).unwrap_or(0);
        for i in 0..count {
            let Ok(obj) = env.get_object_array_element(&techs, i) else { continue };
            let name: Option<String> =
                env.get_string(&JString::from(obj)).ok().map(|s| s.into());
            if let Some(t) = name.as_deref().and_then(technology) {
                technologies.push(t);
            }
        }
        let mailbox = sessions().lock().unwrap().get(&(session as u64)).cloned();
        if let Some(mailbox) = mailbox {
            mailbox.push(Discovered { id, technologies, handle });
        }
    }));
    if result.is_err() {
        log::error!("nfc: panic in nativeTagDiscovered; aborting");
        std::process::abort();
    }
}

/// `RustNfc.nativeOpResult` — a tag operation finished (the shim's I/O
/// thread).
#[no_mangle]
pub extern "system" fn Java_io_idealyst_nfc_RustNfc_nativeOpResult(
    mut env: JNIEnv,
    _class: JClass,
    op: jlong,
    status: jint,
    data: JByteArray,
    message: JString,
    extra: jint,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let data = (!data.is_null()).then(|| env.convert_byte_array(&data).ok()).flatten();
        let message =
            (!message.is_null()).then(|| env.get_string(&message).ok().map(Into::into)).flatten();
        if let Some(tx) = pending().lock().unwrap().remove(&(op as u64)) {
            let _ = tx.send(OpReply { status, data, message, extra });
        }
    }));
    if result.is_err() {
        log::error!("nfc: panic in nativeOpResult; aborting");
        std::process::abort();
    }
}

// Keep the exports in the app `cdylib`'s dynamic symbol table: the JVM
// resolves them by `dlsym`, and an unreferenced `#[no_mangle]` fn in a
// dependency rlib can be dropped by the linker.
#[used]
static KEEP_TAG_DISCOVERED: extern "system" fn(JNIEnv, JClass, jlong, jlong, JByteArray, JObjectArray) =
    Java_io_idealyst_nfc_RustNfc_nativeTagDiscovered;
#[used]
static KEEP_OP_RESULT: extern "system" fn(JNIEnv, JClass, jlong, jint, JByteArray, JString, jint) =
    Java_io_idealyst_nfc_RustNfc_nativeOpResult;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tech_class_names_map_to_technologies() {
        assert_eq!(technology("android.nfc.tech.IsoDep"), Some(Technology::IsoDep));
        assert_eq!(technology("android.nfc.tech.NdefFormatable"), Some(Technology::NdefFormatable));
        assert_eq!(technology("android.nfc.tech.NfcBarcode"), None);
    }
}
