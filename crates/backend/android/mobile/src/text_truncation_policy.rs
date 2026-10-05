//! `max_lines` → `TextView.setMaxLines` + `setEllipsize` mapping, and the
//! decision of when the JNI calls must run.
//!
//! Un-gated (compiles on any host) so the tests run from any platform,
//! same pattern as `border_dash_policy` / `transform_transition_policy`.
//! The JNI half lives in `imp/style.rs` (`apply_rules`).
//!
//! ## Why "untouched" and "no limit" are the same cached state
//!
//! The per-node cache (`NodeAnim::last_max_lines`) starts at `None`, which
//! means "the widget still has its own defaults" — and that is also what
//! "no limit" resets to (`maxLines = Integer.MAX_VALUE`, `ellipsize =
//! null`, TextView's constructor values). So a text that never sets
//! `max_lines` costs no JNI call at all, and widgets that are TextView
//! subclasses with their own defaults (a `Button` title) are never
//! touched unless the author asked for a limit.
//!
//! ## Measurement
//!
//! Nothing here feeds Taffy: the text leaf's measure_fn
//! (`primitives::text::measure_textview`) drives `TextView.measure`, and
//! TextView's own `onMeasure` already caps the height at `maxLines` lines
//! and builds the ellipsized layout at the given width. The only thing
//! measurement needs is for the leaf to be re-measured after a change,
//! which `LayoutTree::set_style` guarantees: Taffy's `set_style` marks
//! the node dirty unconditionally, and `apply_style_impl` calls it right
//! after `apply_rules` on every style apply.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

/// `java.lang.Integer.MAX_VALUE` — TextView's default `maxLines`
/// (`mMaximum`), which a removed limit restores.
pub(crate) const UNLIMITED_MAX_LINES: i32 = i32::MAX;

/// What to push to the TextView for one `max_lines` value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineLimitCall {
    /// Argument to `TextView.setMaxLines(int)`.
    pub(crate) max_lines: i32,
    /// `true` → `setEllipsize(TextUtils.TruncateAt.END)`; `false` →
    /// `setEllipsize(null)` (TextView's default).
    pub(crate) ellipsize_end: bool,
}

/// Normalize the style value: `Some(0)` means "no limit", same as `None`
/// (the `StyleRules::max_lines` contract).
pub(crate) fn effective_limit(max_lines: Option<u32>) -> Option<u32> {
    max_lines.filter(|&n| n > 0)
}

/// The setter arguments for an effective limit. `None` restores the
/// TextView defaults. A limit beyond `i32::MAX` (Java's `int`) is
/// clamped — it is unlimited in practice.
pub(crate) fn line_limit_call(limit: Option<u32>) -> LineLimitCall {
    match limit {
        Some(n) => LineLimitCall {
            max_lines: i32::try_from(n).unwrap_or(UNLIMITED_MAX_LINES),
            ellipsize_end: true,
        },
        None => LineLimitCall { max_lines: UNLIMITED_MAX_LINES, ellipsize_end: false },
    }
}

/// Decide whether this style apply must call the setters. Returns the
/// call to make (and the caller stores `limit` as the new cached value),
/// or `None` when the view already shows `limit`.
///
/// `last` is the cached effective limit; `None` = the view has its
/// defaults (never limited, or a limit was already removed).
pub(crate) fn line_limit_update(
    last: Option<u32>,
    max_lines: Option<u32>,
) -> Option<LineLimitCall> {
    let want = effective_limit(max_lines);
    (want != last).then(|| line_limit_call(want))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_text_never_calls_the_setters() {
        // A text without max_lines (or with 0) on a fresh view: no JNI.
        assert_eq!(line_limit_update(None, None), None);
        assert_eq!(line_limit_update(None, Some(0)), None);
    }

    #[test]
    fn one_line_limit_sets_max_lines_one_with_end_ellipsis() {
        assert_eq!(
            line_limit_update(None, Some(1)),
            Some(LineLimitCall { max_lines: 1, ellipsize_end: true })
        );
    }

    #[test]
    fn multi_line_limit_sets_that_many_lines_with_end_ellipsis() {
        assert_eq!(
            line_limit_update(None, Some(3)),
            Some(LineLimitCall { max_lines: 3, ellipsize_end: true })
        );
    }

    #[test]
    fn identical_reapply_is_skipped() {
        assert_eq!(line_limit_update(Some(2), Some(2)), None);
    }

    #[test]
    fn changing_the_limit_reapplies() {
        assert_eq!(
            line_limit_update(Some(2), Some(1)),
            Some(LineLimitCall { max_lines: 1, ellipsize_end: true })
        );
    }

    #[test]
    fn regression_removing_max_lines_restores_textview_defaults() {
        // A reactive restyle that drops the limit must undo it — otherwise
        // the text stays cut at the old line count with an ellipsis.
        let reset = LineLimitCall { max_lines: i32::MAX, ellipsize_end: false };
        assert_eq!(line_limit_update(Some(1), None), Some(reset));
        assert_eq!(line_limit_update(Some(4), Some(0)), Some(reset));
    }

    #[test]
    fn limit_beyond_java_int_clamps_to_unlimited() {
        assert_eq!(
            line_limit_call(Some(u32::MAX)),
            LineLimitCall { max_lines: i32::MAX, ellipsize_end: true }
        );
    }
}
