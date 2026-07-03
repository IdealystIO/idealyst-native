//! Fallback backend for targets without a native encoder/muxer. Every entry
//! point reports [`MediaWriterError::Unsupported`] so author code degrades
//! predictably rather than silently dropping the recording.

use crate::{MediaInputs, MediaWriterError, RecordConfig};

pub(crate) struct RecordingHandle;

impl RecordingHandle {
    pub(crate) async fn stop(self) -> Result<(), MediaWriterError> {
        Err(MediaWriterError::Unsupported)
    }
}

pub(crate) async fn start(
    _inputs: MediaInputs<'_>,
    _config: &RecordConfig,
) -> Result<RecordingHandle, MediaWriterError> {
    Err(MediaWriterError::Unsupported)
}
