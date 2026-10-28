//! Real capture against the host's default camera. Marked `#[ignore]`
//! because it needs a camera + granted permission, which isn't available
//! in every CI/sandbox — run it explicitly with:
//!
//! ```text
//! cargo test -p camera --test host_capture -- --ignored --nocapture
//! ```
//!
//! It opens a `MediaStream`, subscribes, lets it run briefly, and asserts a
//! frame arrived well-formed (tightly-packed RGBA8). A machine with no camera
//! or denied permission is reported as a skip rather than a failure, so the
//! test is meaningful where hardware exists without being a false negative
//! where it doesn't.
//!
//! Hosts with a real backend are Apple (macOS) and desktop **Linux**
//! (GStreamer V4L2). On Linux with no camera this returns `NoCamera` (a
//! skip); the remaining desktop-Windows stub returns `Unsupported` (also a
//! skip). All non-hardware outcomes are skips, not failures.
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
    let stream = match cam.open(CameraConfig::default()).await {
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

    let sub = stream.subscribe(move |frame| {
        if frame.data.len() != frame.byte_len() || frame.width == 0 || frame.height == 0 {
            malformed_cb.fetch_add(1, Ordering::Relaxed);
        }
        frames_cb.fetch_add(1, Ordering::Relaxed);
    });

    // Let the capture queue deliver a few frames (≈1s at 30fps ⇒ ~30).
    // Frames arrive on AVFoundation's background queue, independent of this
    // thread, so a blocking sleep here doesn't starve them.
    std::thread::sleep(Duration::from_millis(1000));
    drop(sub);
    drop(stream);

    let n_frames = frames.load(Ordering::Relaxed);
    let n_bad = malformed.load(Ordering::Relaxed);
    eprintln!("captured {n_frames} frames in 1s ({n_bad} malformed)");
    assert!(n_frames > 0, "expected at least one capture callback");
    assert_eq!(n_bad, 0, "every frame must be tightly-packed RGBA8");
}

/// Lists the host's cameras and opens each by its id: every listed id must
/// open (a picker that offers a camera `open` then refuses is broken), and an
/// id no camera has must be `NoCamera`.
#[tokio::test]
#[ignore = "requires a real camera + permission"]
async fn every_listed_camera_opens_by_id() {
    let cam = Camera::new();
    let devices = match cam.devices().await {
        Ok(d) => d,
        Err(CameraError::PermissionDenied) => {
            eprintln!("skip: camera permission not granted");
            return;
        }
        Err(CameraError::Unsupported) => {
            eprintln!("skip: no camera backend for this target");
            return;
        }
        Err(e) => panic!("devices failed: {e}"),
    };
    eprintln!("cameras: {devices:#?}");
    for d in &devices {
        assert!(!d.id.is_empty() && !d.label.is_empty(), "id and label are never empty: {d:?}");
        let stream = cam
            .open(CameraConfig::new().with_device(&d.id))
            .await
            .unwrap_or_else(|e| panic!("listed camera {:?} must open: {e}", d.label));
        let frames = Arc::new(AtomicUsize::new(0));
        let counter = frames.clone();
        let sub = stream.subscribe(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        // External webcams can take a few seconds to deliver a first frame.
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while frames.load(Ordering::Relaxed) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("{:?}: first frame after polling, {} so far", d.label, frames.load(Ordering::Relaxed));
        drop(sub);
        drop(stream);
        assert!(frames.load(Ordering::Relaxed) > 0, "{:?} delivered no frames", d.label);
    }

    let missing = cam.open(CameraConfig::new().with_device("no-such-camera")).await;
    assert!(matches!(missing, Err(CameraError::NoCamera)), "an unknown id is NoCamera");
}
