//! Real capture against the host's default input device. Marked
//! `#[ignore]` because it needs a microphone + granted permission, which
//! isn't available in every CI/sandbox — run it explicitly with:
//!
//! ```text
//! cargo test -p microphone --test host_capture -- --ignored --nocapture
//! ```
//!
//! It opens a stream, lets it run briefly, and asserts the callback fired
//! at least once. A machine with no input device is reported as a skip
//! rather than a failure, so the test is meaningful where hardware exists
//! without being a false negative where it doesn't.
#![cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use microphone::{AudioStreamConfig, MicError, Microphone};

#[tokio::test]
#[ignore = "requires a real microphone + permission"]
async fn captures_frames_from_default_device() {
    let frames = Arc::new(AtomicUsize::new(0));
    let chunks = Arc::new(AtomicUsize::new(0));
    let (frames_cb, chunks_cb) = (frames.clone(), chunks.clone());

    let mic = Microphone::new();
    let stream = match mic
        .open(AudioStreamConfig::default(), move |buf| {
            frames_cb.fetch_add(buf.frame_count(), Ordering::Relaxed);
            chunks_cb.fetch_add(1, Ordering::Relaxed);
        })
        .await
    {
        Ok(s) => s,
        Err(MicError::NoInputDevice) => {
            eprintln!("skip: no input device on this host");
            return;
        }
        Err(e) => panic!("open failed: {e}"),
    };

    // Let the audio thread deliver a few chunks.
    std::thread::sleep(Duration::from_millis(500));
    stream.stop();

    let n_chunks = chunks.load(Ordering::Relaxed);
    let n_frames = frames.load(Ordering::Relaxed);
    eprintln!("captured {n_chunks} chunks / {n_frames} frames in 500 ms");
    assert!(n_chunks > 0, "expected at least one capture callback");
    assert!(n_frames > 0, "expected at least one frame of audio");
}

/// A requested rate is delivered by converting the device's capture, at
/// the requested rate and channel count, at real-time pace — the path a
/// 16 kHz dictation stream takes on every native target.
#[tokio::test]
#[ignore = "requires a real microphone + permission"]
async fn captures_at_a_requested_rate_the_device_does_not_run() {
    let frames = Arc::new(AtomicUsize::new(0));
    let formats = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (frames_cb, formats_cb) = (frames.clone(), formats.clone());

    let config = AudioStreamConfig::default().with_sample_rate(16_000).mono();
    let stream = match Microphone::new()
        .open(config, move |buf| {
            frames_cb.fetch_add(buf.frame_count(), Ordering::Relaxed);
            formats_cb.lock().unwrap().push((buf.sample_rate, buf.channels));
        })
        .await
    {
        Ok(s) => s,
        Err(MicError::NoInputDevice) => {
            eprintln!("skip: no input device on this host");
            return;
        }
        Err(e) => panic!("open failed: {e}"),
    };
    std::thread::sleep(Duration::from_millis(1000));
    stream.stop();

    let formats = formats.lock().unwrap();
    assert!(!formats.is_empty(), "expected capture callbacks");
    assert!(formats.iter().all(|f| *f == (16_000, 1)), "{formats:?}");
    let n = frames.load(Ordering::Relaxed);
    eprintln!("captured {n} frames at 16 kHz in 1 s");
    assert!((8_000..=20_000).contains(&n), "about a second of 16 kHz audio, got {n} frames");
}
