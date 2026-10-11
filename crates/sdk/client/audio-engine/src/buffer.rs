//! [`SoundBuffer`] — decoded, shareable PCM a voice plays from.

use std::sync::Arc;

use media_stream::AudioFormat;

use crate::EngineError;

/// The PCM a voice reads. Held in an `Arc` that the render thread shares;
/// see `mixer::Keepalive` for why the render thread never drops the last
/// reference.
#[derive(Debug)]
pub(crate) struct BufferData {
    /// The format the buffer was made from (what [`SoundBuffer::format`]
    /// reports).
    pub(crate) source_format: AudioFormat,
    /// Playback layout: `1` (mono) or `2` (stereo) interleaved channels.
    /// Wider sources are down-mixed once at construction with the Web Audio
    /// "speakers" rules — exactly what a `StereoPannerNode` (channelCount 2,
    /// `"clamped-max"`) does to them in the browser, so both backends feed
    /// the panner the same signal.
    pub(crate) channels: usize,
    /// Sample rate of `samples` (≥ 1).
    pub(crate) sample_rate: u32,
    /// Interleaved samples in the playback layout.
    pub(crate) samples: Vec<f32>,
    /// `samples.len() / channels`.
    pub(crate) frames: usize,
}

/// Decoded, shareable PCM. Cheap to clone (an `Arc` inside); play it on as
/// many voices at once as you like.
///
/// Build one from raw samples ([`from_samples`](Self::from_samples)), from a
/// [`synth::Pcm`] ([`from_pcm`](Self::from_pcm)), or by decoding a WAV / MP3
/// / Ogg-Vorbis / FLAC file ([`decode`](Self::decode)). Decoding is pure Rust
/// (symphonia) on every target, so a buffer holds the same samples on web,
/// mobile and desktop.
#[derive(Clone, Debug)]
pub struct SoundBuffer {
    pub(crate) data: Arc<BufferData>,
}

impl SoundBuffer {
    /// A buffer from interleaved, normalized `f32` samples in `format`. A
    /// trailing partial frame is dropped; a zero channel count or sample
    /// rate is raised to 1.
    pub fn from_samples(format: AudioFormat, interleaved: Vec<f32>) -> SoundBuffer {
        let format = AudioFormat {
            sample_rate: format.sample_rate.max(1),
            channels: format.channels.max(1),
        };
        let src_ch = format.channels as usize;
        let frames = interleaved.len() / src_ch;
        let (channels, samples) = match src_ch {
            1 | 2 => {
                let mut s = interleaved;
                s.truncate(frames * src_ch);
                (src_ch, s)
            }
            _ => (
                2,
                downmix_to_stereo(&interleaved[..frames * src_ch], src_ch),
            ),
        };
        SoundBuffer {
            data: Arc::new(BufferData {
                source_format: format,
                channels,
                sample_rate: format.sample_rate,
                samples,
                frames,
            }),
        }
    }

    /// A buffer holding a copy of a synthesized [`synth::Pcm`].
    pub fn from_pcm(pcm: &synth::Pcm) -> SoundBuffer {
        SoundBuffer::from_samples(pcm.format, pcm.samples.clone())
    }

    /// Decode a whole audio file held in memory: WAV (PCM / float), MP3,
    /// Ogg Vorbis or FLAC. Gapless metadata is honoured, so an MP3's encoder
    /// delay and padding are trimmed (a loop made from one doesn't click).
    pub fn decode(bytes: &[u8]) -> Result<SoundBuffer, EngineError> {
        let (format, samples) = decode::decode(bytes)?;
        Ok(SoundBuffer::from_samples(format, samples))
    }

    /// Length in seconds.
    pub fn duration_secs(&self) -> f64 {
        self.data.frames as f64 / self.data.sample_rate as f64
    }

    /// The format the buffer was made from (decoded file / `Pcm` / samples).
    pub fn format(&self) -> AudioFormat {
        self.data.source_format
    }

    /// The samples as played: interleaved, `playback_channels()` (1 or 2)
    /// wide, at `format().sample_rate`. For the web backend, which builds its
    /// `AudioBuffer` from the same down-mixed data the Rust mixer plays.
    #[doc(hidden)]
    pub fn playback_samples(&self) -> &[f32] {
        &self.data.samples
    }

    /// `1` or `2` — the channel count of [`playback_samples`](Self::playback_samples).
    #[doc(hidden)]
    pub fn playback_channels(&self) -> u16 {
        self.data.channels as u16
    }
}

/// Web Audio "speakers" down-mix to stereo (spec §1.2.4 "Channel Up-Mixing
/// and Down-Mixing"): quad and 5.1 have their own formulas; any other layout
/// falls back to "discrete" (keep the first two channels).
fn downmix_to_stereo(samples: &[f32], ch: usize) -> Vec<f32> {
    const SQRT_HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let mut out = Vec::with_capacity(samples.len() / ch * 2);
    for f in samples.chunks_exact(ch) {
        let (l, r) = match ch {
            // L R SL SR
            4 => (0.5 * (f[0] + f[2]), 0.5 * (f[1] + f[3])),
            // L R C LFE SL SR
            6 => (
                f[0] + SQRT_HALF * (f[2] + f[4]),
                f[1] + SQRT_HALF * (f[2] + f[5]),
            ),
            _ => (f[0], f[1]),
        };
        out.push(l);
        out.push(r);
    }
    out
}

mod decode {
    use std::io::Cursor;

    use media_stream::AudioFormat;
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    use crate::EngineError;

    pub(super) fn decode(bytes: &[u8]) -> Result<(AudioFormat, Vec<f32>), EngineError> {
        let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
        let fmt_opts = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let probed = symphonia::default::get_probe()
            .format(&Hint::new(), mss, &fmt_opts, &MetadataOptions::default())
            .map_err(|e| EngineError::Decode(format!("unrecognized audio: {e}")))?;
        let mut reader = probed.format;
        let track = reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| EngineError::Decode("no audio track".into()))?;
        let track_id = track.id;
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| EngineError::Decode(format!("unsupported codec: {e}")))?;

        let mut samples: Vec<f32> = Vec::new();
        let mut format: Option<AudioFormat> = None;
        let mut scratch: Option<SampleBuffer<f32>> = None;
        loop {
            let packet = match reader.next_packet() {
                Ok(p) => p,
                Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                // A chained stream changing its parameters; one buffer holds
                // one format, so stop at the first link.
                Err(Error::ResetRequired) => break,
                Err(e) => return Err(EngineError::Decode(e.to_string())),
            };
            if packet.track_id() != track_id {
                continue;
            }
            let decoded = match decoder.decode(&packet) {
                Ok(d) => d,
                // A corrupt packet: skip it, as players do.
                Err(Error::DecodeError(_)) => continue,
                Err(e) => return Err(EngineError::Decode(e.to_string())),
            };
            let spec = *decoded.spec();
            let this = AudioFormat {
                sample_rate: spec.rate,
                channels: spec.channels.count() as u16,
            };
            match format {
                None => format = Some(this),
                Some(f) if f != this => break,
                Some(_) => {}
            }
            let needed = decoded.capacity() as u64;
            let buf = match &mut scratch {
                Some(b) if b.capacity() as u64 >= needed * spec.channels.count() as u64 => b,
                _ => scratch.insert(SampleBuffer::<f32>::new(needed, spec)),
            };
            buf.copy_interleaved_ref(decoded);
            samples.extend_from_slice(buf.samples());
        }
        let format = format.ok_or_else(|| EngineError::Decode("no decodable audio".into()))?;
        Ok((format, samples))
    }
}
