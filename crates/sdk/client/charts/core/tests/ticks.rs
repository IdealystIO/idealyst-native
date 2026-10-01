//! Tick selection (`charts_core::ticks`): parity with the plotters 0.3.7
//! output it replaced, the edge cases where plotters never returned, and
//! the label contract (adjacent labels distinct, never "-0") where plotters
//! broke it.

use charts_core::ticks;
use charts_core::Tick;

const MS_PER_DAY: f64 = 86_400_000.0;
/// 2024-03-10T13:47:12.345Z
const T0: f64 = 1_710_078_432_345.0;

fn values(t: &[Tick]) -> Vec<f64> {
    t.iter().map(|t| t.value).collect()
}

fn labels(t: &[Tick]) -> Vec<&str> {
    t.iter().map(|t| t.label.as_str()).collect()
}

/// The 12-decade clamp `scale` applies before every log tick request. The
/// corpus was recorded through it, so replaying the corpus applies it too.
fn clamp_log_range(min: f64, max: f64) -> (f64, f64) {
    let hi = if max > 0.0 { max } else { 1.0 };
    let floor = hi / 10f64.powf(12.0);
    let lo = if min > 0.0 { min.max(floor) } else { floor };
    if hi > lo {
        (lo, hi)
    } else {
        (hi / 10.0, hi)
    }
}

/// `"-0"`, `"-0.0"`, `"-0.000"`… — a zero that kept its sign.
fn is_negative_zero(label: &str) -> bool {
    label.strip_prefix('-').is_some_and(|rest| rest.chars().all(|c| c == '0' || c == '.'))
}

/// The axis-label contract: adjacent labels differ, values strictly
/// advance, and nothing reads as `-0`.
fn assert_distinct_labels(t: &[Tick], what: &str) {
    for pair in t.windows(2) {
        assert!(
            pair[0].label != pair[1].label,
            "{what}: adjacent labels repeat {:?}: {:?}",
            pair[0].label,
            labels(t)
        );
        assert!(pair[0].value != pair[1].value, "{what}: repeated value {}", pair[0].value);
    }
    for l in labels(t) {
        assert!(!is_negative_zero(l), "{what}: negative zero {l:?}");
    }
}

fn dump(t: Vec<Tick>) -> String {
    let mut s = format!("n={} ", t.len());
    for t in t {
        s.push_str(&format!("[{:?} {:?}] ", t.value, t.label));
    }
    s
}

/// Replays every input recorded from plotters 0.3.7 (1,409 of them:
/// linear, log, and time axes across spans from a millisecond to a
/// millennium, tick budgets 0–20, negative/reversed/tiny/huge/one-sided-NaN
/// ranges) and requires byte-identical values and labels. This is the
/// evidence that the in-house port is a drop-in replacement — except on
/// the lines the corpus header lists as deliberate departures, where
/// plotters broke the label contract and the line holds the fixed output.
#[test]
fn matches_plotters_corpus() {
    let corpus = include_str!("goldens/ticks_plotters_0_3_7.txt");
    let mut checked = 0;
    let mut failures = Vec::new();
    for line in corpus.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (head, want) = line.split_once(": ").expect("`<head>: <ticks>`");
        let mut parts = head.split(' ');
        let kind = parts.next().unwrap();
        let a = parts.next().unwrap();
        let b = parts.next().unwrap();
        let w: usize = parts.next().unwrap().strip_prefix("w=").unwrap().parse().unwrap();
        let a: f64 = a.parse().unwrap();
        let got = match kind {
            "lin" => ticks::linear(a, b.parse().unwrap(), w),
            "log" => {
                let (lo, hi) = clamp_log_range(a, b.parse().unwrap());
                ticks::log(lo, hi, w)
            }
            "time" => ticks::time(a, a + b.strip_prefix('+').unwrap().parse::<f64>().unwrap(), w),
            "time-odd" => ticks::time(a, b.parse().unwrap(), w),
            other => panic!("unknown corpus kind {other}"),
        };
        let got = dump(got);
        if got.trim_end() != want.trim_end() {
            failures.push(format!("{head}\n  plotters: {want}\n  in-house: {got}"));
        }
        checked += 1;
    }
    assert_eq!(checked, 1409, "corpus truncated?");
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
}

// ---------------------------------------------------------------- linear

#[test]
fn linear_picks_one_two_five_steps() {
    assert_eq!(values(&ticks::linear(0.0, 10.0, 5)), [0.0, 5.0, 10.0]);
    assert_eq!(values(&ticks::linear(0.0, 10.0, 20)).len(), 11);
    assert_eq!(labels(&ticks::linear(0.1, 0.3, 5)), ["0.1", "0.15", "0.2", "0.25", "0.3"]);
}

#[test]
fn linear_zero_span_is_one_tick() {
    let t = ticks::linear(5.0, 5.0, 5);
    assert_eq!(values(&t), [5.0]);
    assert_eq!(labels(&t), ["5.0"]);
    assert_eq!(values(&ticks::linear(0.0, 0.0, 5)), [0.0]);
}

#[test]
fn linear_negative_and_zero_crossing_ranges() {
    let neg = ticks::linear(-37.0, -3.0, 5);
    assert_eq!(values(&neg), [-30.0, -20.0, -10.0]);
    assert_eq!(labels(&neg), ["-30.0", "-20.0", "-10.0"]);
    let cross = ticks::linear(-5.0, 5.0, 5);
    assert_eq!(values(&cross), [-4.0, -2.0, 0.0, 2.0, 4.0]);
    assert_eq!(labels(&cross), ["-4.0", "-2.0", "0.0", "2.0", "4.0"]);
}

#[test]
fn linear_reversed_range_matches_forward() {
    assert_eq!(ticks::linear(10.0, 0.0, 5), ticks::linear(0.0, 10.0, 5));
}

#[test]
fn linear_tiny_and_huge_ranges() {
    let tiny = ticks::linear(1e-9, 2e-9, 5);
    assert_eq!(tiny.len(), 3);
    assert_eq!(tiny[0].value, 1e-9);

    let huge = ticks::linear(0.0, 1e15, 5);
    assert_eq!(values(&huge), [0.0, 5e14, 1e15]);
    assert_eq!(labels(&huge), ["0.0", "500000000000000.0", "1000000000000000.0"]);
}

/// plotters' printer stopped at 5 decimals, so every tick of a range
/// narrower than 1e-5 printed `"0.0"`. Labels now carry the step's
/// precision; an axis of tiny magnitudes switches to scientific notation.
#[test]
fn regression_linear_sub_1e5_range_labels_are_distinct() {
    assert_eq!(labels(&ticks::linear(1e-9, 2e-9, 5)), ["1.0e-9", "1.5e-9", "2.0e-9"]);
    assert_eq!(
        labels(&ticks::linear(-1e-12, 1e-12, 5)),
        ["-1.0e-12", "-5.0e-13", "0", "5.0e-13", "1.0e-12"]
    );
    // An offset range keeps fixed notation, at the step's precision.
    assert_eq!(
        labels(&ticks::linear(0.999999, 1.000001, 5)),
        ["0.999999", "0.9999995", "1.0", "1.0000005", "1.000001"]
    );
    assert_eq!(
        labels(&ticks::linear(1.0, 1.00000000000001, 5)),
        ["1.0", "1.000000000000005", "1.00000000000001"]
    );
    // The printer also rounded `-1e-5` itself to `"0.0"`.
    assert_eq!(
        labels(&ticks::linear(-1e-5, 1e-5, 5)),
        ["-1.0e-5", "-5.0e-6", "0", "5.0e-6", "1.0e-5"]
    );
    // Steps of 1e-5 and coarser keep plotters' labels exactly.
    assert_eq!(labels(&ticks::linear(0.0, 3e-5, 5)), ["0.0", "0.00001", "0.00002", "0.00003"]);
}

/// A negative value that rounded to zero printed as `"-0.0"` (`{:.0}` of
/// `-0.0` keeps the sign).
#[test]
fn regression_linear_never_labels_negative_zero() {
    for t in [
        ticks::linear(-1e-12, 1e-12, 20),
        ticks::linear(-1e-5, 0.0, 5),
        ticks::linear(-1e-310, 1e-310, 5),
        ticks::linear(-0.0, 0.0, 5),
    ] {
        for l in labels(&t) {
            assert!(!is_negative_zero(l), "{l:?} in {:?}", labels(&t));
        }
    }
    // A lone subnormal tick is labelled with its value, not `"-0.0"`.
    assert_eq!(labels(&ticks::linear(-1e-310, 1e-310, 5)), ["-1e-310"]);
    assert_eq!(labels(&ticks::linear(1e-320, 3e-320, 5)), ["1e-320"]);
}

/// The general rule, swept: over any range f64 can resolve, adjacent
/// linear tick labels differ and none reads as negative zero.
#[test]
fn linear_labels_are_distinct_over_a_sweep_of_ranges() {
    let centers: [f64; 10] = [0.0, 1e-9, -1e-9, 1.0, -1.0, 123.456, -7.25, 1e6, -1e9, 1e12];
    let mut checked = 0;
    for &c in &centers {
        for e in -12..=12 {
            for m in [1.0, 1.7, 3.3, 7.9] {
                let span = m * 10f64.powi(e);
                // Past ~1e-12 relative, the endpoints are a few ulps apart
                // and no labelling can tell them apart.
                if span < c.abs() * 1e-12 {
                    continue;
                }
                for (lo, hi) in [(c, c + span), (c - span / 2.0, c + span / 2.0)] {
                    for w in 2..=20 {
                        let t = ticks::linear(lo, hi, w);
                        assert_distinct_labels(&t, &format!("linear({lo:?}, {hi:?}, {w})"));
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 10_000);
}

#[test]
fn linear_zero_budget_is_empty() {
    assert!(ticks::linear(0.0, 10.0, 0).is_empty());
}

/// plotters hung forever on an infinite bound (`inf / 10` never shrinks
/// its step) and on a span that overflows f64.
#[test]
fn regression_linear_non_finite_span_terminates_empty() {
    assert!(ticks::linear(0.0, f64::INFINITY, 5).is_empty());
    assert!(ticks::linear(f64::NEG_INFINITY, 0.0, 5).is_empty());
    assert!(ticks::linear(-f64::MAX, f64::MAX, 5).is_empty());
}

/// plotters tripped `assert!(!range.0.is_nan())` on an all-NaN range; a
/// one-sided NaN collapses to the finite bound (unchanged behavior).
#[test]
fn regression_linear_nan_bounds_do_not_panic() {
    assert!(ticks::linear(f64::NAN, f64::NAN, 5).is_empty());
    assert_eq!(values(&ticks::linear(f64::NAN, 1.0, 5)), [1.0]);
}

// ---------------------------------------------------------------- log

#[test]
fn log_decades_and_sub_one_ranges() {
    let t = ticks::log(1.0, 1000.0, 5);
    assert_eq!(values(&t), [1.0, 10.0, 100.0, 1000.0]);
    assert_eq!(labels(&t), ["1", "10", "100", "1000"]);
    assert_eq!(labels(&ticks::log(0.001, 1.0, 5)), ["0.01", "0.1", "1"]);
}

#[test]
fn log_multi_decade_skips_decades_to_fit() {
    // 12 decades, budget 5 -> every other decade.
    let t = ticks::log(1.0, 1e12, 5);
    assert_eq!(values(&t), [1.0, 1e2, 1e4, 1e6, 1e8, 1e10, 1e12]);
    // 12 decades, budget 3 -> every third.
    assert_eq!(values(&ticks::log(1.0, 1e12, 3)), [1.0, 1e3, 1e6, 1e9, 1e12]);
}

#[test]
fn log_spare_budget_adds_in_decade_ticks() {
    let t = ticks::log(1.0, 10.0, 20);
    assert_eq!(t.len(), 11);
    assert_eq!(&labels(&t)[..3], ["1", "2", "3"]);
}

#[test]
fn log_single_decade_without_a_power_of_ten_inside() {
    // No decade boundary falls in [2, 3]: no ticks, as with plotters.
    assert!(ticks::log(2.0, 3.0, 5).is_empty());
}

/// plotters hung on these: a zero bound makes the decade walk start at
/// `10^-inf = 0` and multiply zero forever; an infinite bound never ends
/// it; an overflowing `end / start` saturates the decade count.
#[test]
fn regression_log_degenerate_bounds_terminate_empty() {
    assert!(ticks::log(0.0, 0.0, 5).is_empty());
    assert!(ticks::log(1.0, f64::INFINITY, 5).is_empty());
    assert!(ticks::log(1e-300, 1e300, 5).is_empty());
}

// ---------------------------------------------------------------- time

#[test]
fn time_seconds_minutes_hours() {
    let s = ticks::time(T0, T0 + 5_000.0, 5);
    assert_eq!(values(&s)[0], 1_710_078_433_000.0, "rounded up to the next whole second");
    assert_eq!(s.len(), 5);

    let m = ticks::time(T0, T0 + 600_000.0, 5);
    assert_eq!(labels(&m), ["13:48", "13:50", "13:52", "13:54", "13:56"]);

    let h = ticks::time(T0, T0 + 3.0 * 3_600_000.0, 5);
    assert_eq!(labels(&h), ["14:00", "15:00", "16:00"]);
}

#[test]
fn time_days_and_weeks_land_on_utc_midnight() {
    let d = ticks::time(T0, T0 + 3.0 * MS_PER_DAY, 5);
    assert_eq!(labels(&d), ["Mar 11", "Mar 12", "Mar 13"]);
    assert!(d.iter().all(|t| t.value % MS_PER_DAY == 0.0));

    let w = ticks::time(T0, T0 + 45.0 * MS_PER_DAY, 5);
    assert_eq!(labels(&w), ["Mar 11", "Mar 25", "Apr 08", "Apr 22"]);
}

#[test]
fn time_multi_year_uses_year_labels() {
    let y = ticks::time(1_704_067_200_000.0, 1_704_067_200_000.0 + 3650.0 * MS_PER_DAY, 5);
    assert_eq!(labels(&y), ["2024", "2026", "2028", "2030", "2032"]);
}

#[test]
fn time_zero_span_is_empty() {
    assert!(ticks::time(T0, T0, 5).is_empty());
}

#[test]
fn time_reversed_by_under_a_week_gives_one_tick() {
    let t = ticks::time(T0, T0 - MS_PER_DAY, 5);
    assert_eq!(values(&t), [1_710_115_200_000.0]);
}

/// plotters divided by a zero week step on a range reversed by a week or
/// more.
#[test]
fn regression_time_reversed_by_weeks_does_not_panic() {
    assert!(ticks::time(T0, T0 - 30.0 * MS_PER_DAY, 5).is_empty());
}

/// plotters overflowed `10u64.pow(u32::MAX)` when asked for zero ticks over
/// any span shorter than ~292 years.
#[test]
fn regression_time_zero_budget_does_not_panic() {
    assert!(ticks::time(T0, T0 + 3_600_000.0, 0).is_empty());
    assert!(ticks::time(T0, T0 + 400.0 * MS_PER_DAY, 0).is_empty());
}

#[test]
fn time_out_of_calendar_range_falls_back_to_linear() {
    assert_eq!(ticks::time(0.0, 1e18, 5), ticks::linear(0.0, 1e18, 5));
}

/// An infinite bound saturates to `i64::MAX` ms, which is out of chrono's
/// range, so it reaches the linear fallback — where plotters hung.
#[test]
fn regression_time_infinite_bound_terminates() {
    assert!(ticks::time(0.0, f64::INFINITY, 5).is_empty());
}
