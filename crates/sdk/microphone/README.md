# `microphone`

Cross-platform microphone capture — the smallest useful abstraction over
the platform's audio input. Open a stream, receive raw PCM frames in a
callback, drop the stream to stop. No files, no encoding, no opinion about
where the audio goes. That's deliberately left to higher-level SDKs built
on top of this one; this crate's only job is to **establish the stream**.

```rust
use microphone::{Microphone, AudioStreamConfig};

# async fn demo() -> Result<(), microphone::MicError> {
let mic = Microphone::new();

let stream = mic
    .open(AudioStreamConfig::default().mono(), |buf| {
        // Runs on the audio thread (native/Android) or the main thread
        // (web). `buf.samples` is interleaved, normalized f32 in [-1, 1].
        // Copy out what you need and return quickly.
        let peak = buf.samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        let _ = peak;
    })
    .await?;

// Capture runs for as long as `stream` is alive.
stream.stop(); // or just drop it
# Ok(())
# }
```

## What you get

Every backend delivers the **same shape** to your callback — an
[`AudioBuffer`] of **interleaved, normalized `f32` samples in `[-1.0, 1.0]`**
plus the actual `sample_rate` and `channels`. The platforms diverge in
mechanism, not in what you receive, so consumer code is identical
everywhere:

| Target | Mechanism |
| --- | --- |
| macOS / Windows / Linux | [`cpal`] → CoreAudio / WASAPI / ALSA |
| iOS | [`cpal`] → AudioUnit, with an `AVAudioSession` activated for recording |
| Android | `android.media.AudioRecord`, read on a JNI worker thread |
| Web (wasm32) | `getUserMedia` + a Web Audio `ScriptProcessorNode` graph |

The callback is `FnMut`, so it can own and mutate state across chunks. It
must be `Send` on native/Android (it runs on the audio/reader thread) and
need not be on web (it runs on the main thread) — the [`AudioCallback`]
bound encodes that per target, so the same closure compiles everywhere.

## Delivery model

A **raw push callback**, on purpose. The callback fires from the capture
thread with each chunk; you decide what to do with the samples — forward
them into a `Signal`, an async channel, a ring buffer, an encoder. Keeping
this layer unopinionated lets a future SDK add the state-binding /
file-writing / streaming abstractions without this crate having baked in
the wrong one.

## Permissions

This SDK declares the capability it needs in its own `Cargo.toml`:

```toml
[package.metadata.idealyst]
capabilities = ["microphone"]
```

The CLI walks your app's dependency graph at build time, finds that
declaration, and **injects the right platform artifacts automatically** —
you don't hand-edit `Info.plist` or `AndroidManifest.xml`:

- **iOS / macOS** — `NSMicrophoneUsageDescription` (+ the
  `com.apple.security.device.audio-input` entitlement on macOS, for signed
  builds).
- **Android** — `<uses-permission android:name="android.permission.RECORD_AUDIO"/>`.
- **Web** — nothing to declare; the browser prompts on first
  `getUserMedia`. Capture requires a **secure context** (HTTPS or
  `localhost`).

What you *should* add is the **user-facing reason** the OS shows in its
prompt — only you can word that for your app:

```toml
[package.metadata.idealyst.app.permissions]
microphone = "Record voice notes"
```

If you omit it, the build still succeeds but uses a generic reason and
prints a warning — generic iOS usage strings risk App Store rejection, so
treat the default as a stopgap. The CLI also prints each permission it
bundled and which crate requested it, so nothing is added invisibly.

[`Microphone::request_permission`] proactively triggers the prompt where
one exists (and is a no-op success on Windows/Linux). It's optional —
[`Microphone::open`] requests access on its own.

### Android runtime-permission caveat

`request_permission()` checks the current grant and, if missing, fires the
system dialog — but its result is delivered to the Activity's
`onRequestPermissionsResult`, which this SDK does not hook. So the call
returns the *current* (not-yet-granted) state after showing the dialog;
re-check (or retry `open`) once the user has responded. Most apps simply
ensure `RECORD_AUDIO` is granted at startup. `open()` fails fast with
`MicError::PermissionDenied` if it isn't.

## Configuration

[`AudioStreamConfig`] is all-optional; `None` fields defer to the device's
preferred value (the cheapest path — no resampling). Requests that the
device can't honour surface as `MicError::UnsupportedConfig` rather than a
silent substitution, so the `sample_rate` / `channels` you read off each
buffer are authoritative.

```rust
use microphone::AudioStreamConfig;

let _ = AudioStreamConfig::default();                    // device defaults
let _ = AudioStreamConfig::new().mono();                 // force 1 channel
let _ = AudioStreamConfig::new().with_sample_rate(16_000).mono();
```

## Tests

- `tests/portable.rs` — framing math + config builders; runs anywhere.
- `tests/host_capture.rs` — opens the host's default device and asserts
  the callback fires. `#[ignore]`d (needs real hardware + permission); run
  it with:

  ```text
  cargo test -p microphone --test host_capture -- --ignored --nocapture
  ```

[`cpal`]: https://crates.io/crates/cpal
[`AudioBuffer`]: src/buffer.rs
[`AudioStreamConfig`]: src/config.rs
[`AudioCallback`]: src/lib.rs
[`Microphone::request_permission`]: src/lib.rs
[`Microphone::open`]: src/lib.rs
