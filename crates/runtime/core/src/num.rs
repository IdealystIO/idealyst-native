//! Small numeric helpers with wasm-bundle-friendly codegen.

/// Clamp `v` to `[lo, hi]` without `f32::clamp`'s panic path.
///
/// `f32::clamp` asserts `lo <= hi` with a panic message that formats both
/// bounds via `{:?}` — which links `<f32 as Debug>::fmt` and, through it,
/// core's full shortest-representation float formatter (`flt2dec` dragon +
/// grisu, ~12–15 KB of wasm) into every bundle that clamps a float anywhere.
/// The framework's bounds are always well-ordered constants, so the check
/// buys nothing. Use this in framework/backend code instead of `.clamp(..)`.
///
/// Semantics vs `f32::clamp`: a NaN `v` comes out as `lo` (via `max`/`min`'s
/// NaN-ignoring behavior) instead of propagating — for the CSS-/style-bound
/// values this is used on, a defined in-range result beats emitting "NaN".
#[inline]
pub fn clamp_f32(v: f32, lo: f32, hi: f32) -> f32 {
    v.max(lo).min(hi)
}

/// Stable insertion sort for the framework's tiny, bounded lists
/// (breakpoint overlays, container-query thresholds, gradient stops,
/// keyframe stops — all in-practice under ~16 elements).
///
/// Why not `slice::sort_by`: std's stable sort monomorphizes a full
/// drift + quicksort + smallsort stack (~2.5–3 KB of wasm) **per element
/// type**; the all-off baseline carried 71 sort instantiations totalling
/// ~18 KB, dominated by these tiny style lists. Insertion sort compiles
/// to ~100–300 bytes per type, is stable (equal-offset gradient stops are
/// CSS hard stops — relative order is meaningful), and on lists this
/// size is also simply fast. Do NOT use for unbounded/user-sized data —
/// it's O(n²); std sort remains correct there.
///
/// `cmp` returning `Ordering::Equal` keeps the original relative order.
pub fn insertion_sort_by<T>(v: &mut [T], mut cmp: impl FnMut(&T, &T) -> core::cmp::Ordering) {
    for i in 1..v.len() {
        let mut j = i;
        while j > 0 && cmp(&v[j - 1], &v[j]) == core::cmp::Ordering::Greater {
            v.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// Parse a plain decimal number (`"1280"`, `"-3.5"`, `"812.75"`) to `f32`
/// without `f32::from_str`.
///
/// Why: std's float parser links core's full dec2flt machinery — the
/// correctly-rounded slow path plus its power tables, ~5-6 KB of wasm —
/// and ONE reachable call site pays all of it (the web backend's
/// `ssr_viewport` attribute read was that site). The framework's inputs
/// here are self-emitted, small, plain decimals (viewport dimensions,
/// never scientific notation / inf / NaN), where integer math is exact
/// far beyond the needed precision.
///
/// Accepts: optional `-`/`+`, digits, optional `.` + up to 9 fraction
/// digits (further digits are truncated, not rounded). Rejects
/// everything else — including empty input, bare `.`, exponents, and
/// values whose integer part overflows u64. NOT a general float parser;
/// author-facing surfaces that accept arbitrary floats should keep
/// `str::parse`.
pub fn parse_f32_plain(s: &str) -> Option<f32> {
    let b = s.as_bytes();
    let (neg, rest) = match b.first()? {
        b'-' => (true, &b[1..]),
        b'+' => (false, &b[1..]),
        _ => (false, b),
    };
    let mut int: u64 = 0;
    let mut i = 0;
    while i < rest.len() && rest[i].is_ascii_digit() {
        int = int.checked_mul(10)?.checked_add((rest[i] - b'0') as u64)?;
        i += 1;
    }
    let int_digits = i;
    let mut frac: u64 = 0;
    let mut frac_scale: u64 = 1;
    let mut frac_digits = 0;
    if i < rest.len() && rest[i] == b'.' {
        i += 1;
        while i < rest.len() && rest[i].is_ascii_digit() {
            if frac_digits < 9 {
                frac = frac * 10 + (rest[i] - b'0') as u64;
                frac_scale *= 10;
                frac_digits += 1;
            }
            i += 1;
        }
        if frac_digits == 0 && int_digits == 0 {
            return None; // bare "." / "-."
        }
    } else if int_digits == 0 {
        return None; // no digits at all
    }
    if i != rest.len() {
        return None; // trailing junk (incl. exponents)
    }
    let v = int as f64 + (frac as f64 / frac_scale as f64);
    Some(if neg { -v as f32 } else { v as f32 })
}

#[cfg(test)]
mod tests {
    use super::{clamp_f32, insertion_sort_by, parse_f32_plain};

    /// Regression for the bug this fn exists to prevent: `ssr_viewport`'s
    /// `str::parse::<f32>()` was the lone anchor keeping core's dec2flt
    /// float-parse machinery (~5-6 KB) in every web bundle.
    #[test]
    fn regression_parse_f32_plain_matches_std_for_framework_inputs() {
        for s in ["0", "1280", "800", "812.75", "-3.5", "+2.25", "0.125", "99999.999"] {
            let ours = parse_f32_plain(s).unwrap();
            let std = s.parse::<f32>().unwrap();
            assert!(
                (ours - std).abs() <= f32::EPSILON * std.abs().max(1.0),
                "{s}: {ours} vs {std}"
            );
        }
        // Rejections: not a general float parser.
        for s in ["", ".", "-", "1e3", "1.2.3", " 1", "1 ", "abc", "NaN", "inf"] {
            assert!(parse_f32_plain(s).is_none(), "must reject {s:?}");
        }
        // Truncation beyond 9 fraction digits, never a panic/overflow.
        assert!(parse_f32_plain("1.12345678901234").is_some());
        assert!(parse_f32_plain("99999999999999999999999999").is_none(), "u64 overflow rejected");
    }

    #[test]
    fn insertion_sort_sorts_and_is_stable() {
        let mut v = vec![(3, 'a'), (1, 'b'), (2, 'c'), (1, 'd'), (3, 'e')];
        insertion_sort_by(&mut v, |a, b| a.0.cmp(&b.0));
        // Sorted by key; equal keys keep insertion order (b before d, a before e).
        assert_eq!(v, vec![(1, 'b'), (1, 'd'), (2, 'c'), (3, 'a'), (3, 'e')]);

        let mut empty: Vec<i32> = vec![];
        insertion_sort_by(&mut empty, |a, b| a.cmp(b));
        assert!(empty.is_empty());

        let mut one = vec![42];
        insertion_sort_by(&mut one, |a, b| a.cmp(b));
        assert_eq!(one, vec![42]);

        // Floats with the partial_cmp pattern the style sites use.
        let mut f = vec![0.5f32, 0.0, 1.0, 0.25];
        insertion_sort_by(&mut f, |a, b| {
            a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal)
        });
        assert_eq!(f, vec![0.0, 0.25, 0.5, 1.0]);
    }

    #[test]
    fn clamp_f32_clamps_and_defuses_nan() {
        assert_eq!(clamp_f32(0.5, 0.0, 1.0), 0.5);
        assert_eq!(clamp_f32(-2.0, 0.0, 1.0), 0.0);
        assert_eq!(clamp_f32(7.0, 0.0, 1.0), 1.0);
        // NaN input lands on `lo` (defined output), unlike std's clamp.
        assert_eq!(clamp_f32(f32::NAN, 0.0, 1.0), 0.0);
    }
}
