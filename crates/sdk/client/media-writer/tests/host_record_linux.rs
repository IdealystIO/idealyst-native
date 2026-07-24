//! Host smoke test for the Linux **GStreamer** backend.
//!
//! It drives the real public API (`MediaWriter::record` → push frames through
//! the streams' writers → `Recording::stop`) with *synthetic* `MediaStream` /
//! `AudioStream` producers (no camera or microphone hardware), and asserts a
//! non-trivial, real `.webm` lands on disk. This exercises the whole path the
//! cfg cascade selects on Linux — proving the module is actually reached and
//! writes a valid file, the failure mode a silently-degrading encoder hides.

#![cfg(target_os = "linux")]

use media_stream::{AudioStream, MediaStream};
use media_writer::{Container, MediaInputs, MediaWriter, MediaWriterError, RecordConfig};

const W: u32 = 64;
const H: u32 = 48;
const FPS: u32 = 30;
const FRAME_US: u64 = 1_000_000 / FPS as u64;
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;

/// A solid-color tightly-packed RGBA8 frame.
fn frame() -> Vec<u8> {
    let mut v = Vec::with_capacity((W * H * 4) as usize);
    for _ in 0..(W * H) {
        v.extend_from_slice(&[0x20, 0x90, 0xd0, 0xff]);
    }
    v
}

/// Record video + audio through the public API into a real WebM, then verify
/// the file exists, is non-empty, and is a Matroska/WebM (EBML) container.
#[tokio::test]
async fn records_synthetic_av_to_valid_webm() {
    let store = files::app_files("media-writer-linux-host-test").expect("app files store");
    let rel = "host_record_av.webm";

    let (video, vw) = MediaStream::new();
    let (audio, aw) = AudioStream::new();

    let writer = MediaWriter::new();
    let recording = writer
        .record(
            MediaInputs::av(&video, &audio),
            RecordConfig::new(store.clone(), rel)
                .container(Container::WebM)
                .fps(FPS),
        )
        .await
        .expect("start recording (Linux GStreamer backend must be reached)");

    let img = frame();
    let audio_frames = (SAMPLE_RATE / FPS) as usize;
    let chunk = vec![0.0f32; audio_frames * CHANNELS as usize];
    // Space writes slightly so the software VP8 encoder keeps up with the
    // in-flight cap rather than dropping most frames.
    for i in 0..20u64 {
        let pts = i * FRAME_US;
        vw.write_rgba8_at(W, H, &img, pts);
        aw.write_pcm_f32_at(SAMPLE_RATE, CHANNELS, &chunk, pts);
        std::thread::sleep(std::time::Duration::from_millis(4));
    }

    let out = recording.stop().await.expect("finalize recording");
    assert_eq!(out, rel);

    let path = store.local_path(&out).expect("local path on native");
    let bytes = std::fs::read(&path).expect("read recorded file");
    assert!(bytes.len() > 512, "recording too small: {} bytes", bytes.len());
    assert_eq!(
        &bytes[0..4],
        &[0x1A, 0x45, 0xDF, 0xA3],
        "output is not a Matroska/WebM (EBML) container"
    );

    let _ = std::fs::remove_file(&path);
}

/// Video-only recording through the public API also yields a valid WebM.
#[tokio::test]
async fn records_synthetic_video_only_to_valid_webm() {
    let store = files::app_files("media-writer-linux-host-test").expect("store");
    let rel = "host_record_video.webm";

    let (video, vw) = MediaStream::new();
    let writer = MediaWriter::new();
    let recording = writer
        .record(
            MediaInputs::video(&video),
            RecordConfig::new(store.clone(), rel).container(Container::WebM),
        )
        .await
        .expect("start video-only recording");

    let img = frame();
    for i in 0..20u64 {
        vw.write_rgba8_at(W, H, &img, i * FRAME_US);
        std::thread::sleep(std::time::Duration::from_millis(4));
    }

    let out = recording.stop().await.expect("finalize");
    let path = store.local_path(&out).expect("local path");
    let bytes = std::fs::read(&path).expect("read file");
    assert!(bytes.len() > 512, "too small: {} bytes", bytes.len());
    assert_eq!(&bytes[0..4], &[0x1A, 0x45, 0xDF, 0xA3], "not a WebM container");
    let _ = std::fs::remove_file(&path);
}

/// Codec-selection / unsupported path: requesting `Container::Mp4` on a host
/// without an H.264 encoder plugin must fail HONESTLY at `record()` (a `Backend`
/// error naming the missing plugin) rather than write a broken file. On a host
/// that has an H.264 encoder it's allowed to succeed. Never `Unsupported`, never
/// a panic.
#[tokio::test]
async fn mp4_without_h264_encoder_is_rejected_honestly() {
    let store = files::app_files("media-writer-linux-host-test").expect("store");
    let (video, _vw) = MediaStream::new();
    let writer = MediaWriter::new();
    let result = writer
        .record(
            MediaInputs::video(&video),
            RecordConfig::new(store, "should_fail_or_h264.mp4").container(Container::Mp4),
        )
        .await;

    match result {
        Ok(recording) => {
            // This host has an H.264 encoder; the request was honored. Tear it
            // down without asserting file contents (H.264/MP4 specifics).
            let _ = recording.stop().await;
        }
        Err(MediaWriterError::Backend(msg)) => {
            assert!(
                msg.contains("H.264"),
                "error must name the missing H.264 encoder, got: {msg}"
            );
        }
        Err(other) => panic!("expected an honest Backend error for H.264/MP4, got: {other:?}"),
    }
}

/// Empty inputs are still rejected with `NoInput` (the top-level contract,
/// unchanged by the Linux backend).
#[tokio::test]
async fn no_input_is_rejected() {
    let store = files::app_files("media-writer-linux-host-test").expect("store");
    let writer = MediaWriter::new();
    let result = writer
        .record(
            MediaInputs {
                video: None,
                audio: None,
            },
            RecordConfig::new(store, "nope.webm"),
        )
        .await;
    assert!(matches!(result, Err(MediaWriterError::NoInput)));
}
