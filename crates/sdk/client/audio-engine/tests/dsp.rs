//! The DSP primitives against the Web Audio spec formulas, computed
//! independently here.

use std::f64::consts::PI;

use audio_engine::dsp::{lowpass_coefficients, pan_mono, pan_stereo, Param, LOWPASS_Q_DB};

/// The spec's lowpass formula, written out from the spec text (§1.13 —
/// `α_Q = sin ω0 / (2·10^(Q/20))`).
fn spec_lowpass(fs: f64, f0: f64, q_db: f64) -> [f64; 5] {
    let w0 = 2.0 * PI * f0 / fs;
    let alpha_q = w0.sin() / (2.0 * 10f64.powf(q_db / 20.0));
    let b0 = (1.0 - w0.cos()) / 2.0;
    let b1 = 1.0 - w0.cos();
    let b2 = (1.0 - w0.cos()) / 2.0;
    let a0 = 1.0 + alpha_q;
    let a1 = -2.0 * w0.cos();
    let a2 = 1.0 - alpha_q;
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

#[test]
fn biquad_lowpass_coefficients_match_web_audio_spec() {
    assert_eq!(LOWPASS_Q_DB, 1.0, "the spec default Q");
    for (fs, f0) in [
        (44_100.0, 1_000.0),
        (48_000.0, 350.0),
        (48_000.0, 12_000.0),
        (22_050.0, 5_000.0),
        (96_000.0, 40.0),
    ] {
        let got = lowpass_coefficients(fs, f0, LOWPASS_Q_DB);
        let want = spec_lowpass(fs, f0, LOWPASS_Q_DB);
        for (g, w) in got.iter().zip(want.iter()) {
            assert!(
                (g - w).abs() < 1e-12,
                "fs {fs} f0 {f0}: {got:?} vs {want:?}"
            );
        }
        // DC gain of a lowpass is exactly 1: (b0+b1+b2)/(1+a1+a2).
        let dc = (got[0] + got[1] + got[2]) / (1.0 + got[3] + got[4]);
        assert!((dc - 1.0).abs() < 1e-9, "DC gain {dc}");
    }
}

#[test]
fn biquad_lowpass_endpoints_pass_all_and_block_all() {
    assert_eq!(
        lowpass_coefficients(48_000.0, 24_000.0, 1.0),
        [1.0, 0.0, 0.0, 0.0, 0.0]
    );
    assert_eq!(
        lowpass_coefficients(48_000.0, 90_000.0, 1.0),
        [1.0, 0.0, 0.0, 0.0, 0.0]
    );
    assert_eq!(lowpass_coefficients(48_000.0, 0.0, 1.0), [0.0; 5]);
    assert_eq!(lowpass_coefficients(48_000.0, -5.0, 1.0), [0.0; 5]);
}

#[test]
fn equal_power_pan_matches_spec_for_mono_input() {
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let (l, r) = pan_mono(-1.0, 1.0);
    assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6, "{l} {r}");
    let (l, r) = pan_mono(0.0, 1.0);
    assert!((l - h).abs() < 1e-6 && (r - h).abs() < 1e-6, "{l} {r}");
    let (l, r) = pan_mono(1.0, 1.0);
    assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6, "{l} {r}");
    // Equal power everywhere.
    for p in [-0.8f32, -0.3, 0.2, 0.65] {
        let (l, r) = pan_mono(p, 1.0);
        assert!((l * l + r * r - 1.0).abs() < 1e-6);
    }
    // Out-of-range pans clamp.
    assert_eq!(pan_mono(5.0, 1.0), pan_mono(1.0, 1.0));
}

#[test]
fn equal_power_pan_matches_spec_for_stereo_input() {
    let (inl, inr) = (0.5f32, 0.25f32);
    // pan -1: x = 0 → L = inL + inR·cos 0, R = inR·sin 0.
    let (l, r) = pan_stereo(-1.0, inl, inr);
    assert!((l - 0.75).abs() < 1e-6 && r.abs() < 1e-6, "{l} {r}");
    // pan 0: pass-through.
    let (l, r) = pan_stereo(0.0, inl, inr);
    assert!((l - inl).abs() < 1e-6 && (r - inr).abs() < 1e-6, "{l} {r}");
    // pan 1: x = 1 → L = inL·cos(π/2), R = inR + inL·sin(π/2).
    let (l, r) = pan_stereo(1.0, inl, inr);
    assert!(l.abs() < 1e-6 && (r - 0.75).abs() < 1e-6, "{l} {r}");
    // pan 0.5 (spec arithmetic).
    let x = 0.5f64 * PI / 2.0;
    let (l, r) = pan_stereo(0.5, inl, inr);
    assert!((l as f64 - inl as f64 * x.cos()).abs() < 1e-6);
    assert!((r as f64 - (inr as f64 + inl as f64 * x.sin())).abs() < 1e-6);
}

#[test]
fn linear_ramp_hits_target_exactly_on_time() {
    let mut p = Param::new(2.0);
    p.ramp_to(6.0, 4);
    let vals: Vec<f32> = (0..6).map(|_| p.tick()).collect();
    assert_eq!(vals, vec![2.0, 3.0, 4.0, 5.0, 6.0, 6.0]);
    // A ramp of 0 frames jumps.
    p.ramp_to(1.0, 0);
    assert_eq!(p.tick(), 1.0);
    // Retargeting mid-ramp starts from the current value.
    p.ramp_to(3.0, 2);
    p.tick(); // 1.0
    p.ramp_to(0.0, 2); // current 2.0
    assert_eq!(p.tick(), 2.0);
    assert_eq!(p.tick(), 1.0);
    assert_eq!(p.tick(), 0.0);
}
