//! Cross-platform **secure** storage for secrets — auth tokens, API keys,
//! anything that must not be readable by other code, other users, or a
//! casual disk/backup inspection.
//!
//! This is the counterpart to the plaintext `storage` crate: the
//! `SecureStore` to its `AsyncStorage`. It is backed *only* by each
//! platform's real secure facility, and it **refuses, loudly, where real
//! security isn't achievable** rather than pretending:
//!
//! | Platform | Backend | Secure? |
//! | --- | --- | --- |
//! | iOS / macOS | Keychain (Security framework) | yes — OS/Secure-Enclave protected |
//! | Android | AES-GCM keyed by an AndroidKeyStore key (TEE/StrongBox) | yes |
//! | Windows | Credential Manager (via `keyring`) | yes — OS vault |
//! | Linux | Secret Service / GNOME Keyring / KWallet (via `keyring`) | yes — OS vault |
//! | web | **errors** — see below | n/a |
//!
//! # Why web errors (and what to do instead)
//!
//! A browser has **no secure client-side store.** Anything your code can
//! read, any script on your origin can read — so an XSS gets it, no matter
//! how it's "encrypted." Calling browser storage "secure" is the dangerous
//! false pretense this crate exists to avoid, so on web every operation
//! returns [`CredError::Unsupported`].
//!
//! The correct pattern for web secrets is **server-side**: a server
//! function validates the login and sets an **httpOnly** session cookie
//! (which JS can't read), and subsequent server-fn calls send it
//! automatically. The secret never enters the browser's JS at all. The
//! `server` SDK provides `set_cookie` for exactly this (the BFF pattern);
//! see this crate's README.
//!
//! # Using a credential as the server-fn auth token
//!
//! [`Credentials::get`] is synchronous, so it drops straight into the
//! server-fn bearer source. The *same* glue works on every platform:
//!
//! ```ignore
//! let creds = credentials::platform_credentials("myapp");
//! server::configure(
//!     server::ClientConfig::new("https://api.example.com")
//!         .with_credentials(server::bearer({
//!             let creds = creds.clone();
//!             move || creds.get("token").ok().flatten()
//!         })),
//! );
//! ```
//!
//! On native this reads the token from the Keychain/Keystore and attaches
//! `Authorization: Bearer …`. On web `get` errors → `None` → no bearer
//! header is sent, which is correct: the httpOnly session cookie carries
//! auth there instead.

use std::sync::Arc;

/// A secure-storage failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredError {
    /// The OS denied access to the secure store (e.g. a Keychain ACL
    /// rejection, a locked device, a denied biometric).
    Denied,
    /// Secure storage isn't available on this platform — most importantly
    /// the **web**, where there is no secure client store. The string
    /// explains what to do instead.
    Unsupported(String),
    /// The underlying secure backend failed (platform API / crypto error).
    Backend(String),
}

impl std::fmt::Display for CredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredError::Denied => write!(f, "secure store access was denied"),
            CredError::Unsupported(why) => write!(f, "secure storage unavailable: {why}"),
            CredError::Backend(msg) => write!(f, "secure store backend error: {msg}"),
        }
    }
}

impl std::error::Error for CredError {}

/// Synchronous secure key-value access for secrets. Object-safe so an app
/// holds an `Arc<dyn Credentials>` and the backend is chosen per platform
/// by [`platform_credentials`].
///
/// Synchronous because every real backend (Keychain, Keystore) is, and a
/// sync `get` plugs directly into `server::bearer(|| creds.get(k))`.
pub trait Credentials: Send + Sync {
    /// The secret at `key`, or `None` if absent. `Err` on a backend
    /// failure or on a platform without secure storage (web/desktop).
    fn get(&self, key: &str) -> Result<Option<String>, CredError>;
    /// Store `value` at `key` in the secure store, replacing any existing
    /// value. `Err` where secure storage isn't available.
    fn set(&self, key: &str, value: &str) -> Result<(), CredError>;
    /// Remove `key`. `Ok(())` whether or not it was present.
    fn remove(&self, key: &str) -> Result<(), CredError>;
}

// Exactly one native backend compiles per target; web + unsupported
// desktop share the `unsupported` shim.
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::KeychainCredentials;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::KeystoreCredentials;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod desktop;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use desktop::DesktopCredentials;

/// A [`Credentials`] whose every operation fails with
/// [`CredError::Unsupported`]. Used on web (no secure client store) and on
/// desktop platforms whose OS vault isn't wired yet — so a misplaced
/// secret surfaces a loud, explanatory error instead of silently landing
/// somewhere insecure.
pub struct UnsupportedCredentials {
    reason: String,
}

impl UnsupportedCredentials {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl Credentials for UnsupportedCredentials {
    fn get(&self, _key: &str) -> Result<Option<String>, CredError> {
        Err(CredError::Unsupported(self.reason.clone()))
    }
    fn set(&self, _key: &str, _value: &str) -> Result<(), CredError> {
        Err(CredError::Unsupported(self.reason.clone()))
    }
    fn remove(&self, _key: &str) -> Result<(), CredError> {
        Err(CredError::Unsupported(self.reason.clone()))
    }
}

/// The web reason string — points at the server-side / httpOnly pattern.
#[cfg(target_arch = "wasm32")]
const WEB_REASON: &str = "a browser has no secure client-side store (anything readable by your \
    code is readable by any script on your origin). Keep secrets server-side: use a server \
    function that sets an httpOnly session cookie (the BFF pattern), not client storage.";

/// Reason string for desktop platforms with no wired OS vault (i.e. not
/// Windows or Linux — e.g. a BSD). Windows/Linux use the real vault.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "android",
        target_os = "windows",
        target_os = "linux"
    ))
))]
const DESKTOP_REASON: &str = "secure credential storage isn't wired for this OS yet (Windows \
    Credential Manager and Linux Secret Service are supported; others are not).";

/// An `Arc<dyn Credentials>` over the current platform's secure store,
/// namespaced by `name` (the Keychain service / Keystore alias / prefs
/// file). Construction is infallible; on platforms without a secure store
/// (web, and not-yet-wired desktop) the returned store's operations all
/// fail with [`CredError::Unsupported`] carrying guidance.
pub fn platform_credentials(name: &str) -> Arc<dyn Credentials> {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    return Arc::new(apple::KeychainCredentials::new(name));

    #[cfg(target_os = "android")]
    return Arc::new(android::KeystoreCredentials::new(name));

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    return Arc::new(desktop::DesktopCredentials::new(name));

    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        return Arc::new(UnsupportedCredentials::new(WEB_REASON));
    }

    #[cfg(all(
        not(target_arch = "wasm32"),
        not(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "android",
            target_os = "windows",
            target_os = "linux"
        ))
    ))]
    {
        let _ = name;
        return Arc::new(UnsupportedCredentials::new(DESKTOP_REASON));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_errors_on_every_op() {
        let c = UnsupportedCredentials::new("nope");
        assert!(matches!(c.get("k"), Err(CredError::Unsupported(_))));
        assert!(matches!(c.set("k", "v"), Err(CredError::Unsupported(_))));
        assert!(matches!(c.remove("k"), Err(CredError::Unsupported(_))));
    }

    /// `Arc<dyn Credentials>` is the object-safe shape apps hold.
    #[test]
    fn object_safe_behind_arc() {
        let c: Arc<dyn Credentials> = Arc::new(UnsupportedCredentials::new("x"));
        assert!(c.get("k").is_err());
    }
}
