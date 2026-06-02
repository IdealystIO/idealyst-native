//! iOS / macOS biometric auth via **LocalAuthentication** (`LAContext`).
//!
//! - [`availability`](crate::BiometricAuthenticator::availability) calls
//!   `canEvaluatePolicy:error:` and reads `biometryType` to report Touch ID
//!   vs Face ID (vs none / not-enrolled).
//! - [`authenticate`](crate::BiometricAuthenticator::authenticate) calls
//!   `evaluatePolicy:localizedReason:reply:`, whose completion is a
//!   `void(^)(BOOL, NSError*)` block. We bridge that block to the returned
//!   future over a `futures-channel` oneshot (the same pattern the
//!   `microphone` SDK uses for `AVAudioSession.requestRecordPermission:`).
//!
//! ## Object lifetime invariant (why the block owns the context)
//!
//! `evaluatePolicy:` is asynchronous: it `Block_copy`s the reply block onto
//! a private queue and fires it later, off the calling thread. Two things
//! must outlive the call: the **reply block** (the system holds its own
//! retained copy, so dropping ours right after the call is fine) and the
//! **`LAContext`** (its lifetime is *not* guaranteed by the async op). We
//! pin the context's lifetime to the reply block by moving a releasing
//! guard into the block's captured state — when the system finally releases
//! its block copy after firing, the guard drops and releases the context.
//! The returned future therefore holds only the oneshot `Receiver`, which
//! is `Send`, so it parks cleanly on a multi-threaded executor.

use std::cell::Cell;
use std::ptr;

use block2::RcBlock;
use futures_channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::NSString;

use crate::{AuthFuture, AuthRequest, Authentication, BioError, Biometry, BiometricAuthenticator};

// Link LocalAuthentication so the `LAContext` class is registered for
// `class!()` lookup. We use the framework only through the Obj-C runtime,
// so no symbol imports are needed — an empty extern block suffices.
#[link(name = "LocalAuthentication", kind = "framework")]
extern "C" {}

// LAPolicy values (LocalAuthentication/LAContext.h).
const POLICY_BIOMETRICS: isize = 1; // deviceOwnerAuthenticationWithBiometrics
const POLICY_BIOMETRICS_OR_PASSCODE: isize = 2; // deviceOwnerAuthentication

// LABiometryType values.
const BIOMETRY_TOUCH_ID: isize = 1;
const BIOMETRY_FACE_ID: isize = 2;
const BIOMETRY_OPTIC_ID: isize = 4; // visionOS

// LAError codes (negative; LAError.h). Only the ones we map specially.
const ERR_AUTH_FAILED: isize = -1;
const ERR_USER_CANCEL: isize = -2;
const ERR_USER_FALLBACK: isize = -3;
const ERR_SYSTEM_CANCEL: isize = -4;
const ERR_PASSCODE_NOT_SET: isize = -5;
const ERR_BIOMETRY_NOT_AVAILABLE: isize = -6;
const ERR_BIOMETRY_NOT_ENROLLED: isize = -7;
const ERR_BIOMETRY_LOCKOUT: isize = -8;
const ERR_APP_CANCEL: isize = -9;

/// Biometric auth over the Apple LocalAuthentication framework.
#[derive(Default)]
pub struct LocalAuthentication {
    _private: (),
}

impl LocalAuthentication {
    /// Create a LocalAuthentication-backed authenticator.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Releases a `+1`-retained `LAContext` on drop. Moved into the reply
/// block so the context outlives the async `evaluatePolicy:` operation
/// (see the module-level lifetime invariant).
struct CtxGuard(*mut AnyObject);

impl Drop for CtxGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _: () = msg_send![self.0, release];
            }
        }
    }
}

impl BiometricAuthenticator for LocalAuthentication {
    fn availability(&self) -> Biometry {
        unsafe {
            let ctx: *mut AnyObject = msg_send![class!(LAContext), new];
            if ctx.is_null() {
                return Biometry::None;
            }
            let _guard = CtxGuard(ctx);
            let mut err: *mut AnyObject = ptr::null_mut();
            let can: Bool = msg_send![ctx, canEvaluatePolicy: POLICY_BIOMETRICS, error: &mut err];
            if !can.as_bool() {
                return Biometry::None;
            }
            // biometryType is only meaningful once canEvaluatePolicy passed.
            let kind: isize = msg_send![ctx, biometryType];
            match kind {
                BIOMETRY_TOUCH_ID => Biometry::Fingerprint,
                BIOMETRY_FACE_ID => Biometry::Face,
                BIOMETRY_OPTIC_ID => Biometry::Unknown,
                _ => Biometry::Unknown,
            }
        }
    }

    fn authenticate(&self, request: AuthRequest) -> AuthFuture {
        let policy = if request.allow_device_credential {
            POLICY_BIOMETRICS_OR_PASSCODE
        } else {
            POLICY_BIOMETRICS
        };

        let (tx, rx) = oneshot::channel::<Result<Authentication, BioError>>();

        unsafe {
            let ctx: *mut AnyObject = msg_send![class!(LAContext), new];
            if ctx.is_null() {
                return Box::pin(async move {
                    Err(BioError::Backend("LAContext allocation failed".into()))
                });
            }
            let guard = CtxGuard(ctx);

            if let Some(cancel) = request.cancel_label.as_deref() {
                let title = NSString::from_str(cancel);
                let _: () = msg_send![ctx, setLocalizedCancelTitle: &*title];
            }

            // Pre-check so "no biometric enrolled / available" surfaces as
            // a clean Unavailable rather than reaching the prompt path.
            let mut precheck: *mut AnyObject = ptr::null_mut();
            let can: Bool = msg_send![ctx, canEvaluatePolicy: policy, error: &mut precheck];
            if !can.as_bool() {
                let why = ns_error_message(precheck);
                // No async op will run, so release the context now rather
                // than parking the non-`Send` guard inside the future.
                drop(guard);
                return Box::pin(async move { Err(BioError::Unavailable(why)) });
            }

            let reason = NSString::from_str(&request.reason);

            // The reply block fires exactly once. `Cell<Option<_>>` lets an
            // `Fn` block take the sender out on that single call; moving
            // `guard` in pins the LAContext's lifetime to the block.
            let tx_cell = Cell::new(Some(tx));
            let reply = RcBlock::new(move |success: Bool, error: *mut AnyObject| {
                // Keep the context alive until the block has fired.
                let _keep = &guard;
                let result = if success.as_bool() {
                    Ok(Authentication::default())
                } else {
                    Err(map_la_error(error))
                };
                if let Some(tx) = tx_cell.take() {
                    let _ = tx.send(result);
                }
            });

            let _: () = msg_send![
                ctx,
                evaluatePolicy: policy,
                localizedReason: &*reason,
                reply: &*reply,
            ];
            // `evaluatePolicy:` has Block_copy'd `reply`; the system holds a
            // retained copy that keeps `guard` (and thus the context) alive
            // until it fires. Dropping our `reply` here is safe.
        }

        Box::pin(async move {
            match rx.await {
                Ok(result) => result,
                // Sender dropped without firing — the block was never
                // called (shouldn't happen, but map it loudly rather than
                // hanging).
                Err(_) => Err(BioError::Backend(
                    "LocalAuthentication reply was dropped without firing".into(),
                )),
            }
        })
    }
}

/// Map an `LAError` `NSError*` to a typed [`BioError`].
///
/// # Safety
/// `error` must be a valid `NSError*` or null.
unsafe fn map_la_error(error: *mut AnyObject) -> BioError {
    if error.is_null() {
        return BioError::Failed;
    }
    let code: isize = msg_send![error, code];
    match code {
        ERR_AUTH_FAILED => BioError::Failed,
        ERR_USER_CANCEL | ERR_USER_FALLBACK | ERR_SYSTEM_CANCEL | ERR_APP_CANCEL => {
            BioError::Cancelled
        }
        ERR_PASSCODE_NOT_SET | ERR_BIOMETRY_NOT_AVAILABLE | ERR_BIOMETRY_NOT_ENROLLED => {
            BioError::Unavailable(ns_error_message(error))
        }
        ERR_BIOMETRY_LOCKOUT => BioError::Lockout,
        other => BioError::Backend(format!("LAError {other}: {}", ns_error_message(error))),
    }
}

/// Read `NSError.localizedDescription` into a Rust `String`.
///
/// # Safety
/// `error` must be a valid `NSError*` or null.
unsafe fn ns_error_message(error: *mut AnyObject) -> String {
    if error.is_null() {
        return "unknown error".into();
    }
    let desc: Option<Retained<NSString>> = msg_send_id![error, localizedDescription];
    match desc {
        Some(s) => s.to_string(),
        None => String::new(),
    }
}
