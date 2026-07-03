//! The single error type every backend maps its native failures into.

/// A camera capture failure. Each backend translates its platform errors
/// (a denied `AVCaptureDevice` authorization, a rejected web
/// `getUserMedia`, a Camera2 / JNI exception, …) into one of these so
/// callers handle one shape.
#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    /// The user (or OS policy) denied camera access. On web this is a
    /// rejected `getUserMedia`; on iOS/macOS/Android a denied permission
    /// request; on desktop a TCC/privacy denial.
    #[error("camera permission denied")]
    PermissionDenied,

    /// No camera device is available to capture from (no attached camera,
    /// or none matching the requested [`facing`](crate::CameraFacing)).
    #[error("no camera device available")]
    NoCamera,

    /// The requested [`CameraConfig`](crate::CameraConfig) (a resolution /
    /// frame rate / facing) isn't supported by the device. The string
    /// carries the backend's detail.
    #[error("requested camera configuration is unsupported: {0}")]
    UnsupportedConfig(String),

    /// Capture isn't implemented for this build target at all (e.g. a
    /// desktop Linux/Windows build, which has no camera backend yet).
    #[error("camera capture is not supported on this platform")]
    Unsupported,

    /// A catch-all for a backend-specific failure that doesn't map to a
    /// more specific variant. The string is the underlying message.
    #[error("camera backend error: {0}")]
    Backend(String),
}
