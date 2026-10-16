//! The Windows decoder opens a clip and delivers frames, not `Unsupported`.
//!
//! Windows used the stub. Checked here on a real Windows machine: a public
//! MP4 opens (natural size and duration known), `max_dimension` bounds the
//! frames, playback delivers BGRA frames on the stream, and the transport
//! seeks. Needs network access (W3C's trailer, as the video tests use).
#![cfg(windows)]

use std::time::{Duration, Instant};

use video_decode::{DecodeConfig, DecodeSource, VideoDecoder};

const CLIP: &str = "https://media.w3.org/2010/05/sintel/trailer.mp4";

#[test]
fn regression_windows_decodes_frames_from_a_clip() {
    let clip = pollster::block_on(VideoDecoder::new().open(
        DecodeSource::url(CLIP),
        DecodeConfig { max_dimension: Some(320), muted: true, ..Default::default() },
    ))
    .expect("the clip opens");
    let (w, h) = clip.natural_size().expect("a video clip has a size");
    assert!(w >= 320 && h > 0, "natural size {w}x{h}");
    assert!(clip.transport().duration() > 30.0);

    clip.transport().play();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut buf = Vec::new();
    let mut got = None;
    while got.is_none() && Instant::now() < deadline {
        got = clip.frames().latest(&mut buf);
        std::thread::sleep(Duration::from_millis(50));
    }
    let (fw, fh) = got.expect("a frame arrives");
    assert_eq!(fw, 320, "max_dimension bounds the longer side");
    assert_eq!(buf.len(), (fw * fh * 4) as usize);
    clip.transport().seek(10.0);
    let deadline = Instant::now() + Duration::from_secs(10);
    while (clip.transport().position() - 10.0).abs() > 1.5 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!((clip.transport().position() - 10.0).abs() <= 1.5, "seek lands near 10 s");
}
