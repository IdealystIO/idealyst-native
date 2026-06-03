//! Pure index-clamping policy for the iOS backend's anchorless
//! reactive-region splice (`Backend::insert_at`), kept un-gated so the
//! regression coverage runs from any host (`cargo test -p
//! backend-ios-mobile`).
//!
//! The objc-driven `insert_at` path lives in `imp` (`target_os = "ios"`)
//! and is pure UIKit plumbing — the only non-trivial *decision* it makes
//! is clamping the requested splice index to the parent's current subview
//! count, because UIKit's `-[UIView insertSubview:atIndex:]` raises an
//! `NSRangeException` when `index > subviews.count`. That clamp is the one
//! extractable pure bit, so it lives here with a unit test. Same rationale
//! as Android's `layout_policy` module.

/// Clamp a requested splice `index` to the valid `[0, child_count]` range
/// for `-[UIView insertSubview:atIndex:]`.
///
/// `index == child_count` is valid (appends at the end), so the upper
/// bound is inclusive. An out-of-range `index` (which the anchorless
/// `when`/`each` splice can produce in edge cases where preceding static
/// siblings haven't all mounted yet) would otherwise throw an
/// `NSRangeException` and abort the app; clamping appends instead, matching
/// Android's `addView(view, idx)` clamp and the `add_child_at_index`
/// Taffy-side clamp.
// Consumed by `imp::insert_at` (ios-only) and the tests below; on a host
// non-test lib build neither references it, hence the allow.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn clamp_insert_index(index: usize, child_count: usize) -> usize {
    index.min(child_count)
}

#[cfg(test)]
mod tests {
    use super::clamp_insert_index;

    /// An in-range index passes through untouched.
    #[test]
    fn in_range_index_unchanged() {
        assert_eq!(clamp_insert_index(0, 3), 0);
        assert_eq!(clamp_insert_index(1, 3), 1);
        assert_eq!(clamp_insert_index(2, 3), 2);
    }

    /// `index == count` is the valid "append at end" position and must NOT
    /// be clamped down — UIKit accepts it.
    #[test]
    fn index_equal_to_count_appends() {
        assert_eq!(clamp_insert_index(3, 3), 3);
        assert_eq!(clamp_insert_index(0, 0), 0);
    }

    /// Regression: an out-of-range index (the anchorless splice's
    /// `base_index` can exceed the live subview count if preceding static
    /// siblings haven't mounted yet) MUST clamp to the count rather than be
    /// passed to `insertSubview:atIndex:` verbatim — UIKit would raise
    /// `NSRangeException` and abort. Clamping appends, matching Android.
    #[test]
    fn out_of_range_index_clamps_to_count() {
        assert_eq!(clamp_insert_index(5, 3), 3);
        assert_eq!(clamp_insert_index(1, 0), 0);
        assert_eq!(clamp_insert_index(usize::MAX, 2), 2);
    }
}
