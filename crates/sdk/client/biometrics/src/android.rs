//! Android biometric auth via the **framework** `BiometricPrompt`
//! (`android.hardware.biometrics`, API 28+ — *not* androidx, so no extra
//! AAR/gradle dependency) plus `BiometricManager` for availability.
//!
//! ## Why a Kotlin shim
//!
//! `BiometricPrompt.authenticate` takes an abstract
//! `BiometricPrompt.AuthenticationCallback`, and the prompt must be built
//! and shown on the **main (UI) thread**. Subclassing an abstract Java
//! class purely from JNI isn't feasible, so the callback lives in a tiny
//! Kotlin shim — [`RustBiometricPrompt`] — shipped from this crate via
//! `[package.metadata.idealyst.android].runtime_kotlin`. The shim posts the
//! build+show onto the main looper, subclasses the callback, and
//! trampolines terminal outcomes back through the [`nativeResult`] JNI
//! export below.
//!
//! ## Async bridge
//!
//! [`authenticate`](AndroidBiometrics::authenticate) mints a `u64` token,
//! parks the oneshot `Sender` in a process-global registry keyed by that
//! token, and hands the token to the Kotlin shim. When the prompt resolves,
//! `nativeResult(token, code, message)` pulls the sender back out and
//! completes the future. `code` is `0` on success, else the raw Android
//! `BiometricPrompt.BIOMETRIC_ERROR_*` code, mapped to a typed [`BioError`]
//! by [`map_android_result`].
//!
//! ## VERIFICATION
//!
//! Compile-checked for `aarch64-linux-android` here, but **not yet
//! device-verified** — JNI method signatures and the `nativeResult` symbol
//! export resolve only at runtime on a device. Every failure is surfaced as
//! a typed [`BioError`] with the JNI/Android message to make that diagnosis
//! quick (same posture as the `credentials` crate's Android backend). The
//! `nativeResult` export is pinned with `#[used]` so the linker keeps it in
//! the app's `cdylib` dynsym for `dlsym` resolution by the JVM.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use futures_channel::oneshot;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jint, jlong};
use jni::{JNIEnv, JavaVM};

use crate::{AuthFuture, AuthRequest, Authentication, BioError, Biometry, BiometricAuthenticator};

type AuthResult = Result<Authentication, BioError>;

// BiometricManager.Authenticators.BIOMETRIC_STRONG (Class 3 biometrics).
const AUTHENTICATOR_BIOMETRIC_STRONG: i32 = 0x000F;
// BiometricManager.BIOMETRIC_SUCCESS.
const BIOMETRIC_SUCCESS: i32 = 0;

// PackageManager hardware-feature strings (used to refine the reported
// modality — the framework gives no direct "which biometric is enrolled").
const FEATURE_FINGERPRINT: &str = "android.hardware.fingerprint";
const FEATURE_FACE: &str = "android.hardware.biometrics.face";
const FEATURE_IRIS: &str = "android.hardware.biometrics.iris";

// BiometricPrompt.BIOMETRIC_ERROR_* codes we map specially.
const ERR_HW_UNAVAILABLE: jint = 1;
const ERR_UNABLE_TO_PROCESS: jint = 2;
const ERR_TIMEOUT: jint = 3;
const ERR_CANCELED: jint = 5; // system canceled (e.g. screen off)
const ERR_LOCKOUT: jint = 7;
const ERR_LOCKOUT_PERMANENT: jint = 9;
const ERR_USER_CANCELED: jint = 10;
const ERR_NO_BIOMETRICS: jint = 11; // none enrolled
const ERR_HW_NOT_PRESENT: jint = 12;
const ERR_NEGATIVE_BUTTON: jint = 13;

/// Registry of in-flight prompts: token → the sender awaiting its result.
/// A process-global because the JVM trampoline (`nativeResult`) is a free
/// function with no handle back to the originating authenticator.
static PENDING: OnceLock<Mutex<HashMap<u64, oneshot::Sender<AuthResult>>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn pending() -> &'static Mutex<HashMap<u64, oneshot::Sender<AuthResult>>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Biometric auth over Android's framework `BiometricPrompt`.
#[derive(Default)]
pub struct AndroidBiometrics {
    _private: (),
}

impl AndroidBiometrics {
    /// Create an Android biometric authenticator.
    pub fn new() -> Self {
        Self::default()
    }
}

impl BiometricAuthenticator for AndroidBiometrics {
    fn availability(&self) -> Biometry {
        query_availability().unwrap_or(Biometry::None)
    }

    fn authenticate(&self, request: AuthRequest) -> AuthFuture {
        let (tx, rx) = oneshot::channel::<AuthResult>();
        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        pending().lock().unwrap().insert(token, tx);

        if let Err(err) = launch_prompt(&request, token) {
            // The shim never got a chance to call back — resolve the
            // parked sender ourselves with the launch error.
            if let Some(tx) = pending().lock().unwrap().remove(&token) {
                let _ = tx.send(Err(err));
            }
        }

        Box::pin(async move {
            match rx.await {
                Ok(result) => result,
                Err(_) => Err(BioError::Backend(
                    "biometric result channel dropped before a result arrived".into(),
                )),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// JNI helpers
// ---------------------------------------------------------------------------

fn java_vm() -> Result<JavaVM, BioError> {
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| BioError::Backend(format!("invalid JavaVM pointer: {e}")))
}

fn jni_err(e: jni::errors::Error) -> BioError {
    BioError::Backend(format!("JNI: {e}"))
}

/// The host Activity/Context pointer from `ndk_context`.
fn android_context<'a>() -> JObject<'a> {
    let ctx = ndk_context::android_context();
    unsafe { JObject::from_raw(ctx.context().cast()) }
}

/// Query `BiometricManager.canAuthenticate(BIOMETRIC_STRONG)` and, on
/// success, refine the modality from `PackageManager` hardware features.
/// The framework can't say *which* biometric is enrolled, so this is
/// best-effort: present-hardware order is fingerprint → face → iris →
/// [`Biometry::Unknown`].
fn query_availability() -> Result<Biometry, BioError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let ctx = android_context();

    let service_name = env.new_string("biometric").map_err(jni_err)?;
    let manager = env
        .call_method(
            &ctx,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[(&service_name).into()],
        )
        .map_err(jni_err)?
        .l()
        .map_err(jni_err)?;
    if manager.is_null() {
        return Ok(Biometry::None);
    }

    let status = env
        .call_method(
            &manager,
            "canAuthenticate",
            "(I)I",
            &[JValue::Int(AUTHENTICATOR_BIOMETRIC_STRONG)],
        )
        .map_err(jni_err)?
        .i()
        .map_err(jni_err)?;
    if status != BIOMETRIC_SUCCESS {
        return Ok(Biometry::None);
    }

    if has_feature(&mut env, &ctx, FEATURE_FINGERPRINT)? {
        Ok(Biometry::Fingerprint)
    } else if has_feature(&mut env, &ctx, FEATURE_FACE)? {
        Ok(Biometry::Face)
    } else if has_feature(&mut env, &ctx, FEATURE_IRIS)? {
        Ok(Biometry::Iris)
    } else {
        Ok(Biometry::Unknown)
    }
}

fn has_feature(env: &mut JNIEnv<'_>, ctx: &JObject<'_>, name: &str) -> Result<bool, BioError> {
    let pm = env
        .call_method(
            ctx,
            "getPackageManager",
            "()Landroid/content/pm/PackageManager;",
            &[],
        )
        .map_err(jni_err)?
        .l()
        .map_err(jni_err)?;
    let name_j = env.new_string(name).map_err(jni_err)?;
    env.call_method(
        &pm,
        "hasSystemFeature",
        "(Ljava/lang/String;)Z",
        &[(&name_j).into()],
    )
    .map_err(jni_err)?
    .z()
    .map_err(jni_err)
}

/// Invoke the Kotlin shim's static `authenticate`, handing it the token the
/// `nativeResult` callback will return.
fn launch_prompt(request: &AuthRequest, token: u64) -> Result<(), BioError> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(jni_err)?;
    let ctx = android_context();

    let title = env
        .new_string(request.title.as_deref().unwrap_or("Authenticate"))
        .map_err(jni_err)?;
    // The prompt subtitle carries the app's reason copy.
    let subtitle = env.new_string(&request.reason).map_err(jni_err)?;
    let negative = env
        .new_string(request.cancel_label.as_deref().unwrap_or("Cancel"))
        .map_err(jni_err)?;

    env.call_static_method(
        "io/idealyst/biometrics/RustBiometricPrompt",
        "authenticate",
        "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;ZJ)V",
        &[
            (&ctx).into(),
            (&title).into(),
            (&subtitle).into(),
            (&negative).into(),
            JValue::Bool(request.allow_device_credential as u8),
            JValue::Long(token as jlong),
        ],
    )
    .map_err(jni_err)?;
    Ok(())
}

/// Map a `(code, message)` from the Kotlin shim to a typed result.
fn map_android_result(code: jint, message: Option<String>) -> AuthResult {
    match code {
        0 => Ok(Authentication::default()),
        ERR_CANCELED | ERR_USER_CANCELED | ERR_NEGATIVE_BUTTON | ERR_TIMEOUT => {
            Err(BioError::Cancelled)
        }
        ERR_LOCKOUT | ERR_LOCKOUT_PERMANENT => Err(BioError::Lockout),
        ERR_HW_UNAVAILABLE | ERR_NO_BIOMETRICS | ERR_HW_NOT_PRESENT => {
            Err(BioError::Unavailable(message.unwrap_or_default()))
        }
        ERR_UNABLE_TO_PROCESS => Err(BioError::Failed),
        other => Err(BioError::Backend(format!(
            "BiometricPrompt error {other}: {}",
            message.unwrap_or_default()
        ))),
    }
}

// ---------------------------------------------------------------------------
// JNI export — the Kotlin shim's `nativeResult` trampoline.
// ---------------------------------------------------------------------------

/// `RustBiometricPrompt.nativeResult` — the terminal-result trampoline. The
/// Kotlin shim calls this with the token it was given plus the outcome:
/// `code == 0` for success, else the raw Android error code; `message` is
/// the platform error string (or null on success).
///
/// # Safety
/// Called by the JVM with valid `env`/`class`. `message` is a `String?`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_biometrics_RustBiometricPrompt_nativeResult(
    mut env: JNIEnv,
    _class: JClass,
    token: jlong,
    code: jint,
    message: JString,
) {
    // FFI boundary: never unwind into the JVM. Log + abort on panic rather
    // than corrupting the runtime (see the crash-loud-on-panic rule).
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let message = if message.is_null() {
            None
        } else {
            env.get_string(&message).ok().map(|s| s.into())
        };
        let outcome = map_android_result(code, message);
        if let Some(tx) = pending().lock().unwrap().remove(&(token as u64)) {
            let _ = tx.send(outcome);
        }
    }));
    if result.is_err() {
        eprintln!("biometrics: panic in nativeResult JNI trampoline; aborting");
        std::process::abort();
    }
}

/// Pin the `nativeResult` symbol so the linker keeps it in the app
/// `cdylib`'s dynamic symbol table (the JVM resolves it by `dlsym`). Without
/// `#[used]`, a dependency-rlib `#[no_mangle]` export can be GC'd when
/// nothing in Rust references it.
#[used]
static KEEP_NATIVE_RESULT: extern "system" fn(JNIEnv, JClass, jlong, jint, JString) =
    Java_io_idealyst_biometrics_RustBiometricPrompt_nativeResult;
