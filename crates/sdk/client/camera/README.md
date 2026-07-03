# `camera`

Cross-platform camera capture — the video sibling of [`microphone`], and
the smallest useful abstraction over the platform's camera. Open it, get a
[`MediaStream`] — a platform-agnostic live video source — and drop the
stream to stop. No files, no encoding, **no preview widget**. The
[`MediaStream`] is the shared currency this SDK trades with `screen-recorder`
(another producer) and the `video` display layer (a consumer); see the
[`media-stream`] crate.

```rust
use camera::{Camera, CameraConfig};

# async fn demo() -> Result<(), camera::CameraError> {
let cam = Camera::new();

// Open the camera → a live MediaStream. Capture runs while any clone is
// alive; the last drop stops it.
let stream = cam.open(CameraConfig::default().back()).await?;

// Tap raw RGBA8 frames (push). Runs on a capture thread (native/Android)
// or the main thread (web).
let sub = stream.subscribe(|frame| {
    // `frame.data` is tightly-packed top-down RGBA8 of
    // `frame.width * frame.height * 4` bytes. Copy out what you need.
    let (w, h) = (frame.width, frame.height);
    let _ = (w, h, frame.data);
});

// Drop `sub` to stop tapping; drop `stream` to stop capture.
# let _ = sub;
# Ok(())
# }
```

## What you get

A [`MediaStream`] (from the [`media-stream`] crate) exposes two ways to read
the source: [`subscribe`] for push frames and [`latest`] for a pull of the
most recent one. Every backend delivers the **same shape** — a
[`VideoFrame`] of **tightly-packed, top-down `RGBA8`** pixels
(`data.len() == width * height * 4`) plus the actual `width` and `height`.
The platforms diverge in mechanism — and in their native pixel layout —
but all normalize to that one frame format, so consumer code is identical
everywhere:

| Target | Mechanism | Native layout → normalized |
| --- | --- | --- |
| iOS device / macOS | `AVCaptureSession` + `AVCaptureVideoDataOutput` | `BGRA` → `RGBA8` |
| **iOS Simulator** | **synthetic test-pattern stream** (no camera hardware exists) | `RGBA8` |
| Android | `Camera2` + `ImageReader` (via a Kotlin shim) | `YUV_420_888` → `RGBA8` |
| Web (wasm32) | `getUserMedia` + a `<video>`/`<canvas>` frame pump | canvas `RGBA8` |
| desktop Linux / Windows | *not yet implemented* — returns `Unsupported` | — |

**iOS Simulator note:** the iOS Simulator has no camera hardware
(`AVCaptureSession` finds no device), so on the simulator `Camera::open` returns
a **synthetic animated stream** — a calm gradient with a slowly bouncing ball, at
the requested resolution/fps — delivered as a normal `MediaStream`. This lets you
exercise camera UI on the sim with no code changes; the real `AVFoundation`
backend runs on physical devices. The synthetic path is gated `cfg(target_abi =
"sim")`, so it isn't compiled into device builds. (Android emulators already ship
their own synthetic camera, so this is iOS-only.)

The subscribe callback is `FnMut`, so it can own and mutate state across
frames. It must be `Send` on native/Android (it runs on the capture thread)
and need not be on web (it runs on the main thread) — the [`FrameCallback`]
bound encodes that per target, so the same closure compiles everywhere.

## Getting frames on screen

This SDK renders **nothing** — by design. The [`MediaStream`] is meant to be
*consumed*: hand it to the `video` display layer, or read it yourself.

- **Display / composite** — a `video` consumer or a GPU compositor takes the
  `MediaStream`; on web it can attach the stream's zero-copy
  [`native_source`] (a `web_sys::MediaStream`) directly, no per-frame copy.
- **Process / analyze** — `subscribe` and run frames through your own
  pipeline (ML, QR decode, color sampling) off the capture thread.
- **Record / upload** — feed frames to an encoder or a `net` upload.

Because the format is uniform RGBA8 (and the native source is hidden behind
[`media-stream`]), consumer code is the same on every platform.

## Delivery model

A **live source you tap**, on purpose. `subscribe` fires from the capture
thread with each frame; `latest` polls the most recent. The stream stays
unopinionated about display/encoding so a `video` consumer or GPU compositor
can layer on top — and so the platform transport (a web `MediaStream`, an
Apple `CVPixelBuffer`, an Android `SurfaceTexture`) never leaks into your
code.

> **Frames are borrowed.** In a `subscribe` callback `frame.data` points at a
> reused buffer. Copy out what you need before returning; don't stash the
> slice.

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

The runtime **grant** flow (reading the current status and surfacing the OS
prompt) is delegated to the shared `permissions` SDK —
`permissions::request(Permission::Camera)` on iOS/macOS/Android/web. This
crate keeps only the *capture* code; the AVCaptureDevice / `checkSelfPermission`
/ `navigator.permissions` grant logic lives in `permissions`. `camera` still
declares the `camera` capability above (the manifest requirement); only the
grant mechanism moved.

### Android runtime-permission caveat

`request_permission()` delegates to `permissions`, which checks the current
grant and, if missing, fires the system dialog — but its result is delivered
to the Activity's `onRequestPermissionsResult`, which the host must forward to
`permissions` (see its README's request seam). So the call returns the
*current* (not-yet-granted) state after showing the dialog; re-check (or retry
`open`) once the user has responded. `open()` fails fast with
`CameraError::PermissionDenied` if `CAMERA` isn't granted.

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

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p camera` — portable logic (frame-size math + config builders)
- [ ] `cargo test -p camera --test host_capture -- --ignored --nocapture` — opens the host camera, asserts a well-formed RGBA8 frame
- [ ] `cargo build -p camera --target wasm32-unknown-unknown` — web target

**Behavior**
- [ ] **Web** — `getUserMedia` prompt appears (secure context only); `subscribe` delivers live RGBA8 frames at the requested resolution; deny → `PermissionDenied`, no crash.
- [ ] **iOS** — on a **device**, permission prompt with the app's reason; preview shows live frames at the requested resolution + correct orientation; front/back switch works; deny → `PermissionDenied`. On the **Simulator** the synthetic gradient/bouncing-ball stream renders (no hardware). ⚠️ device path not yet re-confirmed since permission moved to the `permissions` SDK — verify the prompt still appears.
- [ ] **Android** — ⚠️ compile-checked only, not yet device-confirmed: permission prompt fires (delegated to `permissions`; host must forward `onRequestPermissionsResult`); `open()` yields RGBA8 frames after grant; deny → `PermissionDenied`.
- [ ] **macOS** — hardware-verified against the built-in camera (`host_capture`); confirm the prompt still appears now that the grant routes through the `permissions` SDK.

**Permissions**
- [ ] Permission prompt still surfaces (grant flow now delegated to the `permissions` SDK); the build-injected `NSCameraUsageDescription` / `CAMERA` carries the app's configured reason.

[`microphone`]: ../microphone/README.md
[`media-stream`]: ../media-stream
[`MediaStream`]: ../media-stream/src/lib.rs
[`VideoFrame`]: ../media-stream/src/lib.rs
[`FrameCallback`]: ../media-stream/src/lib.rs
[`subscribe`]: ../media-stream/src/lib.rs
[`latest`]: ../media-stream/src/lib.rs
[`native_source`]: ../media-stream/src/lib.rs
[`CameraConfig`]: src/config.rs
[`CameraFacing`]: src/config.rs
[`Camera::request_permission`]: src/lib.rs
[`Camera::open`]: src/lib.rs
