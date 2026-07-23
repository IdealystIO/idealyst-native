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

#[cfg(test)]
mod tests {
    use super::clamp_f32;

    #[test]
    fn clamp_f32_clamps_and_defuses_nan() {
        assert_eq!(clamp_f32(0.5, 0.0, 1.0), 0.5);
        assert_eq!(clamp_f32(-2.0, 0.0, 1.0), 0.0);
        assert_eq!(clamp_f32(7.0, 0.0, 1.0), 1.0);
        // NaN input lands on `lo` (defined output), unlike std's clamp.
        assert_eq!(clamp_f32(f32::NAN, 0.0, 1.0), 0.0);
    }
}
