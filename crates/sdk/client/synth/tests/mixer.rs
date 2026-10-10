//! `Mixer` pacing, driven deterministically through `pump_to`. No scheduler
//! is installed in this test binary, so the real-time driver never starts
//! and every frame comes from an explicit pump.

use std::sync::{Arc, Mutex};

use media_stream::AudioFrame;
use synth::{AudioFormat, Mixer, Pcm, MAX_PUMP_MICROS, PUMP_LEAD_MICROS};

const FMT: AudioFormat = AudioFormat { sample_rate: 48_000, channels: 2 };
/// Frames per microsecond-span at 48 kHz.
fn frames(micros: u64) -> usize {
    (micros as u128 * 48_000 / 1_000_000) as usize
}

struct Chunk {
    samples: Vec<f32>,
    format: AudioFormat,
    pts: u64,
}

fn capture(m: &Mixer) -> (Arc<Mutex<Vec<Chunk>>>, media_stream::AudioSubscription) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    let sub = m.stream().subscribe(move |f: &AudioFrame| {
        g.lock().unwrap().push(Chunk { samples: f.samples.to_vec(), format: f.format(), pts: f.pts_micros });
    });
    (got, sub)
}

fn total_frames(chunks: &[Chunk]) -> usize {
    chunks.iter().map(|c| c.samples.len() / 2).sum()
}

#[test]
fn pumps_account_frames_exactly() {
    let m = Mixer::new(FMT);
    let (got, _sub) = capture(&m);
    let t0 = 5_000_000;
    m.pump_to(t0);
    assert_eq!(total_frames(&got.lock().unwrap()), frames(PUMP_LEAD_MICROS), "first pump renders the lead");
    // Uneven pump intervals, including a repeat of the same time.
    for dt in [20_000, 33_333, 33_333, 7_001, 20_000, 120_000] {
        m.pump_to(t0 + dt);
    }
    let end = t0 + 120_000;
    let expected = frames(end - t0 + PUMP_LEAD_MICROS);
    let chunks = got.lock().unwrap();
    assert_eq!(total_frames(&chunks), expected, "no drift: total = (elapsed + lead) * rate");
    // Contiguous, sample-accurate timestamps on the caller's clock.
    let mut frame = 0usize;
    for c in chunks.iter() {
        assert_eq!(c.format, FMT);
        assert_eq!(c.pts, t0 + (frame as u64 * 1_000_000 / 48_000));
        frame += c.samples.len() / 2;
    }
}

#[test]
fn idle_mixer_emits_continuous_silence() {
    let m = Mixer::new(FMT);
    let (got, _sub) = capture(&m);
    for i in 0..10 {
        m.pump_to(i * 20_000);
    }
    let chunks = got.lock().unwrap();
    assert_eq!(chunks.len(), 10, "a chunk every pump");
    assert!(chunks.iter().all(|c| c.samples.iter().all(|&s| s == 0.0)));
    assert_eq!(total_frames(&chunks), frames(9 * 20_000 + PUMP_LEAD_MICROS));
}

#[test]
fn voice_starts_at_the_write_head() {
    let m = Mixer::new(FMT);
    let (got, _sub) = capture(&m);
    m.pump_to(0);
    let sound = Pcm::new(AudioFormat { sample_rate: 48_000, channels: 1 }, vec![0.5; 100]);
    let voice = m.play(&sound);
    assert!(voice.is_playing());
    m.pump_to(20_000);
    let chunks = got.lock().unwrap();
    assert!(chunks[0].samples.iter().all(|&s| s == 0.0), "already-rendered audio is untouched");
    let second = &chunks[1].samples;
    assert!(second[..200].iter().all(|&s| s == 0.5), "mono voice up-mixed into both channels from frame 0");
    assert!(second[200..].iter().all(|&s| s == 0.0), "and ends after 100 frames");
    drop(chunks);
    assert!(!voice.is_playing(), "finished voice is reaped");
    assert_eq!(m.active_voices(), 0);
}

#[test]
fn dropping_a_voice_does_not_stop_it_but_stop_does() {
    let m = Mixer::new(FMT);
    let (got, _sub) = capture(&m);
    m.pump_to(0);
    let long = Pcm::new(FMT, vec![0.25; 48_000 * 2]);
    drop(m.play(&long));
    let kept = m.play(&long);
    m.pump_to(20_000);
    assert!(got.lock().unwrap()[1].samples.iter().all(|&s| s == 0.5), "two voices sum");
    kept.stop();
    assert!(!kept.is_playing());
    m.pump_to(40_000);
    assert!(got.lock().unwrap()[2].samples.iter().all(|&s| s == 0.25), "dropped voice keeps playing, stopped one is gone");
    assert_eq!(m.active_voices(), 1);
    m.stop_all();
    m.pump_to(60_000);
    assert!(got.lock().unwrap()[3].samples.iter().all(|&s| s == 0.0));
}

#[test]
fn mix_clamps_and_gains_apply() {
    let m = Mixer::new(FMT);
    let (got, _sub) = capture(&m);
    m.pump_to(0);
    let hot = Pcm::new(FMT, vec![0.8; 48_000]);
    m.play(&hot);
    m.play(&hot);
    m.pump_to(20_000);
    assert!(got.lock().unwrap()[1].samples.iter().all(|&s| s == 1.0), "1.6 clamps to 1.0");
    m.set_gain(0.25);
    m.pump_to(40_000);
    assert!(got.lock().unwrap()[2].samples.iter().all(|&s| (s - 0.4).abs() < 1e-6));
    m.set_gain(1.0);
    m.stop_all();
    let v = m.play(&Pcm::new(FMT, vec![0.2; 48_000]));
    v.set_gain(0.5);
    m.pump_to(60_000);
    assert!(got.lock().unwrap()[3].samples.iter().all(|&s| (s - 0.1).abs() < 1e-6));
}

#[test]
fn stall_is_capped_and_resyncs() {
    let m = Mixer::new(FMT);
    let (got, _sub) = capture(&m);
    m.pump_to(0);
    let voice = m.play(&Pcm::new(FMT, vec![0.5; 2 * frames(400_000)]));
    // A 10 s stall (hidden tab): one pump must not dump 10 s of audio.
    let stall_end = 10_000_000;
    m.pump_to(stall_end);
    {
        let chunks = got.lock().unwrap();
        let c = &chunks[1];
        assert_eq!(c.samples.len() / 2, frames(MAX_PUMP_MICROS), "capped");
        // The chunk ends at now + lead; its pts jumps over the gap.
        assert_eq!(c.pts + MAX_PUMP_MICROS, stall_end + PUMP_LEAD_MICROS);
        assert!(c.samples.iter().all(|&s| s == 0.5), "voice resumes where it left off");
    }
    assert!(voice.is_playing(), "400 ms voice not consumed by the skipped stall");
    // Back to normal pacing after the resync.
    m.pump_to(stall_end + 20_000);
    let chunks = got.lock().unwrap();
    assert_eq!(chunks[2].samples.len() / 2, frames(20_000));
    assert_eq!(chunks[2].pts, chunks[1].pts + MAX_PUMP_MICROS);
}

#[test]
fn clock_going_backwards_renders_nothing() {
    let m = Mixer::new(FMT);
    let (got, _sub) = capture(&m);
    m.pump_to(1_000_000);
    m.pump_to(900_000);
    assert_eq!(got.lock().unwrap().len(), 1);
}

#[test]
fn play_converts_formats() {
    let m = Mixer::new(FMT);
    let (got, _sub) = capture(&m);
    m.pump_to(0);
    // 24 kHz mono, 50 frames -> 100 frames at 48 kHz stereo.
    m.play(&Pcm::new(AudioFormat { sample_rate: 24_000, channels: 1 }, vec![0.5; 50]));
    m.pump_to(20_000);
    let c = &got.lock().unwrap()[1].samples;
    let nonzero = c.iter().filter(|&&s| s != 0.0).count();
    assert_eq!(nonzero, 200);
}

#[test]
fn stream_handle_keeps_the_mixer_alive() {
    let m = Mixer::new(FMT);
    let stream = m.stream();
    let voice = m.play(&Pcm::new(FMT, vec![0.1; 48_000 * 2]));
    drop(m);
    assert!(voice.is_playing(), "a live stream clone keeps the mixer (and its voices) alive");
    drop(stream);
    assert!(!voice.is_playing(), "last handle gone: mixer freed");
}
