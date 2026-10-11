//! `spatial::params_for` — pan from direction, gain + muffling from distance.

use audio_engine::spatial::{params_for, Attenuation, Listener, UNMUFFLED_HZ};
use audio_engine::VoiceParams;

#[test]
fn pan_follows_left_right_including_behind() {
    let l = Listener::default(); // facing −Z, +X is right
    let att = Attenuation::default();
    let pan = |p: [f32; 3]| params_for(&l, p, &att).1;
    assert!((pan([5.0, 0.0, 0.0]) - 1.0).abs() < 1e-6, "hard right");
    assert!((pan([-5.0, 0.0, 0.0]) + 1.0).abs() < 1e-6, "hard left");
    assert!(pan([0.0, 0.0, -5.0]).abs() < 1e-6, "ahead");
    assert!(pan([0.0, 0.0, 5.0]).abs() < 1e-6, "behind");
    assert!(pan([3.0, 0.0, 3.0]) > 0.5, "behind-right pans right");
    assert!(pan([-3.0, 0.0, 3.0]) < -0.5, "behind-left pans left");
    // Turning the listener around mirrors it.
    let turned = Listener {
        forward: [0.0, 0.0, 1.0],
        ..l
    };
    assert!(params_for(&turned, [5.0, 0.0, 0.0], &att).1 < -0.99);
}

#[test]
fn distance_lowers_gain_and_cutoff_monotonically() {
    let l = Listener::default();
    let att = Attenuation::default();
    let (g0, _, lp0) = params_for(&l, [0.0, 0.0, -0.5], &att);
    assert_eq!(g0, 1.0);
    assert_eq!(lp0, None, "within ref distance: unfiltered");

    let mut last_gain = 1.0f32;
    let mut last_hz = UNMUFFLED_HZ;
    for d in [2.0f32, 4.0, 8.0, 16.0, 32.0, 50.0] {
        let (g, pan, lp) = params_for(&l, [0.0, 0.0, -d], &att);
        let hz = lp.expect("muffled past ref distance");
        assert!(g < last_gain, "gain not falling at {d}");
        assert!(hz < last_hz, "cutoff not falling at {d}");
        assert!(pan.abs() < 1e-6);
        last_gain = g;
        last_hz = hz;
    }
    // Inverse law: ref / (ref + rolloff (d − ref)).
    let (g, _, _) = params_for(&l, [0.0, 0.0, -11.0], &att);
    assert!((g - 1.0 / 11.0).abs() < 1e-6);
    // At max distance the cutoff is the configured muffle; past it nothing moves.
    let (gm, _, lpm) = params_for(&l, [0.0, 0.0, -50.0], &att);
    assert!((lpm.unwrap() - att.muffle_hz_at_max).abs() < 0.5);
    let (gf, _, lpf) = params_for(&l, [0.0, 0.0, -500.0], &att);
    assert_eq!((gm, lpm), (gf, lpf));
}

#[test]
fn muffling_can_be_disabled() {
    let att = Attenuation {
        muffle_hz_at_max: 0.0,
        ..Attenuation::default()
    };
    assert_eq!(
        params_for(&Listener::default(), [0.0, 0.0, -30.0], &att).2,
        None
    );
}

#[test]
fn voice_params_spatial_carries_the_math() {
    let l = Listener::default();
    let att = Attenuation::default();
    let p = VoiceParams::spatial(&l, [4.0, 0.0, 0.0], &att);
    let (g, pan, lp) = params_for(&l, [4.0, 0.0, 0.0], &att);
    assert_eq!((p.gain, p.pan, p.lowpass_hz), (g, pan, lp));
    assert_eq!(p.rate, 1.0);
    assert!(!p.looping);
}
