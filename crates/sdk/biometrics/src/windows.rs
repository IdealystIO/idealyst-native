//! Windows biometric auth via **Windows Hello** — the WinRT
//! `UserConsentVerifier`. Windows Hello abstracts over the enrolled
//! modality (face / fingerprint / PIN), so this backend reports
//! [`Biometry::Unknown`] when a verifier is available rather than claiming a
//! specific one.
//!
//! ## The window-handle interop
//!
//! `UserConsentVerifier.RequestVerificationAsync` is a UWP API that assumes
//! a `CoreWindow`. A Win32 desktop app has none, so it must instead go
//! through `IUserConsentVerifierInterop::RequestVerificationForWindowAsync`,
//! which takes the foreground `HWND` to parent the Hello modal. We obtain
//! the handle with `GetForegroundWindow` (falling back to
//! `GetActiveWindow`), keeping this SDK decoupled from the GPU/winit backend
//! — the same standalone posture the `credentials` crate takes with the OS
//! credential vault.
//!
//! ## VERIFICATION
//!
//! Compile-checked for `x86_64-pc-windows-gnu`; the Hello UI itself is only
//! exercisable on a Windows host with an enrolled verifier.

use windows::core::HSTRING;
use windows::Foundation::IAsyncOperation;
use windows::Security::Credentials::UI::{
    UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::{AuthFuture, AuthRequest, Authentication, BioError, Biometry, BiometricAuthenticator};

/// Biometric auth over Windows Hello (`UserConsentVerifier`).
#[derive(Default)]
pub struct WindowsHello {
    _private: (),
}

impl WindowsHello {
    /// Create a Windows Hello authenticator.
    pub fn new() -> Self {
        Self::default()
    }
}

impl BiometricAuthenticator for WindowsHello {
    fn availability(&self) -> Biometry {
        // `CheckAvailabilityAsync` resolves quickly; block the probe on it
        // (`availability` is a synchronous query).
        match UserConsentVerifier::CheckAvailabilityAsync().and_then(|op| op.get()) {
            Ok(avail) if avail == UserConsentVerifierAvailability::Available => Biometry::Unknown,
            _ => Biometry::None,
        }
    }

    fn authenticate(&self, request: AuthRequest) -> AuthFuture {
        let message = request.reason.clone();
        // The Hello modal is synchronous (`IAsyncOperation::get` blocks
        // until the user responds) and `IAsyncOperation` doesn't implement
        // `IntoFuture` in this build, so run the whole ceremony on a worker
        // thread and bridge its result back over a oneshot. This keeps the
        // returned future non-blocking for whatever executor polls it.
        let (tx, rx) = futures_channel::oneshot::channel::<Result<Authentication, BioError>>();
        std::thread::spawn(move || {
            let _ = tx.send(verify_blocking(message));
        });
        Box::pin(async move {
            rx.await.unwrap_or_else(|_| {
                Err(BioError::Backend(
                    "Windows Hello worker thread ended without a result".into(),
                ))
            })
        })
    }
}

/// Run `RequestVerificationForWindowAsync` to completion (blocking) and map
/// the result. Called on a dedicated worker thread — the HWND is captured
/// here rather than across the thread boundary (it isn't `Send`).
fn verify_blocking(message: String) -> Result<Authentication, BioError> {
    let hwnd = foreground_window();

    // The runtime class's activation factory, narrowed to the interop
    // interface that accepts an HWND.
    let interop: IUserConsentVerifierInterop =
        windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>()
            .map_err(win_err)?;

    let operation: IAsyncOperation<UserConsentVerificationResult> = unsafe {
        interop
            .RequestVerificationForWindowAsync(hwnd, &HSTRING::from(&message))
            .map_err(win_err)?
    };

    let result = operation.get().map_err(win_err)?;
    map_result(result)
}

/// The window to parent the Hello modal to. `GetForegroundWindow` is the
/// app's active top-level window in the common case; `GetActiveWindow`
/// (this thread's active window) is the fallback when there's no system
/// foreground window.
fn foreground_window() -> HWND {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            GetActiveWindow()
        } else {
            fg
        }
    }
}

fn map_result(result: UserConsentVerificationResult) -> Result<Authentication, BioError> {
    use UserConsentVerificationResult as R;
    if result == R::Verified {
        Ok(Authentication::default())
    } else if result == R::Canceled {
        Err(BioError::Cancelled)
    } else if result == R::RetriesExhausted {
        Err(BioError::Lockout)
    } else if result == R::DeviceNotPresent || result == R::NotConfiguredForUser {
        Err(BioError::Unavailable(
            "Windows Hello has no enrolled verifier on this device".into(),
        ))
    } else if result == R::DisabledByPolicy {
        Err(BioError::Unsupported(
            "Windows Hello is disabled by device policy".into(),
        ))
    } else {
        // DeviceBusy and any future value.
        Err(BioError::Backend(format!(
            "UserConsentVerificationResult({})",
            result.0
        )))
    }
}

fn win_err(e: windows::core::Error) -> BioError {
    BioError::Backend(format!("Windows Hello: {e}"))
}
