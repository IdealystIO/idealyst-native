# media-writer

Record live media streams to a file. The **consumer** end of the
[`media-stream`](../media-stream) vocabulary: give it a video `MediaStream`
(from [`camera`](../camera) / [`screen-recorder`](../screen-recorder)) and/or an
audio `AudioStream` (from [`microphone`](../microphone)) and it muxes them to a
playable file, lip-syncing the two by the shared-clock capture timestamps every
producer stamps onto its frames.

```rust
use media_writer::{MediaWriter, MediaInputs, RecordConfig};

let store = files::app_files("recordings")?;
let writer = MediaWriter::new();

// camera video + mic audio → recordings/clip.mp4
let recording = writer
    .record(MediaInputs::av(&camera_stream, &mic_stream),
            RecordConfig::new(store, "clip.mp4"))
    .await?;

// ... later
let path = recording.stop().await?;   // finalize; returns the written path
```

`MediaInputs::video(&v)` / `MediaInputs::audio(&a)` / `MediaInputs::av(&v, &a)`
pick which sources to record. Recording requires **no permission of its own** —
the `camera` / `microphone` / `screen-recorder` SDKs already gate capture; this
SDK only consumes the streams they hand out and writes to the app's own files.

## Backends

| Target        | Mechanism                                        | Output |
| ------------- | ------------------------------------------------ | ------ |
| iOS / macOS   | `AVAssetWriter` (H.264 + AAC) via AVFoundation   | `.mp4` |
| Android       | `MediaCodec` + `MediaMuxer` via a Kotlin shim    | `.mp4` |
| web (wasm32)  | `MediaRecorder` over the streams' `MediaStream`  | `.mp4`/`.webm` |
| other         | `MediaWriterError::Unsupported`                  | —      |

The mechanism diverges per platform; the *output* converges on a playable file
addressed through a [`files`](../files) store + relative path, so the same call
works everywhere.

### How A/V sync works

`media-stream` stamps every video frame and audio chunk with a microsecond
timestamp on one process-wide monotonic clock (`media_stream::clock`). Because a
camera `MediaStream` and a microphone `AudioStream` share that clock, the writer
places each sample on the file's presentation timeline by its own timestamp and
the two tracks stay in sync — even though they were captured by independent
sources on different threads.

### Web container caveat

`MediaRecorder`'s output container is browser-chosen: **Safari yields real MP4;
Chromium yields WebM.** The writer requests `video/mp4` and falls back to
`video/webm`, writing whatever the browser produces to the path you gave. The
bytes are always a valid, playable file; only the container may differ from
`.mp4` on Chromium. This is a genuine platform constraint of the web encoder.

## Verification status

- **macOS** — host-verified: `tests/host_record.rs` feeds synthetic
  `MediaStream` + `AudioStream` producers (no hardware) and asserts a
  non-trivial, real `.mp4` lands on disk.
- **iOS** — shares the macOS `AVAssetWriter` backend; device verification
  pending.
- **Android** — compile-checked for `aarch64-linux-android`; the
  MediaCodec/MediaMuxer path resolves at runtime on a device (same posture as
  the `camera`/`biometrics` Android backends).
- **web** — compile-checked for `wasm32-unknown-unknown`.

## Permissions / linking

- **iOS / macOS** — links `AVFoundation`, `CoreMedia`, `CoreVideo`,
  `AudioToolbox` (collected from the dep graph by the CLI). No usage-description
  of its own.
- **Android** — ships `runtime/kotlin/.../RustMediaWriter.kt` via
  `[package.metadata.idealyst.android].runtime_kotlin`.
