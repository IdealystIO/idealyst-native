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

#[cfg(test)]
mod tests {
    use super::{clamp_f32, insertion_sort_by};

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
