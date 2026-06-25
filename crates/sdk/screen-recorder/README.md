# `screen-recorder`

Cross-platform screen / window / app recording — the capture sibling of
[`camera`]. Call `ScreenRecorder::start(config)` and you get a [`MediaStream`]:
the same platform-agnostic live video source `camera` produces and the `video`
SDK displays. Like `microphone`, it is deliberately unopinionated — it hands you
raw `RGBA8` frames and gets out of the way. **It never writes a file**; encoding
to mp4/webm, muxing audio, and persisting to disk are the job of a higher-level
crate (e.g. `media-writer`) that layers on top.

A second, optional feature is the **private layer**: an overlay subtree that
recordings *don't* capture (the iOS "put the chrome on a separate `UIWindow`"
trick, generalized to every backend).

## What you get

- [`ScreenRecorder`] — a cheap, clonable, backend-agnostic handle that holds no
  OS resources until you `start`.
- `ScreenRecorder::start(config) -> Result<MediaStream, RecorderError>` — begins
  capture and returns a live stream. Capture runs while any clone of the stream
  is alive; dropping the last one stops it and tears down the platform session.
- `ScreenRecorder::request_permission(&source)` — optionally trigger the consent
  flow up front. On most targets the prompt only appears at `start`, so this
  usually just resolves `Ok` to defer to that call.
- [`RecordingConfig`] — builder over `source`, `audio`, `fps`, `size`. Construct
  with `RecordingConfig::new()` (defaults: `Source::ThisApp`, no audio,
  `DEFAULT_FPS` = 30, native size).
- [`Source`] — `ThisApp`, `UserChoice`, `FullScreen`, or `Window(WindowSelector)`.
- [`AudioSource`] — reserved enum (`None` / `App` / `System` / `Microphone` /
  `AppAndMic`) for when audio capture lands; the frame callback is video-only today.
- [`PrivateLayer(children)`] + [`register(&mut backend)`] — the capture-excluded
  overlay (`Element::External`) and its per-backend bootstrap.
- [`RecorderError`] — `Unsupported`, `PermissionDenied`, `UnsupportedSource(&str)`,
  `Platform(String)`.
- Re-exported from `media-stream`: [`MediaStream`], [`VideoFrame`],
  [`PixelFormat`], [`Subscription`], [`FrameCallback`].

## Usage

```rust
use screen_recorder::{ScreenRecorder, RecordingConfig, Source};

# async fn demo() -> Result<(), screen_recorder::RecorderError> {
let recorder = ScreenRecorder::new();

// Begin capturing this app at 60fps → a live MediaStream.
let stream = recorder
    .start(RecordingConfig::new().source(Source::ThisApp).fps(60))
    .await?;

// Tap raw, tightly-packed top-down RGBA8 frames (capture thread on native,
// main thread on web). Copy out what you need — `frame.data` is borrowed.
let sub = stream.subscribe(|frame| {
    let _ = (frame.width, frame.height, frame.data);
});

// …or hand `stream` to the `video` SDK to show the live screen, or to
// `media-writer` to encode it.

// Drop `sub` to stop tapping; drop `stream` to stop capture.
# let _ = sub;
# Ok(())
# }
```

### The private layer (optional)

Designates an overlay subtree the recorder skips — handy for recording
controls, watermarks, or chrome you don't want in the captured output. Bootstrap
once at startup, then wrap children:

```rust
// bootstrap (native builds the capture-excluded window):
// screen_recorder::register(&mut backend);

// in your tree:
// ui! {
//     view {
//         RecordableContent()
//         { screen_recorder::PrivateLayer(vec![ ui! { RecordingControls() } ]) }
//     }
// }
```

Exclusion only applies when recording `Source::ThisApp`.

## Per-platform mechanism

Every backend normalizes to the same `RGBA8` frame; only the capture stack
differs.

| Target | Capture mechanism | Private-layer exclusion |
| --- | --- | --- |
| iOS | ReplayKit (`RPScreenRecorder.startCapture`) → `CMSampleBuffer` | separate `UIWindow` — ReplayKit records the key window only (device-verified) |
| macOS | ScreenCaptureKit (`SCStream` + `SCContentFilter`) | separate `NSWindow`/`NSPanel` excluded via `SCContentFilter` |
| Android | `MediaProjection` + `VirtualDisplay` + `ImageReader` (JNI/Kotlin shim) | separate `WindowManager` window — PixelCopy-excluded |
| Web (wasm32) | `getDisplayMedia` → hidden `<video>` → `<canvas>` pixel pump | inline no-op (TODO: Element Capture `restrictTo`) |
| Windows | `Windows.Graphics.Capture` *(planned)* | `WDA_EXCLUDEFROMCAPTURE` *(planned)* |
| Linux | xdg-desktop-portal ScreenCast + PipeWire *(planned)* | none available |
| other desktop | *not implemented* — returns `RecorderError::Unsupported` | — |

`Source::Window(..)` is desktop-only; iOS/Android treat `UserChoice` as
`ThisApp` (no picker) and reject `Window` with `UnsupportedSource`.

## Permissions

This SDK declares its capability in its own `Cargo.toml`
(`capabilities = ["screen_capture"]`); the CLI walks your app's dependency graph
at build time and injects the right per-platform artifacts:

- **iOS** — ReplayKit prompts at capture; a broadcast extension is needed for
  system-wide capture. `ReplayKit`/`CoreMedia`/`CoreVideo` are auto-linked.
- **macOS** — the system Screen Recording (TCC) permission prompt.
- **Android** — `FOREGROUND_SERVICE_MEDIA_PROJECTION` plus the MediaProjection
  consent dialog (re-prompts each session on API 14+).
- **Web** — nothing to declare; the browser shows the source picker on
  `getDisplayMedia`.

## Tests

- `tests/portable.rs` — config builders + the private-layer `Element::External`
  lowering contract + the skeleton's `Unsupported` contract; runs anywhere.

[`camera`]: ../camera/README.md
[`MediaStream`]: ../media-stream/src/lib.rs
[`VideoFrame`]: ../media-stream/src/lib.rs
[`PixelFormat`]: ../media-stream/src/lib.rs
[`Subscription`]: ../media-stream/src/lib.rs
[`FrameCallback`]: ../media-stream/src/lib.rs
[`ScreenRecorder`]: src/lib.rs
[`RecordingConfig`]: src/config.rs
[`Source`]: src/config.rs
[`AudioSource`]: src/config.rs
[`PrivateLayer(children)`]: src/private_layer.rs
[`register(&mut backend)`]: src/lib.rs
[`RecorderError`]: src/error.rs
