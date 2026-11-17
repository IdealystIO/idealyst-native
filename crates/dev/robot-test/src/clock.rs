//! `app.clock()`: set, freeze, advance or re-zone the app's wall clock —
//! the robot's `clock.*` functions, typed. Both clocks an app reads follow
//! it (the framework's and the `datetime` SDK's); timers keep real time,
//! and the app's server keeps its own clock.
//!
//! ```ignore
//! app.clock().set_utc("2026-11-17T22:30:00Z").freeze();
//! app.test_id("check-in").click();          // a late check-in
//! app.clock().advance(Duration::from_secs(2 * 3600)); // past midnight
//! app.clock().clear();
//! ```

use std::time::Duration;

use serde_json::json;

use crate::App;

/// The app's wall clock, through the robot. From [`App::clock`].
pub struct Clock<'a> {
    pub(crate) app: &'a mut App,
}

impl Clock<'_> {
    /// The clock reads `epoch_ms` (ms since 1970 UTC) now and runs on.
    pub fn set_epoch_ms(&mut self, epoch_ms: i64) -> &mut Self {
        self.app.call_function("clock.set", json!({ "epoch_ms": epoch_ms }));
        self
    }

    /// The clock reads this UTC instant now and runs on:
    /// `"2026-11-17T22:30:00Z"` (seconds and the `Z` optional).
    pub fn set_utc(&mut self, utc: &str) -> &mut Self {
        let ms = parse_utc(utc).unwrap_or_else(|e| panic!("clock().set_utc({utc:?}): {e}"));
        self.set_epoch_ms(ms)
    }

    /// Stop the clock where it reads.
    pub fn freeze(&mut self) -> &mut Self {
        self.app.call_function("clock.freeze", json!({ "frozen": true }));
        self
    }

    /// Let a frozen clock run on from where it reads.
    pub fn resume(&mut self) -> &mut Self {
        self.app.call_function("clock.freeze", json!({ "frozen": false }));
        self
    }

    /// Move the clock forward by `by`.
    pub fn advance(&mut self, by: Duration) -> &mut Self {
        self.advance_ms(by.as_millis() as i64)
    }

    /// Move the clock `ms` milliseconds (negative: back).
    pub fn advance_ms(&mut self, ms: i64) -> &mut Self {
        self.app.call_function("clock.advance", json!({ "ms": ms }));
        self
    }

    /// Report `offset_minutes` (minutes to add to UTC: `-420` is UTC−7) as
    /// the device's zone.
    pub fn zone(&mut self, offset_minutes: i32) -> &mut Self {
        self.app.call_function("clock.zone", json!({ "offset_minutes": offset_minutes }));
        self
    }

    /// Back to the real clock and zone.
    pub fn clear(&mut self) -> &mut Self {
        self.app.call_function("clock.clear", json!({}));
        self
    }

    /// What the app's clock reads now, ms since 1970 UTC.
    pub fn now_epoch_ms(&mut self) -> i64 {
        let v = self.app.call_function("clock.now", json!({}));
        v["epoch_ms"].as_i64().unwrap_or_else(|| panic!("clock.now: unexpected shape {v}"))
    }
}

/// `YYYY-MM-DDTHH:MM[:SS][Z]` (UTC) to ms since 1970.
fn parse_utc(s: &str) -> Result<i64, String> {
    let s = s.trim().trim_end_matches('Z');
    let (date, time) = s.split_once(['T', ' ']).ok_or("expected YYYY-MM-DDTHH:MM[:SS]Z")?;
    let num = |p: &str| p.parse::<i64>().map_err(|_| format!("not a number: {p:?}"));
    let d: Vec<&str> = date.split('-').collect();
    let t: Vec<&str> = time.split(':').collect();
    if d.len() != 3 || !(2..=3).contains(&t.len()) {
        return Err("expected YYYY-MM-DDTHH:MM[:SS]Z".into());
    }
    let (y, m, day) = (num(d[0])?, num(d[1])?, num(d[2])?);
    let (h, min, sec) = (num(t[0])?, num(t[1])?, if t.len() == 3 { num(t[2])? } else { 0 });
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || h > 23 || min > 59 || sec > 59 {
        return Err("a field is out of range".into());
    }
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Ok(((days * 24 + h) * 60 + min) * 60_000 + sec * 1000)
}

#[cfg(test)]
mod tests {
    use super::parse_utc;

    #[test]
    fn parses_utc_instants() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Ok(0));
        assert_eq!(parse_utc("2026-11-17T22:30:00Z"), Ok(1_794_954_600_000));
        assert_eq!(parse_utc("2000-02-29T12:00"), Ok(951_825_600_000));
        assert!(parse_utc("2026-13-01T00:00Z").is_err());
        assert!(parse_utc("yesterday").is_err());
    }
}
