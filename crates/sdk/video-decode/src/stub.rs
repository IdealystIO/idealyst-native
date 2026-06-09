//! Fallback decoder for targets with no implementation (desktop Linux/Windows).
//! `open` always reports [`VideoDecodeError::Unsupported`]; callers keep
//! compiling everywhere.

use media_stream::{AudioWriter, FrameWriter};

use crate::{DecodeConfig, DecodeSource, Opened, VideoDecodeError};

pub(crate) async fn open(
    _source: DecodeSource,
    _config: DecodeConfig,
    _frames: FrameWriter,
    _audio: AudioWriter,
) -> Result<Opened, VideoDecodeError> {
    Err(VideoDecodeError::Unsupported)
}
