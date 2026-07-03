//! Portable tests that run on every target's host toolchain — no camera
//! device required. They lock down the platform-agnostic surface
//! (`VideoFrame` size math, `CameraConfig` builders, the public types'
//! shape). Backend behaviour is exercised by `host_capture.rs`.

use camera::{CameraConfig, CameraFacing, PixelFormat, VideoFrame};

#[test]
fn frame_size_math_is_tightly_packed_rgba8() {
    // 4x2 RGBA8 = 32 bytes.
    let data = [0u8; 4 * 2 * 4];
    let frame = VideoFrame {
        data: &data,
        width: 4,
        height: 2,
        format: PixelFormat::Rgba8,
        pts_micros: 0,
    };
    assert_eq!(frame.pixel_count(), 8);
    assert_eq!(frame.byte_len(), 32);
    assert_eq!(frame.data.len(), frame.byte_len());
}

#[test]
fn config_default_is_device_defaults_primary_camera() {
    let c = CameraConfig::default();
    assert_eq!(c.width, None);
    assert_eq!(c.height, None);
    assert_eq!(c.fps, None);
    assert_eq!(c.facing, CameraFacing::Default);
}

#[test]
fn config_builders_compose() {
    let c = CameraConfig::new()
        .with_resolution(1280, 720)
        .with_fps(30)
        .front();
    assert_eq!(c.width, Some(1280));
    assert_eq!(c.height, Some(720));
    assert_eq!(c.fps, Some(30));
    assert_eq!(c.facing, CameraFacing::Front);
}

#[test]
fn config_front_then_back_takes_last() {
    let c = CameraConfig::new().front().back();
    assert_eq!(c.facing, CameraFacing::Back);
}
