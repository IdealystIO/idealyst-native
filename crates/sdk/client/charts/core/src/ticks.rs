//! Tick selection: which values get a gridline and a label, and the label
//! text.
//!
//! Three axis flavors, one entry point each:
//!
//! - [`linear`] — "nice numbers": the step is 1, 2 or 5 times a power of
//!   ten, chosen as the finest step that still yields at most `max_ticks`
//!   ticks. Labels carry the step's precision (trailing zeros trimmed, at
//!   least one decimal); an axis of magnitudes below 1e-4 whose step needs
//!   more than five places prints in scientific notation.
//! - [`log`] — one tick per decade (or per every n-th decade when there are
//!   too many), plus evenly spaced in-decade ticks when the budget allows.
//! - [`time`] — milliseconds since the Unix epoch, UTC. Spans shorter than
//!   ~292 years get a fixed period from a ladder of human units (1/2/5 ms…,
//!   1/2/5/10/15/20/30 s and min, 1/2/4/8/12 h) aligned to UTC midnight;
//!   longer spans fall back to whole days or whole weeks.
//!
//! # Provenance
//!
//! These are line-for-line ports of the tick selection charts-core used to
//! take from `plotters` 0.3.7 (`coord::ranged1d::types::numeric`,
//! `combinators::logarithmic`, `types::datetime`, and `data::float` for the
//! label printer). The port exists because plotters links `wasm-bindgen`
//! and `web-sys` on wasm32 unconditionally, and its `datetime` feature
//! turns on chrono's default `clock` + `wasmbind` features (`js-sys` /
//! `wasm-bindgen` again) — dependencies a portable crate cannot carry on
//! the web. Owning a few hundred lines of arithmetic is cheaper.
//!
//! The port was diffed against plotters over a corpus of ~2,700 inputs and
//! produced identical values AND labels on every input where plotters
//! returned at all. Beyond the label contract below, it departs from
//! plotters only where plotters did not return — the cases are pinned by
//! tests in `tests/ticks.rs`:
//!
//! - **Non-finite linear spans** (an infinite bound, both bounds NaN, or a
//!   span that overflows f64): plotters either hung forever (its step loop
//!   divides `inf` by 10 without end) or tripped an `assert!`. We return no
//!   ticks.
//! - **Log ranges whose decade count is not finite** (a zero or infinite
//!   bound, or `end / start` overflowing): plotters hung — `inf as usize`
//!   saturates and its multiplier loop then needs ~3.7e18 iterations. We
//!   return no ticks. (`scale` additionally clamps every log domain to 12
//!   decades, so a chart never asks for one.)
//! - **Time axes with `max_ticks == 0`** over a sub-292-year span, and
//!   reversed time ranges of a week or more: plotters panicked (an integer
//!   `pow` overflow, and a division by a zero step, respectively). We
//!   return no ticks. Date arithmetic that would leave chrono's
//!   representable range — a panic in plotters — stops the tick run there.
//!
//! # Label contract (where we deliberately differ from plotters)
//!
//! Adjacent labels on an axis are always distinct, and no label reads as
//! negative zero. plotters broke both, and the outputs that changed to
//! honour them are rewritten in the corpus (`tests/goldens`), each one a
//! line whose plotters output broke the contract:
//!
//! - **Linear ranges narrower than ~1e-5**: plotters printed every value to
//!   at most five places, so `1e-9..2e-9` was labelled `"0.0"` throughout,
//!   a negative value that rounded away printed `"-0.0"`, and even `-1e-5`
//!   itself came out `"0.0"`. Labels now take their precision from the
//!   step (69 corpus lines).
//! - **Log axes with spare tick budget**: the last in-decade tick landed on
//!   the next decade, which the decade walk emitted again (`"9", "10",
//!   "10", "20"`); and labels printed to three fixed places, so every tick
//!   under 0.0005 read `"0"`. The in-decade run now stops short of the next
//!   decade, and small values print in scientific notation (`"1e-5"`) (85
//!   corpus lines).
//!
//! `tests/ticks.rs` sweeps ranges for each axis kind asserting the
//! contract.
//!
//! Every function here takes `f64` and returns plain values; positioning is
//! the scale's job ([`ResolvedAxis::map`](crate::ResolvedAxis::map)), done
//! in `f32` without quantizing to whole pixels.

use chrono::{DateTime, NaiveDate, TimeDelta, Timelike, Utc};

use crate::scale::Tick;

// ---------------------------------------------------------------------------
// Linear
// ---------------------------------------------------------------------------

/// Ticks for a linear axis over `[min, max]` (either order), at most
/// `max_ticks` of them. Labels are formatted by [`linear_labels`].
pub fn linear(min: f64, max: f64, max_ticks: usize) -> Vec<Tick> {
    let values = linear_values(min, max, max_ticks);
    let labels = linear_labels(&values);
    values.into_iter().zip(labels).map(|(value, label)| Tick { value, label }).collect()
}

/// The nice-number key points. Port of plotters'
/// `compute_f64_key_points`, including its float-error compensations.
fn linear_values(a: f64, b: f64, max_points: usize) -> Vec<f64> {
    if max_points == 0 {
        return vec![];
    }

    // `f64::min`/`max` drop a NaN operand, so a one-sided NaN collapses to
    // the single finite bound — kept, since it is what plotters did.
    let range = (a.min(b), b.max(a));

    // Divergence from plotters (which hangs or asserts): see module docs.
    if !range.0.is_finite() || !range.1.is_finite() || !(range.1 - range.0).is_finite() {
        return vec![];
    }

    if (range.0 - range.1).abs() < f64::EPSILON {
        return vec![range.0];
    }

    let mut scale = 10f64.powf((range.1 - range.0).log(10.0).floor());
    // Round tick values to a multiple of this, so a run of `left + k*scale`
    // additions cannot accumulate into `1.00000000001`.
    let mut value_granularity = scale / 10.0;

    fn rem_euclid(a: f64, b: f64) -> f64 {
        let ret = if b > 0.0 { a - (a / b).floor() * b } else { a - (a / b).ceil() * b };
        if (ret - b).abs() < f64::EPSILON {
            0.0
        } else {
            ret
        }
    }

    // Loop invariant for the refinement below: the current scale must not
    // already yield more points than requested.
    if 1 + ((range.1 - range.0) / scale).floor() as usize > max_points {
        scale *= 10.0;
        value_granularity *= 10.0;
    }

    // Refine by /2, /5, /10 for as long as the point count still fits.
    'outer: loop {
        let old_scale = scale;
        for nxt in [2.0, 5.0, 10.0] {
            let mut new_left = range.0 - rem_euclid(range.0, old_scale / nxt);
            if new_left < range.0 {
                new_left += old_scale / nxt;
            }
            let new_right = range.1 - rem_euclid(range.1, old_scale / nxt);

            let npoints = 1.0 + ((new_right - new_left) / old_scale * nxt);

            if npoints.round() as usize > max_points {
                break 'outer;
            }

            scale = old_scale / nxt;
        }
        scale = old_scale / 10.0;
        value_granularity /= 10.0;
    }

    let mut ret = vec![];
    // `left` is split into a granularity-aligned base plus a small relative
    // part: for a large `left`, `left + scale == left` in f64, and a single
    // accumulator would loop forever.
    let left = {
        let mut value = range.0 - rem_euclid(range.0, scale);
        if value < range.0 {
            value += scale;
        }
        value
    };
    let left_base = (left / value_granularity).floor() * value_granularity;
    let mut left_relative = left - left_base;
    let right = range.1 - rem_euclid(range.1, scale);
    while (right - left_relative - left_base) >= -f64::EPSILON {
        let new_left_relative = (left_relative / value_granularity).round() * value_granularity;
        if new_left_relative < 0.0 {
            left_relative += value_granularity;
        }
        ret.push(left_relative + left_base);
        left_relative += scale;
    }
    ret
}

/// Below this magnitude an axis whose step needs more than
/// [`PLOTTERS_DECIMALS`] places is labelled in scientific notation
/// (`"1.5e-9"`): `"0.0000000015"` is the same number, but nobody reads it.
/// Offset ranges (`0.999999..1.000001`) stay in fixed notation — their
/// magnitude is ordinary, only the step is fine.
const SCIENTIFIC_BELOW: f64 = 1e-4;

/// The places plotters' printer always stopped at (`max_decimal: 5`). Any
/// step at or above `10^-PLOTTERS_DECIMALS` labels exactly as plotters did.
const PLOTTERS_DECIMALS: usize = 5;

/// Past this many places f64 has no digits left to show (its 17
/// significant digits are spent); the distinct-label search stops here.
const MAX_LABEL_DECIMALS: usize = 20;

/// Labels for a run of evenly spaced linear ticks.
///
/// The precision comes from the STEP, not from a fixed cap: a step of
/// `5e-7` needs seven places, so every label gets up to seven (trailing
/// zeros trimmed, at least one decimal kept — `"2.0"`, `"0.25"`). plotters
/// printed every value to at most five places, so any range narrower than
/// ~1e-5 came out as a column of `"0.0"`; for steps of 1e-5 and coarser
/// the output here is identical to plotters'.
///
/// Two guarantees every axis gets, whatever the range:
/// - adjacent labels differ — if float noise ever makes two collide at the
///   step's precision, the precision grows until they don't;
/// - no label reads as negative zero (`"-0.0"` was plotters' output for a
///   small negative value: `format!("{:.0}", -0.0)` keeps the sign).
fn linear_labels(values: &[f64]) -> Vec<String> {
    let [first, second, ..] = values else {
        return values.iter().map(|&v| lone_linear_label(v)).collect();
    };
    let step = (second - first).abs();
    // Tick values carry float noise (`0.30000000000000004`), and the zero
    // tick of a symmetric range can come out as `±1e-29`; snap anything
    // that small relative to the step to an exact zero before printing.
    let snap = |v: f64| if v.abs() < step * 1e-6 { 0.0 } else { v };
    // Steps are 1, 2 or 5 times a power of ten; the `+ 1e-9` absorbs a
    // step of `9.999999999e-11` that is really `1e-10`.
    let step_exp = (step.log10() + 1e-9).floor() as i32;
    let decimals = usize::try_from(-step_exp).unwrap_or(0);
    let max_abs = values.iter().fold(0.0f64, |m, v| m.max(v.abs()));

    let render = |extra: usize| -> Vec<String> {
        if decimals > PLOTTERS_DECIMALS && max_abs < SCIENTIFIC_BELOW {
            // One mantissa precision for the whole axis: enough digits to
            // reach the step from the largest tick's exponent.
            let max_exp = max_abs.log10().floor() as i32;
            let mantissa = usize::try_from(max_exp - step_exp).unwrap_or(0) + extra;
            values
                .iter()
                .map(|&v| match snap(v) {
                    0.0 => "0".to_string(),
                    v => format!("{v:.mantissa$e}"),
                })
                .collect()
        } else {
            let places = (decimals + extra).min(MAX_LABEL_DECIMALS);
            values.iter().map(|&v| fixed_label(snap(v), places)).collect()
        }
    };

    let mut extra = 0;
    loop {
        let labels = render(extra);
        let distinct = labels.windows(2).all(|p| p[0] != p[1]);
        if distinct || decimals + extra >= MAX_LABEL_DECIMALS {
            return labels;
        }
        extra += 1;
    }
}

/// `v` to `places` decimals, trailing zeros trimmed to a minimum of one
/// decimal, and a zero that rounded from a negative value printed
/// unsigned.
fn fixed_label(v: f64, places: usize) -> String {
    let mut s = format!("{v:.places$}");
    if s.contains('.') {
        let keep = s.trim_end_matches('0').len();
        s.truncate(keep);
    } else {
        s.push('.');
    }
    if s.ends_with('.') {
        s.push('0');
    }
    if s.starts_with('-') && s[1..].chars().all(|c| c == '0' || c == '.') {
        s.remove(0);
    }
    s
}

/// A single tick has no step to take precision from. It keeps plotters'
/// five-place label unless that would print a nonzero value as zero (a
/// subnormal one-tick range), in which case it prints the value itself in
/// scientific notation.
fn lone_linear_label(v: f64) -> String {
    let fixed = fixed_label(v, PLOTTERS_DECIMALS);
    if v != 0.0 && fixed.chars().all(|c| c == '0' || c == '.') {
        format!("{v:e}")
    } else {
        fixed
    }
}

// ---------------------------------------------------------------------------
// Log
// ---------------------------------------------------------------------------

/// Ticks for a base-10 log axis over `[min, max]`, at most `max_ticks`
/// decade ticks (the in-decade ticks are added only when the budget leaves
/// room for them, so the total can exceed `max_ticks` — plotters' rule).
///
/// A range with a negative bound is mirrored through zero, as plotters did.
/// The caller is expected to have clamped the range to something sane;
/// `scale` does (12 decades).
pub fn log(min: f64, max: f64, max_ticks: usize) -> Vec<Tick> {
    log_values(min, max, max_ticks)
        .into_iter()
        .map(|v| Tick { value: v, label: format_number(v) })
        .collect()
}

/// Port of plotters' `LogRangeExt -> LogCoord` conversion (zero point 0,
/// base 10) followed by `LogCoord::key_points`.
fn log_values(min: f64, max: f64, max_points: usize) -> Vec<f64> {
    let mut start = min;
    let mut end = max;
    let negative = if start < 0.0 || end < 0.0 {
        start = -start;
        end = -end;
        true
    } else {
        false
    };
    if start < end {
        if start == 0.0 {
            start = start.max(end * 1e-5);
        }
    } else if end == 0.0 {
        end = end.max(start * 1e-5);
    }

    let base = 10f64;
    let base_ln = base.ln();

    if start > end {
        std::mem::swap(&mut start, &mut end);
    }

    // Divergence from plotters (which hangs): a zero/negative/non-finite
    // start, or a decade count that is not finite, would drive the
    // `val *= multiplier` walk or the multiplier search below forever.
    let decades = ((end / start).ln().abs() / base_ln).floor();
    if start.is_nan() || start <= 0.0 || !end.is_finite() || !decades.is_finite() {
        return vec![];
    }
    let bold_count = decades.max(1.0) as usize;

    let light_density = if max_points < bold_count {
        0
    } else {
        let density = 1 + (max_points - bold_count) / bold_count;
        let mut exp = 1;
        while exp * 10 <= density {
            exp *= 10;
        }
        exp - 1
    };

    let mut multiplier = base;
    let mut cnt = 1;
    while max_points < bold_count / cnt {
        multiplier *= base;
        cnt += 1;
    }

    let sign = if negative { -1.0 } else { 1.0 };
    // plotters' `is_inf`: a point indistinguishable from the zero point.
    let at_zero = |fv: f64| fv.abs() < f64::EPSILON;

    let mut ret = vec![];
    let mut val = base.powf((start.ln() / base_ln).ceil());
    while val <= end {
        if !at_zero(val) {
            ret.push(sign * val);
        }
        // The next tick of the decade walk. plotters' in-decade loop ran
        // `i` all the way to `light_density`, whose step lands exactly on
        // this value — and the decade walk then emitted it again, so every
        // decade label appeared twice (`"9", "10", "10", "20"`). Stop short
        // of it; the decade walk owns that tick. The `1e-9` slack absorbs
        // the two products rounding differently.
        let next_decade = val * multiplier * (1.0 - 1e-9);
        for i in 1..=light_density {
            let v = val
                * (1.0 + multiplier / f64::from(light_density as u32 + 1) * f64::from(i as u32));
            if v > end || v >= next_decade {
                break;
            }
            // Tests `val`, not `v` — plotters' (harmless) quirk, kept.
            if !at_zero(val) {
                ret.push(sign * v);
            }
        }
        val *= multiplier;
    }
    ret
}

/// Below this magnitude a log label switches to scientific notation
/// (`"1e-5"`); at and above it, fixed decimals (`"0.001"`, `"0.25"`).
const LOG_SCIENTIFIC_BELOW: f64 = 1e-3;

/// Significant digits a log label is rounded to before trimming. Tick
/// values are `k x 10^n` for small `k` (plus float noise such as
/// `0.30000000000000004`); six digits keeps every real digit and drops the
/// noise.
const LOG_LABEL_SIG_DIGITS: i32 = 6;

/// Format a log tick label without trailing noise: integers print bare
/// (`"100"`), other values at six significant digits with trailing zeros
/// trimmed — fixed decimals down to 0.001 (`"0.001"`, `"0.25"`),
/// scientific below that (`"1e-5"`, `"2.5e-7"`).
///
/// plotters printed fractions to three fixed places, so every tick under
/// 0.0005 read `"0"` — twelve-decade axes started `"0", "0", "0.001"` —
/// and in-decade ticks below 0.01 collided once there were more than nine
/// per decade.
fn format_number(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    if v.abs() < LOG_SCIENTIFIC_BELOW {
        let s = format!("{v:.prec$e}", prec = (LOG_LABEL_SIG_DIGITS - 1) as usize);
        let (mantissa, exp) = s.split_once('e').expect("`{:e}` always has an exponent");
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        return format!("{mantissa}e{exp}");
    }
    let places = (LOG_LABEL_SIG_DIGITS - 1 - v.abs().log10().floor() as i32).max(0) as usize;
    let s = format!("{v:.places$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

const NS_PER_SEC: u64 = 1_000_000_000;
const NS_PER_HOUR: u64 = 3_600 * NS_PER_SEC;
const NS_PER_DAY: u64 = 24 * NS_PER_HOUR;
const MS_PER_DAY: f64 = 86_400_000.0;
const MS_PER_YEAR: f64 = 365.0 * MS_PER_DAY;

/// Ticks for a time axis over `[min_ms, max_ms]` (milliseconds since the
/// Unix epoch, UTC), at most `max_ticks` of them.
///
/// Labels match the resolution being shown: `%Y` above three years,
/// `%b %d` above two days, `%H:%M` otherwise. Bounds outside chrono's
/// representable range fall back to [`linear`] so the axis still has
/// ticks.
pub fn time(min_ms: f64, max_ms: f64, max_ticks: usize) -> Vec<Tick> {
    // `as i64` saturates (and maps NaN to 0) — the conversion the axis has
    // always used.
    let to_dt = |ms: f64| DateTime::<Utc>::from_timestamp_millis(ms as i64);
    let (Some(start), Some(end)) = (to_dt(min_ms), to_dt(max_ms)) else {
        return linear(min_ms, max_ms, max_ticks);
    };

    let points = time_values(start, end, max_ticks);
    let span_ms = max_ms - min_ms;
    let fmt = if span_ms > 3.0 * MS_PER_YEAR {
        "%Y"
    } else if span_ms > 2.0 * MS_PER_DAY {
        "%b %d"
    } else {
        "%H:%M"
    };
    points
        .into_iter()
        .map(|dt| Tick { value: dt.timestamp_millis() as f64, label: dt.format(fmt).to_string() })
        .collect()
}

/// Midnight UTC at the start of `date`.
fn midnight(date: NaiveDate) -> DateTime<Utc> {
    date.and_time(chrono::NaiveTime::MIN).and_utc()
}

/// Port of plotters' `RangedDateTime::key_points` for `DateTime<Utc>`.
fn time_values(start: DateTime<Utc>, end: DateTime<Utc>, max_points: usize) -> Vec<DateTime<Utc>> {
    let total_span = end - start;

    // Sub-daily path: a fixed period, aligned so ticks land on multiples of
    // it counted from UTC midnight. `num_nanoseconds` is `None` past ~292
    // years, which routes those spans to the day/week path below.
    if let Some(total_ns) = total_span.num_nanoseconds() {
        match period_per_point(total_ns as u64, max_points) {
            Period::Every(p) => {
                let start_time_ns = u64::from(start.num_seconds_from_midnight()) * NS_PER_SEC
                    + u64::from(start.nanosecond());
                let first = if !start_time_ns.is_multiple_of(p) {
                    start_time_ns + (p - start_time_ns % p)
                } else {
                    start_time_ns
                };
                let mut ret = vec![];
                let Some(mut t) = midnight(start.date_naive())
                    .checked_add_signed(TimeDelta::nanoseconds(first as i64))
                else {
                    return ret;
                };
                while t < end {
                    ret.push(t);
                    match t.checked_add_signed(TimeDelta::nanoseconds(p as i64)) {
                        Some(next) => t = next,
                        None => break,
                    }
                }
                return ret;
            }
            // plotters panicked here (`10u64.pow` overflow with zero ticks
            // requested); see module docs.
            Period::Unrepresentable => return vec![],
            Period::LongerThanADay => {}
        }
    }

    // Otherwise whole dates: the first midnight at or after `start`, to the
    // last midnight at or before `end`.
    let ceil = if start.num_seconds_from_midnight() > 0 {
        start.date_naive().succ_opt()
    } else {
        Some(start.date_naive())
    };
    let Some(first) = ceil else { return vec![] };
    date_values(first, end.date_naive(), max_points).into_iter().map(midnight).collect()
}

enum Period {
    /// One tick every this many nanoseconds.
    Every(u64),
    /// The span wants a period of a day or more; use whole dates instead.
    LongerThanADay,
    /// No sensible period exists (zero ticks requested over a nonzero span).
    Unrepresentable,
}

/// Port of plotters' `compute_period_per_point` with `sub_daily = true`.
///
/// Starts from the power of ten at or below `total / max_points`, then
/// climbs a ladder of human units until the point count fits.
fn period_per_point(total_ns: u64, max_points: usize) -> Period {
    let min_ns_per_point = total_ns as f64 / max_points as f64;
    // `as u32` saturates: +inf (zero points) -> u32::MAX -> pow overflows.
    let Some(actual_ns_per_point) = 10u64.checked_pow(min_ns_per_point.log10().floor() as u32)
    else {
        return Period::Unrepresentable;
    };

    fn climb(
        total_ns: u64,
        mut per_point: u64,
        units: &[u64],
        base: u64,
        max_points: usize,
    ) -> Option<u64> {
        let mut idx = 0;
        while total_ns / per_point > max_points as u64 * units[idx] {
            idx += 1;
            if idx == units.len() {
                idx = 0;
                per_point = per_point.checked_mul(base)?;
            }
        }
        units[idx].checked_mul(per_point)
    }

    let period = if actual_ns_per_point < NS_PER_SEC {
        climb(total_ns, actual_ns_per_point, &[1, 2, 5], 10, max_points)
    } else if actual_ns_per_point < NS_PER_HOUR {
        climb(total_ns, NS_PER_SEC, &[1, 2, 5, 10, 15, 20, 30], 60, max_points)
    } else if actual_ns_per_point < NS_PER_DAY {
        climb(total_ns, NS_PER_HOUR, &[1, 2, 4, 8, 12], 24, max_points)
    } else {
        return Period::LongerThanADay;
    };
    period.map_or(Period::Unrepresentable, Period::Every)
}

/// Port of plotters' `RangedDate::key_points`: every day if they fit, else
/// every week, else every n-th week — all counted from `first`.
fn date_values(first: NaiveDate, last: NaiveDate, max_points: usize) -> Vec<NaiveDate> {
    let span = last - first;
    let total_days = span.num_days();
    let total_weeks = span.num_weeks();
    let step_run = |count: i64, step: fn(i64) -> TimeDelta| -> Vec<NaiveDate> {
        (0..=count).map_while(|i| first.checked_add_signed(step(i))).collect()
    };

    if total_days > 0 && total_days as usize <= max_points {
        return step_run(total_days, TimeDelta::days);
    }
    if total_weeks > 0 && total_weeks as usize <= max_points {
        return step_run(total_weeks, TimeDelta::weeks);
    }
    // Everything within one week (or a reversed range of under a week).
    if total_weeks == 0 {
        return vec![first];
    }

    // `as usize` saturates: zero points -> usize::MAX -> a lone first tick;
    // a reversed range -> 0, where plotters divided by zero (module docs).
    let week_per_point = ((total_weeks as f64) / (max_points as f64)).ceil() as usize;
    if week_per_point == 0 {
        return vec![];
    }
    (0..=(total_weeks as usize / week_per_point))
        .map_while(|idx| first.checked_add_signed(TimeDelta::weeks((idx * week_per_point) as i64)))
        .collect()
}
