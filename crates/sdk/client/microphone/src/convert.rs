//! Streaming conversion from what a device captures to what the caller
//! asked for: a channel remap, then a sample-rate change.
//!
//! The native backends open the device at its OWN format and convert here.
//! Asking the platform for another rate is not safe there: on iOS, a
//! RemoteIO unit opened at a rate the hardware does not run delivers a
//! first buffer of a size cpal did not expect, and coreaudio-rs's input
//! proc then calls `AudioUnitGetProperty` from the IO thread while
//! `AudioUnitStart` holds the unit's lock on the main thread — a deadlock
//! the watchdog ends with `0x8BADF00D` (a 16 kHz request on a 1st-gen iPad
//! Pro). On macOS the same request retunes the input device's nominal rate
//! for every app on the machine, and WASAPI's shared mode refuses it. The
//! web and Android backends get the requested rate from their platform's
//! own resampler (`AudioContext({ sampleRate })`, `AudioRecord`); this is
//! the native equivalent.
//!
//! The rate change is windowed-sinc interpolation with the cutoff at the
//! lower of the two Nyquist frequencies, so downsampling speech captured
//! at 44.1/48 kHz to 16 kHz does not fold its upper band back into it.

/// Taps on each side of a kernel when the rate does not drop. Downsampling
/// widens the kernel by the rate ratio, keeping the transition band the
/// same width relative to the output's Nyquist frequency.
const HALF_TAPS: usize = 16;
/// Most kernel phases precomputed. A rate pair whose exact phase count is
/// larger (rates sharing few factors) rounds each output's phase to the
/// nearest of these, an error far below 16-bit resolution.
const MAX_PHASES: usize = 256;
/// The cutoff, as a fraction of the lower Nyquist frequency: just under it,
/// so the window's transition band ends before aliasing begins.
const CUTOFF: f64 = 0.95;

/// Converts interleaved `f32` frames at `(from_rate, from_channels)` to
/// `(to_rate, to_channels)`, one chunk at a time, keeping the filter's
/// history across chunks so the output is the same however the input was
/// split.
pub(crate) struct Converter {
    from_channels: usize,
    to_channels: usize,
    resampler: Option<Resampler>,
    /// The remapped chunk, before resampling.
    mapped: Vec<f32>,
    /// The output handed back by [`Converter::process`].
    out: Vec<f32>,
}

impl Converter {
    /// `None` when there is nothing to convert.
    pub(crate) fn new(from_rate: u32, from_channels: u16, to_rate: u32, to_channels: u16) -> Option<Self> {
        if from_rate == to_rate && from_channels == to_channels {
            return None;
        }
        let to = to_channels as usize;
        Some(Self {
            from_channels: from_channels as usize,
            to_channels: to,
            resampler: (from_rate != to_rate).then(|| Resampler::new(from_rate, to_rate, to)),
            mapped: Vec::new(),
            out: Vec::new(),
        })
    }

    /// Convert one chunk. The result may be empty while the filter fills
    /// (the first `HALF_TAPS`-odd input frames only prime it).
    pub(crate) fn process(&mut self, input: &[f32]) -> &[f32] {
        remap(input, self.from_channels, self.to_channels, &mut self.mapped);
        match &mut self.resampler {
            None => &self.mapped,
            Some(r) => {
                self.out.clear();
                r.process(&self.mapped, &mut self.out);
                &self.out
            }
        }
    }
}

/// Lay `input` (`from` channels per frame) out as `to` channels per frame:
/// every channel averaged into one for mono, otherwise output channel `c`
/// takes input channel `c % from` (mono duplicated, extra channels dropped).
fn remap(input: &[f32], from: usize, to: usize, out: &mut Vec<f32>) {
    out.clear();
    if from == to {
        out.extend_from_slice(input);
        return;
    }
    for frame in input.chunks_exact(from) {
        if to == 1 {
            out.push(frame.iter().sum::<f32>() / from as f32);
        } else {
            out.extend((0..to).map(|c| frame[c % from]));
        }
    }
}

/// Windowed-sinc rate change of interleaved frames, `channels` wide.
///
/// Output frame `k` sits at input time `k · from / to`. With `g =
/// gcd(from, to)`, that is input frame `pos` plus `frac / phases` of a
/// frame, where `phases = to / g` and each output advances `frac` by
/// `from / g`. Each phase's kernel is computed once.
struct Resampler {
    channels: usize,
    half: usize,
    /// Input frames per output frame, in units of `1 / phases`.
    step: usize,
    phases: usize,
    /// `kernels[p]` holds the `2 · half` weights for phase `p` of the
    /// table (`table_phases` of them), normalized to sum to one.
    kernels: Vec<Vec<f32>>,
    table_phases: usize,
    /// Input frames not yet consumed, the filter's history included.
    buf: Vec<f32>,
    /// The next output's input frame, as an index into `buf`.
    pos: usize,
    /// The next output's fraction past `pos`, in `0..phases`.
    frac: usize,
}

impl Resampler {
    fn new(from: u32, to: u32, channels: usize) -> Self {
        let g = gcd(from as usize, to as usize);
        let phases = to as usize / g;
        let step = from as usize / g;
        let down = (from as f64 / to as f64).max(1.0);
        let half = (HALF_TAPS as f64 * down).ceil() as usize;
        // Cycles per input frame.
        let cutoff = 0.5 * CUTOFF / down;
        let table_phases = phases.min(MAX_PHASES);
        let kernels = (0..table_phases)
            .map(|p| {
                let offset = p as f64 / table_phases as f64;
                // Taps at input frames pos-half+1 ..= pos+half.
                let mut k: Vec<f64> = (0..2 * half)
                    .map(|i| {
                        let x = i as f64 - (half as f64 - 1.0) - offset;
                        sinc(2.0 * cutoff * x) * blackman(x / half as f64)
                    })
                    .collect();
                let sum: f64 = k.iter().sum();
                k.iter_mut().for_each(|w| *w /= sum);
                k.into_iter().map(|w| w as f32).collect()
            })
            .collect();
        // `half` frames of silence ahead of the first input, so the first
        // output is centred on the first real frame.
        let buf = vec![0.0; half * channels];
        Self { channels, half, step, phases, kernels, table_phases, buf, pos: half, frac: 0 }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.channels;
        self.buf.extend_from_slice(input);
        let frames = self.buf.len() / ch;
        // Output while every tap of the kernel has arrived.
        while self.pos + self.half < frames {
            let table = self.frac * self.table_phases / self.phases;
            let kernel = &self.kernels[table];
            let first = self.pos + 1 - self.half;
            for c in 0..ch {
                let mut acc = 0.0f32;
                for (i, w) in kernel.iter().enumerate() {
                    acc += self.buf[(first + i) * ch + c] * w;
                }
                out.push(acc);
            }
            self.frac += self.step;
            self.pos += self.frac / self.phases;
            self.frac %= self.phases;
        }
        // Keep only what later outputs still read.
        let keep_from = (self.pos + 1).saturating_sub(self.half).min(frames);
        self.buf.drain(..keep_from * ch);
        self.pos -= keep_from;
    }
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// The Blackman window over `x ∈ [-1, 1]`, zero outside it.
fn blackman(x: f64) -> f64 {
    if x.abs() >= 1.0 {
        return 0.0;
    }
    let a = std::f64::consts::PI * x;
    0.42 + 0.5 * a.cos() + 0.08 * (2.0 * a).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, rate: u32, seconds: f64, channels: usize) -> Vec<f32> {
        let n = (rate as f64 * seconds) as usize;
        (0..n)
            .flat_map(|i| {
                let v = (2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64).sin() as f32;
                std::iter::repeat(v).take(channels)
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Feed `input` in chunks of `chunk` frames, as a device delivers it.
    fn run(c: &mut Converter, input: &[f32], from_channels: usize, chunk: usize) -> Vec<f32> {
        input.chunks(chunk * from_channels).flat_map(|part| c.process(part).to_vec()).collect()
    }

    #[test]
    fn nothing_to_convert_is_no_converter() {
        assert!(Converter::new(48_000, 1, 48_000, 1).is_none());
    }

    #[test]
    fn speech_band_tone_survives_48k_to_16k() {
        let mut c = Converter::new(48_000, 1, 16_000, 1).unwrap();
        let out = run(&mut c, &tone(1_000.0, 48_000, 1.0, 1), 1, 1024);
        // One second in, one second out, less the filter's tail.
        assert!((15_900..=16_000).contains(&out.len()), "{}", out.len());
        // A full-scale sine's RMS is 1/√2; skip the priming edge.
        let r = rms(&out[200..out.len() - 200]);
        assert!((r - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "rms {r}");
    }

    #[test]
    fn a_tone_above_the_new_nyquist_is_filtered_not_folded() {
        // 12 kHz is above 16 kHz's 8 kHz Nyquist; dropping samples without a
        // filter would fold it to 4 kHz at full level.
        let mut c = Converter::new(48_000, 1, 16_000, 1).unwrap();
        let out = run(&mut c, &tone(12_000.0, 48_000, 0.5, 1), 1, 1024);
        let r = rms(&out[200..out.len() - 200]);
        assert!(r < 0.01, "rms {r}");
    }

    #[test]
    fn a_rate_pair_with_many_phases_keeps_time_and_level() {
        // 44.1 kHz → 16 kHz: 160 phases, stepping 441 per output.
        let mut c = Converter::new(44_100, 1, 16_000, 1).unwrap();
        let out = run(&mut c, &tone(440.0, 44_100, 1.0, 1), 1, 940);
        assert!((15_900..=16_000).contains(&out.len()), "{}", out.len());
        let r = rms(&out[200..out.len() - 200]);
        assert!((r - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "rms {r}");
    }

    #[test]
    fn the_output_does_not_depend_on_how_the_input_was_chunked() {
        let input = tone(700.0, 48_000, 0.25, 1);
        let whole = run(&mut Converter::new(48_000, 1, 16_000, 1).unwrap(), &input, 1, input.len());
        let small = run(&mut Converter::new(48_000, 1, 16_000, 1).unwrap(), &input, 1, 97);
        assert_eq!(whole.len(), small.len());
        assert!(whole.iter().zip(&small).all(|(a, b)| (a - b).abs() < 1e-6));
    }

    #[test]
    fn channels_are_averaged_to_mono_and_duplicated_from_it() {
        let mut down = Converter::new(48_000, 2, 48_000, 1).unwrap();
        assert_eq!(down.process(&[1.0, 0.0, 0.5, 0.5]), &[0.5, 0.5]);
        let mut up = Converter::new(48_000, 1, 48_000, 2).unwrap();
        assert_eq!(up.process(&[0.25, -1.0]), &[0.25, 0.25, -1.0, -1.0]);
    }

    #[test]
    fn stereo_resamples_each_channel_on_its_own() {
        // Left a tone, right silent: the right channel stays silent.
        let left = tone(1_000.0, 48_000, 0.25, 1);
        let stereo: Vec<f32> = left.iter().flat_map(|&l| [l, 0.0]).collect();
        let mut c = Converter::new(48_000, 2, 16_000, 2).unwrap();
        let out = run(&mut c, &stereo, 2, 512);
        let right: Vec<f32> = out.chunks_exact(2).map(|f| f[1]).collect();
        assert!(right.iter().all(|v| v.abs() < 1e-6));
        let left_out: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
        assert!(rms(&left_out[200..left_out.len() - 200]) > 0.69);
    }
}
