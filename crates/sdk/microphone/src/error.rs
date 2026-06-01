//! The single error type every backend maps its native failures into.

/// A microphone capture failure. Each backend translates its platform
/// errors (cpal `BuildStreamError`, a denied web `getUserMedia`, a JNI
/// exception, …) into one of these so callers handle one shape.
#[derive(Debug, thiserror::Error)]
pub enum MicError {
    /// The user (or OS policy) denied microphone access. On web this is
    /// a rejected `getUserMedia`; on iOS/Android a denied permission
    /// request; on desktop a TCC/privacy denial.
    #[error("microphone permission denied")]
    PermissionDenied,

    /// No audio input device is available to capture from.
    #[error("no audio input device available")]
    NoInputDevice,

    /// The requested [`AudioStreamConfig`](crate::AudioStreamConfig) (a
    /// sample rate / channel count) isn't supported by the device. The
    /// string carries the backend's detail.
    #[error("requested audio configuration is unsupported: {0}")]
    UnsupportedConfig(String),

    /// Capture isn't implemented for this build target at all.
    #[error("microphone capture is not supported on this platform")]
    Unsupported,

    /// A catch-all for a backend-specific failure that doesn't map to a
    /// more specific variant. The string is the underlying message.
    #[error("audio backend error: {0}")]
    Backend(String),
}
