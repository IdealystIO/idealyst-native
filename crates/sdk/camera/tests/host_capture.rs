//! Real capture against the host's default camera. Marked `#[ignore]`
//! because it needs a camera + granted permission, which isn't available
//! in every CI/sandbox — run it explicitly with:
//!
//! ```text
//! cargo test -p camera --test host_capture -- --ignored --nocapture
//! ```
//!
//! It opens a stream, lets it run briefly, and asserts the callback fired
//! at least once with a well-formed (tightly-packed RGBA8) frame. A machine
//! with no camera or denied permission is reported as a skip rather than a
//! failure, so the test is meaningful where hardware exists without being a
//! false negative where it doesn't.
//!
//! Currently the only host with a real backend is Apple (macOS); the
//! desktop Linux/Windows stub returns `Unsupported`, which the test treats
//! as a skip.
#![cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use camera::{Camera, CameraConfig, CameraError};

#[tokio::test]
#[ignore = "requires a real camera + permission"]
async fn captures_frames_from_default_camera() {
    let frames = Arc::new(AtomicUsize::new(0));
    // Records a malformed frame (data length != width*height*4) if one ever
    // arrives, so the normalization contract is actually asserted.
    let malformed = Arc::new(AtomicUsize::new(0));
    let (frames_cb, malformed_cb) = (frames.clone(), malformed.clone());

    let cam = Camera::new();
    let stream = match cam
        .open(CameraConfig::default(), move |frame| {
            if frame.data.len() != frame.byte_len() || frame.width == 0 || frame.height == 0 {
                malformed_cb.fetch_add(1, Ordering::Relaxed);
            }
            frames_cb.fetch_add(1, Ordering::Relaxed);
        })
        .await
    {
        Ok(s) => s,
        Err(CameraError::NoCamera) => {
            eprintln!("skip: no camera on this host");
            return;
        }
        Err(CameraError::PermissionDenied) => {
            eprintln!("skip: camera permission not granted");
            return;
        }
        Err(CameraError::Unsupported) => {
            eprintln!("skip: no camera backend for this target");
            return;
        }
        Err(e) => panic!("open failed: {e}"),
    };

    // Let the capture queue deliver a few frames (≈1s at 30fps ⇒ ~30).
    // Frames arrive on AVFoundation's background queue, independent of this
    // thread, so a blocking sleep here doesn't starve them.
    std::thread::sleep(Duration::from_millis(1000));
    stream.stop();

    let n_frames = frames.load(Ordering::Relaxed);
    let n_bad = malformed.load(Ordering::Relaxed);
    eprintln!("captured {n_frames} frames in 1s ({n_bad} malformed)");
    assert!(n_frames > 0, "expected at least one capture callback");
    assert_eq!(n_bad, 0, "every frame must be tightly-packed RGBA8");
}
