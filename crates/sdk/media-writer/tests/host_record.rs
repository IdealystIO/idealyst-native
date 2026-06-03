//! Host smoke test for the Apple `AVAssetWriter` backend, run on macOS.
//!
//! It feeds *synthetic* `MediaStream` + `AudioStream` producers (no camera or
//! microphone hardware) and asserts a non-trivial, real `.mp4` lands on disk —
//! exercising the full RGBA→CVPixelBuffer + PCM→CMSampleBuffer + mux path,
//! including the lazy writer-start that waits for both inputs' formats.

#![cfg(target_os = "macos")]

use media_stream::{AudioStream, MediaStream};
use media_writer::{MediaInputs, MediaWriter, RecordConfig};

const W: u32 = 64;
const H: u32 = 48;
const FPS: u32 = 30;
const FRAME_US: u64 = 1_000_000 / FPS as u64;
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;

#[tokio::test]
async fn records_synthetic_av_to_real_mp4() {
    let store = files::app_files("media-writer-host-test").expect("app files store");
    let rel = "host_record.mp4";

    let (video, vw) = MediaStream::new();
    let (audio, aw) = AudioStream::new();

    let writer = MediaWriter::new();
    let recording = writer
        .record(
            MediaInputs::av(&video, &audio),
            RecordConfig::new(store.clone(), rel).fps(FPS),
        )
        .await
        .expect("start recording");

    // One full color frame + one ~one-frame audio chunk per tick, stamped on a
    // shared microsecond timeline so the muxer can align them.
    let frame = vec![0u8; (W * H * 4) as usize];
    let audio_frames = (SAMPLE_RATE / FPS) as usize; // ≈ one video frame of audio
    let chunk = vec![0.0f32; audio_frames * CHANNELS as usize];
    for i in 0..30u64 {
        let pts = i * FRAME_US;
        vw.write_rgba8_at(W, H, &frame, pts);
        aw.write_pcm_f32_at(SAMPLE_RATE, CHANNELS, &chunk, pts);
    }

    let out = recording.stop().await.expect("finalize recording");
    assert_eq!(out, rel);

    let path = store.local_path(&out).expect("local path on native");
    let bytes = std::fs::read(&path).expect("read recorded file");

    // A real MP4: the `ftyp` box brand sits at bytes 4..8, and 30 frames of
    // H.264 + AAC is comfortably more than a couple KB.
    assert!(
        bytes.len() > 2_000,
        "recording too small: {} bytes",
        bytes.len()
    );
    assert_eq!(&bytes[4..8], b"ftyp", "output is not an MP4 container");

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn no_input_is_rejected() {
    let store = files::app_files("media-writer-host-test").expect("store");
    let writer = MediaWriter::new();
    let result = writer
        .record(
            MediaInputs {
                video: None,
                audio: None,
            },
            RecordConfig::new(store, "nope.mp4"),
        )
        .await;
    assert!(
        matches!(result, Err(media_writer::MediaWriterError::NoInput)),
        "must reject empty inputs"
    );
}
