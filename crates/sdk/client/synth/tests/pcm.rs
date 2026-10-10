//! `Pcm` operations, format conversion, WAV encode/decode, and the
//! `media_stream` / `audio` pipeline seams.

use std::sync::{Arc, Mutex};

use media_stream::{AudioFrame, AudioStream};
use synth::{tone, AudioFormat, Pcm, SynthError, Waveform};

const MONO_8K: AudioFormat = AudioFormat { sample_rate: 8_000, channels: 1 };
const STEREO_8K: AudioFormat = AudioFormat { sample_rate: 8_000, channels: 2 };

fn ramp(n: usize) -> Pcm {
    Pcm::new(MONO_8K, (0..n).map(|i| i as f32 / n as f32).collect())
}

#[test]
fn silence_and_durations() {
    let p = Pcm::silence(STEREO_8K, 0.5);
    assert_eq!(p.frame_count(), 4_000);
    assert_eq!(p.samples.len(), 8_000);
    assert!((p.duration_secs() - 0.5).abs() < 1e-9);
    assert_eq!(Pcm::silence(MONO_8K, -1.0).frame_count(), 0);
}

#[test]
fn new_drops_a_trailing_partial_frame() {
    assert_eq!(Pcm::new(STEREO_8K, vec![0.1, 0.2, 0.3]).samples, vec![0.1, 0.2]);
}

#[test]
fn gain_and_mix_clamp() {
    let loud = Pcm::new(MONO_8K, vec![0.8, -0.8, 0.2]);
    assert_eq!(loud.clone().gain(2.0).samples, vec![1.0, -1.0, 0.4]);
    let mixed = loud.clone().mix(&loud);
    assert_eq!(mixed.samples, vec![1.0, -1.0, 0.4]);
}

#[test]
fn mix_at_offsets_and_extends() {
    let base = Pcm::new(MONO_8K, vec![0.1; 4]);
    let hit = Pcm::new(MONO_8K, vec![0.5; 4]);
    // 2 frames at 8 kHz = 0.25 ms.
    let out = base.mix_at(&hit, 2.0 / 8_000.0);
    assert_eq!(out.samples.len(), 6);
    let expect = [0.1, 0.1, 0.6, 0.6, 0.5, 0.5];
    for (a, b) in out.samples.iter().zip(expect) {
        assert!((a - b).abs() < 1e-6, "{:?}", out.samples);
    }
}

#[test]
fn mix_converts_the_other_format() {
    let base = Pcm::silence(STEREO_8K, 0.01);
    let other = Pcm::new(AudioFormat { sample_rate: 4_000, channels: 1 }, vec![0.5; 40]);
    let out = base.mix(&other);
    assert_eq!(out.format, STEREO_8K);
    assert_eq!(out.frame_count(), 80);
    assert!(out.samples.iter().all(|&s| (s - 0.5).abs() < 1e-6));
}

#[test]
fn concat_reverse_fades() {
    let r = ramp(4);
    let c = r.clone().concat(&r);
    assert_eq!(c.frame_count(), 8);
    assert_eq!(r.clone().reverse().samples, vec![0.75, 0.5, 0.25, 0.0]);

    let stereo = Pcm::new(STEREO_8K, vec![1.0, -1.0, 2.0, -2.0]);
    assert_eq!(stereo.reverse().samples, vec![2.0, -2.0, 1.0, -1.0], "frames reversed, channels kept");

    let ones = Pcm::new(MONO_8K, vec![1.0; 8_000]);
    let faded = ones.clone().fade_in(0.5).fade_out(0.5);
    assert_eq!(faded.samples[0], 0.0);
    assert_eq!(*faded.samples.last().unwrap(), 0.0);
    assert!((faded.samples[2_000] - 0.5).abs() < 1e-3);
}

#[test]
fn normalize_sets_peak_and_ignores_silence() {
    let p = Pcm::new(MONO_8K, vec![0.1, -0.25, 0.05]).normalize(1.0);
    assert!((p.peak() - 1.0).abs() < 1e-6);
    assert!((p.samples[0] - 0.4).abs() < 1e-6);
    assert_eq!(Pcm::silence(MONO_8K, 0.01).normalize(1.0).peak(), 0.0);
}

#[test]
fn resample_and_remix_lengths() {
    let p = tone(Waveform::Sine, 100.0, 1.0, MONO_8K);
    assert_eq!(p.clone().resample(44_100).frame_count(), 44_100);
    assert_eq!(p.clone().resample(16_000).frame_count(), 16_000);
    assert_eq!(p.clone().resample(4_000).frame_count(), 4_000);
    let st = p.clone().to_stereo();
    assert_eq!(st.format, STEREO_8K);
    assert_eq!(st.samples.len(), 16_000);
    let back = st.to_mono();
    assert_eq!(back.samples, p.samples, "mono -> stereo -> mono is lossless");
    // Resampled sine keeps its frequency.
    let up = p.resample(48_000);
    let zc = up.samples.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as i64;
    assert!((zc - 200).abs() <= 2, "zc {zc}");
}

#[test]
fn filters_attenuate_the_right_band() {
    let fmt = AudioFormat { sample_rate: 48_000, channels: 1 };
    let low = tone(Waveform::Sine, 100.0, 0.5, fmt);
    let high = tone(Waveform::Sine, 8_000.0, 0.5, fmt);
    let rms = |p: &Pcm| (p.samples[4_800..].iter().map(|s| s * s).sum::<f32>() / (p.samples.len() - 4_800) as f32).sqrt();
    let lp_low = rms(&low.clone().low_pass(500.0));
    let lp_high = rms(&high.clone().low_pass(500.0));
    assert!(lp_low > 0.6 && lp_high < 0.08, "low-pass: low {lp_low} high {lp_high}");
    let hp_low = rms(&low.high_pass(2_000.0));
    let hp_high = rms(&high.high_pass(2_000.0));
    assert!(hp_low < 0.06 && hp_high > 0.6, "high-pass: low {hp_low} high {hp_high}");
}

#[test]
fn bitcrush_quantizes_to_levels() {
    let p = tone(Waveform::Sine, 50.0, 0.1, MONO_8K).bitcrush(2);
    let mut levels: Vec<i32> = p.samples.iter().map(|s| (s * 3.0).round() as i32).collect();
    levels.sort();
    levels.dedup();
    assert_eq!(levels, vec![-3, -1, 1, 3], "4 levels: ±1, ±1/3");
    assert!(tone(Waveform::Sine, 50.0, 0.1, MONO_8K).bitcrush(1).samples.iter().all(|&s| s == 1.0 || s == -1.0));
}

// ----- WAV -----------------------------------------------------------------

#[test]
fn wav_header_bytes() {
    let p = Pcm::new(STEREO_8K, vec![0.0, 1.0, -1.0, 0.5]);
    let w = p.to_wav();
    assert_eq!(w.len(), 44 + 8);
    assert_eq!(&w[0..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 8);
    assert_eq!(&w[8..16], b"WAVEfmt ");
    assert_eq!(u32::from_le_bytes(w[16..20].try_into().unwrap()), 16);
    assert_eq!(u16::from_le_bytes(w[20..22].try_into().unwrap()), 1, "PCM");
    assert_eq!(u16::from_le_bytes(w[22..24].try_into().unwrap()), 2, "channels");
    assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 8_000, "rate");
    assert_eq!(u32::from_le_bytes(w[28..32].try_into().unwrap()), 32_000, "byte rate");
    assert_eq!(u16::from_le_bytes(w[32..34].try_into().unwrap()), 4, "block align");
    assert_eq!(u16::from_le_bytes(w[34..36].try_into().unwrap()), 16, "bits");
    assert_eq!(&w[36..40], b"data");
    assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 8);
    let s: Vec<i16> = w[44..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    assert_eq!(s, vec![0, 32_767, -32_767, 16_384]);
}

#[test]
fn wav_round_trip_within_one_lsb() {
    let p = synth::presets::explosion().to_stereo();
    let back = Pcm::from_wav(&p.to_wav()).unwrap();
    assert_eq!(back.format, p.format);
    assert_eq!(back.samples.len(), p.samples.len());
    for (a, b) in p.samples.iter().zip(&back.samples) {
        assert!((a - b).abs() <= 1.0 / 32_768.0, "{a} vs {b}");
    }
}

/// Build a WAV with an arbitrary fmt chunk + data, plus a junk chunk (odd
/// length, so a pad byte) before `fmt ` to prove unknown chunks are skipped.
fn wav(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8], extensible: bool) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&(if extensible { 0xFFFEu16 } else { tag }).to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    let align = channels as u32 * bits as u32 / 8;
    fmt.extend_from_slice(&(rate * align).to_le_bytes());
    fmt.extend_from_slice(&(align as u16).to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    if extensible {
        fmt.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        fmt.extend_from_slice(&bits.to_le_bytes()); // valid bits
        fmt.extend_from_slice(&0u32.to_le_bytes()); // channel mask
        fmt.extend_from_slice(&tag.to_le_bytes()); // SubFormat GUID…
        fmt.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
    }
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    body.extend_from_slice(b"LIST");
    body.extend_from_slice(&3u32.to_le_bytes());
    body.extend_from_slice(&[1, 2, 3, 0]); // 3 bytes + pad
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    body.extend_from_slice(&fmt);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(data.len() as u32).to_le_bytes());
    body.extend_from_slice(data);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

#[test]
fn from_wav_decodes_float32() {
    let vals = [0.0f32, 0.5, -0.25, 1.0];
    let data: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
    let p = Pcm::from_wav(&wav(3, 1, 22_050, 32, &data, false)).unwrap();
    assert_eq!(p.format, AudioFormat { sample_rate: 22_050, channels: 1 });
    assert_eq!(p.samples, vals);
}

#[test]
fn from_wav_decodes_extensible_float32() {
    let vals = [0.125f32, -0.5];
    let data: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
    let p = Pcm::from_wav(&wav(3, 2, 48_000, 32, &data, true)).unwrap();
    assert_eq!(p.format.channels, 2);
    assert_eq!(p.samples, vals);
}

#[test]
fn from_wav_decodes_unsigned_8bit() {
    let p = Pcm::from_wav(&wav(1, 1, 8_000, 8, &[128, 255, 0, 192], false)).unwrap();
    assert_eq!(p.samples, vec![0.0, 127.0 / 128.0, -1.0, 0.5]);
}

#[test]
fn from_wav_decodes_signed_24bit() {
    // 0x400000 = +0.5, 0xC00000 = -0.5, 0x7FFFFF ≈ +1, 0x800000 = -1.
    let data = [0x00, 0x00, 0x40, 0x00, 0x00, 0xC0, 0xFF, 0xFF, 0x7F, 0x00, 0x00, 0x80];
    let p = Pcm::from_wav(&wav(1, 2, 44_100, 24, &data, false)).unwrap();
    assert_eq!(p.frame_count(), 2);
    assert_eq!(p.samples[0], 0.5);
    assert_eq!(p.samples[1], -0.5);
    assert!((p.samples[2] - 1.0).abs() < 1e-6);
    assert_eq!(p.samples[3], -1.0);
}

#[test]
fn from_wav_decodes_signed_32bit() {
    let data: Vec<u8> = [i32::MIN, 0, 1 << 30].iter().flat_map(|v| v.to_le_bytes()).collect();
    let p = Pcm::from_wav(&wav(1, 1, 8_000, 32, &data, false)).unwrap();
    assert_eq!(p.samples, vec![-1.0, 0.0, 0.5]);
}

#[test]
fn from_wav_rejects_garbage_and_unsupported() {
    assert!(matches!(Pcm::from_wav(b"not a wav"), Err(SynthError::InvalidWav(_))));
    let mut no_data = wav(1, 1, 8_000, 16, &[], false);
    let at = no_data.windows(4).position(|w| w == b"data").unwrap();
    no_data.truncate(at);
    assert!(matches!(Pcm::from_wav(&no_data), Err(SynthError::InvalidWav(_))));
    // µ-law (tag 7) is well-formed but not decoded.
    assert!(matches!(Pcm::from_wav(&wav(7, 1, 8_000, 8, &[0, 1], false)), Err(SynthError::UnsupportedWav(_))));
}

// ----- pipeline seams ------------------------------------------------------

#[test]
fn from_frame_and_append_frame_convert_formats() {
    let samples = [0.5f32, -0.5, 0.25, -0.25];
    let frame = AudioFrame { samples: &samples, sample_rate: 8_000, channels: 2, pts_micros: 0 };
    let p = Pcm::from_frame(&frame);
    assert_eq!(p.format, STEREO_8K);
    assert_eq!(p.samples, samples);

    let mut mono = Pcm::silence(AudioFormat { sample_rate: 16_000, channels: 1 }, 0.0);
    mono.append_frame(&frame);
    assert_eq!(mono.frame_count(), 4, "2 frames at 8 kHz -> 4 at 16 kHz");
    assert_eq!(mono.samples[0], 0.0, "stereo averaged to mono");
}

#[test]
fn write_to_delivers_every_sample_to_a_subscriber() {
    let (stream, writer) = AudioStream::new();
    let got = Arc::new(Mutex::new(Vec::<f32>::new()));
    let formats = Arc::new(Mutex::new(Vec::new()));
    let (g, f) = (got.clone(), formats.clone());
    let _sub = stream.subscribe(move |fr: &AudioFrame| {
        g.lock().unwrap().extend_from_slice(fr.samples);
        f.lock().unwrap().push(fr.format());
    });
    let p = tone(Waveform::Sine, 440.0, 0.3, STEREO_8K);
    p.write_to(&writer);
    assert_eq!(*got.lock().unwrap(), p.samples);
    assert!(formats.lock().unwrap().iter().all(|&f| f == STEREO_8K));
    assert!(formats.lock().unwrap().len() > 1, "written in chunks");
}

#[test]
fn to_source_is_the_wav_bytes() {
    let p = synth::presets::blip();
    assert_eq!(p.to_source(), audio::AudioSource::bytes(p.to_wav()));
}

#[test]
fn load_hands_wav_to_the_platform_player() {
    // Native targets have a real player (or an honest NotSupported); either
    // way `load` must resolve without panicking and never report the WAV
    // undecodable — 16-bit PCM WAV is the one format every player takes.
    let res = pollster::block_on(synth::presets::coin().load());
    // Apple targets have AVAudioPlayer: the synthesized WAV must actually load.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    assert!(res.is_ok(), "AVAudioPlayer refused a synthesized WAV: {:?}", res.err());
    if let Err(e) = res {
        assert!(!matches!(e, audio::AudioError::Decode(_)), "WAV rejected as undecodable: {e}");
    }
}
