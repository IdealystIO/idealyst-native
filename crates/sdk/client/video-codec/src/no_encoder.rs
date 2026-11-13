//! No encoder on this platform yet.

use crate::{EncoderConfig, EventSink, MediaStream, VideoCodecError};

pub(crate) struct Encoder;

impl Encoder {
    pub(crate) fn request_keyframe(&self) {}
}

pub(crate) fn start(
    _stream: &MediaStream,
    _config: EncoderConfig,
    _sink: EventSink,
) -> Result<(Encoder, (u32, u32)), VideoCodecError> {
    Err(VideoCodecError::Unsupported("encoding"))
}
