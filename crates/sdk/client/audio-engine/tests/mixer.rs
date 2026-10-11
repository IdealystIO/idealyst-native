//! The Rust mixer, end to end through `AudioEngine::offline`: what a device
//! would play, sample by sample.

use audio_engine::{
    AudioEngine, SoundBuffer, VoiceParams, MAX_VOICES, QUANTUM, STREAM_BACKLOG_SECS,
};
use media_stream::{AudioFormat, AudioStream};

const SR: u32 = 48_000;

fn mono(samples: Vec<f32>) -> SoundBuffer {
    SoundBuffer::from_samples(
        AudioFormat {
            sample_rate: SR,
            channels: 1,
        },
        samples,
    )
}

/// A stereo buffer with L = R = `v` — the stereo panner at pan 0 passes it
/// through untouched, so output == gain × v exactly.
fn stereo_dc(v: f32, frames: usize) -> SoundBuffer {
    SoundBuffer::from_samples(
        AudioFormat {
            sample_rate: SR,
            channels: 2,
        },
        vec![v; frames * 2],
    )
}

fn left(out: &[f32]) -> Vec<f32> {
    out.chunks_exact(2).map(|f| f[0]).collect()
}

fn sine(freq: f32, secs: f32, rate: u32) -> Vec<f32> {
    let n = (secs * rate as f32) as usize;
    (0..n)
        .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5)
        .collect()
}

/// Rising zero crossings in `s`.
fn crossings(s: &[f32]) -> usize {
    s.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
}

/// Deterministic white noise in [-0.5, 0.5].
fn noise(n: usize) -> Vec<f32> {
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        })
        .collect()
}

/// Power of `s` in DFT bins whose frequency lies in `[lo, hi)` Hz, Hann
/// windowed: the low-passed signal is dominated by low frequencies, and a
/// rectangular window's cut at the edges would leak them across the high
/// band, burying the attenuation being measured.
fn band_power(s: &[f32], rate: u32, lo: f64, hi: f64) -> Vec<(f64, f64)> {
    let n = s.len();
    let s: Vec<f32> = s
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            x * (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos()) as f32
        })
        .collect();
    let mut out = Vec::new();
    for k in 1..n / 2 {
        let f = k as f64 * rate as f64 / n as f64;
        if f < lo || f >= hi {
            continue;
        }
        let w = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in s.iter().enumerate() {
            re += x as f64 * (w * i as f64).cos();
            im -= x as f64 * (w * i as f64).sin();
        }
        out.push((f, re * re + im * im));
    }
    out
}

/// |H(e^{jω})|² of normalized biquad coefficients.
fn biquad_power(c: [f64; 5], f: f64, rate: u32) -> f64 {
    let w = 2.0 * std::f64::consts::PI * f / rate as f64;
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let nr = c[0] + c[1] * c1 + c[2] * c2;
    let ni = -(c[1] * s1 + c[2] * s2);
    let dr = 1.0 + c[3] * c1 + c[4] * c2;
    let di = -(c[3] * s1 + c[4] * s2);
    (nr * nr + ni * ni) / (dr * dr + di * di)
}

#[test]
fn lowpassed_white_noise_loses_its_high_band_by_the_spec_amount() {
    let input = noise(SR as usize);
    let buf = mono(input);
    let window = 4096;
    let skip = 8192; // past the filter's start transient

    let render = |lowpass: Option<f32>| {
        let (engine, mut r) = AudioEngine::offline(SR, 2);
        let mut p = VoiceParams::new();
        p.lowpass_hz = lowpass;
        engine.play(&buf, p);
        let out = r.render_frames(skip + window);
        left(&out)[skip..].to_vec()
    };
    let dry = render(None);
    let wet = render(Some(1_000.0));

    let c = audio_engine::dsp::lowpass_coefficients(SR as f64, 1_000.0, 1.0);
    let dry_hi = band_power(&dry, SR, 8_000.0, 20_000.0);
    let wet_hi = band_power(&wet, SR, 8_000.0, 20_000.0);
    let dry_sum: f64 = dry_hi.iter().map(|b| b.1).sum();
    let wet_sum: f64 = wet_hi.iter().map(|b| b.1).sum();
    let expected: f64 = dry_hi
        .iter()
        .map(|(f, p)| p * biquad_power(c, *f, SR))
        .sum();

    let measured_db = 10.0 * (wet_sum / dry_sum).log10();
    let expected_db = 10.0 * (expected / dry_sum).log10();
    assert!(
        measured_db < -30.0,
        "high band only {measured_db:.1} dB down"
    );
    assert!(
        (measured_db - expected_db).abs() < 1.5,
        "measured {measured_db:.2} dB vs spec response {expected_db:.2} dB"
    );

    // The passband is untouched (within the Q=1 dB bump).
    let dry_lo: f64 = band_power(&dry, SR, 20.0, 300.0).iter().map(|b| b.1).sum();
    let wet_lo: f64 = band_power(&wet, SR, 20.0, 300.0).iter().map(|b| b.1).sum();
    let lo_db = 10.0 * (wet_lo / dry_lo).log10();
    assert!(lo_db.abs() < 1.0, "passband changed by {lo_db:.2} dB");
}

#[test]
fn pan_positions_match_spec_through_the_mixer() {
    let h = std::f32::consts::FRAC_1_SQRT_2;
    for (pan, want) in [
        (-1.0f32, (1.0f32, 0.0f32)),
        (0.0, (h, h)),
        (1.0, (0.0, 1.0)),
    ] {
        let (engine, mut r) = AudioEngine::offline(SR, 2);
        engine.play(&mono(vec![1.0; 1_000]), VoiceParams::new().pan(pan));
        let out = r.render_frames(10);
        assert!(
            (out[4] - want.0).abs() < 1e-6 && (out[5] - want.1).abs() < 1e-6,
            "pan {pan}: {out:?}"
        );
    }
}

#[test]
fn rate_two_halves_duration_and_doubles_pitch() {
    let buf = mono(sine(440.0, 0.5, SR));
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let normal = engine.play(&buf, VoiceParams::new());
    let a = left(&r.render_secs(0.2));
    normal.stop(0.0);
    let (engine2, mut r2) = AudioEngine::offline(SR, 2);
    let fast = engine2.play(&buf, VoiceParams::new().rate(2.0));
    let b = left(&r2.render_secs(0.2));
    let (ca, cb) = (crossings(&a), crossings(&b));
    assert!(
        (cb as i64 - 2 * ca as i64).abs() <= 1,
        "crossings {ca} vs {cb}"
    );

    // 0.25 s of audio total at rate 2.
    let rest = left(&r2.render_secs(0.1));
    let last_sound = rest
        .iter()
        .rposition(|s| s.abs() > 1e-4)
        .map(|i| i + b.len())
        .unwrap_or(0);
    let expected = (0.25 * SR as f64) as usize;
    assert!(
        last_sound.abs_diff(expected) < 64,
        "sound ended at frame {last_sound}, want ~{expected}"
    );
    assert!(!fast.is_playing());
}

#[test]
fn buffer_rate_is_converted_to_output_rate() {
    // A 24 kHz buffer of 0.1 s still lasts 0.1 s at 48 kHz, at the same pitch.
    let src = SoundBuffer::from_samples(
        AudioFormat {
            sample_rate: 24_000,
            channels: 1,
        },
        sine(300.0, 0.1, 24_000),
    );
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let v = engine.play(&src, VoiceParams::new());
    let out = left(&r.render_secs(0.12));
    let last = out.iter().rposition(|s| s.abs() > 1e-4).unwrap();
    assert!(last.abs_diff(4_800) < 64, "ended at {last}");
    let c = crossings(&out[..4_800]);
    assert!((29..=30).contains(&c), "300 Hz over 0.1 s: {c} crossings");
    assert!(!v.is_playing());
}

#[test]
fn gain_ramp_is_linear_and_reaches_target_on_time() {
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let v = engine.play(
        &stereo_dc(1.0, SR as usize),
        VoiceParams::new().gain(0.0).looping(true),
    );
    v.set_gain(1.0, 0.01); // 480 frames
    let out = left(&r.render_frames(1_000));
    for (k, s) in out.iter().enumerate().take(481) {
        let want = k as f32 / 480.0;
        assert!((s - want).abs() < 1e-5, "frame {k}: {s} vs {want}");
    }
    assert!(out[480..].iter().all(|&s| s == 1.0), "target held exactly");
}

#[test]
fn ramps_issued_later_start_at_the_next_quantum() {
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let v = engine.play(
        &stereo_dc(1.0, SR as usize),
        VoiceParams::new().looping(true),
    );
    let _ = r.render_frames(QUANTUM * 3);
    v.set_gain(0.0, 0.0);
    let out = left(&r.render_frames(QUANTUM));
    assert!(out.iter().all(|&s| s == 0.0));
}

#[test]
fn looping_wraps_the_whole_buffer() {
    let n = 1_000;
    let ramp: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
    let buf = SoundBuffer::from_samples(
        AudioFormat {
            sample_rate: SR,
            channels: 2,
        },
        ramp.iter().flat_map(|&v| [v, v]).collect(),
    );
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let v = engine.play(&buf, VoiceParams::new().looping(true));
    let out = left(&r.render_frames(n * 3 + 10));
    for i in 0..n {
        assert_eq!(out[i], ramp[i]);
        assert_eq!(out[n + i], ramp[i], "second pass frame {i}");
        assert_eq!(out[2 * n + i], ramp[i], "third pass frame {i}");
    }
    assert!(v.is_playing());

    // set_looping(false) lets the current pass finish, then the voice ends.
    v.set_looping(false);
    let rest = left(&r.render_frames(n * 2));
    assert!(!v.is_playing());
    let last = rest.iter().rposition(|&s| s != 0.0).unwrap();
    assert!(last < n, "played past the end of the pass");
}

#[test]
fn start_offset_skips_into_the_buffer() {
    let n = 4_800;
    let ramp: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
    let buf = SoundBuffer::from_samples(
        AudioFormat {
            sample_rate: SR,
            channels: 2,
        },
        ramp.iter().flat_map(|&v| [v, v]).collect(),
    );
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    engine.play(&buf, VoiceParams::new().start_offset(0.05));
    let out = left(&r.render_frames(10));
    assert_eq!(out[0], ramp[2_400]);
}

#[test]
fn stop_fades_linearly_then_frees_the_voice() {
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let v = engine.play(
        &stereo_dc(0.5, SR as usize),
        VoiceParams::new().looping(true),
    );
    let _ = r.render_frames(QUANTUM);
    v.stop(0.01);
    v.set_gain(1.0, 0.0); // ignored: the fade owns the gain
    let out = left(&r.render_frames(QUANTUM * 6));
    for (k, s) in out.iter().enumerate().take(481) {
        let want = 0.5 * (1.0 - k as f32 / 480.0);
        assert!((s - want).abs() < 1e-5, "frame {k}: {s} vs {want}");
    }
    assert!(out[480..].iter().all(|&s| s == 0.0));
    assert!(!v.is_playing());
    assert_eq!(engine.active_voices(), 0);
}

#[test]
fn voice_cap_steals_the_oldest_one_shot() {
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let long = stereo_dc(0.001, SR as usize);
    let looped = engine.play(&long, VoiceParams::new().looping(true));
    let voices: Vec<_> = (0..MAX_VOICES - 1)
        .map(|_| engine.play(&long, VoiceParams::new()))
        .collect();
    let _ = r.render_frames(QUANTUM);
    assert_eq!(engine.active_voices(), MAX_VOICES);

    let newest = engine.play(&long, VoiceParams::new());
    let _ = r.render_frames(QUANTUM);
    assert!(newest.is_playing());
    assert!(looped.is_playing(), "looping voices are never stolen");
    assert!(!voices[0].is_playing(), "the oldest one-shot was stolen");
    assert!(voices[1..].iter().all(|v| v.is_playing()));
    assert_eq!(engine.active_voices(), MAX_VOICES);
}

#[test]
fn voice_cap_rejects_when_everything_loops() {
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let long = stereo_dc(0.001, 1_000);
    let loops: Vec<_> = (0..MAX_VOICES)
        .map(|_| engine.play(&long, VoiceParams::new().looping(true)))
        .collect();
    let extra = engine.play(&long, VoiceParams::new());
    let _ = r.render_frames(QUANTUM);
    assert!(!extra.is_playing());
    assert!(loops.iter().all(|v| v.is_playing()));
}

#[test]
fn stream_voice_plays_pushed_pcm_then_starves_to_silence() {
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let (stream, writer) = AudioStream::new();
    let v = engine.play_stream(&stream, VoiceParams::new());
    // 20 ms of stereo 0.5 at the output rate.
    writer.write_pcm_f32(SR, 2, &vec![0.5; 960 * 2]);
    // 1024 = 8 whole quanta, so the next render starts on a fresh quantum.
    let out = left(&r.render_frames(1_024));
    assert!(
        out[..960].iter().all(|&s| (s - 0.5).abs() < 1e-6),
        "pushed PCM plays as-is"
    );
    assert!(out[960..].iter().all(|&s| s == 0.0), "starved → silence");
    assert!(v.is_playing(), "a starved stream voice keeps playing");

    // More PCM later resumes it.
    writer.write_pcm_f32(SR, 2, &vec![0.25; 100 * 2]);
    let out = left(&r.render_frames(QUANTUM));
    assert!(out[..100].iter().all(|&s| (s - 0.25).abs() < 1e-6));
    assert!(out[100..].iter().all(|&s| s == 0.0));

    v.stop(0.0);
    let _ = r.render_frames(QUANTUM);
    assert!(!v.is_playing());
}

#[test]
fn stream_voice_caps_backlog_dropping_the_oldest() {
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let (stream, writer) = AudioStream::new();
    engine.play_stream(&stream, VoiceParams::new());
    // 0.5 s pushed at once: 0.4 s of 0.1 then 0.1 s of 0.9. Only the newest
    // STREAM_BACKLOG_SECS survives.
    let mut pcm = vec![0.1f32; (0.4 * SR as f32) as usize * 2];
    pcm.extend(vec![0.9f32; (0.1 * SR as f32) as usize * 2]);
    writer.write_pcm_f32(SR, 2, &pcm);
    let backlog = (STREAM_BACKLOG_SECS as f64 * SR as f64).round() as usize;
    let out = left(&r.render_frames(backlog + 1_000));
    assert!(
        out[..backlog].iter().all(|&s| (s - 0.9).abs() < 1e-6),
        "oldest audio was dropped"
    );
    assert!(out[backlog..].iter().all(|&s| s == 0.0));
}

#[test]
fn stream_voice_resamples_to_the_output_rate() {
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let (stream, writer) = AudioStream::new();
    engine.play_stream(&stream, VoiceParams::new().pan(-1.0));
    // 50 ms of a 24 kHz mono 200 Hz sine → ~2400 output frames, 10 cycles.
    writer.write_pcm_f32(24_000, 1, &sine(200.0, 0.05, 24_000));
    let out = left(&r.render_frames(3_000));
    let last = out.iter().rposition(|s| s.abs() > 1e-4).unwrap();
    assert!(last.abs_diff(2_400) < 8, "ended at {last}");
    let c = crossings(&out[..2_400]);
    assert!((9..=10).contains(&c), "200 Hz over 50 ms: {c} crossings");
}

#[test]
fn master_gain_applies_before_the_clamp() {
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let buf = stereo_dc(0.8, SR as usize);
    engine.play(&buf, VoiceParams::new().looping(true));
    engine.play(&buf, VoiceParams::new().looping(true));
    let out = r.render_frames(QUANTUM);
    assert!(out.iter().all(|&s| s == 1.0), "1.6 clamps to 1");
    engine.set_master_gain(0.5, 0.0);
    let out = r.render_frames(QUANTUM);
    assert!(out.iter().all(|&s| (s - 0.8).abs() < 1e-6), "0.5 × 1.6");

    // A negative mix clamps to -1.
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let neg = stereo_dc(-0.9, SR as usize);
    engine.play(&neg, VoiceParams::new().gain(3.0));
    assert!(r.render_frames(QUANTUM).iter().all(|&s| s == -1.0));
}

#[test]
fn mono_and_wide_outputs_map_like_web_audio_speakers() {
    // Mono out: (L + R) / 2.
    let (engine, mut r) = AudioEngine::offline(SR, 1);
    engine.play(&mono(vec![1.0; 500]), VoiceParams::new().pan(-1.0));
    let out = r.render_frames(10);
    assert!((out[3] - 0.5).abs() < 1e-6);
    // Quad out: L, R, then silence.
    let (engine, mut r) = AudioEngine::offline(SR, 4);
    engine.play(&stereo_dc(0.5, 500), VoiceParams::new());
    let out = r.render_frames(2);
    assert_eq!(&out[..4], &[0.5, 0.5, 0.0, 0.0]);
}

#[test]
fn lowpass_toggles_without_clicks() {
    let buf = mono(sine(200.0, 1.0, SR));
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let v = engine.play(&buf, VoiceParams::new().looping(true).pan(-1.0));
    let mut all = left(&r.render_secs(0.05));
    v.set_lowpass(Some(300.0), 0.05);
    all.extend(left(&r.render_secs(0.1)));
    v.set_lowpass(None, 0.05);
    all.extend(left(&r.render_secs(0.1)));
    v.set_lowpass(Some(2_000.0), 0.05);
    all.extend(left(&r.render_secs(0.1)));
    // A click is a corner: a jump in the slope. A smooth 200 Hz signal —
    // even one whose level swells while the cutoff sweeps — has a second
    // difference of ~amp·ω² ≈ 4e-4 per frame; a step of size d has ~d.
    let (at, worst) = all
        .windows(3)
        .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
        .enumerate()
        .fold((0, 0.0f32), |m, (i, d)| if d > m.1 { (i, d) } else { m });
    assert!(worst < 0.003, "corner of {worst} at frame {at}");
    // Sanity: a hard 0-ramp switch on the same loop IS detected.
    let (engine, mut r) = AudioEngine::offline(SR, 2);
    let v = engine.play(&buf, VoiceParams::new().looping(true).pan(-1.0));
    let mut hard = left(&r.render_frames(QUANTUM * 20 + 37));
    v.set_gain(0.2, 0.0);
    hard.extend(left(&r.render_frames(QUANTUM * 4)));
    let hard_worst = hard
        .windows(3)
        .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
        .fold(0.0, f32::max);
    assert!(
        hard_worst > 0.003,
        "the detector missed a real click ({hard_worst})"
    );
}

#[test]
fn offline_rendering_is_deterministic() {
    let run = || {
        let (engine, mut r) = AudioEngine::offline(44_100, 2);
        let buf = mono(noise(10_000));
        let v = engine.play(
            &buf,
            VoiceParams::new().rate(1.37).lowpass(3_000.0).pan(0.3),
        );
        let mut out = r.render_frames(2_000);
        v.set_pan(-0.6, 0.02);
        v.set_rate(0.7, 0.03);
        out.extend(r.render_frames(5_000));
        out
    };
    assert_eq!(run(), run());
}

#[test]
fn engine_drop_ends_voices_and_neuters_handles() {
    let (engine, r) = AudioEngine::offline(SR, 2);
    let v = engine.play(&stereo_dc(0.5, 100), VoiceParams::new().looping(true));
    drop(engine);
    drop(r);
    assert!(!v.is_playing());
    v.set_gain(0.2, 0.1); // no engine: a no-op, not a panic
}
