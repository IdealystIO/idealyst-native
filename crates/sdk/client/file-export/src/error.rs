//! The error type the exporter surfaces.

/// What can go wrong presenting the save UI or writing the file.
///
/// Note that the user *dismissing* the picker is **not** an error — that's
/// [`SaveOutcome::Cancelled`](crate::SaveOutcome::Cancelled). These variants
/// are genuine failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ExportError {
    /// Saving isn't implemented for this platform/target.
    #[error("file export is not supported on this platform")]
    Unsupported,

    /// There was no window / activity to present the picker from (the app
    /// has no foreground UI surface).
    #[error("no foreground window to present the save dialog from")]
    NoPresenter,

    /// The source couldn't be read, or the chosen destination couldn't be
    /// written.
    #[error("i/o error: {0}")]
    Io(String),

    /// The platform picker / file API reported an error — the message carries
    /// the backend's reason.
    #[error("save dialog error: {0}")]
    Backend(String),
}
