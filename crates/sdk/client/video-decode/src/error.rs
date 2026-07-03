//! Error type for [`VideoDecoder`](crate::VideoDecoder).

/// What can go wrong opening / decoding a clip.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VideoDecodeError {
    /// The source URL was malformed or pointed at nothing openable.
    #[error("invalid or unreadable source: {0}")]
    BadSource(String),
    /// The platform decoder failed to open or configure the clip.
    #[error("decoder backend error: {0}")]
    Backend(String),
    /// File-based video decode is not implemented on this target.
    #[error("video decoding is not supported on this platform")]
    Unsupported,
}
