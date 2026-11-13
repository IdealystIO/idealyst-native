# `video-codec`

Live video encoding and decoding on the platform's own codecs. An `Encoder`
takes a capture `MediaStream` (`screen-recorder`, `camera`) and emits each
encoded frame as it comes out; a `Decoder` takes encoded frames and shows
them as a `MediaStream` the `video` SDK plays. Moving the frames between
them (a socket, QUIC datagrams) is the app's job. Both are realtime: no
muxing, no buffering for smoothness, keyframes on request. For playing a
video file, see `video-decode`.

```rust,ignore
use screen_recorder::{RecordingConfig, ScreenRecorder, Source};
use video_codec::{Codec, Decoder, EncodedFrame, Encoder, EncoderConfig, EncoderEvent};

// Sender (Android): the screen, hardware-encoded.
let screen = ScreenRecorder::new()
    .start(RecordingConfig::new().source(Source::FullScreen).max_edge(1280))
    .await?;
let encoder = Encoder::start(&screen, EncoderConfig::realtime(), |event| match event {
    EncoderEvent::Frame(frame) => send(frame),
    EncoderEvent::Ended(why) => log(why),
    _ => {}
})?;
encoder.request_keyframe(); // the receiver lost a frame

// Receiver (web): frames in, a stream out.
let mut decoder = Decoder::new(Codec::H264)?;
if !decoder.decode(&EncodedFrame { data, pts_us, key }) {
    ask_sender_for_keyframe();
}
// decoder.stream() → the `video` SDK.
```

## Platforms

| | Encoder | Decoder |
|---|---|---|
| Android | MediaCodec H.264, from a stream's `PreviewSource` | — |
| web | — | WebCodecs `VideoDecoder` |

Everything else is `VideoCodecError::Unsupported`.

- **Encoder (Android).** The encoder claims the stream's
  `media_stream::PreviewSource` and hands it MediaCodec's input surface, so
  the capture renders straight into the encoder and no pixel passes through
  the app. While it holds the claim the stream's CPU subscribers and any
  display of the same stream get no frames (`InUse` if another consumer
  holds it, `NoSurface` if the stream has no such source). Settings are
  realtime: baseline profile where the encoder takes it, CBR where
  supported, keyframes only on request, the last frame repeated every
  quarter second while the picture is still, and SPS/PPS in front of every
  keyframe so a decoder can join at any one. `on_event` runs on the codec's
  thread.
- **Sources.** A whole-screen `screen-recorder` capture is upright and is
  the verified case. A `camera` stream encodes as the sensor delivers it:
  turned by the source's `PreviewGeometry::sensor_orientation` and, for a
  front camera, mirrored. The receiver turns it.
- **Decoder (web).** Made at the first keyframe from the codec string its
  SPS gives (`h264::codec_string`), remade when a keyframe names another
  (the sender restarted at another size) or after a decode error. Each
  frame is drawn onto a canvas as soon as it is decoded, and the canvas's
  `captureStream()` backs the stream. `decode` returns `false` when nothing
  can be decoded until a keyframe.

## Frame loss

H.264 here has no B-frames, so each frame depends on the ones before it
back to a keyframe. A receiver that loses one asks the sender for a
keyframe (`Encoder::request_keyframe`); until then `Decoder::decode` says
`false`.

## Status

Verified on a Lenovo TB336FU (Android 16) with a device-owner agent: a
`screen-recorder` whole-screen stream encoded by MediaCodec and decoded in
a browser. Not verified: an encoder that refuses the baseline profile (the
fallback path), and camera sources.
