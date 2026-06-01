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
