# `synth`

Procedural audio **synthesis** for games and UI sounds — tones, pitch sweeps,
noise bursts, ADSR envelopes, filters, bit-crushing and a handful of ready
presets — rendered in pure Rust and handed to the framework's audio/media
pipeline. It is the "make a sound out of nothing" peer of
[`microphone`](../microphone) (captures sound) and [`audio`](../audio) (plays
prepared sound), and the one crate that depends on both `audio` and
[`media-stream`](../media-stream) (`audio` deliberately does not depend on
`media-stream`).

```rust
use synth::{sweep, Envelope, Sweep, Waveform, DEFAULT_FORMAT};

# async fn demo() -> Result<(), audio::AudioError> {
// A classic "pew": a square wave diving from 1.2 kHz to 180 Hz.
let laser = sweep(Waveform::Square, 1200.0, 180.0, 0.25, Sweep::Exponential, DEFAULT_FORMAT)
    .apply_envelope(Envelope::new(0.003, 0.05, 0.5, 0.12))
    .gain(0.45);

// Encode to WAV and load it as an `audio::Sound`; replay as often as you like.
let sound = laser.load().await?;
let _playback = sound.play();
# Ok(())
# }
```

## How it works

Sounds are synthesized **ahead of time** into an owned `Pcm` buffer, not by a
real-time per-sample callback engine. A game's effects are short and known in
advance, so rendering each once is cheaper and simpler than running a DSP
graph on an audio thread. The generators are pure functions: the same inputs
give the same samples on every platform (noise takes an explicit seed), so a
sound can be unit-tested sample by sample.

A buffer then leaves through one of two exits:

- **Playback**: `Pcm::to_wav()` encodes 16-bit PCM WAV, the one format every
  platform player behind `audio` decodes (HTMLAudioElement, AVAudioPlayer,
  MediaPlayer, GStreamer). `Pcm::to_source()` wraps it in an
  `audio::AudioSource`, and `Pcm::load().await` returns an `audio::Sound`.
- **Streams**: the `Mixer` is a live `media_stream::AudioStream` that keeps
  pace with real time. `media-writer` records it like a microphone stream, and
  on web the stream bridges into a real `MediaStreamTrack`.
  `Pcm::write_to(&AudioWriter)` pushes a whole buffer at once, for consumers
  that are already listening and process offline.

Captured audio comes back the other way: `Pcm::from_frame` and
`Pcm::append_frame` turn a microphone's `AudioFrame`s into a buffer that can
be mixed, trimmed, encoded and played.

## What you get

**`Pcm`**: `format: AudioFormat` plus interleaved `samples: Vec<f32>` in
`[-1, 1]`, the same layout as `media_stream::AudioFrame`. Each transform takes
`self` and returns the result, so effects chain.

- Construct: `Pcm::new(format, samples)`, `Pcm::silence(format, secs)`,
  `Pcm::from_frame(&AudioFrame)`, `append_frame(&mut self, &AudioFrame)`,
  `Pcm::from_wav(&[u8])`.
- Inspect: `frame_count()`, `duration_secs()`, `peak()`.
- Level: `gain(f32)`, `normalize(peak)`, `apply_envelope(Envelope)`,
  `fade_in(secs)`, `fade_out(secs)`.
- Combine: `mix(&Pcm)`, `mix_at(&Pcm, offset_secs)` (summed, then clamped),
  `concat(&Pcm)`, `reverse()`. If the other buffer's format differs, it is
  converted first.
- Format: `convert(AudioFormat)`, `resample(rate)` (linear interpolation),
  `to_mono()`, `to_stereo()`.
- Colour: `low_pass(hz)` and `high_pass(hz)` (one-pole, 6 dB/octave),
  `bitcrush(bits)`.
- Exits: `to_wav()`, `to_source()`, `load().await`, `write_to(&AudioWriter)`.

**Generators**, all rendering at full scale (`±1`) into every channel of the
format you pass:

- `tone(Waveform, freq_hz, secs, format)`. `Waveform` is `Sine`, `Square`,
  `Sawtooth`, `Triangle` or `Pulse(duty)`.
- `sweep(Waveform, from_hz, to_hz, secs, Sweep, format)`. The phase carries
  over from sample to sample, so even a steep sweep has no clicks.
  `Sweep::Exponential` moves by equal musical intervals (the natural sound for
  lasers and risers); `Sweep::Linear` moves by equal hertz.
- `noise(Noise, secs, format, seed)`. `Noise` is `White`, `Pink` (−3 dB per
  octave) or `Brown` (−6 dB per octave, a rumble). It uses a SplitMix64
  generator, so the same seed gives the same noise everywhere.
- `silence(secs, format)`.

**`Envelope { attack, decay, sustain, release }`** is an ADSR laid over the
buffer's own length. The attack ramps 0 → 1 and the decay ramps 1 → sustain.
The sustain level is held until the last `release` seconds, which ramp down
to 0 and end exactly at the end of the buffer. If the three times add up to
more than the buffer, they are scaled down to fit. `Envelope::percussive(attack,
length)` is the hit/blip shape: attack, then a fall to silence.

**`presets`**: `blip`, `laser`, `explosion`, `coin`, `hit`, `jump`,
`power_down`. Each is a few lines over the primitives at `DEFAULT_FORMAT`
(44.1 kHz mono). Read the source to see how they are built, or copy one as a
starting point.

**`Mixer`** is a live audio bus:

```rust
let bus = synth::Mixer::new(synth::AudioFormat { sample_rate: 48_000, channels: 2 });
let stream = bus.stream();             // an AudioStream: hand it to media-writer, etc.
bus.play(&synth::presets::coin());     // fire-and-forget
let voice = bus.play(&synth::presets::power_down());
voice.stop();                          // or cut one short
```

- It emits audio **continuously**, with silence between sounds, so a
  recording's timeline has no holes. Timestamps count samples and sit on the
  shared `media_stream::clock`, so a muxer keeps them in sync with
  `camera` / `screen-recorder` video.
- `play(&Pcm) -> Voice` starts the buffer at the write head, at most
  `PUMP_LEAD_MICROS` (60 ms) ahead of real time. The buffer is converted to
  the mixer's format once, when you call `play`.
- **Dropping a `Voice` does not stop the sound.** A sound effect plays to its
  end without you keeping anything alive; `Voice::stop()` cuts it short. This
  is the opposite of `audio::Playback`, which stops when dropped. A mixer
  voice is finite by construction, and holding a handle for every gunshot
  would only clutter game code. `Voice` also has `is_playing()` and
  `set_gain()`.
- `set_gain` (master), `stop_all`, `active_voices`, `format`.
- **Pacing.** Every `PUMP_TICK_MS` (20 ms), a timer chain on the host
  scheduler calls `pump()`. Each pump renders exactly the frames owed up to
  `now + 60 ms`. The chain uses `setTimeout` rather than
  `requestAnimationFrame`, because rAF stops in hidden tabs and a recording
  has to keep running. If no scheduler is installed (a plain native unit
  test, for example), nothing pumps on its own; call `pump_to(now_micros)` /
  `pump()` yourself. `pump_to` is deterministic, which makes it the seam for
  testing.
- **Stalls are capped.** After a long pause (a tab hidden for 10 s, a
  debugger break), a pump renders at most `MAX_PUMP_MICROS` (250 ms) and skips
  the rest. The timestamps jump forward over the gap, and voices resume where
  they left off. Dumping seconds of audio at once would be worse: the web
  `MediaStreamTrack` bridge keeps at most about 1 s of audio and drops the
  oldest beyond that.
- **Lifetime.** `Mixer` is a cheap `Clone` handle. The mixer and its timer
  stay alive while any `Mixer` clone **or any clone of its stream** is alive,
  so a recorder that holds only the stream keeps getting audio. After the
  last one drops, the timer chain ends on its next tick.

`Pcm::write_to` is the wrong tool for live audio. A stream only delivers to
subscribers that exist when the samples are written, and on web anything
beyond about 1 s written at once loses its beginning. Use the `Mixer` for
live audio.

**`SynthError`**: `InvalidWav` (a malformed RIFF/WAVE file) or
`UnsupportedWav` (an encoding this crate doesn't decode, such as ADPCM or
µ-law). `from_wav` decodes 8/16/24/32-bit integer PCM and 32/64-bit float,
both plain and `WAVE_FORMAT_EXTENSIBLE`, and skips chunks it doesn't know.

## Per-platform mechanism

None: synthesis, WAV encoding and the mixer are pure Rust and behave
identically on every target. Platform behavior comes from the crates it
feeds. `audio` does playback; on Windows and other targets without a player,
`load` returns `NotSupported`. `media-stream` and `media-writer` do stream
transport and recording.

## Permissions

None. Synthesis is pure computation. Any permission for playback or
recording belongs to `audio` / `media-writer`.

## Scope

This crate renders sounds ahead of time and mixes them live into a stream.
Some things are deliberately left out:

- **A real-time per-sample engine.** There are no oscillators that change
  while a sound plays, and no DSP graph on an audio thread. Render a new
  `Pcm` instead.
- **Spatial audio and reverb/delay effects.** These belong in a layer on top.
- **Band-limited oscillators and resampling.** The square and saw waves are
  naive, which suits chiptune SFX. A large down-sample of bright material can
  alias, so render at the rate you need.

## Testing checklist

**Automated**
- [x] `cargo test -p synth`: waveform shape (sine peak and zero crossings,
  square/pulse duty, saw/triangle start at 0), sweep phase continuity and
  frequency trajectory, noise determinism and white > pink > brown
  brightness, envelope key points and proportional fit, filters, bitcrush
  levels, resample/remix lengths, mix clamping, WAV header bytes, round trip
  within one LSB, decode of float32 / extensible / 8-bit / 24-bit / 32-bit
  fixtures, `write_to` delivery, the `audio::AudioSource` seam, and `Mixer`
  pacing (exact frame accounting, continuous silence, voice offset,
  stop vs drop, gains, stall cap, the timer chain's lifetime)
- [x] `cargo check -p synth --target wasm32-unknown-unknown`: web target
- [x] `cargo build -p synth --features catalog`: recipes compile

**Behavior**
- [ ] **Web**: `presets::laser().load()` then `play()` is audible; a `Mixer`
  stream recorded with `media-writer` contains the effects at the right times.
- [ ] **iOS / macOS / Android / Linux**: `Pcm::load` plays through `audio`'s
  native player.
