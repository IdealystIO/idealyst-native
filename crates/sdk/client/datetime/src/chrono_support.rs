//! `chrono` conversions (the `chrono` feature).
//!
//! chrono is used with no default features: it does the calendar math, this
//! crate reads the clock. The `From` impls let an app holding chrono values
//! move between the two freely.

use chrono::{DateTime, FixedOffset, TimeZone, Utc};

use crate::{local_offset_at, now_utc, Timestamp, UtcOffset};

/// The current instant as a chrono UTC datetime — the replacement for
/// `chrono::Utc::now()`.
pub fn now_chrono_utc() -> DateTime<Utc> {
    now_utc().into()
}

/// The current local date and time, with the local offset in effect right
/// now — the replacement for `chrono::Local::now()`.
///
/// Returns `DateTime<FixedOffset>` rather than `DateTime<Local>`: chrono's
/// `Local` zone is its own clock-reading type (the `clock` feature), the very
/// thing this crate replaces. Every accessor an app calls on
/// `Local::now()` — `.date_naive()`, `.time()`, `.naive_local()`,
/// `.offset()`, `.format(..)` — exists on this type with the same meaning.
pub fn now_chrono_local() -> DateTime<FixedOffset> {
    to_chrono_local(now_utc())
}

/// `at` in the local zone, with the offset the zone had **at that instant**
/// (its DST state then, not now) — the replacement for
/// `dt.with_timezone(&chrono::Local)`.
pub fn to_chrono_local(at: Timestamp) -> DateTime<FixedOffset> {
    let offset: FixedOffset = local_offset_at(at).into();
    DateTime::<Utc>::from(at).with_timezone(&offset)
}

impl From<Timestamp> for DateTime<Utc> {
    /// Exact for every instant chrono can represent (±262,000 years);
    /// saturates to `DateTime::<Utc>::MIN_UTC` / `MAX_UTC` beyond that.
    fn from(t: Timestamp) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp_micros(t.unix_micros()).unwrap_or(if t.unix_micros() < 0 {
            DateTime::<Utc>::MIN_UTC
        } else {
            DateTime::<Utc>::MAX_UTC
        })
    }
}

impl<Tz: TimeZone> From<DateTime<Tz>> for Timestamp {
    /// The instant a chrono datetime names, whatever its zone. Truncates
    /// sub-microsecond precision.
    fn from(dt: DateTime<Tz>) -> Timestamp {
        Timestamp::from_unix_micros(dt.timestamp_micros())
    }
}

impl From<UtcOffset> for FixedOffset {
    fn from(o: UtcOffset) -> FixedOffset {
        // Every `UtcOffset` is inside ±24 h, exactly `FixedOffset`'s range.
        FixedOffset::east_opt(o.seconds()).expect("UtcOffset is always within ±24 h")
    }
}

impl From<FixedOffset> for UtcOffset {
    fn from(o: FixedOffset) -> UtcOffset {
        UtcOffset::from_seconds(o.local_minus_utc()).expect("FixedOffset is always within ±24 h")
    }
}
