//! Tick selection: which values get a gridline and a label, and the label
//! text.
//!
//! Three axis flavors, one entry point each:
//!
//! - [`linear`] — "nice numbers": the step is 1, 2 or 5 times a power of
//!   ten, chosen as the finest step that still yields at most `max_ticks`
//!   ticks. Labels print the shortest decimal that is exact to 5 places.
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
//! produces identical values AND labels on every input where plotters
//! returned at all. It deliberately departs from plotters only where
//! plotters did not return — the cases are pinned by tests in
//! `tests/ticks.rs`:
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
//! Every function here takes `f64` and returns plain values; positioning is
//! the scale's job ([`ResolvedAxis::map`](crate::ResolvedAxis::map)), done
//! in `f32` without quantizing to whole pixels.

use chrono::{DateTime, NaiveDate, TimeDelta, Timelike, Utc};

use crate::scale::Tick;

// ---------------------------------------------------------------------------
// Linear
// ---------------------------------------------------------------------------

/// Ticks for a linear axis over `[min, max]` (either order), at most
/// `max_ticks` of them.
pub fn linear(min: f64, max: f64, max_ticks: usize) -> Vec<Tick> {
    linear_values(min, max, max_ticks)
        .into_iter()
        .map(|v| Tick { value: v, label: format_decimal(v) })
        .collect()
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

/// Print a tick value as the shortest decimal within 1e-5 of it, never in
/// scientific notation, always with at least one decimal (`"2.0"`,
/// `"0.25"`, `"-0.00001"`). Port of plotters' `FloatPrettyPrinter` with
/// `allow_scientific: false, min_decimal: 1, max_decimal: 5` — the
/// formatter its `f64` range used.
fn format_decimal(n: f64) -> String {
    const MAX_DECIMAL: i32 = 5;
    const MIN_DECIMAL: usize = 1;
    let (tn, p) = find_minimal_repr(n, 10f64.powi(-MAX_DECIMAL));
    float_to_string(tn, p, MIN_DECIMAL)
}

/// The fewest decimal digits `p` such that `n` rounded to `p` places is
/// within `eps` of `n`, and that rounded value.
fn find_minimal_repr(n: f64, eps: f64) -> (f64, usize) {
    if eps >= 1.0 {
        return (n, 0);
    }
    if n - n.floor() < eps {
        (n.floor(), 0)
    } else if n.ceil() - n < eps {
        (n.ceil(), 0)
    } else {
        let (rem, pre) = find_minimal_repr((n - n.floor()) * 10.0, eps * 10.0);
        (n.floor() + rem / 10.0, pre + 1)
    }
}

fn float_to_string(n: f64, max_precision: usize, min_decimal: usize) -> String {
    let (mut result, mut count) = {
        let (sign, n) = if n < 0.0 { ("-", -n) } else { ("", n) };
        let int_part = n.floor();

        let dec_part =
            ((n.abs() - int_part.abs()) * 10f64.powi(max_precision as i32)).round() as u64;

        if dec_part == 0 || max_precision == 0 {
            (format!("{sign}{int_part:.0}"), 0)
        } else {
            let mut dec_result = format!("{dec_part}");
            // `saturating_sub`: plotters wrote a plain `-`, which underflows
            // (a panic in debug) if the fraction rounds up to a whole
            // `10^max_precision`. Identical output otherwise.
            let leading = "0".repeat(max_precision.saturating_sub(dec_result.len()));

            while let Some(c) = dec_result.pop() {
                if c != '0' {
                    dec_result.push(c);
                    break;
                }
            }

            (format!("{sign}{int_part:.0}.{leading}{dec_result}"), leading.len() + dec_result.len())
        }
    };

    if count == 0 && min_decimal > 0 {
        result.push('.');
    }
    while count < min_decimal {
        result.push('0');
        count += 1;
    }
    result
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
        for i in 1..=light_density {
            let v = val
                * (1.0 + multiplier / f64::from(light_density as u32 + 1) * f64::from(i as u32));
            if v > end {
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

/// Format a log tick label without trailing noise: integers print bare
/// (`"100"`), fractions to at most three places (`"0.001"`, `"0"` for
/// anything smaller).
fn format_number(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
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
