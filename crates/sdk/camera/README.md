# `camera`

Cross-platform camera capture — the video sibling of [`microphone`], and
the smallest useful abstraction over the platform's camera. Open a stream,
receive raw pixel frames in a callback, drop the stream to stop. No files,
no encoding, **no preview widget**, no opinion about where the frames go.
That's deliberately left to higher-level SDKs (or your app); this crate's
only job is to **establish the stream and hand you pixels**.

```rust
use camera::{Camera, CameraConfig};

# async fn demo() -> Result<(), camera::CameraError> {
let cam = Camera::new();

let stream = cam
    .open(CameraConfig::default().back(), |frame| {
        // Runs on a capture thread (native/Android) or the main thread
        // (web). `frame.data` is tightly-packed, top-down RGBA8 of
        // `frame.width * frame.height * 4` bytes. Copy out what you need
        // and return quickly.
        let (w, h) = (frame.width, frame.height);
        let _ = (w, h, frame.data);
    })
    .await?;

// Capture runs for as long as `stream` is alive.
stream.stop(); // or just drop it
# Ok(())
# }
```

## What you get

Every backend delivers the **same shape** to your callback — a
[`VideoFrame`] of **tightly-packed, top-down `RGBA8`** pixels
(`data.len() == width * height * 4`) plus the actual `width` and `height`.
The platforms diverge in mechanism — and in their native pixel layout —
but all normalize to that one frame format, so consumer code is identical
everywhere:

| Target | Mechanism | Native layout → normalized |
| --- | --- | --- |
| iOS / macOS | `AVCaptureSession` + `AVCaptureVideoDataOutput` | `BGRA` → `RGBA8` |
| Android | `Camera2` + `ImageReader` (via a Kotlin shim) | `YUV_420_888` → `RGBA8` |
| Web (wasm32) | `getUserMedia` + a `<video>`/`<canvas>` frame pump | canvas `RGBA8` |
| desktop Linux / Windows | *not yet implemented* — returns `Unsupported` | — |

The callback is `FnMut`, so it can own and mutate state across frames. It
must be `Send` on native/Android (it runs on the capture thread) and need
not be on web (it runs on the main thread) — the [`FrameCallback`] bound
encodes that per target, so the same closure compiles everywhere.

## Getting frames on screen

This SDK renders **nothing** — by design (see [`microphone`]'s posture).
A `VideoFrame` is just RGBA bytes; do whatever you want with them:

- **Show a live preview** — upload each frame into a `graphics` surface (a
  GPU texture you own) and draw it, or `drawImage`/`putImageData` it onto a
  canvas on web.
- **Process / analyze** — run frames through your own pipeline (ML, QR
  decode, color sampling) off the capture thread.
- **Record / upload** — feed frames to an encoder or a `net` upload.

Because the format is uniform RGBA8, the consumer code is the same on every
platform.

## Delivery model

A **raw push callback**, on purpose. The callback fires from the capture
thread with each frame; you decide what to do with the pixels. Keeping this
layer unopinionated lets a future SDK add preview-surface / recording /
streaming abstractions without this crate having baked in the wrong one.

> **Frames are borrowed.** `frame.data` points at a backend-owned buffer
> that's reused for the next frame. Copy out what you need before the
> callback returns; don't stash the slice.

## Permissions

This SDK declares the capability it needs in its own `Cargo.toml`:

```toml
[package.metadata.idealyst]
capabilities = ["camera"]
```

The CLI walks your app's dependency graph at build time, finds that
declaration, and **injects the right platform artifacts automatically** —
you don't hand-edit `Info.plist` or `AndroidManifest.xml`:

- **iOS / macOS** — `NSCameraUsageDescription` (+ the
  `com.apple.security.device.camera` entitlement on macOS, for signed
  builds).
- **Android** — `<uses-permission android:name="android.permission.CAMERA"/>`.
- **Web** — nothing to declare; the browser prompts on first
  `getUserMedia`. Capture requires a **secure context** (HTTPS or
  `localhost`).

What you *should* add is the **user-facing reason** the OS shows in its
prompt — only you can word that for your app:

```toml
[package.metadata.idealyst.app.permissions]
camera = "Scan documents"
```

If you omit it, the build still succeeds but uses a generic reason and
prints a warning — generic iOS usage strings risk App Store rejection, so
treat the default as a stopgap. The CLI also prints each permission it
bundled and which crate requested it, so nothing is added invisibly.

[`Camera::request_permission`] proactively triggers the prompt where one
exists. It's optional — [`Camera::open`] requests access on its own.

### Android runtime-permission caveat

`request_permission()` checks the current grant and, if missing, fires the
system dialog — but its result is delivered to the Activity's
`onRequestPermissionsResult`, which this SDK does not hook. So the call
returns the *current* (not-yet-granted) state after showing the dialog;
re-check (or retry `open`) once the user has responded. `open()` fails fast
with `CameraError::PermissionDenied` if `CAMERA` isn't granted.

## Configuration

[`CameraConfig`] is all-optional resolution / frame rate plus a
[`CameraFacing`]; `None`/`Default` fields defer to the device's preferred
value. Requests the device can't honour surface as
`CameraError::UnsupportedConfig` rather than a silent substitution, so the
`width` / `height` you read off each frame are authoritative.

```rust
use camera::CameraConfig;

let _ = CameraConfig::default();                       // primary camera, device defaults
let _ = CameraConfig::new().front();                   // selfie camera
let _ = CameraConfig::new().back().with_resolution(1280, 720).with_fps(30);
```

On Apple, an explicit resolution is honoured by selecting a matching
`AVCaptureDeviceFormat` (else `UnsupportedConfig`); on Android the shim
matches a `Camera2` output size; on web it's passed as a `getUserMedia`
constraint.

## Tests

- `tests/portable.rs` — frame size math + config builders; runs anywhere.
- `tests/host_capture.rs` — opens the host's default camera and asserts the
  callback fires with a well-formed RGBA8 frame. `#[ignore]`d (needs real
  hardware + permission); run it with:

  ```text
  cargo test -p camera --test host_capture -- --ignored --nocapture
  ```

  Verified on macOS against the built-in camera. The Android backend is
  compile-checked for `aarch64-linux-android` but, like the `biometrics`
  SDK's Android path, is **not yet device-verified**.

[`microphone`]: ../microphone/README.md
[`VideoFrame`]: src/frame.rs
[`CameraConfig`]: src/config.rs
[`CameraFacing`]: src/config.rs
[`FrameCallback`]: src/lib.rs
[`Camera::request_permission`]: src/lib.rs
[`Camera::open`]: src/lib.rs
