# `denoise`

Neural noise suppression as an [`AudioStream`](../media-stream) transformer.
Hand it a noisy `AudioStream` and get a clean one back, with every chunk run
through **DeepFilterNet 3** (real-time speech enhancement). It drops into the
media stack anywhere a stream flows: `microphone -> denoise -> media-writer`,
camera audio, a decoded file — anything that produces an `AudioStream`.

The denoising is pure-Rust computation on `f32` buffers (inference via `tract`),
so **one** implementation covers macOS / iOS / Android / desktop / web — there
is no per-OS native code. Only the *execution context* differs.

This is noise *suppression* (single-stream speech enhancement), **not** acoustic
echo cancellation — that needs a far-end reference signal and is a different SDK.

## What you get

- **`Denoiser`** — a cheap, `Copy` handle you configure once and reuse across
  many streams:
  - `Denoiser::new()` — embedded DeepFilterNet 3 model, zero setup (**native only**).
  - `Denoiser::with_weights(&'static [u8])` — caller-supplied model `.tar.gz`
    (e.g. `DeepFilterNet3_onnx.tar.gz`); the only constructor on **web**, where
    the model is deliberately not baked into the wasm binary.
  - `.attenuation_limit_db(f32)` — cap how much the background is attenuated
    (e.g. `12.0`) for a more natural sound; default is unlimited suppression.
  - `.post_filter(bool)` — enable DeepFilterNet's speech post-filter (off by default).
  - `.process(&AudioStream).await -> Result<AudioStream, DenoiseError>` — start
    denoising and return the enhanced stream.
- **`AudioStream`** — re-exported from `media-stream` for convenience.
- **`DenoiseError`** — `ModelInit` (model failed to load/build) or `Spawn`
  (worker thread couldn't start, native only). Both are setup-time faults; once
  `process` returns `Ok`, streaming itself doesn't error (a transient per-frame
  model fault emits silence for that block).

**Output is always 48 kHz mono** — DeepFilterNet's native format. Input of any
rate / channel count is accepted: it's downmixed to mono and resampled to 48 kHz
first. The output stream carries its own 48 kHz monotonic clock, not the input's
timeline (note this if muxing against the *original* audio/video for lip-sync).
Algorithmic latency is roughly **~30 ms** (one hop of buffering + the model's
lookahead) — fine for recording and one-way streaming.

Lifecycle: `process` runs as long as the returned stream (or a clone) is held
and the input keeps producing; dropping the last clone tears the pipeline down.

## Usage

```rust
use denoise::{Denoiser, AudioStream};

# fn noisy_input() -> AudioStream { AudioStream::new().0 }
# async fn demo() -> Result<(), denoise::DenoiseError> {
// `noisy` is any AudioStream — microphone, camera audio, a decoded file.
let noisy = noisy_input();

let clean = Denoiser::new()
    .attenuation_limit_db(12.0)   // leave a little background in (optional)
    .process(&noisy)
    .await?;

// `clean` is a 48 kHz mono AudioStream — subscribe, record, or play it.
let _ = clean;
# Ok(())
# }
```

On **web**, fetch the model and pass its bytes instead of `new()`:

```rust
use denoise::Denoiser;

# async fn demo(weights: &'static [u8], noisy: &denoise::AudioStream) -> Result<(), denoise::DenoiseError> {
// `weights` is the DeepFilterNet3_onnx.tar.gz bytes, fetched at startup and
// stashed in a `static` (it must be `'static`).
let clean = Denoiser::with_weights(weights).process(noisy).await?;
# let _ = clean;
# Ok(())
# }
```

## Per-platform mechanism

The DSP is identical everywhere; only *where* inference runs differs:

| Target | Model | Execution context |
| --- | --- | --- |
| macOS / iOS / Android / desktop | embedded (`new()`) or supplied (`with_weights`) | a background processing thread, fed by the input subscription over a channel, so inference never blocks the audio thread |
| web (wasm32) | supplied via `with_weights` (not bundled into the wasm binary) | the same engine, run inline on the main thread inside the subscribe callback (wasm has no threads) |

`process` is `async` because building the model (the expensive step) happens
there — on native it runs on the worker thread and awaits its completion without
blocking the caller; on web it builds inline. Init failures surface as
`DenoiseError`.

## Permissions

None. This SDK consumes an existing `AudioStream` and needs no OS permission of
its own — the producer (e.g. `microphone`) already owns the capture permission.
