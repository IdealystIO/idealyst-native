//! Error type for the screen-recorder SDK.

use thiserror::Error;

/// Everything that can go wrong establishing or running a recording.
#[derive(Debug, Error)]
pub enum RecorderError {
    /// This target has no capture backend implemented yet. The skeleton
    /// returns this from every entry point until a platform's `imp`
    /// module is filled in.
    #[error("screen recording is not yet implemented on this platform")]
    Unsupported,

    /// The user declined the platform's capture consent prompt
    /// (ReplayKit / MediaProjection / Screen Recording TCC / portal /
    /// the `getDisplayMedia` picker).
    #[error("the screen recording permission was denied")]
    PermissionDenied,

    /// The requested [`crate::Source`] isn't expressible on this target
    /// — e.g. recording an arbitrary other window on iOS/Android, which
    /// only the desktop backends support. The string names the source.
    #[error("source not available on this platform: {0}")]
    UnsupportedSource(&'static str),

    /// A platform capture API failed at runtime. The string carries the
    /// backend-specific diagnostic (NSError description, JNI exception
    /// message, DOM exception name, …).
    #[error("screen recording failed: {0}")]
    Platform(String),
}
