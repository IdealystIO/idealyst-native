//! Platform-free decisions of the Android (AAudio) backend: API-level
//! gating, the open-attempt order, buffer sizing, latency math, reopen
//! back-off and the `i16` sample conversion.
//!
//! Kept apart from `android.rs` (which only compiles for Android) so every
//! rule here is unit-tested on the host by `cargo test -p audio-engine`.
//! `android.rs` asserts at compile time that the AAudio constants mirrored
//! below equal the NDK header values (`ndk-sys`).

// On the host only the tests use these.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::time::Duration;

/// AAudio ships in Android 8.0 (API 26), but the engine uses it from 8.1
/// (API 27): 8.0's AAudio has known defects, and Google's own Oboe library
/// only selects AAudio from API 27 for that reason (falling back to OpenSL
/// ES, which this engine does not implement). Below the gate the engine
/// reports `EngineError::Unsupported` — an honest "no engine audio" rather
/// than glitchy audio on 8.0. (`libaaudio.so` is still loaded with `dlopen`,
/// so the app itself loads on every API level.)
pub(crate) const AAUDIO_MIN_API: u32 = 27;

/// `AAudioStreamBuilder_setUsage` / `setContentType` arrived in Android 9
/// (API 28). On 26/27 the stream gets AAudio's defaults (usage MEDIA,
/// content type MUSIC).
pub(crate) const USAGE_ATTRS_MIN_API: u32 = 28;

/// `AAUDIO_FORMAT_PCM_I16` (mirrored; checked against `ndk-sys` on Android).
pub(crate) const FORMAT_PCM_I16: i32 = 1;
/// `AAUDIO_FORMAT_PCM_FLOAT` (mirrored; checked against `ndk-sys` on Android).
pub(crate) const FORMAT_PCM_FLOAT: i32 = 2;

/// How many device bursts the stream buffers. Two = double buffering: one
/// burst playing while the next is rendered — the AAudio docs' and Oboe's
/// recommended starting point for low latency. Fewer underruns than one
/// burst, half the latency of AAudio's default (usually the full capacity).
pub(crate) const BUFFER_BURSTS: i32 = 2;

/// The channel count we ask for. The mixer is stereo inside; AAudio
/// up/down-mixes to the device if it differs, and the callback renders
/// whatever count the opened stream reports.
pub(crate) const CHANNELS: i32 = 2;

/// Sample format of an opened stream, as the data callback writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SampleFormat {
    F32,
    I16,
}

impl SampleFormat {
    pub(crate) fn to_aaudio(self) -> i32 {
        match self {
            SampleFormat::F32 => FORMAT_PCM_FLOAT,
            SampleFormat::I16 => FORMAT_PCM_I16,
        }
    }

    /// The format an opened stream reports; `None` = one we don't write
    /// (we only ever request float or i16, so this means a broken device).
    pub(crate) fn from_aaudio(v: i32) -> Option<SampleFormat> {
        match v {
            FORMAT_PCM_FLOAT => Some(SampleFormat::F32),
            FORMAT_PCM_I16 => Some(SampleFormat::I16),
            _ => None,
        }
    }
}

/// Requested sharing mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sharing {
    /// The MMAP fast path: the app writes straight into the device buffer,
    /// bypassing the system mixer. Lowest latency where supported.
    Exclusive,
    Shared,
}

/// The order `openStream` is tried in. Exclusive first (lowest latency);
/// AAudio may already hand back a shared stream for an exclusive request,
/// but on some devices the exclusive open *fails* instead, so shared is
/// tried explicitly next. Float before i16 because the mixer renders float
/// (i16 costs a conversion pass and 8 bits of headroom); i16 is the last
/// resort for an implementation that rejects float output.
pub(crate) const OPEN_ATTEMPTS: [(Sharing, SampleFormat); 3] = [
    (Sharing::Exclusive, SampleFormat::F32),
    (Sharing::Shared, SampleFormat::F32),
    (Sharing::Shared, SampleFormat::I16),
];

/// Parse `ro.build.version.sdk` (the bytes `__system_property_get` wrote,
/// possibly NUL-terminated). `None` = unreadable.
pub(crate) fn parse_sdk_level(raw: &[u8]) -> Option<u32> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    std::str::from_utf8(&raw[..end]).ok()?.trim().parse().ok()
}

/// Whether to try AAudio at all. An unreadable level (`None`) still tries —
/// the `dlopen` of `libaaudio.so` is the real gate and fails cleanly below
/// API 27 (see [`AAUDIO_MIN_API`]).
pub(crate) fn aaudio_allowed(api: Option<u32>) -> bool {
    api.is_none_or(|a| a >= AAUDIO_MIN_API)
}

/// Whether to call the API-28 usage / content-type setters: the symbols
/// must exist (resolved by `dlsym`), and a readable level must be ≥ 28.
pub(crate) fn usage_attrs_allowed(api: Option<u32>, symbols_present: bool) -> bool {
    symbols_present && api.is_none_or(|a| a >= USAGE_ATTRS_MIN_API)
}

/// The buffer size to set after opening: [`BUFFER_BURSTS`] bursts, clamped
/// to the stream's capacity. `None` when the stream reports no usable burst
/// (keep AAudio's default).
pub(crate) fn target_buffer_frames(frames_per_burst: i32, capacity: i32) -> Option<i32> {
    if frames_per_burst <= 0 {
        return None;
    }
    let want = frames_per_burst.saturating_mul(BUFFER_BURSTS);
    Some(if capacity > 0 { want.min(capacity) } else { want })
}

/// Output latency from an `AAudioStream_getTimestamp` sample (Oboe's
/// `calculateLatencyMillis`): the hardware presented frame `hw_frame` at
/// `hw_time_ns`; the app has written up to `frames_written` by `now_ns`
/// (same clock). The last written frame will be presented
/// `(frames_written − hw_frame) / rate` after `hw_time_ns`; latency is that
/// instant minus now. `None` for a nonsensical (negative) result or rate 0.
pub(crate) fn latency_from_timestamp(
    frames_written: i64,
    hw_frame: i64,
    hw_time_ns: i64,
    now_ns: i64,
    sample_rate: u32,
) -> Option<Duration> {
    if sample_rate == 0 {
        return None;
    }
    let delta_frames = frames_written.checked_sub(hw_frame)? as i128;
    let delta_ns = delta_frames * 1_000_000_000 / sample_rate as i128;
    let presented_at = hw_time_ns as i128 + delta_ns;
    let latency = presented_at - now_ns as i128;
    (latency >= 0).then(|| Duration::from_nanos(latency as u64))
}

/// Fallback estimate before the stream runs (no timestamp yet): the time
/// the buffered frames take to play.
pub(crate) fn latency_from_buffer(buffer_frames: i32, sample_rate: u32) -> Option<Duration> {
    (buffer_frames > 0 && sample_rate > 0)
        .then(|| Duration::from_secs_f64(buffer_frames as f64 / sample_rate as f64))
}

/// Delay before reopen attempt `attempt` (0-based) after a disconnect, or
/// `None` = give up. The first try is immediate (a headphone unplug has a
/// speaker ready at once); later ones back off for routes that take a
/// moment to appear (Bluetooth). Five tries over ~1.5 s.
pub(crate) fn reopen_delay(attempt: u32) -> Option<Duration> {
    const DELAYS_MS: [u64; 5] = [0, 100, 200, 400, 800];
    DELAYS_MS.get(attempt as usize).map(|&ms| Duration::from_millis(ms))
}

/// Float → i16 for an i16 stream: clamp to [-1, 1], scale by 32767, round.
/// (The mixer already clamps; the clamp here keeps NaN/overflow safe.)
#[inline]
pub(crate) fn f32_to_i16(s: f32) -> i16 {
    if s.is_nan() {
        return 0;
    }
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_level_parses_property_bytes() {
        assert_eq!(parse_sdk_level(b"34\0\0\0\0"), Some(34));
        assert_eq!(parse_sdk_level(b"21"), Some(21));
        assert_eq!(parse_sdk_level(b" 28 \0"), Some(28));
        assert_eq!(parse_sdk_level(b"\0"), None);
        assert_eq!(parse_sdk_level(b""), None);
        assert_eq!(parse_sdk_level(b"Q\0"), None);
    }

    #[test]
    fn aaudio_is_gated_at_api_27() {
        assert!(!aaudio_allowed(Some(21)));
        assert!(!aaudio_allowed(Some(25)));
        // 8.0's AAudio is skipped, as Oboe does.
        assert!(!aaudio_allowed(Some(26)));
        assert!(aaudio_allowed(Some(27)));
        assert!(aaudio_allowed(Some(34)));
        // Unknown level: dlopen decides.
        assert!(aaudio_allowed(None));
    }

    #[test]
    fn usage_attrs_need_api_28_and_the_symbols() {
        assert!(!usage_attrs_allowed(Some(26), true));
        assert!(!usage_attrs_allowed(Some(27), true));
        assert!(usage_attrs_allowed(Some(28), true));
        assert!(!usage_attrs_allowed(Some(30), false));
        assert!(usage_attrs_allowed(None, true));
        assert!(!usage_attrs_allowed(None, false));
    }

    #[test]
    fn open_attempts_prefer_exclusive_float_and_end_on_shared_i16() {
        assert_eq!(OPEN_ATTEMPTS[0], (Sharing::Exclusive, SampleFormat::F32));
        assert_eq!(OPEN_ATTEMPTS[1], (Sharing::Shared, SampleFormat::F32));
        assert_eq!(OPEN_ATTEMPTS[2], (Sharing::Shared, SampleFormat::I16));
    }

    #[test]
    fn formats_round_trip_and_unknown_is_rejected() {
        for f in [SampleFormat::F32, SampleFormat::I16] {
            assert_eq!(SampleFormat::from_aaudio(f.to_aaudio()), Some(f));
        }
        assert_eq!(SampleFormat::from_aaudio(3), None); // PCM_I24_PACKED
        assert_eq!(SampleFormat::from_aaudio(0), None); // UNSPECIFIED
    }

    #[test]
    fn buffer_is_two_bursts_clamped_to_capacity() {
        assert_eq!(target_buffer_frames(192, 3072), Some(384));
        assert_eq!(target_buffer_frames(96, 4096), Some(192));
        assert_eq!(target_buffer_frames(2048, 3000), Some(3000));
        assert_eq!(target_buffer_frames(192, 0), Some(384));
        assert_eq!(target_buffer_frames(0, 3072), None);
        assert_eq!(target_buffer_frames(-1, 3072), None);
    }

    #[test]
    fn latency_from_timestamp_matches_oboe_formula() {
        // 48 kHz; the app is 480 frames (10 ms) ahead of the frame the
        // hardware presented 2 ms ago → the newest frame plays in 8 ms.
        let now = 1_000_000_000;
        let d = latency_from_timestamp(10_480, 10_000, now - 2_000_000, now, 48_000).unwrap();
        assert_eq!(d, Duration::from_millis(8));
        // Presentation timestamp in the future (common for MMAP).
        let d = latency_from_timestamp(1_000, 1_000, now + 3_000_000, now, 48_000).unwrap();
        assert_eq!(d, Duration::from_millis(3));
        // Stale timestamp: newest frame already "played" → nonsense.
        assert_eq!(latency_from_timestamp(1_000, 1_000, now - 5_000_000, now, 48_000), None);
        assert_eq!(latency_from_timestamp(1, 0, now, now, 0), None);
    }

    #[test]
    fn latency_from_buffer_is_buffer_duration() {
        assert_eq!(latency_from_buffer(480, 48_000), Some(Duration::from_millis(10)));
        assert_eq!(latency_from_buffer(0, 48_000), None);
        assert_eq!(latency_from_buffer(480, 0), None);
    }

    #[test]
    fn reopen_backs_off_then_gives_up() {
        assert_eq!(reopen_delay(0), Some(Duration::ZERO));
        let delays: Vec<_> = (0..10).map_while(reopen_delay).collect();
        assert_eq!(delays.len(), 5);
        assert!(delays.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(reopen_delay(5), None);
    }

    #[test]
    fn i16_conversion_clamps_and_rounds() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), 32767);
        assert_eq!(f32_to_i16(-1.0), -32767);
        assert_eq!(f32_to_i16(2.0), 32767);
        assert_eq!(f32_to_i16(-3.0), -32767);
        assert_eq!(f32_to_i16(0.5), 16384); // 16383.5 rounds away from zero
        assert_eq!(f32_to_i16(f32::NAN), 0);
    }
}
