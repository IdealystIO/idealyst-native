//! Internal sample-level helpers shared by [`Pcm`](crate::Pcm), the
//! generators and the [`Mixer`](crate::Mixer): channel remixing, linear
//! resampling, and the deterministic PRNG behind [`noise`](crate::noise).

/// Re-lay interleaved `samples` from `in_ch` to `out_ch` channels.
///
/// - `in == out`: copied.
/// - down to mono: each output sample is the mean of the input channels.
/// - up from mono: the one channel is duplicated into every output channel.
/// - otherwise (N → M): output channel `c` is the mean of every input channel
///   `i` with `i % out_ch == c` (down-mix), or input channel `c % in_ch`
///   (up-mix) — stereo → quad repeats L R L R, quad → stereo folds 0+2 / 1+3.
pub(crate) fn remix(samples: &[f32], in_ch: usize, out_ch: usize) -> Vec<f32> {
    let in_ch = in_ch.max(1);
    let out_ch = out_ch.max(1);
    if in_ch == out_ch {
        return samples.to_vec();
    }
    let frames = samples.len() / in_ch;
    let mut out = Vec::with_capacity(frames * out_ch);
    for f in 0..frames {
        let frame = &samples[f * in_ch..(f + 1) * in_ch];
        for c in 0..out_ch {
            let v = if in_ch > out_ch {
                let mut sum = 0.0;
                let mut n = 0u32;
                let mut i = c;
                while i < in_ch {
                    sum += frame[i];
                    n += 1;
                    i += out_ch;
                }
                sum / n.max(1) as f32
            } else {
                frame[c % in_ch]
            };
            out.push(v);
        }
    }
    out
}

/// Linear-interpolation resample of interleaved `samples` (`channels` wide)
/// from `src_rate` to `dst_rate`. Output length is
/// `round(frames * dst / src)` frames, so durations are preserved.
///
/// Linear interpolation is deliberately simple: it is plenty for game SFX and
/// for matching a captured stream's rate to a buffer's, and keeps the crate
/// dependency-free. It does not band-limit, so a large down-sample of
/// bright material can alias — render at the target rate when that matters.
pub(crate) fn resample(samples: &[f32], channels: usize, src_rate: u32, dst_rate: u32) -> Vec<f32> {
    let ch = channels.max(1);
    if src_rate == dst_rate || src_rate == 0 || dst_rate == 0 {
        return samples.to_vec();
    }
    let in_frames = samples.len() / ch;
    if in_frames == 0 {
        return Vec::new();
    }
    let out_frames =
        ((in_frames as u128 * dst_rate as u128 + src_rate as u128 / 2) / src_rate as u128) as usize;
    let step = src_rate as f64 / dst_rate as f64;
    let mut out = Vec::with_capacity(out_frames * ch);
    for i in 0..out_frames {
        let pos = i as f64 * step;
        let i0 = (pos.floor() as usize).min(in_frames - 1);
        let i1 = (i0 + 1).min(in_frames - 1);
        let t = (pos - i0 as f64) as f32;
        for c in 0..ch {
            let a = samples[i0 * ch + c];
            let b = samples[i1 * ch + c];
            out.push(a + (b - a) * t);
        }
    }
    out
}

/// SplitMix64 — a tiny, high-quality, fully deterministic 64-bit generator.
/// Chosen over pulling in `rand` because noise must be reproducible from a
/// seed on every target (including wasm) and needs nothing more than
/// uniform floats.
pub(crate) struct SplitMix64(u64);

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[-1, 1)`. Uses the top 24 bits (an `f32` mantissa's worth)
    /// so every value is exactly representable.
    pub(crate) fn next_bipolar(&mut self) -> f32 {
        let unit = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32;
        unit * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remix_stereo_to_mono_averages() {
        assert_eq!(remix(&[1.0, 0.0, 0.5, 0.5], 2, 1), vec![0.5, 0.5]);
    }

    #[test]
    fn remix_mono_to_stereo_duplicates() {
        assert_eq!(remix(&[0.25, -0.5], 1, 2), vec![0.25, 0.25, -0.5, -0.5]);
    }

    #[test]
    fn resample_doubles_length_and_interpolates() {
        let out = resample(&[0.0, 1.0], 1, 1, 2);
        assert_eq!(out, vec![0.0, 0.5, 1.0, 1.0]);
    }

    #[test]
    fn prng_is_in_range() {
        let mut r = SplitMix64::new(7);
        for _ in 0..10_000 {
            let v = r.next_bipolar();
            assert!((-1.0..1.0).contains(&v));
        }
    }
}
