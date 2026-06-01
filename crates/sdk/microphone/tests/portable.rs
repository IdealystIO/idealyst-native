//! Portable tests that run on every target's host toolchain — no audio
//! device required. They lock down the platform-agnostic surface
//! (`AudioBuffer` framing math, `AudioStreamConfig` builders, the public
//! types' shape). Backend behaviour is exercised by `host_capture.rs`.

use microphone::{AudioBuffer, AudioStreamConfig};

#[test]
fn buffer_frame_count_mono() {
    let samples = [0.0f32; 480];
    let buf = AudioBuffer {
        samples: &samples,
        sample_rate: 48_000,
        channels: 1,
    };
    assert_eq!(buf.frame_count(), 480);
    // 480 frames / 48 kHz = 10 ms.
    assert!((buf.duration_secs() - 0.01).abs() < 1e-9);
}

#[test]
fn buffer_frame_count_stereo_is_interleaved() {
    // 4 interleaved stereo frames => 8 samples.
    let samples = [0.1, -0.1, 0.2, -0.2, 0.3, -0.3, 0.4, -0.4];
    let buf = AudioBuffer {
        samples: &samples,
        sample_rate: 44_100,
        channels: 2,
    };
    assert_eq!(buf.frame_count(), 4);
}

#[test]
fn buffer_zero_rate_is_zero_duration_not_nan() {
    let samples = [0.0f32; 10];
    let buf = AudioBuffer {
        samples: &samples,
        sample_rate: 0,
        channels: 1,
    };
    assert_eq!(buf.duration_secs(), 0.0);
}

#[test]
fn config_default_is_device_defaults() {
    let c = AudioStreamConfig::default();
    assert_eq!(c.sample_rate, None);
    assert_eq!(c.channels, None);
}

#[test]
fn config_builders_compose() {
    let c = AudioStreamConfig::new().with_sample_rate(16_000).mono();
    assert_eq!(c.sample_rate, Some(16_000));
    assert_eq!(c.channels, Some(1));
}
