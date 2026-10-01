//! The local UTC offset at an instant, from the OS zone database.
//!
//! ```
//! // Seconds to add to UTC to reach local civil time at that instant.
//! let offset = zone_offset::local_offset_seconds_at(1_721_044_800);
//! assert!(offset.abs() <= 24 * 3600);
//! ```
//!
//! One lookup, two users: `runtime-shared`'s native default wall clock
//! (what `runtime_core::time::local_offset_minutes()` reads on every
//! non-web backend) and the `datetime` SDK's `local_offset_at`. See the
//! crate's `Cargo.toml` for why it is a crate of its own.
//!
//! Every call is fresh: on unix `tzset` runs before each `localtime_r`, on
//! Windows the dynamic zone is read per call. A zone change in the system
//! settings mid-process, or a DST transition, shows up on the next call.

#![deny(missing_docs)]

/// Seconds to add to UTC to get local civil time at `unix_seconds`
/// (seconds since `1970-01-01T00:00:00Z`), under the zone rule in force at
/// that instant: a July instant in New York gives `-14400`, a January one
/// `-18000`, and 2006-03-20 gives `-18000` because US DST started in April
/// that year.
///
/// Returns `0` (UTC) when the platform has no zone database reachable
/// (wasm32, an unrecognised OS) or the lookup fails.
pub fn local_offset_seconds_at(unix_seconds: i64) -> i32 {
    imp::offset_seconds_at(unix_seconds)
}

/// The offset in force right now — [`local_offset_seconds_at`] for the
/// current `SystemTime`.
///
/// Native only in practice: `SystemTime::now()` panics on
/// `wasm32-unknown-unknown`, so wasm callers must read their instant from
/// the host (`Date.now()`) and call [`local_offset_seconds_at`] — or, as
/// the web backend does, not call into this crate at all.
pub fn local_offset_seconds_now() -> i32 {
    let secs = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        // Clock set before 1970: the negative distance, rounded toward the
        // past like the positive side.
        Err(e) => {
            let d = e.duration();
            let whole = i64::try_from(d.as_secs()).unwrap_or(i64::MAX);
            -(whole + i64::from(d.subsec_nanos() > 0))
        }
    };
    local_offset_seconds_at(secs)
}

// ---------------------------------------------------------------------------
// unix: libc `localtime_r`
// ---------------------------------------------------------------------------

#[cfg(all(unix, not(target_arch = "wasm32")))]
mod imp {
    /// Unix (Apple, Linux, Android, BSD): `localtime_r` fills `tm_gmtoff`
    /// with the offset the zone database gives for that very instant, so a
    /// historic or future DST rule is applied, not today's.
    pub(crate) fn offset_seconds_at(unix_seconds: i64) -> i32 {
        // POSIX `tzset` (Apple libc, glibc, musl, bionic all export it); the
        // `libc` crate doesn't declare it.
        extern "C" {
            fn tzset();
        }
        // A 32-bit `time_t` (armv7 Android) can't hold every i64 second;
        // clamp to its range — the zone rule at the edge is the best
        // available answer.
        let t: libc::time_t = libc::time_t::try_from(unix_seconds).unwrap_or(if unix_seconds < 0 {
            libc::time_t::MIN
        } else {
            libc::time_t::MAX
        });
        // SAFETY: `tzset` takes no arguments. It is called because POSIX
        // lets `localtime_r` skip it, and only `tzset` re-reads `$TZ` and the
        // system zone — without it, a zone change mid-process (the user
        // moving to another zone in Settings) would go unseen on glibc and
        // Apple libc. On Android, bionic's `tzset` reads the
        // `persist.sys.timezone` property when `$TZ` is unset, which is how
        // the device zone reaches an app process. `localtime_r` writes only
        // into the `tm` we own; a zeroed `tm` is a valid initial value
        // (all-integer struct plus a nullable `tm_zone`).
        unsafe {
            tzset();
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&t, &mut tm).is_null() {
                return 0;
            }
            i32::try_from(tm.tm_gmtoff).unwrap_or(0)
        }
    }
}

// ---------------------------------------------------------------------------
// Windows: `SystemTimeToTzSpecificLocalTimeEx`
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use crate::filetime;
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        FileTimeToSystemTime, GetDynamicTimeZoneInformation, SystemTimeToFileTime,
        SystemTimeToTzSpecificLocalTimeEx, DYNAMIC_TIME_ZONE_INFORMATION,
    };

    /// Convert the UTC instant to local time under the *dynamic* zone
    /// information (which carries the historical DST rules per year — the
    /// non-`Ex` call would apply this year's rule to every year), then
    /// subtract. The tick arithmetic lives in [`filetime`] so it is tested
    /// on every host; only the three Win32 calls are Windows-only.
    pub(crate) fn offset_seconds_at(unix_seconds: i64) -> i32 {
        let Some(ticks) = filetime::ticks_from_unix_seconds(unix_seconds) else {
            // Before 1601 or past FILETIME's range: no rule to apply.
            return 0;
        };
        let to_ft = |t: u64| {
            let (lo, hi) = filetime::split(t);
            FILETIME {
                dwLowDateTime: lo,
                dwHighDateTime: hi,
            }
        };
        let from_ft = |f: &FILETIME| filetime::join(f.dwLowDateTime, f.dwHighDateTime);

        // SAFETY: every pointer is to a local we own, sized for the call;
        // the zone struct and SYSTEMTIMEs are plain-old-data, valid when
        // zeroed.
        unsafe {
            let mut tz: DYNAMIC_TIME_ZONE_INFORMATION = std::mem::zeroed();
            // Returns TIME_ZONE_ID_INVALID (u32::MAX) on failure.
            if GetDynamicTimeZoneInformation(&mut tz) == u32::MAX {
                return 0;
            }
            let utc_ft = to_ft(ticks);
            let mut utc: SYSTEMTIME = std::mem::zeroed();
            if FileTimeToSystemTime(&utc_ft, &mut utc) == 0 {
                return 0;
            }
            let mut local: SYSTEMTIME = std::mem::zeroed();
            if SystemTimeToTzSpecificLocalTimeEx(&tz, &utc, &mut local) == 0 {
                return 0;
            }
            // Both sides go back through SYSTEMTIME so they carry the same
            // millisecond truncation; their difference is the offset exactly.
            let mut local_ft: FILETIME = std::mem::zeroed();
            let mut utc_ms_ft: FILETIME = std::mem::zeroed();
            if SystemTimeToFileTime(&local, &mut local_ft) == 0
                || SystemTimeToFileTime(&utc, &mut utc_ms_ft) == 0
            {
                return 0;
            }
            filetime::offset_seconds(from_ft(&utc_ms_ft), from_ft(&local_ft))
        }
    }
}

/// FILETIME tick arithmetic for the Windows arm, compiled on every target.
///
/// Pure on purpose: Windows' zone APIs read the registry and ignore `$TZ`,
/// and this repo's CI host is macOS, so the Win32 path cannot be driven
/// with a chosen zone in a test. Everything around the three Win32 calls —
/// the epoch shift, the 100 ns scaling, the range edges, the low/high word
/// split, the difference → seconds conversion — lives here and is unit
/// tested on the host; the Win32 calls themselves are covered by
/// `cargo check --target x86_64-pc-windows-gnu`.
#[cfg_attr(not(windows), allow(dead_code))]
mod filetime {
    /// FILETIME counts 100 ns ticks since 1601-01-01; this many seconds
    /// separate that epoch from Unix's.
    pub(crate) const UNIX_TO_FILETIME_SECONDS: i64 = 11_644_473_600;
    /// 100 ns ticks per second.
    pub(crate) const TICKS_PER_SECOND: i64 = 10_000_000;

    /// The FILETIME tick count for a Unix instant, or `None` before 1601
    /// (FILETIME is unsigned) or on overflow.
    pub(crate) fn ticks_from_unix_seconds(unix_seconds: i64) -> Option<u64> {
        unix_seconds
            .checked_add(UNIX_TO_FILETIME_SECONDS)
            .and_then(|s| s.checked_mul(TICKS_PER_SECOND))
            .and_then(|t| u64::try_from(t).ok())
    }

    /// `(dwLowDateTime, dwHighDateTime)`.
    pub(crate) fn split(ticks: u64) -> (u32, u32) {
        (ticks as u32, (ticks >> 32) as u32)
    }

    /// Inverse of [`split`].
    pub(crate) fn join(low: u32, high: u32) -> u64 {
        (u64::from(high) << 32) | u64::from(low)
    }

    /// Local minus UTC, in whole seconds; `0` if the difference can't be
    /// an offset (out of `i32`).
    pub(crate) fn offset_seconds(utc_ticks: u64, local_ticks: u64) -> i32 {
        let diff = local_ticks as i64 - utc_ticks as i64;
        i32::try_from(diff / TICKS_PER_SECOND).unwrap_or(0)
    }
}

/// No zone database reachable (wasm32, an unrecognised OS): local time is
/// UTC.
#[cfg(not(any(all(unix, not(target_arch = "wasm32")), windows)))]
mod imp {
    pub(crate) fn offset_seconds_at(_unix_seconds: i64) -> i32 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::filetime::*;

    #[test]
    fn filetime_epoch_shift_and_scale() {
        // The Unix epoch is 116444736000000000 ticks after 1601-01-01 —
        // the constant every Win32 time conversion uses.
        assert_eq!(ticks_from_unix_seconds(0), Some(116_444_736_000_000_000));
        // 1601-01-01 itself is tick 0; one second earlier has no FILETIME.
        assert_eq!(ticks_from_unix_seconds(-UNIX_TO_FILETIME_SECONDS), Some(0));
        assert_eq!(ticks_from_unix_seconds(-UNIX_TO_FILETIME_SECONDS - 1), None);
        assert_eq!(ticks_from_unix_seconds(i64::MAX), None);
    }

    #[test]
    fn filetime_word_split_round_trips() {
        let t = ticks_from_unix_seconds(1_721_044_800).unwrap();
        let (lo, hi) = split(t);
        assert_ne!(hi, 0, "a 2024 instant needs the high word");
        assert_eq!(join(lo, hi), t);
    }

    #[test]
    fn filetime_difference_is_the_offset() {
        let utc = ticks_from_unix_seconds(1_705_320_000).unwrap();
        let est = ticks_from_unix_seconds(1_705_320_000 - 5 * 3600).unwrap();
        let ist = ticks_from_unix_seconds(1_705_320_000 + 5 * 3600 + 30 * 60).unwrap();
        assert_eq!(offset_seconds(utc, est), -18_000);
        assert_eq!(offset_seconds(utc, ist), 19_800);
        assert_eq!(offset_seconds(utc, utc), 0);
    }
}
