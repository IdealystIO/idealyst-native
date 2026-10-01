//! Native clock: `std::time::SystemTime` for the instant, the OS zone
//! database for the offset, and `$TZ` / `iana-time-zone` for the zone name.

use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn now_micros() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_micros()).unwrap_or(i64::MAX),
        // A clock set before 1970: report the negative distance rather than
        // clamping to 0, so the civil date stays right.
        Err(e) => i64::try_from(e.duration().as_micros()).map_or(i64::MIN, |m| -m),
    }
}

// ---------------------------------------------------------------------------
// Offset at an instant
// ---------------------------------------------------------------------------

/// Unix (Apple, Linux, Android, BSD): `localtime_r` fills `tm_gmtoff` with
/// the offset the zone database gives for that very instant, so a historic
/// or future DST rule is applied, not today's.
#[cfg(unix)]
pub(crate) fn offset_seconds_at(unix_micros: i64) -> i32 {
    // POSIX `tzset` (Apple libc, glibc, musl, bionic all export it); the
    // `libc` crate doesn't declare it.
    extern "C" {
        fn tzset();
    }
    let seconds = unix_micros.div_euclid(1_000_000);
    // A 32-bit `time_t` (armv7 Android) can't hold every i64 second; clamp
    // to its range — the zone rule at the edge is the best available answer.
    let t: libc::time_t = libc::time_t::try_from(seconds).unwrap_or(if seconds < 0 {
        libc::time_t::MIN
    } else {
        libc::time_t::MAX
    });
    // SAFETY: `tzset` takes no arguments. It is called because POSIX lets
    // `localtime_r` skip it, and only `tzset` re-reads `$TZ` and the system
    // zone — without it, a zone change mid-process (the user moving to
    // another zone in Settings) would go unseen on glibc and Apple libc.
    // `localtime_r` writes only into the `tm` we own; a zeroed `tm` is a
    // valid initial value (all-integer struct plus a nullable `tm_zone`).
    unsafe {
        tzset();
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return 0;
        }
        i32::try_from(tm.tm_gmtoff).unwrap_or(0)
    }
}

/// Windows: convert the UTC instant to local time under the *dynamic* zone
/// information (which carries the historical DST rules per year — the
/// non-`Ex` call would apply this year's rule to every year), then subtract.
#[cfg(windows)]
pub(crate) fn offset_seconds_at(unix_micros: i64) -> i32 {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        FileTimeToSystemTime, GetDynamicTimeZoneInformation, SystemTimeToFileTime,
        SystemTimeToTzSpecificLocalTimeEx, DYNAMIC_TIME_ZONE_INFORMATION,
    };

    // FILETIME counts 100 ns ticks since 1601-01-01; 11_644_473_600 s
    // separate that epoch from Unix's.
    const UNIX_TO_FILETIME_SECONDS: i64 = 11_644_473_600;
    const TICKS_PER_MICRO: i64 = 10;
    const TICKS_PER_SECOND: i64 = 10_000_000;

    let ticks = match unix_micros
        .checked_add(UNIX_TO_FILETIME_SECONDS * 1_000_000)
        .and_then(|m| m.checked_mul(TICKS_PER_MICRO))
    {
        Some(t) if t >= 0 => t as u64,
        // Before 1601 or past FILETIME's range: no rule to apply.
        _ => return 0,
    };
    let to_ft = |t: u64| FILETIME {
        dwLowDateTime: t as u32,
        dwHighDateTime: (t >> 32) as u32,
    };
    let from_ft = |f: &FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;

    // SAFETY: every pointer is to a local we own, sized for the call; the
    // zone struct and SYSTEMTIMEs are plain-old-data, valid when zeroed.
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
        let diff = from_ft(&local_ft) as i64 - from_ft(&utc_ms_ft) as i64;
        i32::try_from(diff / TICKS_PER_SECOND).unwrap_or(0)
    }
}

/// No zone database reachable (e.g. `wasm32-wasi`, an unrecognised OS):
/// local time is UTC.
#[cfg(not(any(unix, windows)))]
pub(crate) fn offset_seconds_at(_unix_micros: i64) -> i32 {
    0
}

// ---------------------------------------------------------------------------
// Zone name
// ---------------------------------------------------------------------------

pub(crate) fn timezone() -> Option<String> {
    // `$TZ` first on unix: it is what `localtime_r` used for the offset, so
    // the name and the offset describe the same zone even when the system
    // setting differs. Windows' zone APIs ignore `$TZ`, so it isn't
    // consulted there.
    #[cfg(unix)]
    if let Some(tz) = std::env::var_os("TZ") {
        return tz.to_str().and_then(iana_from_tz_env).map(str::to_owned);
    }
    iana_time_zone::get_timezone()
        .ok()
        .filter(|s| !s.is_empty())
}

/// The IANA name a `$TZ` value spells, or `None` when it is a POSIX rule
/// string (`EST5EDT,M3.2.0,M11.1.0`, `<+0330>-3:30`) or empty — those
/// describe an offset rule, not a named zone.
///
/// Accepted shapes: `Area/City`, `:Area/City` (glibc's "file" prefix), an
/// absolute path into a `zoneinfo` directory, and the bare `UTC` / `GMT`
/// aliases.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn iana_from_tz_env(value: &str) -> Option<&str> {
    let v = value.strip_prefix(':').unwrap_or(value);
    let v = match v.find("zoneinfo/") {
        Some(i) => &v[i + "zoneinfo/".len()..],
        None if v.starts_with('/') => return None,
        None => v,
    };
    let named = v.contains('/') || matches!(v, "UTC" | "GMT" | "Etc/UTC");
    let clean = !v.is_empty()
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'));
    (named && clean).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::iana_from_tz_env;

    #[test]
    fn tz_env_names_and_rules() {
        assert_eq!(
            iana_from_tz_env("America/New_York"),
            Some("America/New_York")
        );
        assert_eq!(iana_from_tz_env(":Europe/Berlin"), Some("Europe/Berlin"));
        assert_eq!(
            iana_from_tz_env("/usr/share/zoneinfo/Asia/Kolkata"),
            Some("Asia/Kolkata")
        );
        assert_eq!(iana_from_tz_env("Etc/GMT+5"), Some("Etc/GMT+5"));
        assert_eq!(iana_from_tz_env("UTC"), Some("UTC"));
        assert_eq!(iana_from_tz_env("EST5EDT,M3.2.0,M11.1.0"), None);
        assert_eq!(iana_from_tz_env("<+0330>-3:30"), None);
        assert_eq!(iana_from_tz_env("/etc/localtime"), None);
        assert_eq!(iana_from_tz_env(""), None);
    }
}
