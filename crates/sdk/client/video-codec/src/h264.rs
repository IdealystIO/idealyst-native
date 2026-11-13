//! What a decoder needs to know about an H.264 stream, read from the stream.

/// The WebCodecs codec string (`avc1.PPCCLL`: profile, constraint flags and
/// level) for an Annex B frame that carries a sequence parameter set, as every
/// keyframe from [`crate::Encoder`] does. `None` when the frame has none.
pub fn codec_string(frame: &[u8]) -> Option<String> {
    nal_units(frame)
        .find(|nal| nal.len() >= 4 && nal[0] & 0x1f == NAL_SPS)
        .map(|sps| format!("avc1.{:02X}{:02X}{:02X}", sps[1], sps[2], sps[3]))
}

/// Whether an Annex B frame opens with a sequence parameter set.
pub fn starts_with_sps(frame: &[u8]) -> bool {
    nal_units(frame).next().is_some_and(|nal| nal.first().is_some_and(|b| b & 0x1f == NAL_SPS))
}

/// `nal_unit_type` of a sequence parameter set.
const NAL_SPS: u8 = 7;

/// The NAL units of an Annex B stream, start codes removed.
pub fn nal_units(stream: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .map(|&start| {
            // The next unit's start code, less the leading zero of a
            // four-byte one.
            let code = start - 3;
            if code > 0 && stream[code - 1] == 0 {
                code - 1
            } else {
                code
            }
        })
        .chain(std::iter::once(stream.len()))
        .collect();
    starts.into_iter().zip(ends).map(move |(start, end)| &stream[start..end.max(start)])
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYFRAME: [u8; 20] =
        [0, 0, 0, 1, 0x67, 0x42, 0xC0, 0x28, 0xAA, 0, 0, 0, 1, 0x68, 0xCE, 0, 0, 1, 0x65, 0x88];

    #[test]
    fn reads_profile_and_level_from_the_sps() {
        assert_eq!(codec_string(&KEYFRAME).as_deref(), Some("avc1.42C028"));
    }

    #[test]
    fn a_frame_without_an_sps_has_no_codec_string() {
        assert_eq!(codec_string(&[0, 0, 0, 1, 0x41, 0x9A, 0x00]), None);
        assert_eq!(codec_string(&[]), None);
    }

    #[test]
    fn splits_on_three_and_four_byte_start_codes() {
        let units: Vec<&[u8]> = nal_units(&KEYFRAME).collect();
        assert_eq!(units, vec![&[0x67, 0x42, 0xC0, 0x28, 0xAA][..], &[0x68, 0xCE][..], &[0x65, 0x88][..]]);
    }

    #[test]
    fn knows_a_keyframe_that_carries_its_parameter_sets() {
        assert!(starts_with_sps(&KEYFRAME));
        assert!(!starts_with_sps(&[0, 0, 1, 0x65, 0x88]));
    }
}
