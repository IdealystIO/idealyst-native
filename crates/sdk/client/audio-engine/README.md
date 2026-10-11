# `audio-engine`

Low-latency, mixable, spatial-ready sound for games. Many sounds at once,
each with its own gain, stereo pan, low-pass filter and pitch that can change
**while it plays**, looping, playback of live `AudioStream`s, and a master
bus — with the same output on every platform.

```rust
use audio_engine::{AudioEngine, SoundBuffer, VoiceParams};

# async fn demo() -> Result<(), audio_engine::EngineError> {
let engine = AudioEngine::start().await?;
let shot = SoundBuffer::decode(include_bytes!("shot.ogg"))?;

// Fire-and-forget: overlapping shots layer.
engine.play(&shot, VoiceParams::new().gain(0.8).pan(-0.4).rate(1.05));

// A loop steered while it plays.
let wake = engine.play(&shot, VoiceParams::new().looping(true).lowpass(400.0));
wake.set_lowpass(Some(4_000.0), 0.25); // open the filter over 250 ms
wake.set_rate(1.3, 0.25);              // and raise the pitch
wake.stop(0.5);                        // fade out, then free
# Ok(())
# }
```

## Why a separate crate from `audio`

[`audio`](../audio) drives one platform player per loaded file
(HTMLAudioElement, AVAudioPlayer, MediaPlayer, GStreamer) with a volume knob.
That's the right tool for music and UI sounds, but a game needs more:

- **Layering.** Rapid sounds (gunfire) must overlap. Android's `audio`
  reuses one `MediaPlayer` per sound, so a second `play()` restarts it.
- **Per-sound processing.** Pan, low-pass and pitch per voice, ramped live,
  instead of pre-rendering every variant.
- **Latency.** Platform players add tens to hundreds of ms of start-up; here a
  voice starts at the next 128-frame render quantum of an already-running
  output.
- **Live streams.** Nothing in `audio` plays a `media_stream::AudioStream`
  (a `synth::Mixer` bus, a microphone); `play_stream` does.

`audio` stays the simple file player; nothing in it changed.

## API

**`AudioEngine`** — a `Clone` handle (UI thread, not `Send`); the output
closes when the last clone drops.

- `AudioEngine::start().await -> Result<AudioEngine, EngineError>` — open the
  default output.
- `play(&SoundBuffer, VoiceParams) -> Voice`,
  `play_stream(&AudioStream, VoiceParams) -> Voice`.
- `set_master_gain(gain, ramp_secs)`, `sample_rate()`, `output_latency()`,
  `active_voices()`.
- `resume()` — web: `AudioContext.resume()`, call from a gesture (or after
  `runtime_core::user_activated()` turns true); native: restarts a suspended
  stream. `suspend()` — pause output, e.g. when `app_visible()` goes false.
- `AudioEngine::offline(sample_rate, channels) -> (AudioEngine,
  OfflineRenderer)` — the same mixer with no device; `render(&mut [f32])`,
  `render_frames(n)`, `render_secs(s)`. Deterministic: tests, offline
  bounces, and the web spec-convergence check.

**`SoundBuffer`** — decoded PCM behind an `Arc`; clone freely, play on any
number of voices.

- `SoundBuffer::decode(&[u8])` — WAV (int / float PCM), MP3, Ogg Vorbis,
  FLAC, decoded in pure Rust (symphonia) on every target, so every platform
  plays the same samples. MP3 encoder delay/padding is trimmed (gapless).
- `SoundBuffer::from_samples(AudioFormat, Vec<f32>)`,
  `SoundBuffer::from_pcm(&synth::Pcm)`.
- `duration_secs()`, `format()`.

**`VoiceParams`** — `gain` (linear), `pan` (−1…1), `rate` (playback rate =
pitch, 0…`MAX_RATE`), `lowpass_hz: Option<f32>`, `looping`,
`start_offset_secs`; builder methods `VoiceParams::new().gain(..).pan(..)
.rate(..).lowpass(..).looping(..).start_offset(..)`, and
`VoiceParams::spatial(&listener, pos, &attenuation)`.

**`Voice`** — `Clone`; **dropping it does not stop the sound** (one-shots are
fire-and-forget, like `synth::Voice`), so stop loops explicitly.

- `stop(fade_secs)` — ramp gain to 0, then free. Later `set_gain`s are
  ignored.
- `set_gain`, `set_pan`, `set_rate`, `set_lowpass(Option<f32>, ramp_secs)`
  — linear ramps from the current value; `0` = immediate.
  `set_lowpass(None, r)` ramps the cutoff to Nyquist (where the filter is
  transparent) before removing it, and `Some` on an unfiltered voice enters
  from Nyquist — no clicks.
- `set_looping(bool)` — off lets the current pass finish.
- `set_spatial(&listener, pos, &attenuation, ramp_secs)`, `is_playing()`.

**`spatial`** — pure math: `params_for(&Listener, source, &Attenuation) ->
(gain, pan, Option<lowpass_hz>)`. Pan is the source direction projected on
the listener's right vector (stereo can't tell front from back); gain is
inverse distance clamped to `[ref_distance, max_distance]`; the low-pass
slides geometrically from 20 kHz at `ref_distance` to `muffle_hz_at_max` at
`max_distance`, so far sounds read as far, not just quiet.

**`EngineError`** — `NoOutputDevice`, `Unsupported`, `Decode(String)`,
`Backend(String)`.

## Semantics (identical on every platform)

The web backend uses the browser's own Web Audio nodes; every other target
uses the crate's Rust mixer. Both follow the **Web Audio spec** formulas, so
they converge:

| Feature | Web Audio node | Rule |
| --- | --- | --- |
| Signal chain | `AudioBufferSourceNode → GainNode → BiquadFilterNode → StereoPannerNode → GainNode(master) → destination` | per voice, in that order |
| Low-pass | `BiquadFilterNode` `"lowpass"` | RBJ coefficients as the spec writes them, Q = 1 dB (the default); cutoff clamped to `[0, Nyquist]`; Nyquist = pass-through |
| Pan | `StereoPannerNode` | equal-power, spec's separate mono / stereo-input formulas |
| Rate | `AudioBufferSourceNode.playbackRate` | linear interpolation of the buffer; k-rate (one value per 128-frame quantum); also converts the buffer's rate to the output rate |
| Ramps | `linearRampToValueAtTime` | linear over `ramp_secs` from the current value; exact target on time |
| Looping | `loop = true` | whole buffer |
| Timing | render quanta | commands take effect at the next 128-frame boundary |
| Wide sources | channel down-mix | >2-channel buffers / streams down-mixed with the spec's "speakers" rules (quad, 5.1; else discrete) |
| Output | destination | master gain, then speakers mapping (mono = (L+R)/2; >2 ch = L, R, silence), then clamp to `[-1, 1]` |

**Voice cap: 128** (`MAX_VOICES`). When full, `play` steals the **oldest
non-looping buffer voice** (cut immediately). Looping and stream voices are
never stolen; if every voice is one, the new voice doesn't start
(`is_playing()` is false).

**Stream voices** (`play_stream`) convert the stream's PCM to the output rate
(continuous-phase linear resampling) and to mono/stereo as it arrives, keep at
most `STREAM_BACKLOG_SECS` (100 ms) buffered — dropping the oldest beyond
that, like the `media-stream` web bridge — and play silence while starved.
Gain / pan / low-pass apply; `rate`, `looping` and `start_offset_secs` are
ignored (a live stream can't be sped up or rewound). A stream voice plays
until `stop`.

## Per-platform mechanism

| Target | Output | Mixer | Status |
| --- | --- | --- | --- |
| macOS | cpal → CoreAudio | Rust | runnable; device smoke test passes |
| iOS | cpal → CoreAudio (RemoteIO) + `AVAudioSession` | Rust | compile-checked |
| Linux | cpal → ALSA | Rust | builds + tests pass in Docker (no sound card there: device smoke test SKIPs) |
| Windows | cpal → WASAPI | Rust | compile-checked |
| Android 8.1+ (API 27) | AAudio, loaded with `dlopen` | Rust | compile-checked (`aarch64-linux-android`); links at API 21 with no `libaaudio` dependency; **not run on a device** |
| Android < 8.1 | — | — | `EngineError::Unsupported` (no AAudio before 8.0; 8.0's is skipped as Oboe does); the library still loads |
| web (wasm32) | Web Audio node graph (`AudioContext`, `latencyHint: "interactive"`) | browser | runs in Chrome; browser tests pass, including sample-by-sample convergence with the Rust mixer (max difference 1.2e-6) |

### Latency

- **Desktop / Linux / Windows**: the engine asks cpal for a fixed device
  buffer of `TARGET_BUFFER_MS` (5 ms → 256 frames at 44.1/48 kHz, clamped to
  the device's range) and falls back to the device default if refused.
  Shared-mode WASAPI typically floors at ~10 ms.
- **iOS**: cpal's iOS host doesn't take a fixed size, so the engine asks
  `AVAudioSession` for a 5 ms preferred IO buffer instead.
- **Android**: AAudio low-latency mode, exclusive (MMAP) sharing where the
  device allows it, buffer set to two bursts (the burst is the device's own
  transfer size; two is AAudio's and Oboe's recommended starting point).
  `output_latency()` comes from
  `AAudioStream_getTimestamp` (when the newest written frame will be
  presented, minus now); before the stream has a timestamp it's the buffer's
  length.
- cpal targets: `output_latency()` is measured from the device callback's
  own timestamps (playback instant − callback instant, plus the buffer), so it reflects the
  real hardware path once audio is running; before the first callback it's
  the fixed buffer's length, or `None`.

### Web

The browser is the mixer: each voice is a chain of native Web Audio nodes,
`AudioBufferSourceNode` (or `MediaStreamAudioSourceNode`) → `GainNode` →
`BiquadFilterNode` (`"lowpass"`, Q 1) → `StereoPannerNode` → master
`GainNode` → destination. The behavior matches the Rust mixer:

- **No filter** (`None`) leaves the biquad in place with its cutoff at
  Nyquist, where the lowpass passes everything through unchanged. Turning
  the filter on or off is then a cutoff ramp, so it never clicks.
- **Ramps** are `cancelScheduledValues` + `setValueAtTime(current, now)` +
  `linearRampToValueAtTime`. `current` comes from the ramp the engine
  scheduled last, not from `AudioParam.value`, because `value` lags a
  running ramp by up to one render quantum.
- **`stop(fade)`** ramps the gain to 0 and calls `stop(now + fade)` on the
  source. Its `ended` event frees the voice and sets `is_playing()` to false.
  Stream voices have no `stop`, so a silent `ConstantSourceNode` schedules
  their end on the audio clock instead.
- **Buffers** are uploaded into an `AudioBuffer` once per `SoundBuffer` per
  engine and cached, so replaying a sound only creates nodes. The cached
  `AudioBuffer` is released after the `SoundBuffer` drops.
- **Voice cap**: the same 128-voice rule, applied when `play` is called. The
  Rust mixer applies it one render quantum later.
- **Starting suspended**: a context created before any user gesture starts
  `suspended`. The engine keeps that context. `resume()` wakes it, and so does
  the first `play` after `runtime_core::user_activated()` is true, unless the
  app called `suspend()` itself.
- **Latency**: `output_latency()` = `baseLatency + outputLatency`, with a
  missing `outputLatency` counted as 0 (older Safari).

Differences on the web:

- **Stream voices** play the stream's `native_source()` `MediaStream`. For a
  microphone this is the browser's own track: no copy, lowest latency. A
  synthetic stream (`synth::Mixer`) is routed through the `media-stream`
  WebAudio bridge, and the bridge's buffering (a ~43 ms `ScriptProcessorNode`
  buffer and a 1 s backlog cap) sets the latency instead of
  `STREAM_BACKLOG_SECS`.
- A voice that finished playing leaves `active_voices()` when its `ended`
  event arrives. That event comes slightly after the audio thread finishes
  the voice, not at the render quantum.
- An empty `SoundBuffer` (0 frames) never starts, because a Web Audio
  `AudioBuffer` cannot be empty.

### iOS audio session

On start the engine puts the app's `AVAudioSession` into
`IOS_SESSION_CATEGORY` = **Ambient** — game sound mixes with the player's
music and respects the silent switch — but only if the session is still in
the system default (SoloAmbient). An app that picked its own category (e.g.
`microphone` sets PlayAndRecord for capture) keeps it; switching a recording
session to Ambient would kill the capture. `AVFoundation` is declared in
`[package.metadata.idealyst.ios]`.

### Android (AAudio)

- **Why `dlopen`.** AAudio is API 26+. The idealyst CLI links Android
  libraries at API 21, whose NDK sysroot has no `libaaudio.so`, and a library
  that *links* AAudio fails `System.loadLibrary` on an API 21–25 device —
  the app would crash at launch, not just lose sound. So the backend reads
  `ro.build.version.sdk`, and on API 27+ `dlopen`s `libaaudio.so` and
  resolves each entry point with `dlsym`. Below 27 (8.0's AAudio has known
  defects; Oboe also starts at 27), or if the library can't be opened,
  `AudioEngine::start` returns `EngineError::Unsupported`. That's
  also why the `ndk` crate's `audio` feature isn't used (it links aaudio);
  `ndk-sys` provides only the header types and constants.
- **Stream.** Output, `AAUDIO_PERFORMANCE_MODE_LOW_LATENCY`, the device's
  native sample rate, stereo. Open order: exclusive + float → shared + float
  → shared + i16 (an i16 stream gets the float mix converted in the
  callback, through a scratch reserved before the stream starts).
- **Usage attributes.** On API 28+ the stream is tagged usage **GAME**,
  content type **SONIFICATION**, so the system routes and ducks it as game
  sound and the media/game volume applies. On 26/27 those setters don't
  exist and AAudio's defaults (MEDIA / MUSIC) apply. The engine does not
  request audio focus (game effects normally shouldn't pause the player's
  music); an app that wants focus requests it through Android's
  `AudioManager` itself.
- **Disconnects.** Unplugging headphones (or losing a Bluetooth route)
  kills the stream and fires AAudio's error callback. AAudio forbids
  reopening from inside that callback, so it hands off to a short-lived
  thread that closes the dead stream and opens a new one on the new default
  route, asking for the engine's original sample rate so
  `sample_rate()` stays true and playing voices continue. It retries with a
  back-off (0, 100, 200, 400, 800 ms) and keeps a `suspend()`ed engine
  paused.
- `suspend()` / `resume()` = `AAudioStream_requestPause` /
  `requestStart`; dropping the last `AudioEngine` closes the stream.
- The data callback takes no lock and allocates nothing; a panic inside it
  is caught, logged to logcat and aborts (it must never unwind into
  AAudio).

## How the native mixer runs

- The handle side (`AudioEngine` / `Voice`) sends commands through a bounded
  lock-free queue (`crossbeam_queue::ArrayQueue`, allocated once); the audio
  callback drains it at each render quantum. No locks on the render path; no
  allocation in steady state (voice table and scratch are preallocated;
  commands move by value). If the queue is full — only while the output is
  suspended or stalled — the handle side holds the overflow in order and
  flushes it on its next call.
- **No frees on the audio thread.** Voices carry `Arc`s (PCM, stream ring,
  status flag) into the callback. The handle side keeps its own clone of each
  plus a per-voice "claim" token the callback drops last; it releases its
  clones only after seeing the claim's count fall to 1. So the callback only
  ever decrements reference counts, and a multi-megabyte buffer is always
  freed on the main thread.
- Stream PCM is resampled on the stream producer's thread into a per-voice
  frame ring (`ArrayQueue::force_push` drops the oldest past the backlog cap).

## Permissions

None for playback.

## Testing

- `cargo test -p audio-engine` — biquad coefficients against the spec
  formula; low-passed white noise loses its high band by the amount the
  spec's response predicts; equal-power pan at −1/0/1 (mono and stereo
  input); rate 2 halves duration and doubles zero crossings; buffer-rate
  conversion; linear ramps hitting targets on time; looping and
  `set_looping(false)`; stop fades; voice stealing and rejection; stream voices
  (pushed PCM, starvation silence, backlog cap, resampling); master gain +
  clamp; output channel mapping; click-free filter toggling; determinism;
  deferred frees (the render side never drops the last `Arc`); decoding
  WAV (via `synth`) / FLAC / Ogg Vorbis / MP3 fixtures; spatial math; and a
  device smoke test that opens the real default output (prints `SKIP` with
  no device).
- The Android backend's rules (API gating, open order, buffer sizing,
  timestamp latency math, reopen back-off, i16 conversion) are unit tests in
  `src/backend/aaudio_policy.rs`, run on the host.
- `cargo check -p audio-engine --target aarch64-linux-android` (with
  `CC_aarch64_linux_android` pointing at the NDK clang).
- Web, in a real browser: `cargo test -p audio-engine --target
  wasm32-unknown-unknown --lib` with `CHROMEDRIVER=<driver matching Chrome>`
  and `WASM_BINDGEN_TEST_ONLY_WEB=1` (the workspace runner supplies the
  web-glue JS; don't use `wasm-pack`). The main test renders one scene twice,
  once through the Web Audio graph on an `OfflineAudioContext` and once
  through `AudioEngine::offline`, and compares every sample within 1e-5. The
  scene covers low-pass, pan, rate 1.5, buffer-rate conversion, gain, pan and
  master ramps, a ramp that interrupts another one, and filter on/off. The
  measured difference is 1.2e-6. Other tests cover: play / stop / loop /
  `ended` state; ramps that are linear, reach their target on time and start
  from the current value; voice stealing; reuse of cached buffers; start
  offsets; and a real `AudioContext` playing a stream voice.
- `cargo check -p audio-engine --target aarch64-apple-ios`,
  `--target x86_64-pc-windows-gnu`, `--target wasm32-unknown-unknown`.
- `cargo build -p audio-engine --features catalog` — recipes compile.
