//! Windows Sink Writer: synthetic video + audio through the public API land
//! as a real MP4.
//!
//! Windows used the stub (`Unsupported`). Drives `MediaWriter::record` with
//! synthetic `MediaStream` / `AudioStream` producers (no camera or microphone)
//! and checks the file is an MP4 (`ftyp`) of plausible size; a WebM request
//! is written as MP4 under the matching name.
#![cfg(windows)]

use media_stream::{AudioStream, MediaStream};
use media_writer::{Container, MediaInputs, MediaWriter, RecordConfig};

const W: u32 = 64;
const H: u32 = 48;
const FPS: u32 = 30;
const FRAME_US: u64 = 1_000_000 / FPS as u64;
const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;

fn record(container: Container, rel: &str, with_audio: bool) -> (String, Vec<u8>) {
    let store = files::app_files("media-writer-windows-host-test").expect("store");
    let (video, vw) = MediaStream::new();
    let (audio, aw) = AudioStream::new();
    let inputs = if with_audio { MediaInputs::av(&video, &audio) } else { MediaInputs::video(&video) };
    let recording = pollster::block_on(
        MediaWriter::new().record(inputs, RecordConfig::new(store.clone(), rel).container(container).fps(FPS)),
    )
    .expect("recording starts");
    let img: Vec<u8> = (0..W * H).flat_map(|i| [(i % 255) as u8, 0x90, 0xd0, 0xff]).collect();
    let chunk = vec![0.25f32; (RATE / FPS) as usize * CHANNELS as usize];
    for i in 0..45u64 {
        let pts = 1_000_000 + i * FRAME_US;
        vw.write_rgba8_at(W, H, &img, pts);
        if with_audio {
            aw.write_pcm_f32_at(RATE, CHANNELS, &chunk, pts);
        }
    }
    let out = pollster::block_on(recording.stop()).expect("finalize");
    let path = store.local_path(&out).expect("local path");
    let bytes = std::fs::read(&path).expect("read recording");
    if out == "av.mp4" {
        // Decode it back: the file plays, at the recorded size and length.
        let clip = pollster::block_on(video_decode::VideoDecoder::new().open(
            video_decode::DecodeSource::url(path.to_string_lossy().into_owned()),
            video_decode::DecodeConfig::default(),
        ))
        .expect("the recording opens in Media Foundation");
        assert_eq!(clip.natural_size(), Some((W, H)));
        let d = clip.transport().duration();
        assert!((1.2..=1.8).contains(&d), "45 frames at 30 fps ≈ 1.5 s, got {d}");
    }
    let _ = std::fs::remove_file(&path);
    (out, bytes)
}

#[test]
fn regression_windows_records_av_to_mp4() {
    let (out, bytes) = record(Container::Mp4, "av.mp4", true);
    assert_eq!(out, "av.mp4");
    assert!(bytes.len() > 1024, "{} bytes", bytes.len());
    assert_eq!(&bytes[4..8], b"ftyp", "an MP4 (ISO BMFF) file");
}

#[test]
fn webm_requests_are_recorded_as_mp4() {
    let (out, bytes) = record(Container::WebM, "video.webm", false);
    assert_eq!(out, "video.mp4");
    assert_eq!(&bytes[4..8], b"ftyp");
}

/// Recording over a file another program holds open (the gallery's player
/// showing the last recording under the same name) failed with Windows'
/// raw sharing-violation text. It now says the file is open elsewhere.
#[test]
fn regression_an_output_file_held_open_elsewhere_is_named_as_such() {
    use std::os::windows::fs::OpenOptionsExt;
    let store = files::app_files("media-writer-windows-host-test").expect("store");
    let rel = "held.mp4";
    let path = store.local_path(rel).expect("local path");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // No sharing at all: nobody else may open it.
    let held = std::fs::OpenOptions::new().write(true).create(true).share_mode(0).open(&path).expect("hold the file");
    let (video, vw) = MediaStream::new();
    let recording = pollster::block_on(MediaWriter::new().record(MediaInputs::video(&video), RecordConfig::new(store.clone(), rel)))
        .expect("recording starts");
    let img = vec![0u8; (W * H * 4) as usize];
    for i in 0..5u64 {
        vw.write_rgba8_at(W, H, &img, i * FRAME_US);
    }
    let err = pollster::block_on(recording.stop()).expect_err("the file is held");
    assert!(err.to_string().contains("open in another program"), "{err}");
    drop(held);
    let _ = std::fs::remove_file(&path);
}
