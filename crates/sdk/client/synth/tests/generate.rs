//! Generators: waveform correctness, sweep phase continuity, noise
//! determinism and colour, envelope shape, presets.

use synth::{noise, presets, sweep, tone, AudioFormat, Envelope, Noise, Pcm, Sweep, Waveform};

const MONO: AudioFormat = AudioFormat {
    sample_rate: 48_000,
    channels: 1,
};

fn zero_crossings(s: &[f32]) -> usize {
    s.windows(2)
        .filter(|w| (w[0] < 0.0 && w[1] >= 0.0) || (w[0] >= 0.0 && w[1] < 0.0))
        .count()
}

#[test]
fn sine_has_unit_peak_and_two_crossings_per_cycle() {
    let p = tone(Waveform::Sine, 440.0, 1.0, MONO);
    assert_eq!(p.frame_count(), 48_000);
    assert!((p.peak() - 1.0).abs() < 1e-3, "peak {}", p.peak());
    assert_eq!(p.samples[0], 0.0, "sine starts at zero (no click)");
    let zc = zero_crossings(&p.samples) as i64;
    // 440 cycles -> 880 sign changes (±2 for the endpoints).
    assert!((zc - 880).abs() <= 2, "zero crossings {zc}");
}

#[test]
fn square_is_exactly_plus_minus_one_with_half_duty() {
    let p = tone(Waveform::Square, 100.0, 0.5, MONO);
    assert!(p.samples.iter().all(|&s| s == 1.0 || s == -1.0));
    let high = p.samples.iter().filter(|&&s| s > 0.0).count() as f64;
    assert!((high / p.samples.len() as f64 - 0.5).abs() < 0.01);
}

#[test]
fn pulse_duty_sets_high_fraction() {
    let p = tone(Waveform::Pulse(0.25), 100.0, 1.0, MONO);
    let high = p.samples.iter().filter(|&&s| s > 0.0).count() as f64;
    assert!((high / p.samples.len() as f64 - 0.25).abs() < 0.01);
}

#[test]
fn sawtooth_and_triangle_start_at_zero_and_stay_in_range() {
    for w in [Waveform::Sawtooth, Waveform::Triangle] {
        let p = tone(w, 300.0, 0.2, MONO);
        assert!(p.samples[0].abs() < 1e-6, "{w:?} starts at {}", p.samples[0]);
        assert!(p.peak() <= 1.0 && p.peak() > 0.95, "{w:?} peak {}", p.peak());
    }
}

#[test]
fn tone_writes_every_channel() {
    let stereo = AudioFormat { sample_rate: 8_000, channels: 2 };
    let p = tone(Waveform::Sine, 100.0, 0.1, stereo);
    assert_eq!(p.samples.len(), 800 * 2);
    for f in p.samples.chunks(2) {
        assert_eq!(f[0], f[1]);
    }
}

#[test]
fn sweep_is_phase_continuous() {
    // A steep sine sweep: the largest sample-to-sample step must never
    // exceed what the highest instantaneous frequency allows
    // (2π f / rate for a unit sine) — any phase reset would show up as a
    // jump far above that bound.
    for curve in [Sweep::Linear, Sweep::Exponential] {
        let p = sweep(Waveform::Sine, 100.0, 4_000.0, 0.5, curve, MONO);
        let bound = std::f32::consts::TAU * 4_000.0 / 48_000.0 * 1.01;
        let max_step = p.samples.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(max_step <= bound, "{curve:?}: step {max_step} > {bound}");
    }
}

#[test]
fn sweep_frequency_rises_over_time() {
    let p = sweep(Waveform::Sine, 200.0, 2_000.0, 1.0, Sweep::Exponential, MONO);
    let first = zero_crossings(&p.samples[..4_800]);
    let last = zero_crossings(&p.samples[43_200..]);
    assert!(last > first * 5, "crossings first {first} last {last}");
    // Exponential: the midpoint frequency is the geometric mean (~632 Hz).
    let mid = zero_crossings(&p.samples[21_600..26_400]) as f32 / 2.0 / 0.1;
    assert!((mid - 632.0).abs() < 60.0, "mid freq {mid}");
}

#[test]
fn noise_is_deterministic_per_seed_and_distinct_across_seeds() {
    for kind in [Noise::White, Noise::Pink, Noise::Brown] {
        let a = noise(kind, 0.2, MONO, 42);
        let b = noise(kind, 0.2, MONO, 42);
        let c = noise(kind, 0.2, MONO, 43);
        assert_eq!(a, b, "{kind:?} same seed");
        assert_ne!(a, c, "{kind:?} different seed");
        assert!(a.peak() <= 1.0 && a.peak() > 0.05, "{kind:?} peak {}", a.peak());
    }
}

fn mean_abs_diff(p: &Pcm) -> f32 {
    p.samples.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / p.samples.len() as f32
}

fn rms(p: &Pcm) -> f32 {
    (p.samples.iter().map(|s| s * s).sum::<f32>() / p.samples.len() as f32).sqrt()
}

#[test]
fn noise_colour_tilts_toward_low_frequencies() {
    // Normalising first-difference energy by RMS measures "brightness":
    // white > pink > brown.
    let bright = |k| {
        let p = noise(k, 1.0, MONO, 9);
        mean_abs_diff(&p) / rms(&p)
    };
    let (w, p, b) = (bright(Noise::White), bright(Noise::Pink), bright(Noise::Brown));
    assert!(w > p && p > b, "white {w} pink {p} brown {b}");
}

#[test]
fn envelope_hits_key_points() {
    let e = Envelope::new(0.1, 0.1, 0.5, 0.2);
    let total = 1.0;
    assert_eq!(e.gain_at(0.0, total), 0.0);
    assert!((e.gain_at(0.05, total) - 0.5).abs() < 1e-5, "mid attack");
    assert!((e.gain_at(0.1, total) - 1.0).abs() < 1e-5, "attack peak");
    assert!((e.gain_at(0.15, total) - 0.75).abs() < 1e-5, "mid decay");
    assert!((e.gain_at(0.5, total) - 0.5).abs() < 1e-5, "sustain");
    assert!((e.gain_at(0.9, total) - 0.25).abs() < 1e-5, "mid release");
    assert_eq!(e.gain_at(1.0, total), 0.0, "ends at zero");
    assert_eq!(e.gain_at(1.5, total), 0.0);
}

#[test]
fn envelope_scales_segments_to_fit_a_short_buffer() {
    // A+D+R = 0.4 s over a 0.2 s buffer: every segment halves.
    let e = Envelope::new(0.1, 0.1, 0.5, 0.2);
    assert!((e.gain_at(0.05, 0.2) - 1.0).abs() < 1e-5, "attack peak at 0.05");
    assert!((e.gain_at(0.1, 0.2) - 0.5).abs() < 1e-5, "decay done at 0.1");
    assert!((e.gain_at(0.15, 0.2) - 0.25).abs() < 1e-5, "mid release");
}

#[test]
fn apply_envelope_shapes_a_buffer() {
    let p = tone(Waveform::Square, 100.0, 1.0, MONO).apply_envelope(Envelope::percussive(0.01, 1.0));
    assert_eq!(p.samples[0], 0.0);
    let early = p.samples[480..1_000].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let late = p.samples[44_000..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(early > 0.95, "early {early}");
    assert!(late < 0.1, "late {late}");
}

#[test]
fn presets_render_bounded_nonsilent_sounds() {
    for (name, p) in [
        ("blip", presets::blip()),
        ("laser", presets::laser()),
        ("explosion", presets::explosion()),
        ("coin", presets::coin()),
        ("hit", presets::hit()),
        ("jump", presets::jump()),
        ("power_down", presets::power_down()),
    ] {
        assert_eq!(p.format, synth::DEFAULT_FORMAT, "{name}");
        assert!(p.frame_count() > 0, "{name}");
        assert!(p.peak() > 0.1 && p.peak() <= 1.0, "{name} peak {}", p.peak());
        assert!(p.samples.iter().all(|s| s.is_finite()), "{name}");
    }
    assert_eq!(presets::explosion(), presets::explosion(), "deterministic");
}
