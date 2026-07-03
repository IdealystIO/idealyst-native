//! The chunk of audio handed to the callback.

/// One chunk of captured PCM, borrowed for the duration of the callback
/// call. Copy out what you need before returning — `samples` points at a
/// backend-owned buffer that's reused for the next chunk.
///
/// Samples are **normalized `f32` in `[-1.0, 1.0]`**, **interleaved** by
/// channel (`L R L R …` for stereo). Every backend converts its native
/// format (cpal `i16`/`u16`/`f32`, Android `AudioRecord` `i16`, Web Audio
/// `f32`) into this one shape, so consumer code is identical everywhere —
/// the backends diverge in mechanism, not in what you receive.
pub struct AudioBuffer<'a> {
    /// Interleaved, normalized `f32` PCM for this chunk.
    pub samples: &'a [f32],
    /// The actual sample rate of these samples, in Hz. Authoritative —
    /// it reflects what the device gave, not what was requested.
    pub sample_rate: u32,
    /// The actual channel count of these samples. `samples.len()` is
    /// `frame_count * channels`.
    pub channels: u16,
}

impl AudioBuffer<'_> {
    /// Number of sample frames in this chunk (one frame = one sample per
    /// channel).
    pub fn frame_count(&self) -> usize {
        let ch = self.channels.max(1) as usize;
        self.samples.len() / ch
    }

    /// Wall-clock duration this chunk represents, in seconds.
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frame_count() as f64 / self.sample_rate as f64
    }
}
