//! The error type the picker surfaces.

/// What can go wrong presenting the open UI or reading a picked file.
///
/// Note that the user *dismissing* the picker is **not** an error — that's
/// [`PickOutcome::Cancelled`](crate::PickOutcome::Cancelled). These variants
/// are genuine failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PickError {
    /// Picking isn't implemented for this platform/target.
    #[error("file picking is not supported on this platform")]
    Unsupported,

    /// There was no window / activity to present the picker from (the app has
    /// no foreground UI surface).
    #[error("no foreground window to present the file picker from")]
    NoPresenter,

    /// A picked file couldn't be opened/read, or a destination couldn't be
    /// written (`copy_to`).
    #[error("i/o error: {0}")]
    Io(String),

    /// The platform picker / file API reported an error — the message carries
    /// the backend's reason.
    #[error("file picker error: {0}")]
    Backend(String),
}
