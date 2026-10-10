//! [`Pcm`] — the owned sample buffer every generator renders into and every
//! pipeline exit reads from.

use media_stream::{AudioFormat, AudioFrame, AudioWriter};

use crate::dsp;
use crate::envelope::Envelope;
use crate::error::SynthError;

/// Frames a buffer of `secs` holds at `format`'s rate (rounded to nearest;
/// negative / NaN durations are empty).
pub(crate) fn frames_for(format: AudioFormat, secs: f32) -> usize {
    if secs.is_nan() || secs <= 0.0 {
        return 0;
    }
    (secs as f64 * format.sample_rate as f64).round() as usize
}

/// Frames per chunk [`Pcm::write_to`] pushes. ~23 ms at 44.1 kHz: small
/// enough that a subscriber sees a steady chunk cadence, large enough to keep
/// per-chunk overhead negligible.
const WRITE_CHUNK_FRAMES: usize = 1024;

/// An owned buffer of audio: interleaved, normalized `f32` samples plus the
/// [`AudioFormat`] they are in — the same layout as
/// [`media_stream::AudioFrame`], owned instead of borrowed.
///
/// `samples.len()` is `frame_count * channels`; samples are in `[-1, 1]`
/// (every operation that can push them out of range — [`gain`](Self::gain),
/// [`mix`](Self::mix), [`normalize`](Self::normalize) — clamps).
///
/// Transformations take `self` and return the result, so effects chain:
///
/// ```
/// use synth::{noise, Envelope, Noise, DEFAULT_FORMAT};
/// let boom = noise(Noise::Brown, 0.6, DEFAULT_FORMAT, 7)
///     .low_pass(400.0)
///     .apply_envelope(Envelope::percussive(0.005, 0.6))
///     .normalize(0.9);
/// assert_eq!(boom.frame_count(), 26_460);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    /// Sample rate + channel count of [`samples`](Self::samples).
    pub format: AudioFormat,
    /// Interleaved (`L R L R …`), normalized `f32` samples.
    pub samples: Vec<f32>,
}

impl Pcm {
    /// A buffer from raw interleaved samples. A trailing partial frame (when
    /// `samples.len()` isn't a multiple of the channel count) is dropped.
    pub fn new(format: AudioFormat, mut samples: Vec<f32>) -> Self {
        let ch = format.channels.max(1) as usize;
        samples.truncate(samples.len() / ch * ch);
        Pcm { format, samples }
    }

    /// `secs` of silence.
    pub fn silence(format: AudioFormat, secs: f32) -> Self {
        let ch = format.channels.max(1) as usize;
        Pcm {
            format,
            samples: vec![0.0; frames_for(format, secs) * ch],
        }
    }

    /// Copy one captured [`AudioFrame`] (e.g. from a `microphone`
    /// [`AudioStream::subscribe`](media_stream::AudioStream::subscribe)
    /// callback) into a buffer in the frame's own format.
    pub fn from_frame(frame: &AudioFrame) -> Self {
        Pcm::new(frame.format(), frame.samples.to_vec())
    }

    /// Append a captured [`AudioFrame`], converting it to this buffer's format
    /// first (re-mixed and linearly resampled) if it differs. Use it to
    /// accumulate a stream into one buffer:
    ///
    /// ```ignore
    /// let take = Arc::new(Mutex::new(Pcm::silence(DEFAULT_FORMAT, 0.0)));
    /// let sink = take.clone();
    /// let _sub = mic.subscribe(move |f| sink.lock().unwrap().append_frame(f));
    /// ```
    ///
    /// Each frame is resampled on its own, so a stream whose rate differs from
    /// the buffer's can be off by up to one frame per chunk at the seams —
    /// inaudible for capture, and avoided entirely by accumulating in the
    /// stream's own format.
    pub fn append_frame(&mut self, frame: &AudioFrame) {
        let converted = convert_samples(frame.samples, frame.format(), self.format);
        self.samples.extend_from_slice(&converted);
    }

    /// Number of sample frames (one sample per channel).
    pub fn frame_count(&self) -> usize {
        self.samples.len() / self.format.channels.max(1) as usize
    }

    /// Duration in seconds.
    pub fn duration_secs(&self) -> f64 {
        if self.format.sample_rate == 0 {
            return 0.0;
        }
        self.frame_count() as f64 / self.format.sample_rate as f64
    }

    /// The peak absolute sample value.
    pub fn peak(&self) -> f32 {
        self.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    // ----- level -----------------------------------------------------------

    /// Multiply every sample by `factor`, clamping to `[-1, 1]`.
    pub fn gain(mut self, factor: f32) -> Self {
        for s in &mut self.samples {
            *s = (*s * factor).clamp(-1.0, 1.0);
        }
        self
    }

    /// Scale so the loudest sample is exactly `peak` (clamped to `0..=1`).
    /// A silent buffer is returned unchanged.
    pub fn normalize(self, peak: f32) -> Self {
        let current = self.peak();
        if current <= 0.0 {
            return self;
        }
        let target = peak.clamp(0.0, 1.0);
        self.gain(target / current)
    }

    /// Shape the amplitude with an ADSR [`Envelope`] laid over the whole
    /// buffer (see [`Envelope`] for the fixed-length semantics).
    pub fn apply_envelope(mut self, envelope: Envelope) -> Self {
        let ch = self.format.channels.max(1) as usize;
        let rate = self.format.sample_rate.max(1) as f32;
        let total = self.frame_count() as f32 / rate;
        for (i, frame) in self.samples.chunks_mut(ch).enumerate() {
            let g = envelope.gain_at(i as f32 / rate, total);
            for s in frame {
                *s *= g;
            }
        }
        self
    }

    /// Linear fade from silence over the first `secs`.
    pub fn fade_in(mut self, secs: f32) -> Self {
        let n = frames_for(self.format, secs).min(self.frame_count());
        let ch = self.format.channels.max(1) as usize;
        for (i, frame) in self.samples.chunks_mut(ch).take(n).enumerate() {
            let g = i as f32 / n as f32;
            frame.iter_mut().for_each(|s| *s *= g);
        }
        self
    }

    /// Linear fade to silence over the last `secs` (the final frame is `0`).
    pub fn fade_out(mut self, secs: f32) -> Self {
        let frames = self.frame_count();
        let n = frames_for(self.format, secs).min(frames);
        let ch = self.format.channels.max(1) as usize;
        let start = frames - n;
        for (i, frame) in self.samples.chunks_mut(ch).skip(start).enumerate() {
            let g = if n <= 1 { 0.0 } else { 1.0 - i as f32 / (n - 1) as f32 };
            frame.iter_mut().for_each(|s| *s *= g);
        }
        self
    }

    // ----- combining -------------------------------------------------------

    /// Sum `other` into this buffer from the start (see [`mix_at`](Self::mix_at)).
    pub fn mix(self, other: &Pcm) -> Self {
        self.mix_at(other, 0.0)
    }

    /// Sum `other` into this buffer starting `offset_secs` in, clamping the
    /// result to `[-1, 1]`. `other` is converted to this buffer's format first.
    /// The result is as long as whichever ends later (this buffer is padded
    /// with silence if `other` runs past its end).
    pub fn mix_at(mut self, other: &Pcm, offset_secs: f32) -> Self {
        let ch = self.format.channels.max(1) as usize;
        let other = convert_samples(&other.samples, other.format, self.format);
        let start = frames_for(self.format, offset_secs) * ch;
        let end = start + other.len();
        if end > self.samples.len() {
            self.samples.resize(end, 0.0);
        }
        for (dst, src) in self.samples[start..end].iter_mut().zip(&other) {
            *dst = (*dst + src).clamp(-1.0, 1.0);
        }
        self
    }

    /// Append `other` after this buffer (converted to this buffer's format).
    pub fn concat(mut self, other: &Pcm) -> Self {
        let other = convert_samples(&other.samples, other.format, self.format);
        self.samples.extend_from_slice(&other);
        self
    }

    /// Play backwards (frame order reversed; channel order within each frame
    /// kept).
    pub fn reverse(mut self) -> Self {
        let ch = self.format.channels.max(1) as usize;
        let frames: Vec<f32> = self.samples.chunks(ch).rev().flatten().copied().collect();
        self.samples = frames;
        self
    }

    // ----- format ----------------------------------------------------------

    /// Convert to `format` (channel re-mix, then linear resample).
    pub fn convert(self, format: AudioFormat) -> Self {
        if self.format == format {
            return self;
        }
        Pcm {
            samples: convert_samples(&self.samples, self.format, format),
            format,
        }
    }

    /// Resample to `sample_rate` (linear interpolation; duration preserved).
    pub fn resample(self, sample_rate: u32) -> Self {
        let format = AudioFormat {
            sample_rate,
            channels: self.format.channels,
        };
        self.convert(format)
    }

    /// Down-mix to one channel (mean of the channels).
    pub fn to_mono(self) -> Self {
        let format = AudioFormat {
            sample_rate: self.format.sample_rate,
            channels: 1,
        };
        self.convert(format)
    }

    /// Re-mix to two channels (mono is duplicated; wider layouts fold down).
    pub fn to_stereo(self) -> Self {
        let format = AudioFormat {
            sample_rate: self.format.sample_rate,
            channels: 2,
        };
        self.convert(format)
    }

    // ----- filters & colour ------------------------------------------------

    /// One-pole (6 dB/octave) low-pass at `cutoff_hz`: darkens / muffles.
    /// Brown noise through a low-pass with a decay is the classic explosion.
    pub fn low_pass(mut self, cutoff_hz: f32) -> Self {
        let ch = self.format.channels.max(1) as usize;
        let rate = self.format.sample_rate.max(1) as f32;
        let a = 1.0 - (-std::f32::consts::TAU * cutoff_hz.max(0.0) / rate).exp();
        let mut y = vec![0.0f32; ch];
        for frame in self.samples.chunks_mut(ch) {
            for (c, s) in frame.iter_mut().enumerate() {
                y[c] += a * (*s - y[c]);
                *s = y[c];
            }
        }
        self
    }

    /// One-pole (6 dB/octave) high-pass at `cutoff_hz`: thins out / removes
    /// rumble and DC.
    pub fn high_pass(mut self, cutoff_hz: f32) -> Self {
        let ch = self.format.channels.max(1) as usize;
        let rate = self.format.sample_rate.max(1) as f32;
        let rc = 1.0 / (std::f32::consts::TAU * cutoff_hz.max(1e-3));
        let dt = 1.0 / rate;
        let alpha = rc / (rc + dt);
        let mut prev_x = vec![0.0f32; ch];
        let mut y = vec![0.0f32; ch];
        for frame in self.samples.chunks_mut(ch) {
            for (c, s) in frame.iter_mut().enumerate() {
                y[c] = alpha * (y[c] + *s - prev_x[c]);
                prev_x[c] = *s;
                *s = y[c];
            }
        }
        self
    }

    /// Quantize to `bits` of resolution (`1..=16`): `2^bits` evenly spaced
    /// levels across `[-1, 1]`. The retro "8-bit" crunch — `4` is gritty,
    /// `1` turns anything into a square.
    pub fn bitcrush(mut self, bits: u32) -> Self {
        let bits = bits.clamp(1, 16);
        let step = 2.0 / ((1u32 << bits) - 1) as f32;
        for s in &mut self.samples {
            *s = (((*s + 1.0) / step).round() * step - 1.0).clamp(-1.0, 1.0);
        }
        self
    }

    // ----- pipeline exits --------------------------------------------------

    /// Encode as a 16-bit PCM RIFF/WAVE file — the one encoding every platform
    /// player `audio` drives (HTMLAudioElement, AVAudioPlayer, MediaPlayer,
    /// GStreamer) decodes. Samples are clamped and rounded to `i16`.
    pub fn to_wav(&self) -> Vec<u8> {
        crate::wav::encode(self)
    }

    /// Decode a WAV file: 8/16/24/32-bit integer PCM, 32/64-bit float, plain
    /// or `WAVE_FORMAT_EXTENSIBLE`. Unknown chunks are skipped.
    pub fn from_wav(bytes: &[u8]) -> Result<Pcm, SynthError> {
        crate::wav::decode(bytes)
    }

    /// This buffer as an [`audio::AudioSource`] (WAV bytes), for
    /// [`audio::load`].
    pub fn to_source(&self) -> audio::AudioSource {
        audio::AudioSource::bytes(self.to_wav())
    }

    /// Load this buffer as an [`audio::Sound`] — encode to WAV, then
    /// [`audio::load`]. Load once, then [`Sound::play`](audio::Sound::play) as
    /// often as you like; that is the cheap path for a game's SFX.
    pub async fn load(&self) -> Result<audio::Sound, audio::AudioError> {
        audio::load(self.to_source()).await
    }

    /// Push the whole buffer through an [`AudioWriter`] at once, in ~1024-frame
    /// chunks with sample-accurate timestamps. For **offline** consumers that
    /// are already subscribed (a test, an analysis tap, a recorder that has
    /// started).
    ///
    /// Two limits make this the wrong tool for live playback:
    ///
    /// - a stream only delivers to subscribers present at write time — a
    ///   consumer that subscribes later misses everything written here;
    /// - on web, the `MediaStreamTrack` bridge buffers at most ~1 s of
    ///   audio and **drops the oldest** beyond that, so dumping a long buffer
    ///   at once loses its beginning.
    ///
    /// For live audio use a [`Mixer`](crate::Mixer), which paces output in
    /// real time.
    pub fn write_to(&self, writer: &AudioWriter) {
        let ch = self.format.channels.max(1) as usize;
        for chunk in self.samples.chunks(WRITE_CHUNK_FRAMES * ch) {
            writer.write_pcm_f32(self.format.sample_rate, self.format.channels, chunk);
        }
    }
}

/// Interleaved `samples` in `from` re-laid into `to` (re-mix, then resample).
pub(crate) fn convert_samples(samples: &[f32], from: AudioFormat, to: AudioFormat) -> Vec<f32> {
    let remixed = dsp::remix(samples, from.channels as usize, to.channels as usize);
    dsp::resample(&remixed, to.channels as usize, from.sample_rate, to.sample_rate)
}
