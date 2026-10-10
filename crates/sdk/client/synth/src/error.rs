//! [`SynthError`].

/// Errors from decoding audio into a [`Pcm`](crate::Pcm). Synthesis itself
/// cannot fail; only parsing external bytes can.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SynthError {
    /// The bytes are not a well-formed RIFF/WAVE file (bad magic, truncated
    /// header, missing `fmt ` or `data` chunk).
    #[error("invalid WAV: {0}")]
    InvalidWav(String),
    /// A well-formed WAV in an encoding this crate does not decode (e.g.
    /// ADPCM, µ-law, or an unusual bit depth).
    #[error("unsupported WAV encoding: {0}")]
    UnsupportedWav(String),
}
