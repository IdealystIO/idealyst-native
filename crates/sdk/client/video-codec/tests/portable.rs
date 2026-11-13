//! What holds on every host: the configuration, and the honest refusal of a
//! direction a platform has no codec for.

use video_codec::{Codec, Decoder, Encoder, EncoderConfig, MediaStream, VideoCodecError};

#[test]
fn realtime_is_h264_at_two_and_a_half_megabits_and_thirty_fps() {
    let config = EncoderConfig::realtime();
    assert_eq!(config, EncoderConfig { codec: Codec::H264, bitrate: 2_500_000, fps: 30 });
    assert_eq!(EncoderConfig::default(), config);
    assert_eq!(config.bitrate(800_000).fps(15).bitrate, 800_000);
}

#[cfg(not(target_os = "android"))]
#[test]
fn encoding_is_refused_off_android() {
    let (stream, _writer) = MediaStream::new();
    let err = Encoder::start(&stream, EncoderConfig::realtime(), |_| {}).unwrap_err();
    assert_eq!(err, VideoCodecError::Unsupported("encoding"));
}

#[test]
fn decoding_is_refused_off_the_web() {
    assert_eq!(Decoder::new(Codec::H264).unwrap_err(), VideoCodecError::Unsupported("decoding"));
}
