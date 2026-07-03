//! The error type the updater surfaces.

use crate::manifest::{ManifestError, SignatureError};

/// What can go wrong across a check / download / install cycle.
///
/// Note that "already up to date" and "this build can't self-update" are
/// **not** errors — they're [`UpdateState::UpToDate`](crate::UpdateState::UpToDate)
/// and [`UpdateState::Unsupported`](crate::UpdateState::Unsupported). These
/// variants are genuine failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UpdateError {
    /// Self-update isn't available on this platform/target (iOS, Android,
    /// web, or an unrecognized target). The app should defer to the store /
    /// a page reload.
    #[error("self-update is not supported on this platform")]
    Unsupported,

    /// The release manifest couldn't be fetched (network / HTTP error).
    #[error("could not fetch release manifest: {0}")]
    Fetch(String),

    /// The fetched manifest didn't parse or used an unknown schema.
    #[error(transparent)]
    Manifest(#[from] ManifestError),

    /// A release entry's signature didn't verify — refuse to install.
    #[error("release signature check failed: {0}")]
    Signature(#[from] SignatureError),

    /// The downloaded artifact's SHA-256 didn't match the signed digest.
    #[error("downloaded artifact digest mismatch (expected {expected}, got {actual})")]
    DigestMismatch {
        /// The digest the signed manifest promised.
        expected: String,
        /// The digest we actually computed over the download.
        actual: String,
    },

    /// The platform installer / relaunch step failed — the message carries the
    /// backend's reason (Sparkle error, MSIX failure, AppImage swap failure).
    #[error("install failed: {0}")]
    Install(String),

    /// There is no update to download — `download_and_install` was called
    /// before a `check` found one.
    #[error("no update available to install")]
    NothingToInstall,
}
