# `video-decode`

Cross-platform video-**file** decoding into live streams. Where [`camera`] and
[`screen-recorder`] open a live *capture* source, this SDK opens a *clip* (a file
path / URL / bytes), decodes it on the platform's own media stack, and produces
the SAME currency the capture SDKs do — a [`MediaStream`] of tightly-packed
`RGBA8` frames plus an optional [`AudioStream`] of interleaved-`f32` PCM — along
with a `Transport` to play / pause / seek / mute and to read position + duration.

## Why this exists

The native `video` SDK renders a clip in an OS player **overlay view**, outside
any GPU canvas scene — so it can't be drawn over and a canvas surface-capture
recorder never sees it. This SDK instead hands the decoded pixels back as a
[`MediaStream`], so a canvas can composite the clip *into* its scene (ink draws
over it; the recorder captures it) and the [`AudioStream`] feeds a recording's
audio mux.

## What you get

- [`VideoDecoder`] — a cheap, clonable handle that holds no resources until you
  `open` a clip.
- `VideoDecoder::open(source, config) -> Result<DecodedVideo, VideoDecodeError>`
  — opens and begins decoding. Decode runs while the returned value (or its
  frames stream) is alive; dropping it stops decode and releases the player.
- [`DecodeSource`] — `Url(String)` (a `file://`, `http(s)://`, or `data:` URL)
  or `Bytes(Vec<u8>)`. Convenience constructors `DecodeSource::url(..)` /
  `DecodeSource::bytes(..)`. On native, `Bytes` is materialized to a temp
  `file://`; on web it becomes a `Blob` object URL.
- [`DecodeConfig`] — `autoplay`, `loop_playback`, `muted`, `max_dimension`. All
  default to a paused, unmuted, non-looping clip at natural size
  (`DecodeConfig::default()`).
- [`DecodedVideo`] — the result bundle:
  - `.frames() -> &MediaStream` — the decoded `RGBA8` video frames.
  - `.audio() -> Option<&AudioStream>` — decoded PCM, or `None` if the clip has
    no audio track.
  - `.transport() -> &Transport` — playback control + state.
  - `.natural_size() -> Option<(u32, u32)>` — the clip's pixel size, if known.
- [`Transport`] — cloneable `Rc` handle: `play`, `pause`, `seek(secs)`,
  `seek_preview(secs)` (fast/approximate for live scrubbing), `set_muted`,
  `set_rate`, and the getters `position`, `duration`, `is_playing`, `is_muted`.
- [`VideoDecodeError`] — `BadSource(String)`, `Backend(String)`, `Unsupported`.
- Re-exported from `media-stream`: [`MediaStream`], [`AudioStream`].

## Usage

```rust
use video_decode::{VideoDecoder, DecodeConfig, DecodeSource};

# async fn demo() -> Result<(), video_decode::VideoDecodeError> {
let dec = VideoDecoder::new();
let clip = dec
    .open(
        DecodeSource::url("file:///clip.mp4"),
        DecodeConfig { max_dimension: Some(512), ..Default::default() },
    )
    .await?;

// Composite frames into a GPU canvas — poll the newest on a render tick…
if let Some(frame) = clip.frames().latest() {
    let _ = (frame.width, frame.height, frame.data);
}
// …or subscribe for push delivery.

// Hand the clip's audio to `media-writer` for the recording's mux.
let _has_audio = clip.audio().is_some();

// Drive playback + a scrubber.
let t = clip.transport();
t.play();
let _pos = t.position();      // seconds
let _dur = t.duration();      // seconds (may resolve a beat after open)
t.seek_preview(10.0);         // while dragging
t.seek(10.0);                 // exact, on landing

// Drop `clip` (or its frames stream) to stop decode.
# Ok(())
# }
```

Muting via `Transport::set_muted` only silences the *player's* output — the
[`AudioStream`] keeps carrying PCM for the recorder regardless.

## Per-platform mechanism

Every backend normalizes video to `RGBA8` frames and audio to interleaved-`f32`
PCM; only the decode stack differs.

| Target | Mechanism |
| --- | --- |
| iOS / macOS | `AVPlayer` + `AVPlayerItemVideoOutput` (frames) + an `MTAudioProcessingTap` on the item's audio mix (PCM) |
| Android | `MediaExtractor` + `MediaCodec` via a Kotlin shim |
| Web (wasm32) | hidden `<video>` + offscreen `<canvas>` frame pump; WebAudio `MediaElementSource → ScriptProcessor` PCM tap |
| desktop Linux / Windows | *not implemented* — returns `VideoDecodeError::Unsupported` |

## Permissions

None to request — this decodes media you already have. On Apple targets the CLI
auto-links `AVFoundation`/`CoreMedia`/`CoreVideo`/`MediaToolbox`/`CoreFoundation`.
Web `data:`/`Blob` decoding works in any context; remote URLs are subject to the
usual CORS rules.

## Tests

- `tests/host_open.rs` (macOS) — opens a real local clip and exercises `open()`
  + the transport getters and the frame pump. Point it at a file with
  `VIDEO_DECODE_TEST_FILE=/path/clip.mp4` and run:

  ```text
  cargo test -p video-decode --test host_open -- --nocapture
  ```

[`camera`]: ../camera/README.md
[`screen-recorder`]: ../screen-recorder/README.md
[`MediaStream`]: ../media-stream/src/lib.rs
[`AudioStream`]: ../media-stream/src/lib.rs
[`VideoDecoder`]: src/lib.rs
[`DecodeSource`]: src/lib.rs
[`DecodeConfig`]: src/lib.rs
[`DecodedVideo`]: src/lib.rs
[`Transport`]: src/lib.rs
[`VideoDecodeError`]: src/error.rs
