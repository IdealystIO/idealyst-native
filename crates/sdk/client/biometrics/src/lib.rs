//! Cross-platform **biometric authentication** — the raw "prove the device
//! owner is present" capability.
//!
//! This is the biometric counterpart to the [`credentials`] crate's stance
//! on secure storage: it is backed *only* by each platform's real
//! biometric facility, and it **refuses, loudly, where no such facility
//! exists** rather than pretending a gate happened.
//!
//! | Platform | Backend | Notes |
//! | --- | --- | --- |
//! | iOS / macOS | LocalAuthentication (`LAContext`) | Face ID / Touch ID |
//! | Android | `BiometricPrompt` + `BiometricManager` (framework, no androidx) | fingerprint / face / iris |
//! | Windows | Windows Hello (`UserConsentVerifier`) | face / fingerprint / PIN |
//! | Linux / other | **errors** | no standard biometric API |
//! | web | WebAuthn (`navigator.credentials.get`) | a passkey assertion the **server** verifies |
//!
//! # The capability
//!
//! Two operations, intentionally minimal (higher-level policy — *what* the
//! gate guards — belongs in app code or a richer SDK, per the project's
//! "SDKs stay unopinionated" rule):
//!
//! - [`BiometricAuthenticator::availability`] — what biometric, if any, is
//!   usable on this device *right now* (hardware present **and** enrolled).
//! - [`BiometricAuthenticator::authenticate`] — present the OS prompt and
//!   resolve success / a typed failure.
//!
//! ```no_run
//! # async fn demo() -> Result<(), biometrics::BioError> {
//! use biometrics::{platform_biometrics, AuthRequest, Biometry};
//!
//! let bio = platform_biometrics();
//! if matches!(bio.availability(), Biometry::None) {
//!     // No usable biometric — fall back to a password screen.
//!     return Ok(());
//! }
//!
//! bio.authenticate(AuthRequest::new("Unlock your vault")).await?;
//! // Authenticated. On native there's nothing more to do; on web the
//! // returned `Authentication::assertion` must be verified server-side.
//! # Ok(())
//! # }
//! ```
//!
//! # Why web is different (WebAuthn, not a local gate)
//!
//! A browser has **no local "is the owner present" API.** The only
//! biometric path on the web is **WebAuthn**: the platform authenticator
//! signs a server-issued challenge with a passkey, and the resulting
//! *assertion* is meaningful only when a **relying-party server verifies
//! the signature.** A browser-side success with nothing checking the
//! signature is trivially spoofable, so this crate does not pretend
//! otherwise:
//!
//! - On web, [`AuthRequest`] **must** carry a [`WebAuthnRequest`] (the
//!   server's challenge + relying-party id). Without one, web
//!   [`authenticate`](BiometricAuthenticator::authenticate) returns
//!   [`BioError::Unsupported`] explaining that a challenge is required.
//! - On success, web returns the [`WebAuthnAssertion`] in
//!   [`Authentication::assertion`]. **Send it to your server and verify
//!   it there** — that verification *is* the authentication.
//! - On native, the OS verifies locally; [`Authentication::assertion`] is
//!   `None` and there is nothing to send anywhere.
//!
//! [`credentials`]: https://docs.rs/credentials

#![deny(missing_docs)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Public value types
// ---------------------------------------------------------------------------

/// What biometric modality, if any, is usable on this device. Reflects
/// both hardware presence **and** enrollment — a phone with a fingerprint
/// sensor but no enrolled finger reports [`Biometry::None`], because an
/// [`authenticate`](BiometricAuthenticator::authenticate) call would fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Biometry {
    /// No usable biometric: no hardware, none enrolled, or the platform
    /// has no biometric API at all.
    None,
    /// Fingerprint — Touch ID (Apple), Android fingerprint, Windows Hello
    /// fingerprint.
    Fingerprint,
    /// Face — Face ID (Apple), Android face unlock, Windows Hello face.
    Face,
    /// Iris (Android).
    Iris,
    /// A usable biometric exists but the OS doesn't report which modality
    /// (e.g. Windows Hello, which abstracts over face/fingerprint/PIN).
    Unknown,
}

/// A biometric authentication failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BioError {
    /// No usable biometric on this device — no hardware, or nothing
    /// enrolled. The string carries the platform's reason. Distinct from
    /// [`BioError::Unsupported`]: the *platform* supports biometrics, this
    /// *device* just can't satisfy the request right now.
    Unavailable(String),
    /// The user dismissed the prompt (tapped Cancel / hit Esc / used the
    /// system "fall back" affordance without a configured fallback).
    Cancelled,
    /// The biometric was presented but did not match (wrong finger/face),
    /// and the OS gave up without locking out.
    Failed,
    /// Too many failed attempts — biometrics are temporarily (or until
    /// device-credential re-auth) locked out by the OS.
    Lockout,
    /// Biometric authentication isn't supported on this target at all
    /// (Linux/BSD without a standard API; web without a WebAuthn
    /// challenge). The string explains what to do instead.
    Unsupported(String),
    /// The underlying platform API failed (a system/WinRT/JNI error).
    Backend(String),
}

impl std::fmt::Display for BioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BioError::Unavailable(why) => write!(f, "no usable biometric: {why}"),
            BioError::Cancelled => write!(f, "biometric prompt cancelled by the user"),
            BioError::Failed => write!(f, "biometric did not match"),
            BioError::Lockout => write!(f, "biometrics locked out after too many attempts"),
            BioError::Unsupported(why) => write!(f, "biometric auth unavailable: {why}"),
            BioError::Backend(msg) => write!(f, "biometric backend error: {msg}"),
        }
    }
}

impl std::error::Error for BioError {}

/// A request to authenticate. Build it with [`AuthRequest::new`] and the
/// chainable setters; the only required field is the human-readable
/// [`reason`](AuthRequest::reason) the OS shows in its prompt.
#[derive(Debug, Clone, Default)]
pub struct AuthRequest {
    /// The localized reason shown in the system prompt ("Unlock your
    /// vault"). Required by iOS/macOS; used as the prompt subtitle on
    /// Android and the message on Windows Hello. Ignored on web (WebAuthn
    /// has no app-supplied prompt copy).
    pub reason: String,
    /// Prompt title (Android `setTitle`, the bold first line). Falls back
    /// to a generic title when `None`. Unused on iOS/macOS/Windows, whose
    /// prompts derive their title from the OS / app.
    pub title: Option<String>,
    /// Label for the prompt's negative button (Android `setNegativeButton`,
    /// iOS `localizedCancelTitle`). Defaults to the platform's "Cancel".
    pub cancel_label: Option<String>,
    /// Allow the device passcode / PIN / password as a fallback when
    /// biometrics fail or aren't enrolled. Maps to
    /// `LAPolicy.deviceOwnerAuthentication` (Apple),
    /// `BiometricManager.Authenticators.DEVICE_CREDENTIAL` (Android), and
    /// is implied by Windows Hello. When `false` (default), only a real
    /// biometric satisfies the gate.
    pub allow_device_credential: bool,
    /// **Web only, required there.** The WebAuthn challenge + relying-party
    /// parameters. Ignored on native; on web, its absence makes
    /// [`authenticate`](BiometricAuthenticator::authenticate) return
    /// [`BioError::Unsupported`].
    pub web_authn: Option<WebAuthnRequest>,
}

impl AuthRequest {
    /// A request that shows `reason` in the system prompt. Biometrics only
    /// (no device-credential fallback) until you opt in with
    /// [`allow_device_credential`](AuthRequest::allow_device_credential).
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            ..Default::default()
        }
    }

    /// Set the Android prompt title (the bold first line).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the negative-button / cancel label.
    pub fn cancel_label(mut self, label: impl Into<String>) -> Self {
        self.cancel_label = Some(label.into());
        self
    }

    /// Allow the device passcode/PIN as a fallback (see the field docs).
    pub fn allow_device_credential(mut self, allow: bool) -> Self {
        self.allow_device_credential = allow;
        self
    }

    /// Attach the WebAuthn challenge required on web (ignored on native).
    pub fn web_authn(mut self, request: WebAuthnRequest) -> Self {
        self.web_authn = Some(request);
        self
    }
}

/// The WebAuthn parameters for a web authentication ceremony. These come
/// from your **server**, which issues the random `challenge` and later
/// verifies the returned [`WebAuthnAssertion`].
#[derive(Debug, Clone, Default)]
pub struct WebAuthnRequest {
    /// Relying-party id — usually your registrable domain (`example.com`).
    /// `None` lets the browser default it to the current origin's domain.
    pub rp_id: Option<String>,
    /// The server-issued random challenge bytes. Must be unguessable and
    /// single-use; the assertion signs over it.
    pub challenge: Vec<u8>,
    /// Raw ids of the credentials (passkeys) the user may assert with. Empty
    /// = let the platform offer any discoverable credential for the rp.
    pub allow_credentials: Vec<Vec<u8>>,
    /// Ceremony timeout in milliseconds (`None` = browser default).
    pub timeout_ms: Option<u32>,
}

/// The outcome of a successful [`authenticate`](BiometricAuthenticator::authenticate).
///
/// On native this is empty — the OS verified the user locally and there's
/// nothing to carry. On web it holds the [`WebAuthnAssertion`] your server
/// must verify; *that verification is the real authentication.*
#[derive(Debug, Clone, Default)]
pub struct Authentication {
    /// Web only: the assertion to send to your relying-party server for
    /// signature verification. `None` on native.
    pub assertion: Option<WebAuthnAssertion>,
}

/// A WebAuthn assertion produced by `navigator.credentials.get`. Every
/// field is raw bytes destined for a server that knows the user's stored
/// public key; this crate does not (and cannot) verify it locally.
#[derive(Debug, Clone, Default)]
pub struct WebAuthnAssertion {
    /// The asserting credential's raw id.
    pub credential_id: Vec<u8>,
    /// `authenticatorData` — rp-id hash, flags (incl. user-verified), and
    /// signature counter.
    pub authenticator_data: Vec<u8>,
    /// `clientDataJSON` — the challenge, origin, and ceremony type the
    /// browser signed over.
    pub client_data_json: Vec<u8>,
    /// The assertion `signature` over (authenticatorData ‖ hash(clientDataJSON)).
    pub signature: Vec<u8>,
    /// The user handle the authenticator returned, if any.
    pub user_handle: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// The async return type.
//
// Native/Windows/Android backends run their completion off the calling
// thread, so the future must be `Send` to be held across an `.await` in a
// multi-threaded executor. The web backend awaits a JS promise on the
// single wasm thread holding non-`Send` JS values, so `Send` is both
// unnecessary and unsatisfiable there. One cfg'd alias keeps the trait
// signature identical on every target. (Mirrors `microphone`'s
// `AudioCallback` Send-split.)
// ---------------------------------------------------------------------------

/// The boxed future [`authenticate`](BiometricAuthenticator::authenticate)
/// returns. `Send` everywhere except web (single-threaded wasm).
#[cfg(not(target_arch = "wasm32"))]
pub type AuthFuture = Pin<Box<dyn Future<Output = Result<Authentication, BioError>> + Send>>;

/// See the non-wasm definition; on web the `Send` bound is dropped.
#[cfg(target_arch = "wasm32")]
pub type AuthFuture = Pin<Box<dyn Future<Output = Result<Authentication, BioError>>>>;

/// Biometric authentication for the current platform. Object-safe so an app
/// holds an `Arc<dyn BiometricAuthenticator>` and the backend is chosen per
/// platform by [`platform_biometrics`].
pub trait BiometricAuthenticator: Send + Sync {
    /// What biometric modality is usable on this device *right now*
    /// (hardware present and enrolled). [`Biometry::None`] when an
    /// [`authenticate`](BiometricAuthenticator::authenticate) call would
    /// fail for lack of a usable biometric, including on platforms with no
    /// biometric API.
    ///
    /// Synchronous and cheap — safe to call while building UI to decide
    /// whether to offer a biometric affordance. On web it reports whether a
    /// platform authenticator (passkey) exists, best-effort.
    fn availability(&self) -> Biometry;

    /// Present the platform's biometric prompt and resolve the outcome.
    ///
    /// Resolves `Ok(`[`Authentication`]`)` once the user authenticates, or
    /// a typed [`BioError`] (cancelled, failed, locked out, unavailable,
    /// unsupported, backend). The future is `'static` — it owns `request`
    /// and borrows nothing from `self`.
    fn authenticate(&self, request: AuthRequest) -> AuthFuture;
}

// ---------------------------------------------------------------------------
// Backend selection. Exactly one real backend compiles per target; Linux,
// other desktops, and the no-API fallthrough share the `Unsupported` shim.
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::LocalAuthentication;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::AndroidBiometrics;

// `windows.rs` is reached through `win` so the module name doesn't collide
// with the `windows` crate it depends on.
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod win;
#[cfg(target_os = "windows")]
pub use win::WindowsHello;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::WebAuthn;

/// A [`BiometricAuthenticator`] whose [`authenticate`](BiometricAuthenticator::authenticate)
/// always fails with [`BioError::Unsupported`] and whose
/// [`availability`](BiometricAuthenticator::availability) is always
/// [`Biometry::None`]. Used on platforms with no biometric API (Linux,
/// BSD, terminal/headless hosts) so a misrouted call surfaces a loud,
/// explanatory error instead of silently "succeeding."
pub struct UnsupportedBiometrics {
    reason: String,
}

impl UnsupportedBiometrics {
    /// Build an unsupported authenticator whose errors carry `reason`,
    /// explaining why biometric auth isn't available on this target.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl BiometricAuthenticator for UnsupportedBiometrics {
    fn availability(&self) -> Biometry {
        Biometry::None
    }

    fn authenticate(&self, _request: AuthRequest) -> AuthFuture {
        let reason = self.reason.clone();
        Box::pin(async move { Err(BioError::Unsupported(reason)) })
    }
}

/// Reason string for platforms with no biometric API wired (Linux, BSD,
/// and any non-Apple/Android/Windows/web target — e.g. a terminal host).
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "android",
        target_os = "windows"
    ))
))]
const UNSUPPORTED_REASON: &str = "this platform has no standard biometric authentication API \
    (Linux/BSD desktops expose no portable, app-usable biometric gate). Use a password/passphrase \
    or, for the web, the WebAuthn path this crate's web backend provides.";

/// An `Arc<dyn BiometricAuthenticator>` over the current platform's
/// biometric facility. Construction is infallible; on platforms without a
/// biometric API the returned authenticator reports [`Biometry::None`] and
/// every [`authenticate`](BiometricAuthenticator::authenticate) call fails
/// with [`BioError::Unsupported`] carrying guidance.
pub fn platform_biometrics() -> Arc<dyn BiometricAuthenticator> {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    return Arc::new(apple::LocalAuthentication::new());

    #[cfg(target_os = "android")]
    return Arc::new(android::AndroidBiometrics::new());

    #[cfg(target_os = "windows")]
    return Arc::new(win::WindowsHello::new());

    #[cfg(target_arch = "wasm32")]
    return Arc::new(web::WebAuthn::new());

    #[cfg(all(
        not(target_arch = "wasm32"),
        not(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "android",
            target_os = "windows"
        ))
    ))]
    return Arc::new(UnsupportedBiometrics::new(UNSUPPORTED_REASON));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_reports_none_and_errors() {
        let b = UnsupportedBiometrics::new("nope");
        assert_eq!(b.availability(), Biometry::None);
        let fut = b.authenticate(AuthRequest::new("x"));
        let out = futures_lite_block_on(fut);
        assert!(matches!(out, Err(BioError::Unsupported(_))));
    }

    /// `Arc<dyn BiometricAuthenticator>` is the object-safe shape apps hold.
    #[test]
    fn object_safe_behind_arc() {
        let b: Arc<dyn BiometricAuthenticator> = Arc::new(UnsupportedBiometrics::new("x"));
        assert_eq!(b.availability(), Biometry::None);
    }

    #[test]
    fn auth_request_builder_threads_fields() {
        let req = AuthRequest::new("Unlock")
            .title("Sign in")
            .cancel_label("Not now")
            .allow_device_credential(true);
        assert_eq!(req.reason, "Unlock");
        assert_eq!(req.title.as_deref(), Some("Sign in"));
        assert_eq!(req.cancel_label.as_deref(), Some("Not now"));
        assert!(req.allow_device_credential);
        assert!(req.web_authn.is_none());
    }

    #[test]
    fn web_authn_request_attaches() {
        let req = AuthRequest::new("Unlock").web_authn(WebAuthnRequest {
            rp_id: Some("example.com".into()),
            challenge: vec![1, 2, 3],
            allow_credentials: vec![vec![9, 9]],
            timeout_ms: Some(60_000),
        });
        let wa = req.web_authn.expect("web_authn set");
        assert_eq!(wa.rp_id.as_deref(), Some("example.com"));
        assert_eq!(wa.challenge, vec![1, 2, 3]);
    }

    #[test]
    fn bio_error_display_is_human_readable() {
        assert!(BioError::Cancelled.to_string().contains("cancelled"));
        assert!(BioError::Lockout.to_string().contains("locked out"));
        assert!(BioError::Unsupported("why".into())
            .to_string()
            .contains("why"));
    }

    /// Minimal inline executor so the core tests don't pull a runtime dep.
    /// Polls a future that we know completes immediately (the Unsupported
    /// authenticator's `async move { Err(..) }`).
    fn futures_lite_block_on<F: Future>(mut fut: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn noop(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        // Safety: `fut` is owned and not moved again after pinning.
        let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(v) => return v,
                Poll::Pending => continue,
            }
        }
    }
}
