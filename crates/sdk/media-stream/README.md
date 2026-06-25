# `media-stream`

The common currency between capture SDKs that **produce** media (`camera`,
`screen-recorder`, `microphone`) and the layers that **consume** it (a display
component, a GPU compositor, a file writer, a denoiser). A developer wires
`camera -> video` and never names a platform type — the per-platform transport
(a web `MediaStream`, an Apple `CVPixelBuffer`/`IOSurface`, an Android
`SurfaceTexture`) is hidden behind one handle.

The crate is deliberately **thin and GPU-free**: a real-time GPU compositor is
a layer on top that consumes a stream, not part of this crate.

## What you get

Two peer abstractions with the same shape — a `!Send` consumer handle plus a
`Send` producer writer, paired by `::new()`:

- **`MediaStream`** — a live *video* source.
  - `MediaStream::new() -> (MediaStream, FrameWriter)` — build both halves.
  - `subscribe(cb) -> Subscription` — push: `cb(&VideoFrame)` per frame
    (tightly-packed top-down `RGBA8`).
  - `latest(&mut buf) -> Option<(w, h)>` / `latest_pts()` / `generation()` —
    pull the most recent frame (a GPU blit samples this; compare `generation`
    to skip unchanged frames).
  - `native_source() -> Option<Rc<dyn Any>>` / `set_native_source(..)` — the
    opaque zero-copy frame source a same-platform display downcasts.
  - `screenshot().await -> Option<Screenshot>` — one-shot still as CPU `RGBA8`.
  - `attach_stopper(f)` — teardown run when the last clone drops.
- **`FrameWriter`** (the `Send` producer side, handed to the capture thread):
  - `write_rgba8(w, h, data)` / `write_bgra8(w, h, data)` — push a frame
    (BGRA is swizzled to RGBA), stamped with `clock::now_micros()`.
  - `*_at(.., pts_micros)` variants for a real hardware capture timestamp.
  - `wants_cpu_frames()` — gate per-frame CPU normalization when only a
    native-source display is attached.
- **`AudioStream`** / **`AudioWriter`** — the audio peer (same `new` / `subscribe`
  / `latest` / `native_source` / `attach_stopper` shape). `AudioWriter::write_pcm_f32(sample_rate, channels, samples)`
  pushes interleaved, normalized `f32` PCM; PTS is **sample-derived** (a running
  frame count anchored to the shared clock), not wall-clock, so it stays
  monotonic and gap-free under bursty delivery.
- **`clock::now_micros()`** — a process-wide capture clock. Independent
  producers (a camera + a microphone) land on **one shared timeline**, which is
  exactly what a muxer needs to lip-sync audio against video.

Frame/chunk types: `VideoFrame` (`data`, `width`, `height`, `format`,
`pts_micros`), `AudioFrame` (`samples`, `sample_rate`, `channels`, `pts_micros`,
+ `frame_count()` / `duration_secs()` / `format()`), `PixelFormat::Rgba8`,
`AudioFormat`, `Screenshot`.

## Usage

Producing a stream (a capture backend):

```rust
use media_stream::{MediaStream, FrameWriter};

fn spawn_capture(_w: FrameWriter) -> Box<dyn FnOnce()> { Box::new(|| {}) }

let (stream, writer) = MediaStream::new();
let stop = spawn_capture(writer);          // capture thread calls writer.write_rgba8(..)
stream.attach_stopper(move || stop());
// hand `stream` to the consumer; dropping the last clone stops capture.
```

Consuming it (analysis / display / processing):

```rust
use media_stream::{MediaStream, VideoFrame};

# fn demo(stream: &MediaStream) {
// Push: a callback per frame. Keep it fast — copy pixels out, don't process in place.
let _sub = stream.subscribe(|f: &VideoFrame| {
    let (w, h) = (f.width, f.height);
    let pixels: &[u8] = f.data;          // tightly-packed RGBA8, w*h*4
    let _ = (w, h, pixels, f.pts_micros);
});

// Pull: read the most recent frame only when it changed.
let mut buf = Vec::new();
let mut last_gen = 0;
if stream.generation() != last_gen {
    if let Some((w, h)) = stream.latest(&mut buf) {
        last_gen = stream.generation();
        let _ = (w, h, &buf);
    }
}
# }
```

Audio mirrors it exactly via `AudioStream` / `AudioWriter` / `AudioFrame`.

## Per-platform mechanism

The CPU tap (`subscribe` / `latest`) is identical everywhere — RGBA8 video,
interleaved-`f32` audio. Only the opaque `native_source` (the zero-copy
fast-path) differs:

| Target | `native_source` (video) | Clock |
| --- | --- | --- |
| macOS | `IOSurface` (`SurfaceSource`), set as `CALayer.contents` | `Instant`, monotonic |
| iOS | `CMSampleBuffer`, enqueued into `AVSampleBufferDisplayLayer` | `Instant`, monotonic |
| Android | a `SurfaceTexture` published by the producer | `Instant`, monotonic |
| web (wasm32) | a `web_sys::MediaStream` (e.g. canvas `captureStream()`) | `Date.now()`-rebased (ordering hint only) |

On macOS, `MediaStream::with_surface_capture()` wires the IOSurface
self-capture path (`FrameWriter::publish_surface` + `SurfaceSource`); on web it
shares a native slot the canvas fills via `FrameWriter::publish_native_source`;
on other native targets it is identical to `new()` so callers stay portable.

## Permissions

None. This crate only carries already-captured frames between a producer and a
consumer — the **producer** SDK (`camera`, `microphone`, `screen-recorder`)
owns the OS capture permission.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet. Tick each
item as you exercise it. This crate is a pure-Rust abstraction, so most of its
coverage is automated; the per-backend items only exercise the opaque
`native_source` zero-copy slot.

**Automated**
- [ ] `cargo test -p media-stream` — producer/consumer channel: a producer feeds frames and `subscribe`/`latest` deliver them in order; PTS stays monotonic; the shared `clock::now_micros()` timeline is consistent across producers
- [ ] `cargo build -p media-stream --target wasm32-unknown-unknown` — web target

**Behavior**
- [ ] **Web** — the `web_sys::MediaStream` `native_source` round-trips: `set_native_source` → `native_source()` downcast yields the same handle for a zero-copy display.
- [ ] **iOS** — ⚠️ not yet device-confirmed: a `CMSampleBuffer` `native_source` round-trips for `AVSampleBufferDisplayLayer` enqueue.
- [ ] **Android** — ⚠️ compile-checked only, not yet device-confirmed: a `SurfaceTexture` `native_source` round-trips.
- [ ] **macOS** — the `IOSurface` (`SurfaceSource`) `native_source` round-trips and can be set as `CALayer.contents`; `with_surface_capture()` wires the self-capture path.
