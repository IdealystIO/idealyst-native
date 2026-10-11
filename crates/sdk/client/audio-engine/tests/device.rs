//! Smoke test: the real default output opens (and plays a short, quiet
//! blip) on this machine. Prints SKIP and passes when there's no output
//! device (CI containers). Native desktop/iOS only: web and Android have
//! their own backend tests, so the whole file is gated (otherwise its
//! imports are unused there).
#![cfg(not(any(target_arch = "wasm32", target_os = "android")))]

use std::time::Duration;

use audio_engine::{AudioEngine, EngineError, SoundBuffer, VoiceParams};
use media_stream::AudioFormat;

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
#[test]
fn default_output_opens_and_plays() {
    let engine = match pollster::block_on(AudioEngine::start()) {
        Ok(e) => e,
        Err(EngineError::NoOutputDevice) => {
            println!("SKIP: no audio output device");
            return;
        }
        Err(e) => panic!("AudioEngine::start failed: {e}"),
    };
    assert!(engine.sample_rate() > 0);

    // 50 ms of a very quiet 1 kHz tone.
    let rate = 48_000;
    let samples: Vec<f32> = (0..rate / 20)
        .map(|i| (2.0 * std::f32::consts::PI * 1_000.0 * i as f32 / rate as f32).sin() * 0.01)
        .collect();
    let blip = SoundBuffer::from_samples(
        AudioFormat {
            sample_rate: rate,
            channels: 1,
        },
        samples,
    );
    let voice = engine.play(&blip, VoiceParams::new());
    assert!(voice.is_playing());

    // The device callback runs on its own thread: the voice must play out.
    let mut waited = Duration::ZERO;
    while voice.is_playing() && waited < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
        waited += Duration::from_millis(10);
    }
    assert!(!voice.is_playing(), "the output never rendered the voice");
    assert_eq!(engine.active_voices(), 0);
    println!(
        "output: {} Hz, latency {:?}",
        engine.sample_rate(),
        engine.output_latency()
    );
    assert!(
        engine.output_latency().is_some(),
        "latency measured after callbacks ran"
    );

    engine.suspend();
    engine.resume();
}
