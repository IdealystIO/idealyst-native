+++
title = "Media: Capture & Recording"
order = 70
tags = ["media", "camera", "microphone", "screen-recorder", "recording", "audio", "video", "sdk"]
+++

# Media: capture & recording

Idealyst's media SDKs share one shape: a **producer** opens a platform capture
source and hands you a **stream**; **consumers** (a display, a GPU compositor,
a file recorder) read that stream. You wire `camera → video` or
`camera + microphone → media-writer` and never name a platform type — each SDK
is internally cfg-gated and converges on identical behavior across iOS, macOS,
Android, and web.

```text
 producers                streams                consumers
 ─────────                ───────                ─────────
 camera ───────────┐
 screen-recorder ──┼────► MediaStream  (video) ─┬─► video (display)
                   │                            └─► media-writer ─► .mp4 file
 microphone ───────┴────► AudioStream  (audio) ───┘
```

There are two stream abstractions, and they are peers:

- **`MediaStream`** — a live **video** source. Produced by `camera` and
  `screen-recorder`.
- **`AudioStream`** — a live **audio** source. Produced by `microphone`.

Both expose the same two access paths over one source:

1. **CPU tap** — `subscribe(|frame| …)` pushes normalized frames
   (`RGBA8` video / interleaved-`f32` audio); `latest(&mut buf)` polls the most
   recent one. Lowest-common-denominator, works everywhere.
2. **Native source** — `native_source()` returns the platform's own
   zero-copy/native handle (a web `MediaStream`, an Apple `CVPixelBuffer`
   pool, an Android `SurfaceTexture`) for a same-platform consumer to bind
   without copying. Authors rarely touch it directly.

Every frame and audio chunk is stamped with a microsecond timestamp on one
process-wide monotonic clock (`media_stream::clock`), so two **independent**
sources (a camera and a microphone) share a timeline — which is what lets the
recorder lip-sync them.

---

## The useful entry points

### Open a camera → `MediaStream`

```rust
use camera::{Camera, CameraConfig};

let stream = Camera::new().open(CameraConfig::default().back()).await?;
let sub = stream.subscribe(|frame| {
    // frame.data is tightly-packed RGBA8, frame.width × frame.height
});
// drop `sub` to stop tapping; drop `stream` to stop capture.
```

### Record the screen → `MediaStream`

```rust
use screen_recorder::{ScreenRecorder, RecordingConfig, Source};

let stream = ScreenRecorder::new()
    .start(RecordingConfig { source: Source::Screen, ..Default::default() })
    .await?;
// same MediaStream the camera produces — tap it or hand it to `video`.
```

### Open a microphone → `AudioStream` (or a raw callback)

```rust
use microphone::{Microphone, AudioStreamConfig};

// Abstracted: an AudioStream you can record or play back.
let audio = Microphone::new().open_stream(AudioStreamConfig::default()).await?;

// Or the raw minimal form — a callback of PCM chunks, no stream object:
let mic = Microphone::new();
let handle = mic.open(AudioStreamConfig::default().mono(), |buf| {
    // buf.samples: interleaved f32 in [-1.0, 1.0]
}).await?;
```

Use `open_stream` when a *consumer* needs an `AudioStream` (recording it to a
file, or a future audio-playback layer that binds the platform's native audio
pipeline). Use the raw `open` callback for the smallest case — level metering,
analysis — where you just want the samples.

### Record video + audio to a file → `media-writer`

```rust
use media_writer::{MediaWriter, MediaInputs, RecordConfig};

let store = files::app_files("recordings")?;
let recording = MediaWriter::new()
    .record(MediaInputs::av(&camera_stream, &mic_stream),
            RecordConfig::new(store, "clip.mp4"))
    .await?;

// ... later
let path = recording.stop().await?;   // finalize; returns the written path
```

`MediaInputs::video(&v)` / `::audio(&a)` / `::av(&v, &a)` choose which sources
to record. The writer muxes them into one `.mp4`, lip-synced by their shared
capture timestamps. Recording needs **no permission of its own** — the capture
SDKs already gate access; the writer only consumes their streams and writes to
the app's files.

### Export the file to the user → `file-export`

`media-writer` writes to the **app sandbox** (via `files`). To hand the result
to the *user* — "Save to Files" / a save dialog at a location they pick — use
`file-export`:

```rust
use file_export::{FileExport, SaveRequest, SaveOutcome};

// `path` came from `recording.stop()`; resolve the real on-disk file.
let local = store.local_path(&path).expect("native path");
let outcome = FileExport::new()
    .save(SaveRequest::path("clip.mp4", "video/mp4", local))
    .await?;
```

It maps to each platform's native save UI — iOS `UIDocumentPickerViewController`,
macOS `NSSavePanel`, Android Storage Access Framework, Windows `IFileSaveDialog`,
Linux `xdg-desktop-portal`, web `showSaveFilePicker()`. Because the picker is
user-initiated, it needs **no storage permission**. On web (no filesystem path)
read the bytes from the store and use `SaveRequest::bytes(...)`. The user
dismissing the picker is `SaveOutcome::Cancelled`, not an error.

---

## Permissions

The capture SDKs declare the *capability*; you supply the user-facing *reason*
in your app manifest under `[package.metadata.idealyst.app.permissions]`, and
the CLI injects the platform artifacts (iOS/macOS `NS*UsageDescription`,
Android `CAMERA` / `RECORD_AUDIO`) from the dependency graph. The web prompts
on first `getUserMedia` / `getDisplayMedia`.

| SDK | Capability | iOS / macOS | Android |
| --- | --- | --- | --- |
| `camera` | `camera` | `NSCameraUsageDescription` | `CAMERA` |
| `microphone` | `microphone` | `NSMicrophoneUsageDescription` | `RECORD_AUDIO` |
| `screen-recorder` | `screen_capture` | ReplayKit / ScreenCaptureKit consent | MediaProjection |

---

## Platform support

| Capability | iOS | macOS | Android | web |
| --- | --- | --- | --- | --- |
| `camera` | ✅ AVFoundation | ✅ AVFoundation | ✅ Camera2 | ✅ getUserMedia |
| `microphone` | ✅ cpal + AVAudioSession | ✅ cpal | ✅ AudioRecord | ✅ Web Audio |
| `screen-recorder` | ✅ ReplayKit | ✅ ScreenCaptureKit | ✅ MediaProjection | ✅ getDisplayMedia |
| `media-writer` | ✅ AVAssetWriter | ✅ AVAssetWriter | ✅ MediaCodec + MediaMuxer | ✅ MediaRecorder¹ |

¹ On web, `MediaRecorder`'s container is browser-chosen: Safari yields real
MP4, Chromium yields WebM. The bytes are always a valid, playable file.

---

## See also

- The `video` SDK consumes a `MediaStream` to display a live source.
- Demos: `camera-demo`, `mic-demo`, `screenshare-preview-demo`,
  `media-sources-demo`, `media-recorder-demo`.
