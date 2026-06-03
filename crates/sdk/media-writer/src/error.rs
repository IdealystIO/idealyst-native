//! The error type the writer surfaces.

/// What can go wrong starting or finishing a recording.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MediaWriterError {
    /// [`record`](crate::MediaWriter::record) was called with neither a video
    /// nor an audio input — there is nothing to write.
    #[error("no input stream provided (need at least one of video / audio)")]
    NoInput,

    /// Recording isn't implemented for this platform/target.
    #[error("media recording is not supported on this platform")]
    Unsupported,

    /// The output destination couldn't be resolved to a real local file path
    /// for a native muxer (a native backend requires a filesystem path —
    /// see [`files::FileStore::local_path`]).
    #[error("output path could not be resolved to a local file")]
    NoLocalPath,

    /// Writing the finished file (or, on web, the recorded blob) failed.
    #[error("file error: {0}")]
    File(#[from] files::FileError),

    /// The platform encoder/muxer rejected the configuration or a sample —
    /// the message carries the backend's reason.
    #[error("encoder error: {0}")]
    Backend(String),
}
