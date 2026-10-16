//! The Windows decoder opens a clip and delivers frames, not `Unsupported`.
//!
//! Windows used the stub. Checked here on a real Windows machine: a public
//! MP4 opens (natural size and duration known), `max_dimension` bounds the
//! frames, playback delivers BGRA frames on the stream, and the transport
//! seeks. Needs network access (W3C's trailer, as the video tests use). The
//! audio tap is checked on a WAV the test writes.
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
    assert!(clip.audio().is_some(), "the trailer's sound track is tapped too");

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

/// A mono 48 kHz 16-bit WAV: 2 s of 500 Hz, then 2 s of 1500 Hz.
fn two_tone_wav() -> std::path::PathBuf {
    const RATE: u32 = 48_000;
    let mut pcm = Vec::new();
    for i in 0..RATE * 4 {
        let t = i as f64 / RATE as f64;
        let f = if t < 2.0 { 500.0 } else { 1500.0 };
        let v = (0.5 * (2.0 * std::f64::consts::PI * f * t).sin() * i16::MAX as f64) as i16;
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(&pcm);
    let path = std::env::temp_dir().join(format!("video-decode-two-tone-{}.wav", std::process::id()));
    std::fs::write(&path, wav).expect("write the clip");
    path
}

/// The tone in `samples` (mono), from its zero crossings.
fn frequency(samples: &[f32], rate: u32) -> f64 {
    let crossings = samples.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    crossings as f64 / 2.0 / (samples.len() as f64 / rate as f64)
}

fn wait(d: Duration) {
    std::thread::sleep(d);
}

/// `audio()` was always `None` on Windows: the Media Engine plays a clip's
/// sound but exposes no samples, so a recording of a clip had no audio.
/// Now the clip's PCM arrives as it plays — the right tone, in real time,
/// nothing while paused, the new position's tone after a seek — and the
/// player's mute doesn't silence it.
#[test]
fn regression_windows_delivers_the_clips_audio_as_it_plays() {
    use std::sync::{Arc, Mutex};
    let path = two_tone_wav();
    let clip = pollster::block_on(VideoDecoder::new().open(
        DecodeSource::url(format!("file:///{}", path.display().to_string().replace('\\', "/"))),
        DecodeConfig { muted: true, ..Default::default() },
    ))
    .expect("the clip opens");
    let audio = clip.audio().expect("a clip with a sound track has an audio stream");
    let got: Arc<Mutex<Vec<f32>>> = Arc::default();
    let rate: Arc<Mutex<Option<(u32, u16)>>> = Arc::default();
    let (g, r) = (got.clone(), rate.clone());
    let _sub = audio.subscribe(move |f: &media_stream::AudioFrame| {
        *r.lock().unwrap() = Some((f.sample_rate, f.channels));
        g.lock().unwrap().extend_from_slice(f.samples);
    });
    let take = || std::mem::take(&mut *got.lock().unwrap());

    wait(Duration::from_millis(400));
    assert!(take().is_empty(), "nothing before play");

    clip.transport().play();
    wait(Duration::from_millis(1200));
    let first = take();
    assert_eq!(*rate.lock().unwrap(), Some((48_000, 1)), "the clip's own format");
    let secs = first.len() as f64 / 48_000.0;
    assert!((0.8..=1.4).contains(&secs), "real time: {secs:.2} s of audio in 1.2 s");
    let f = frequency(&first[4800..], 48_000);
    assert!((f - 500.0).abs() < 25.0, "the first tone, {f:.0} Hz");
    assert!(first.iter().any(|v| v.abs() > 0.4), "audible despite the player's mute");

    clip.transport().pause();
    wait(Duration::from_millis(200));
    take();
    wait(Duration::from_millis(400));
    assert!(take().is_empty(), "nothing while paused");

    clip.transport().seek(2.5);
    clip.transport().play();
    wait(Duration::from_millis(800));
    let after = take();
    assert!(after.len() > 4800 * 3, "delivery resumes after the seek ({} samples)", after.len());
    let f = frequency(&after[2400..], 48_000);
    assert!((f - 1500.0).abs() < 50.0, "the tone at the new position, {f:.0} Hz");
    drop(clip);
    let _ = std::fs::remove_file(path);
}
