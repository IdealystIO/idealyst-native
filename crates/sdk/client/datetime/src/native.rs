//! Native clock: `std::time::SystemTime` for the instant, the OS zone
//! database (via `zone-offset`) for the offset, and `$TZ` / `iana-time-zone` for the zone name.

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

/// The zone database's offset at that instant (historic and future DST
/// rules applied), through `zone-offset` — libc `localtime_r` on unix
/// (Apple, Linux, Android, BSD), `SystemTimeToTzSpecificLocalTimeEx` on
/// Windows, UTC elsewhere. The same lookup backs the runtime's native wall
/// clock (`runtime_core::time::local_offset_minutes()`), so this SDK and
/// the framework's own date UI never disagree about the offset.
pub(crate) fn offset_seconds_at(unix_micros: i64) -> i32 {
    // Zone offsets are whole seconds, so flooring to the second (toward the
    // past, also for pre-1970 instants) cannot change the answer.
    zone_offset::local_offset_seconds_at(unix_micros.div_euclid(1_000_000))
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
