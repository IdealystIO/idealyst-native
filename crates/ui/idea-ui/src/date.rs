//! Civil date/time core for the date components ([`Calendar`],
//! [`DatePicker`], [`DateInput`], …).
//!
//! [`CivilDate`] / [`CivilTime`] / [`CivilDateTime`] are plain
//! timezone-less calendar values (proleptic Gregorian) — exactly what a
//! form control edits. There is deliberately **no dependency on
//! `chrono`/`time`**: the components need day arithmetic, weekday
//! computation, and token formatting, and carrying a full datetime
//! library into every wasm bundle for that is the wrong trade. The
//! day-count conversions use Howard Hinnant's `days_from_civil` /
//! `civil_from_days` algorithms (exact over the full `i32` year range).
//!
//! "Today" comes from the runtime's wall-clock seam
//! (`runtime_core::time::epoch_millis` + `local_offset_minutes`),
//! installed per backend at mount — `js Date` on web, `NSTimeZone` on
//! macOS, UTC `SystemTime` elsewhere. Before a source is installed the
//! epoch reads `0` and [`CivilDate::today`] reports 1970-01-01; in
//! practice components build after mount, where a source is present.
//!
//! Formatting/parsing is token-based ([`format_date`], [`parse_date`],
//! …): `YYYY MM M DD D` for dates, `HH H hh h mm ss A a` for times,
//! any other character a literal. Parsing is lenient about digit width
//! (`M` and `MM` both accept `3` or `03`) so typed input like
//! `3/4/2026` round-trips; formatting is strict (`MM` zero-pads).
//! Month/weekday display names live in [`DateLabels`] — English by
//! default, replaceable per call site for i18n.

use std::rc::Rc;

use runtime_core::IdealystSchema;

// ---------------------------------------------------------------------------
// Weekday
// ---------------------------------------------------------------------------

/// Day of the week, Monday-first (ISO 8601). `as_index` is the
/// Monday-based ordinal used to index [`DateLabels::weekdays_short`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, IdealystSchema)]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    /// Monday = 0 … Sunday = 6.
    pub fn as_index(self) -> u8 {
        match self {
            Weekday::Monday => 0,
            Weekday::Tuesday => 1,
            Weekday::Wednesday => 2,
            Weekday::Thursday => 3,
            Weekday::Friday => 4,
            Weekday::Saturday => 5,
            Weekday::Sunday => 6,
        }
    }

    /// Inverse of [`as_index`](Self::as_index), modulo 7.
    pub fn from_index(i: u8) -> Self {
        match i % 7 {
            0 => Weekday::Monday,
            1 => Weekday::Tuesday,
            2 => Weekday::Wednesday,
            3 => Weekday::Thursday,
            4 => Weekday::Friday,
            5 => Weekday::Saturday,
            _ => Weekday::Sunday,
        }
    }

    /// The day `n` places after `self` (wraps).
    pub fn add(self, n: u8) -> Self {
        Weekday::from_index(self.as_index() + (n % 7))
    }
}

// ---------------------------------------------------------------------------
// CivilDate
// ---------------------------------------------------------------------------

/// A timezone-less calendar date (proleptic Gregorian). Fields are
/// public but only ever valid — construct via [`CivilDate::new`] /
/// [`CivilDate::from_days`], which validate/normalize.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, IdealystSchema)]
pub struct CivilDate {
    /// Calendar year (e.g. `2026`).
    pub year: i32,
    /// 1-based month, `1..=12`.
    pub month: u8,
    /// 1-based day of month, `1..=days_in_month`.
    pub day: u8,
}

impl CivilDate {
    /// Validated constructor — `None` for an impossible date
    /// (`2026-02-30`, month `0`/`13`, …).
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        if !(1..=12).contains(&month) {
            return None;
        }
        if day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self { year, month, day })
    }

    /// Days since the Unix epoch (`1970-01-01` = 0; negative before).
    /// Hinnant's `days_from_civil`, exact for all `i32` years.
    pub fn to_days(self) -> i64 {
        let y = i64::from(self.year) - i64::from(self.month <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400; // [0, 399]
        let m = i64::from(self.month);
        let d = i64::from(self.day);
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1; // [0, 365]
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
        era * 146_097 + doe - 719_468
    }

    /// Inverse of [`to_days`](Self::to_days) — Hinnant's
    /// `civil_from_days`.
    pub fn from_days(days: i64) -> Self {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097; // [0, 146096]
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
        let mp = (5 * doy + 2) / 153; // [0, 11]
        let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
        let m = mp + if mp < 10 { 3 } else { -9 }; // [1, 12]
        Self {
            year: (y + i64::from(m <= 2)) as i32,
            month: m as u8,
            day: d as u8,
        }
    }

    /// Weekday of this date. Day 0 (`1970-01-01`) was a Thursday.
    pub fn weekday(self) -> Weekday {
        Weekday::from_index(((self.to_days() + 3).rem_euclid(7)) as u8)
    }

    /// This date shifted by `n` days (negative = backwards).
    pub fn add_days(self, n: i64) -> Self {
        Self::from_days(self.to_days() + n)
    }

    /// This date shifted by `n` calendar months, day-of-month clamped
    /// into the target month (`Jan 31 + 1` → `Feb 28`/`29`) — the
    /// behavior a month-nav header needs.
    pub fn add_months(self, n: i32) -> Self {
        let total = i64::from(self.year) * 12 + i64::from(self.month) - 1 + i64::from(n);
        let year = total.div_euclid(12) as i32;
        let month = (total.rem_euclid(12) + 1) as u8;
        let day = self.day.min(days_in_month(year, month));
        Self { year, month, day }
    }

    /// The first day of this date's month.
    pub fn first_of_month(self) -> Self {
        Self { day: 1, ..self }
    }

    /// Today in the user's local timezone, from the runtime wall-clock
    /// seam (see the module docs for the pre-mount `0` reading).
    pub fn today() -> Self {
        let local_millis = runtime_core::time::epoch_millis()
            + i64::from(runtime_core::time::local_offset_minutes()) * 60_000;
        Self::from_days(local_millis.div_euclid(86_400_000))
    }
}

/// `CivilDate` → `Reactive<Option<CivilDate>>` as `Static(Some(...))`.
/// Optional date props (`min`/`max` on `Calendar`, `DatePicker`,
/// `DateInput`, …) are typed `Reactive<Option<CivilDate>>`; this lets a
/// call site pass a bare date without writing `Some(...)`, matching
/// runtime-core's `String` → `Reactive<Option<String>>` shorthand for
/// optional-text props.
impl From<CivilDate> for runtime_core::Reactive<Option<CivilDate>> {
    fn from(d: CivilDate) -> Self {
        runtime_core::Reactive::Static(Some(d))
    }
}

/// `true` for Gregorian leap years.
pub fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Number of days in `month` of `year` (`month` must be `1..=12`;
/// out-of-range months report `0` so callers' validation stays total).
pub fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// CivilTime / CivilDateTime
// ---------------------------------------------------------------------------

/// A timezone-less time of day, second precision.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, IdealystSchema)]
pub struct CivilTime {
    /// `0..=23`.
    pub hour: u8,
    /// `0..=59`.
    pub minute: u8,
    /// `0..=59`.
    pub second: u8,
}

impl CivilTime {
    /// Validated constructor — `None` for out-of-range fields.
    pub fn new(hour: u8, minute: u8, second: u8) -> Option<Self> {
        (hour <= 23 && minute <= 59 && second <= 59).then_some(Self { hour, minute, second })
    }

    /// Midnight (`00:00:00`).
    pub const MIDNIGHT: CivilTime = CivilTime { hour: 0, minute: 0, second: 0 };
}

/// A timezone-less date + time-of-day pair.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, IdealystSchema)]
pub struct CivilDateTime {
    pub date: CivilDate,
    pub time: CivilTime,
}

impl CivilDateTime {
    pub fn new(date: CivilDate, time: CivilTime) -> Self {
        Self { date, time }
    }

    /// Now in the user's local timezone (see [`CivilDate::today`]).
    pub fn now() -> Self {
        let local_millis = runtime_core::time::epoch_millis()
            + i64::from(runtime_core::time::local_offset_minutes()) * 60_000;
        let days = local_millis.div_euclid(86_400_000);
        let secs = (local_millis.rem_euclid(86_400_000) / 1000) as u32;
        Self {
            date: CivilDate::from_days(days),
            time: CivilTime {
                hour: (secs / 3600) as u8,
                minute: ((secs / 60) % 60) as u8,
                second: (secs % 60) as u8,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Display labels
// ---------------------------------------------------------------------------

/// Month/weekday display names for the calendar UI. English by
/// default ([`DateLabels::english`]); construct your own (or map the
/// fields through your i18n layer) and pass it to the components'
/// `labels` prop to localize. Weekday arrays are Monday-first
/// ([`Weekday::as_index`] order) regardless of the calendar's
/// `first_weekday` — the components rotate for display.
#[derive(Clone, Debug, PartialEq)]
pub struct DateLabels {
    /// Full month names, January-first.
    pub months: [String; 12],
    /// Abbreviated month names, January-first (calendar header).
    pub months_short: [String; 12],
    /// Abbreviated weekday names, Monday-first (column headers).
    pub weekdays_short: [String; 7],
}

impl DateLabels {
    pub fn english() -> Rc<Self> {
        fn arr<const N: usize>(items: [&str; N]) -> [String; N] {
            items.map(str::to_string)
        }
        Rc::new(Self {
            months: arr([
                "January", "February", "March", "April", "May", "June", "July", "August",
                "September", "October", "November", "December",
            ]),
            months_short: arr([
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov",
                "Dec",
            ]),
            weekdays_short: arr(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]),
        })
    }

    /// Full name of `month` (1-based).
    pub fn month_name(&self, month: u8) -> &str {
        &self.months[usize::from(month.clamp(1, 12) - 1)]
    }

    /// Short name of `weekday`.
    pub fn weekday_short(&self, weekday: Weekday) -> &str {
        &self.weekdays_short[usize::from(weekday.as_index())]
    }
}

// ---------------------------------------------------------------------------
// Token formatting / parsing
// ---------------------------------------------------------------------------

/// One lexed unit of a format string. `pub(crate)` for the smart-typing
/// mask ([`crate::date_mask`]), which drives segment advancement off the
/// same token stream the parser/formatter use.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum Token {
    Year4,
    Month2,
    Month1,
    Day2,
    Day1,
    Hour24Two,
    Hour24One,
    Hour12Two,
    Hour12One,
    Minute2,
    Second2,
    /// `A` → `AM`/`PM`, `a` → `am`/`pm`.
    Meridiem { upper: bool },
    Literal(char),
}

/// Longest-match tokenizer over the format tokens documented in the
/// module docs. Any unrecognized character is a literal.
pub(crate) fn lex(fmt: &str) -> Vec<Token> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let rest = &chars[i..];
        let run = |c: char| rest.iter().take_while(|&&x| x == c).count();
        let (tok, len) = match rest[0] {
            'Y' => (Token::Year4, run('Y').min(4)),
            'M' => {
                if run('M') >= 2 {
                    (Token::Month2, 2)
                } else {
                    (Token::Month1, 1)
                }
            }
            'D' => {
                if run('D') >= 2 {
                    (Token::Day2, 2)
                } else {
                    (Token::Day1, 1)
                }
            }
            'H' => {
                if run('H') >= 2 {
                    (Token::Hour24Two, 2)
                } else {
                    (Token::Hour24One, 1)
                }
            }
            'h' => {
                if run('h') >= 2 {
                    (Token::Hour12Two, 2)
                } else {
                    (Token::Hour12One, 1)
                }
            }
            'm' => {
                if run('m') >= 2 {
                    (Token::Minute2, 2)
                } else {
                    // A single `m` formats unpadded but parses the same;
                    // treat as minute for symmetry.
                    (Token::Minute2, 1)
                }
            }
            's' => (Token::Second2, run('s').min(2).max(1)),
            'A' => (Token::Meridiem { upper: true }, 1),
            'a' => (Token::Meridiem { upper: false }, 1),
            c => (Token::Literal(c), 1),
        };
        out.push(tok);
        i += len;
    }
    out
}

/// Fields accumulated by the shared parser / consumed by the shared
/// formatter.
#[derive(Default, Copy, Clone)]
struct Fields {
    year: Option<i32>,
    month: Option<u8>,
    day: Option<u8>,
    /// Hour as written; 12h-ness tracked by `meridiem_pm`'s presence.
    hour: Option<u8>,
    minute: Option<u8>,
    second: Option<u8>,
    meridiem_pm: Option<bool>,
    saw_hour12: bool,
}

fn format_with(tokens: &[Token], date: Option<CivilDate>, time: Option<CivilTime>) -> String {
    let mut out = String::new();
    for tok in tokens {
        match *tok {
            Token::Year4 => {
                if let Some(d) = date {
                    out.push_str(&format!("{:04}", d.year));
                }
            }
            Token::Month2 => {
                if let Some(d) = date {
                    out.push_str(&format!("{:02}", d.month));
                }
            }
            Token::Month1 => {
                if let Some(d) = date {
                    out.push_str(&d.month.to_string());
                }
            }
            Token::Day2 => {
                if let Some(d) = date {
                    out.push_str(&format!("{:02}", d.day));
                }
            }
            Token::Day1 => {
                if let Some(d) = date {
                    out.push_str(&d.day.to_string());
                }
            }
            Token::Hour24Two => {
                if let Some(t) = time {
                    out.push_str(&format!("{:02}", t.hour));
                }
            }
            Token::Hour24One => {
                if let Some(t) = time {
                    out.push_str(&t.hour.to_string());
                }
            }
            Token::Hour12Two | Token::Hour12One => {
                if let Some(t) = time {
                    let h12 = (t.hour + 11) % 12 + 1;
                    if matches!(tok, Token::Hour12Two) {
                        out.push_str(&format!("{h12:02}"));
                    } else {
                        out.push_str(&h12.to_string());
                    }
                }
            }
            Token::Minute2 => {
                if let Some(t) = time {
                    out.push_str(&format!("{:02}", t.minute));
                }
            }
            Token::Second2 => {
                if let Some(t) = time {
                    out.push_str(&format!("{:02}", t.second));
                }
            }
            Token::Meridiem { upper } => {
                if let Some(t) = time {
                    let s = match (t.hour >= 12, upper) {
                        (false, true) => "AM",
                        (true, true) => "PM",
                        (false, false) => "am",
                        (true, false) => "pm",
                    };
                    out.push_str(s);
                }
            }
            Token::Literal(c) => out.push(c),
        }
    }
    out
}

/// Parse `input` against `tokens`. Digit tokens accept 1–2 digits
/// (4 for the year) regardless of padding; literals must match
/// exactly except that any run of spaces matches any run of spaces.
fn parse_with(tokens: &[Token], input: &str) -> Option<Fields> {
    let chars: Vec<char> = input.trim().chars().collect();
    let mut pos = 0usize;
    let mut f = Fields::default();

    fn take_digits(chars: &[char], pos: &mut usize, max: usize) -> Option<u32> {
        let start = *pos;
        while *pos < chars.len() && *pos - start < max && chars[*pos].is_ascii_digit() {
            *pos += 1;
        }
        if *pos == start {
            return None;
        }
        chars[start..*pos].iter().collect::<String>().parse().ok()
    }

    for tok in tokens {
        match *tok {
            Token::Year4 => f.year = Some(take_digits(&chars, &mut pos, 4)? as i32),
            Token::Month2 | Token::Month1 => {
                f.month = Some(u8::try_from(take_digits(&chars, &mut pos, 2)?).ok()?)
            }
            Token::Day2 | Token::Day1 => {
                f.day = Some(u8::try_from(take_digits(&chars, &mut pos, 2)?).ok()?)
            }
            Token::Hour24Two | Token::Hour24One => {
                f.hour = Some(u8::try_from(take_digits(&chars, &mut pos, 2)?).ok()?)
            }
            Token::Hour12Two | Token::Hour12One => {
                f.hour = Some(u8::try_from(take_digits(&chars, &mut pos, 2)?).ok()?);
                f.saw_hour12 = true;
            }
            Token::Minute2 => {
                f.minute = Some(u8::try_from(take_digits(&chars, &mut pos, 2)?).ok()?)
            }
            Token::Second2 => {
                f.second = Some(u8::try_from(take_digits(&chars, &mut pos, 2)?).ok()?)
            }
            Token::Meridiem { .. } => {
                // Optional surrounding-space tolerance comes from the
                // space-literal rule; here match am/pm case-insensitively.
                let rest: String = chars[pos..].iter().collect();
                let lower = rest.to_lowercase();
                if lower.starts_with("am") {
                    f.meridiem_pm = Some(false);
                    pos += 2;
                } else if lower.starts_with("pm") {
                    f.meridiem_pm = Some(true);
                    pos += 2;
                } else {
                    return None;
                }
            }
            Token::Literal(' ') => {
                // Any run of spaces in the input matches a space literal.
                while pos < chars.len() && chars[pos] == ' ' {
                    pos += 1;
                }
            }
            Token::Literal(c) => {
                if pos < chars.len() && chars[pos] == c {
                    pos += 1;
                } else {
                    return None;
                }
            }
        }
    }
    (pos == chars.len()).then_some(f)
}

impl Fields {
    fn to_date(self) -> Option<CivilDate> {
        CivilDate::new(self.year?, self.month?, self.day?)
    }

    fn to_time(self) -> Option<CivilTime> {
        let mut hour = self.hour?;
        if self.saw_hour12 {
            // 12h clock: `12 am` → 0, `12 pm` → 12. A 12h format with
            // no meridiem token parsed is ambiguous — reject.
            let pm = self.meridiem_pm?;
            if hour == 0 || hour > 12 {
                return None;
            }
            hour = (hour % 12) + if pm { 12 } else { 0 };
        } else if let Some(pm) = self.meridiem_pm {
            // Meridiem alongside a 24h token: accept when consistent
            // (`13 pm` is nonsense; `1 pm` was lexed 24h → treat as 12h).
            if hour > 12 {
                return None;
            }
            hour = (hour % 12) + if pm { 12 } else { 0 };
        }
        CivilTime::new(hour, self.minute?, self.second.unwrap_or(0))
    }
}

/// Format `date` with a date-token format string (e.g. `"YYYY-MM-DD"`,
/// `"D/M/YYYY"`). Time tokens produce nothing.
pub fn format_date(date: CivilDate, fmt: &str) -> String {
    format_with(&lex(fmt), Some(date), None)
}

/// Format `time` with a time-token format string (e.g. `"HH:mm"`,
/// `"h:mm A"`). Date tokens produce nothing.
pub fn format_time(time: CivilTime, fmt: &str) -> String {
    format_with(&lex(fmt), None, Some(time))
}

/// Format a datetime with a combined format string
/// (e.g. `"YYYY-MM-DD HH:mm"`).
pub fn format_datetime(dt: CivilDateTime, fmt: &str) -> String {
    format_with(&lex(fmt), Some(dt.date), Some(dt.time))
}

/// Parse a date. `None` unless every date field is present, in range,
/// and the whole input is consumed. Lenient about zero-padding.
pub fn parse_date(input: &str, fmt: &str) -> Option<CivilDate> {
    parse_with(&lex(fmt), input)?.to_date()
}

/// Parse a time of day (see [`parse_date`] for the leniency rules).
pub fn parse_time(input: &str, fmt: &str) -> Option<CivilTime> {
    parse_with(&lex(fmt), input)?.to_time()
}

/// Parse a combined datetime.
pub fn parse_datetime(input: &str, fmt: &str) -> Option<CivilDateTime> {
    let f = parse_with(&lex(fmt), input)?;
    Some(CivilDateTime { date: f.to_date()?, time: f.to_time()? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_conversion_round_trips_across_centuries() {
        // Epoch anchor + widely-known fixtures.
        assert_eq!(CivilDate::new(1970, 1, 1).unwrap().to_days(), 0);
        assert_eq!(CivilDate::from_days(0), CivilDate::new(1970, 1, 1).unwrap());
        // Every ~97th day over ±400 years crosses leap/century
        // boundaries in both directions.
        let mut day = CivilDate::new(1826, 3, 7).unwrap().to_days();
        let end = CivilDate::new(2226, 3, 7).unwrap().to_days();
        while day <= end {
            let d = CivilDate::from_days(day);
            assert_eq!(d.to_days(), day, "round-trip failed at {d:?}");
            assert!(CivilDate::new(d.year, d.month, d.day).is_some());
            day += 97;
        }
    }

    #[test]
    fn regression_bare_civil_date_coerces_into_optional_reactive_prop() {
        // The DateInput/DatePicker recipes write `max = CivilDate::today()`
        // against props typed `Reactive<Option<CivilDate>>`; without the
        // `From<CivilDate>` impl above, that (and the `idealyst docs` build,
        // which compiles the recipes) fails with E0277.
        let d = CivilDate::new(2026, 8, 3).unwrap();
        match runtime_core::Reactive::<Option<CivilDate>>::from(d) {
            runtime_core::Reactive::Static(v) => assert_eq!(v, Some(d)),
            _ => panic!("bare CivilDate must coerce to a static Some(date)"),
        }
    }

    #[test]
    fn weekdays_match_known_dates() {
        // 1970-01-01 Thursday; 2000-01-01 Saturday; 2026-08-03 Monday.
        assert_eq!(CivilDate::new(1970, 1, 1).unwrap().weekday(), Weekday::Thursday);
        assert_eq!(CivilDate::new(2000, 1, 1).unwrap().weekday(), Weekday::Saturday);
        assert_eq!(CivilDate::new(2026, 8, 3).unwrap().weekday(), Weekday::Monday);
        // Pre-epoch (negative day counts must not skew the modulo):
        // 1969-12-31 was a Wednesday.
        assert_eq!(CivilDate::new(1969, 12, 31).unwrap().weekday(), Weekday::Wednesday);
    }

    #[test]
    fn leap_years_and_month_lengths() {
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2026));
        assert!(!is_leap_year(1900)); // century, not ÷400
        assert!(is_leap_year(2000)); // ÷400
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2026, 4), 30);
        assert_eq!(days_in_month(2026, 12), 31);
    }

    #[test]
    fn constructor_rejects_impossible_dates() {
        assert!(CivilDate::new(2026, 2, 30).is_none());
        assert!(CivilDate::new(2026, 13, 1).is_none());
        assert!(CivilDate::new(2026, 0, 1).is_none());
        assert!(CivilDate::new(2026, 1, 0).is_none());
        assert!(CivilDate::new(2024, 2, 29).is_some());
        assert!(CivilTime::new(24, 0, 0).is_none());
        assert!(CivilTime::new(23, 60, 0).is_none());
        assert!(CivilTime::new(23, 59, 59).is_some());
    }

    #[test]
    fn add_months_clamps_day_of_month() {
        let jan31 = CivilDate::new(2026, 1, 31).unwrap();
        assert_eq!(jan31.add_months(1), CivilDate::new(2026, 2, 28).unwrap());
        assert_eq!(jan31.add_months(13), CivilDate::new(2027, 2, 28).unwrap());
        let jan31_leap = CivilDate::new(2024, 1, 31).unwrap();
        assert_eq!(jan31_leap.add_months(1), CivilDate::new(2024, 2, 29).unwrap());
        // Across year boundaries in both directions.
        assert_eq!(
            CivilDate::new(2026, 1, 15).unwrap().add_months(-1),
            CivilDate::new(2025, 12, 15).unwrap()
        );
        assert_eq!(
            CivilDate::new(2026, 12, 15).unwrap().add_months(1),
            CivilDate::new(2027, 1, 15).unwrap()
        );
    }

    #[test]
    fn ordering_is_chronological() {
        // Derived Ord on (year, month, day) — field order is load-bearing.
        let a = CivilDate::new(2025, 12, 31).unwrap();
        let b = CivilDate::new(2026, 1, 1).unwrap();
        assert!(a < b);
        let t1 = CivilTime::new(9, 30, 0).unwrap();
        let t2 = CivilTime::new(10, 0, 0).unwrap();
        assert!(t1 < t2);
        assert!(CivilDateTime::new(a, t2) < CivilDateTime::new(b, t1));
    }

    #[test]
    fn format_covers_the_token_set() {
        let d = CivilDate::new(2026, 3, 7).unwrap();
        assert_eq!(format_date(d, "YYYY-MM-DD"), "2026-03-07");
        assert_eq!(format_date(d, "D/M/YYYY"), "7/3/2026");
        let t = CivilTime::new(14, 5, 9).unwrap();
        assert_eq!(format_time(t, "HH:mm:ss"), "14:05:09");
        assert_eq!(format_time(t, "h:mm A"), "2:05 PM");
        assert_eq!(format_time(t, "hh:mm a"), "02:05 pm");
        let midnight = CivilTime::MIDNIGHT;
        assert_eq!(format_time(midnight, "h:mm A"), "12:00 AM");
        let noon = CivilTime::new(12, 0, 0).unwrap();
        assert_eq!(format_time(noon, "h:mm A"), "12:00 PM");
        assert_eq!(
            format_datetime(CivilDateTime::new(d, t), "YYYY-MM-DD HH:mm"),
            "2026-03-07 14:05"
        );
    }

    #[test]
    fn parse_is_lenient_about_padding_and_spaces() {
        let expected = CivilDate::new(2026, 3, 7).unwrap();
        assert_eq!(parse_date("2026-03-07", "YYYY-MM-DD"), Some(expected));
        assert_eq!(parse_date("2026-3-7", "YYYY-MM-DD"), Some(expected));
        assert_eq!(parse_date("7/3/2026", "D/M/YYYY"), Some(expected));
        assert_eq!(parse_date("07/03/2026", "D/M/YYYY"), Some(expected));
        assert_eq!(parse_date("  2026-03-07  ", "YYYY-MM-DD"), Some(expected));
        let dt = parse_datetime("2026-3-7  9:05 pm", "YYYY-MM-DD h:mm a").unwrap();
        assert_eq!(dt.date, expected);
        assert_eq!(dt.time, CivilTime::new(21, 5, 0).unwrap());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_date("2026-02-30", "YYYY-MM-DD").is_none()); // impossible
        assert!(parse_date("2026-03", "YYYY-MM-DD").is_none()); // incomplete
        assert!(parse_date("2026-03-07x", "YYYY-MM-DD").is_none()); // trailing junk
        assert!(parse_date("03/07", "YYYY-MM-DD").is_none()); // wrong shape
        assert!(parse_date("", "YYYY-MM-DD").is_none());
        assert!(parse_time("25:00", "HH:mm").is_none()); // out of range
        assert!(parse_time("13:00 pm", "h:mm a").is_none()); // 13 on a 12h clock
        assert!(parse_time("9:05", "h:mm a").is_none()); // 12h without meridiem
    }

    #[test]
    fn twelve_hour_parse_maps_edges_correctly() {
        assert_eq!(parse_time("12:00 am", "h:mm a"), CivilTime::new(0, 0, 0));
        assert_eq!(parse_time("12:00 pm", "h:mm a"), CivilTime::new(12, 0, 0));
        assert_eq!(parse_time("1:00 pm", "h:mm a"), CivilTime::new(13, 0, 0));
        assert_eq!(parse_time("11:59 PM", "h:mm A"), CivilTime::new(23, 59, 0));
    }

    #[test]
    fn format_parse_round_trips() {
        let fmts = ["YYYY-MM-DD", "D/M/YYYY", "MM/DD/YYYY", "YYYY.MM.DD"];
        let d = CivilDate::new(2026, 11, 3).unwrap();
        for fmt in fmts {
            assert_eq!(parse_date(&format_date(d, fmt), fmt), Some(d), "fmt {fmt}");
        }
        let tfmts = ["HH:mm", "HH:mm:ss", "h:mm A", "hh:mm a"];
        let t = CivilTime::new(0, 7, 0).unwrap();
        for fmt in tfmts {
            assert_eq!(parse_time(&format_time(t, fmt), fmt), Some(t), "fmt {fmt}");
        }
    }

    #[test]
    fn weekday_index_round_trips_and_wraps() {
        for i in 0..7 {
            assert_eq!(Weekday::from_index(i).as_index(), i);
        }
        assert_eq!(Weekday::Sunday.add(1), Weekday::Monday);
        assert_eq!(Weekday::Monday.add(6), Weekday::Sunday);
    }

    #[test]
    fn labels_default_english() {
        let l = DateLabels::english();
        assert_eq!(l.month_name(1), "January");
        assert_eq!(l.month_name(12), "December");
        assert_eq!(l.weekday_short(Weekday::Monday), "Mon");
        assert_eq!(l.weekday_short(Weekday::Sunday), "Sun");
    }
}
