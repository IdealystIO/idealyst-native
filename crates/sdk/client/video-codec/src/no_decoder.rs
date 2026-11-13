//! No decoder on this platform yet.

use crate::{Codec, EncodedFrame, MediaStream, VideoCodecError};

pub(crate) struct Decoder;

impl Decoder {
    pub(crate) fn new(_codec: Codec) -> Result<Decoder, VideoCodecError> {
        Err(VideoCodecError::Unsupported("decoding"))
    }

    pub(crate) fn decode(&mut self, _frame: &EncodedFrame) -> bool {
        false
    }

    pub(crate) fn stream(&self) -> MediaStream {
        unreachable!("no decoder is made on this platform")
    }

    pub(crate) fn size(&self) -> Option<(u32, u32)> {
        None
    }
}
