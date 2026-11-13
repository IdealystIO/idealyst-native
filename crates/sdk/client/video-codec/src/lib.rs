//! Live video encoding and decoding on the platform's own codecs.
//!
//! An [`Encoder`] takes a capture [`MediaStream`] (`screen-recorder`,
//! `camera`) and emits each encoded frame as it comes out; a [`Decoder`]
//! takes encoded frames and shows them as a [`MediaStream`] the `video` SDK
//! plays. Between them the frames go however the app moves bytes: a socket,
//! QUIC datagrams. Both are realtime, not file tools: no muxing, no
//! buffering for smoothness, keyframes when asked.
//!
//! ```ignore
//! // Sender (Android): the screen, hardware-encoded.
//! let screen = ScreenRecorder::new()
//!     .start(RecordingConfig::new().source(Source::FullScreen).max_edge(1280))
//!     .await?;
//! let encoder = Encoder::start(&screen, EncoderConfig::realtime(), |event| match event {
//!     EncoderEvent::Frame(frame) => send(frame),
//!     EncoderEvent::Ended(why) => log(why),
//!     _ => {}
//! })?;
//! encoder.request_keyframe(); // the receiver lost a frame
//!
//! // Receiver (web): frames in, a stream out.
//! let mut decoder = Decoder::new(Codec::H264)?;
//! if !decoder.decode(&frame) {
//!     ask_for_keyframe();
//! }
//! video::Video(VideoProps { source: video::stream(decoder.stream()), ..Default::default() })
//! ```
//!
//! # Platforms
//!
//! | | Encoder | Decoder |
//! |---|---|---|
//! | Android | MediaCodec, from a stream's `PreviewSource` | — |
//! | web | — | WebCodecs |
//!
//! A dash is [`VideoCodecError::Unsupported`].
//!
//! # Sources
//!
//! The Android encoder takes a stream whose native source is a
//! `media_stream::PreviewSource`: a whole-screen `screen-recorder` capture
//! (verified), or a `camera`. It encodes the buffers as the source renders
//! them, which for a camera is in the sensor's orientation and, for a front
//! camera, mirrored: the receiver turns the picture by the source's
//! `PreviewGeometry::sensor_orientation` itself. A screen capture is
//! upright.
//!
//! # Frame loss
//!
//! H.264 here has no B-frames, so every frame depends on the one before it
//! back to a keyframe. A receiver that loses one cannot decode another until
//! the next keyframe: it tells the sender, which calls
//! [`Encoder::request_keyframe`]. [`Decoder::decode`] says when it is in that
//! state.
#![deny(missing_docs)]

use std::fmt;

pub mod h264;

pub use media_stream::MediaStream;

#[cfg_attr(all(not(target_arch = "wasm32"), target_os = "android"), path = "android.rs")]
#[cfg_attr(not(all(not(target_arch = "wasm32"), target_os = "android")), path = "no_encoder.rs")]
mod encoder_imp;

#[cfg_attr(target_arch = "wasm32", path = "web.rs")]
#[cfg_attr(not(target_arch = "wasm32"), path = "no_decoder.rs")]
mod decoder_imp;

/// A video codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Codec {
    /// H.264 / AVC, Annex B, baseline profile where the encoder allows it.
    H264,
}

/// How to encode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderConfig {
    /// The codec.
    pub codec: Codec,
    /// Bits per second the encoder aims at.
    pub bitrate: u32,
    /// Frames per second the encoder is set up for. A still source sends
    /// fewer (the last frame is repeated at most every quarter second).
    pub fps: u32,
}

impl EncoderConfig {
    /// H.264 at 2.5 Mb/s and 30 fps: legible text on a tablet-sized screen
    /// scaled to 1280 pixels, on any uplink a site has.
    pub fn realtime() -> Self {
        EncoderConfig { codec: Codec::H264, bitrate: 2_500_000, fps: 30 }
    }

    /// Set the bitrate, in bits per second.
    pub fn bitrate(mut self, bits_per_second: u32) -> Self {
        self.bitrate = bits_per_second;
        self
    }

    /// Set the frame rate.
    pub fn fps(mut self, fps: u32) -> Self {
        self.fps = fps;
        self
    }
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self::realtime()
    }
}

/// One encoded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedFrame {
    /// The frame's data. H.264: Annex B, and a keyframe opens with its SPS
    /// and PPS, so a decoder can start at any keyframe.
    pub data: Vec<u8>,
    /// Presentation time, in microseconds.
    pub pts_us: u64,
    /// A keyframe: decodable without the frames before it.
    pub key: bool,
}

/// What an [`Encoder`] reports, on its own thread.
#[derive(Debug)]
#[non_exhaustive]
pub enum EncoderEvent {
    /// A frame came out of the encoder.
    Frame(EncodedFrame),
    /// The encoder stopped without being asked to (a codec error, the
    /// source going away). No more events follow.
    Ended(String),
}

/// Why encoding or decoding could not start.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum VideoCodecError {
    /// Not on this platform (yet), for the named direction.
    #[error("video {0} is not supported on this platform")]
    Unsupported(&'static str),
    /// The stream offers no surface to encode from: it is not a capture that
    /// renders into one (on Android, a stream whose native source is not a
    /// `PreviewSource`).
    #[error("this stream has no surface source to encode from")]
    NoSurface,
    /// Another consumer holds the stream's surface source.
    #[error("another consumer holds this stream's surface")]
    InUse,
    /// The platform refused.
    #[error("video codec failed: {0}")]
    Platform(String),
}

/// A running encoder. Dropping it stops encoding and gives the stream's
/// surface back.
pub struct Encoder {
    inner: encoder_imp::Encoder,
    size: (u32, u32),
}

impl Encoder {
    /// Encode `stream` from now on. `on_event` is called on the encoder's own
    /// thread for each frame, so it must be quick and `Send`: hand the frame
    /// to whatever sends it.
    ///
    /// The encoder takes the stream's surface source, so frames go from the
    /// capture to the encoder without being copied; while it holds it, the
    /// stream's CPU subscribers and any display of the same stream get no
    /// frames. Keep `stream` alive as long as the encoder: dropping its last
    /// clone stops the capture.
    pub fn start(
        stream: &MediaStream,
        config: EncoderConfig,
        on_event: impl Fn(EncoderEvent) + Send + Sync + 'static,
    ) -> Result<Encoder, VideoCodecError> {
        let (inner, size) = encoder_imp::start(stream, config, Box::new(on_event))?;
        Ok(Encoder { inner, size })
    }

    /// The encoded picture's size, in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Make the next frame a keyframe.
    pub fn request_keyframe(&self) {
        self.inner.request_keyframe();
    }
}

impl fmt::Debug for Encoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encoder").field("size", &self.size).finish_non_exhaustive()
    }
}

/// A running decoder, showing what it decodes as a [`MediaStream`].
pub struct Decoder {
    inner: decoder_imp::Decoder,
}

impl Decoder {
    /// A decoder for `codec`. It is set up from the first keyframe, which
    /// names the profile and size.
    pub fn new(codec: Codec) -> Result<Decoder, VideoCodecError> {
        Ok(Decoder { inner: decoder_imp::Decoder::new(codec)? })
    }

    /// Decode `frame` and show it. `false` when it could not be decoded and
    /// nothing can be until a keyframe: the decoder errored, or this is a
    /// delta frame and it has had no keyframe yet. Ask the sender for one.
    pub fn decode(&mut self, frame: &EncodedFrame) -> bool {
        self.inner.decode(frame)
    }

    /// What the decoder shows: hand it to the `video` SDK.
    pub fn stream(&self) -> MediaStream {
        self.inner.stream()
    }

    /// The decoded picture's size, once a frame has been shown.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.inner.size()
    }
}

impl fmt::Debug for Decoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decoder").field("size", &self.size()).finish_non_exhaustive()
    }
}

/// The boxed event sink the backends hold.
pub(crate) type EventSink = Box<dyn Fn(EncoderEvent) + Send + Sync>;
