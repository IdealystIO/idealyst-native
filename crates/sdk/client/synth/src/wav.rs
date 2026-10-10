//! RIFF/WAVE encode (16-bit PCM) and decode (int 8/16/24/32, float 32/64).

use media_stream::AudioFormat;

use crate::error::SynthError;
use crate::pcm::Pcm;

const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Encode `pcm` as a canonical 44-byte-header 16-bit PCM WAV.
pub(crate) fn encode(pcm: &Pcm) -> Vec<u8> {
    let channels = pcm.format.channels.max(1);
    let rate = pcm.format.sample_rate;
    let block_align = channels as u32 * 2;
    let data_len = pcm.samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&FORMAT_PCM.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block_align).to_le_bytes()); // byte rate
    out.extend_from_slice(&(block_align as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in &pcm.samples {
        // Scale by 32767 (not 32768) so +1.0 maps to i16::MAX without
        // overflow and the mapping is symmetric around zero.
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    // `data_len` is always even (2 bytes/sample), so no RIFF pad byte.
    out
}

struct Fmt {
    tag: u16,
    channels: u16,
    rate: u32,
    bits: u16,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Decode a WAV file into a [`Pcm`].
pub(crate) fn decode(bytes: &[u8]) -> Result<Pcm, SynthError> {
    let invalid = |m: &str| SynthError::InvalidWav(m.to_string());
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(invalid("missing RIFF/WAVE header"));
    }
    let mut fmt: Option<Fmt> = None;
    let mut data: Option<&[u8]> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(bytes, at + 4) as usize;
        let body_start = at + 8;
        // Streaming writers sometimes leave the size as 0xFFFFFFFF (or just
        // wrong); clamp to what is actually there rather than rejecting.
        let body_end = body_start.saturating_add(size).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(invalid("fmt chunk shorter than 16 bytes"));
                }
                let mut tag = u16_at(body, 0);
                if tag == FORMAT_EXTENSIBLE {
                    // WAVE_FORMAT_EXTENSIBLE: cbSize(2) validBits(2)
                    // channelMask(4) then the SubFormat GUID, whose first two
                    // bytes are the real format tag.
                    if body.len() < 26 {
                        return Err(invalid("extensible fmt chunk too short"));
                    }
                    tag = u16_at(body, 24);
                }
                fmt = Some(Fmt {
                    tag,
                    channels: u16_at(body, 2),
                    rate: u32_at(body, 4),
                    bits: u16_at(body, 14),
                });
            }
            b"data" => data = Some(body),
            _ => {} // LIST, fact, cue, bext, … — not needed to decode samples.
        }
        // Chunks are word-aligned: an odd-sized chunk is followed by a pad byte.
        at = body_start.saturating_add(size).saturating_add(size & 1);
    }
    let fmt = fmt.ok_or_else(|| invalid("no fmt chunk"))?;
    let data = data.ok_or_else(|| invalid("no data chunk"))?;
    if fmt.channels == 0 || fmt.rate == 0 {
        return Err(invalid("zero channels or sample rate"));
    }
    let unsupported = || {
        SynthError::UnsupportedWav(format!("format tag {} at {} bits", fmt.tag, fmt.bits))
    };
    let samples: Vec<f32> = match (fmt.tag, fmt.bits) {
        // 8-bit WAV is unsigned with a 128 midpoint.
        (FORMAT_PCM, 8) => data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        // ÷32767 mirrors `encode`'s ×32767, so our own WAVs round-trip to
        // within half a quantization step; -32768 lands a hair past -1 and
        // is clamped.
        (FORMAT_PCM, 16) => data
            .chunks_exact(2)
            .map(|b| (i16::from_le_bytes([b[0], b[1]]) as f32 / i16::MAX as f32).max(-1.0))
            .collect(),
        (FORMAT_PCM, 24) => data
            .chunks_exact(3)
            .map(|b| {
                // Sign-extend by placing the 3 bytes in the top of an i32.
                let v = i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
                v as f32 / 8_388_608.0
            })
            .collect(),
        (FORMAT_PCM, 32) => data
            .chunks_exact(4)
            .map(|b| (i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64 / 2_147_483_648.0) as f32)
            .collect(),
        (FORMAT_FLOAT, 32) => data
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]).clamp(-1.0, 1.0))
            .collect(),
        (FORMAT_FLOAT, 64) => data
            .chunks_exact(8)
            .map(|b| {
                let mut a = [0u8; 8];
                a.copy_from_slice(b);
                (f64::from_le_bytes(a) as f32).clamp(-1.0, 1.0)
            })
            .collect(),
        _ => return Err(unsupported()),
    };
    Ok(Pcm::new(
        AudioFormat {
            sample_rate: fmt.rate,
            channels: fmt.channels,
        },
        samples,
    ))
}
