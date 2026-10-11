//! `SoundBuffer::decode` across the supported containers.
//!
//! WAV is generated here with `synth`. The compressed fixtures in
//! `tests/fixtures/` are a 0.5 s, 44.1 kHz mono 440 Hz sine (ffmpeg's `sine`
//! source, amplitude 1/8), encoded with:
//!
//! ```text
//! ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=0.5" -ac 1 -c:a flac       sine440.flac
//! ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=0.5" -ac 1 -c:a libvorbis -q:a 2 sine440.ogg
//! ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=0.5" -ac 1 -c:a libmp3lame -b:a 64k sine440.mp3
//! ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=0.25" -ac 2 -c:a flac       stereo48k.flac
//! ```

use audio_engine::{EngineError, SoundBuffer};
use media_stream::AudioFormat;

const FLAC: &[u8] = include_bytes!("fixtures/sine440.flac");
const OGG: &[u8] = include_bytes!("fixtures/sine440.ogg");
const MP3: &[u8] = include_bytes!("fixtures/sine440.mp3");
const STEREO_FLAC: &[u8] = include_bytes!("fixtures/stereo48k.flac");

const MONO_44K: AudioFormat = AudioFormat {
    sample_rate: 44_100,
    channels: 1,
};

/// Frequency from rising zero crossings over the middle of the buffer
/// (skipping codec warm-up at the edges).
fn pitch(buf: &SoundBuffer) -> f64 {
    let s = buf.playback_samples();
    let rate = buf.format().sample_rate as f64;
    let ch = buf.playback_channels() as usize;
    let mono: Vec<f32> = s.chunks_exact(ch).map(|f| f[0]).collect();
    let (a, b) = (mono.len() / 10, mono.len() * 9 / 10);
    let mid = &mono[a..b];
    let c = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    c as f64 / (mid.len() as f64 / rate)
}

fn peak(buf: &SoundBuffer) -> f32 {
    buf.playback_samples()
        .iter()
        .fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn decodes_wav_from_synth_sample_exact_to_16_bit() {
    let pcm = synth::tone(synth::Waveform::Sine, 440.0, 0.25, synth::DEFAULT_FORMAT).gain(0.5);
    let wav = pcm.to_wav();
    let buf = SoundBuffer::decode(&wav).unwrap();
    assert_eq!(buf.format(), synth::DEFAULT_FORMAT);
    assert_eq!(buf.playback_samples().len(), pcm.samples.len());
    for (a, b) in buf.playback_samples().iter().zip(&pcm.samples) {
        assert!((a - b).abs() <= 1.0 / 32_768.0 + 1e-6, "{a} vs {b}");
    }
    // And `from_pcm` is the lossless path.
    let direct = SoundBuffer::from_pcm(&pcm);
    assert_eq!(direct.playback_samples(), &pcm.samples[..]);
    assert!((direct.duration_secs() - 0.25).abs() < 1e-3);
}

#[test]
fn decodes_flac() {
    let buf = SoundBuffer::decode(FLAC).unwrap();
    assert_eq!(buf.format(), MONO_44K);
    assert!(
        (buf.duration_secs() - 0.5).abs() < 1e-3,
        "{}",
        buf.duration_secs()
    );
    assert!((pitch(&buf) - 440.0).abs() < 5.0, "{}", pitch(&buf));
    assert!((peak(&buf) - 0.125).abs() < 0.01, "{}", peak(&buf));
}

#[test]
fn decodes_ogg_vorbis() {
    let buf = SoundBuffer::decode(OGG).unwrap();
    assert_eq!(buf.format(), MONO_44K);
    assert!(
        (buf.duration_secs() - 0.5).abs() < 0.01,
        "{}",
        buf.duration_secs()
    );
    assert!((pitch(&buf) - 440.0).abs() < 5.0, "{}", pitch(&buf));
}

#[test]
fn decodes_mp3_with_gapless_trim() {
    let buf = SoundBuffer::decode(MP3).unwrap();
    assert_eq!(buf.format(), MONO_44K);
    // Encoder delay/padding trimmed: within a frame of the source length
    // (without the trim it's ~1 MP3 frame = 26 ms longer).
    assert!(
        (buf.duration_secs() - 0.5).abs() < 0.015,
        "{}",
        buf.duration_secs()
    );
    assert!((pitch(&buf) - 440.0).abs() < 5.0, "{}", pitch(&buf));
}

#[test]
fn decodes_stereo_flac_keeping_its_format() {
    let buf = SoundBuffer::decode(STEREO_FLAC).unwrap();
    assert_eq!(
        buf.format(),
        AudioFormat {
            sample_rate: 48_000,
            channels: 2
        }
    );
    assert_eq!(buf.playback_channels(), 2);
    assert!((buf.duration_secs() - 0.25).abs() < 1e-3);
}

#[test]
fn rejects_garbage() {
    match SoundBuffer::decode(b"definitely not audio, just some bytes") {
        Err(EngineError::Decode(_)) => {}
        other => panic!("expected a decode error, got {other:?}"),
    }
}

#[test]
fn wide_layouts_downmix_with_web_audio_speaker_rules() {
    // 5.1: L R C LFE SL SR → L + √½(C + SL), R + √½(C + SR).
    let buf = SoundBuffer::from_samples(
        AudioFormat {
            sample_rate: 48_000,
            channels: 6,
        },
        vec![0.1, 0.2, 0.3, 0.9, 0.4, 0.5],
    );
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let s = buf.playback_samples();
    assert!((s[0] - (0.1 + h * 0.7)).abs() < 1e-6);
    assert!((s[1] - (0.2 + h * 0.8)).abs() < 1e-6);
    assert_eq!(buf.format().channels, 6);
    // Quad: ½(L + SL), ½(R + SR).
    let quad = SoundBuffer::from_samples(
        AudioFormat {
            sample_rate: 48_000,
            channels: 4,
        },
        vec![0.2, 0.4, 0.6, 0.8],
    );
    assert_eq!(quad.playback_samples(), &[0.4, 0.6]);
}
