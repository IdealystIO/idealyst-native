//! Pure layout-scheduling policy for the Android backend, kept un-gated so the
//! regression coverage runs from any host (`cargo test -p backend-android-mobile`).
//!
//! The JNI-driven `insert` path lives in `imp` (`target_os = "android"`); this
//! module holds only the decision it makes, so the policy is testable without a
//! live `View` tree. Same rationale as `sticky_compute` — see its module docs.

/// Should a just-completed `insert` kick a coalesced layout pass?
///
/// A subtree that mounts AFTER the initial build's `finish()` layout pass — a
/// portal opening, or any reactive control-flow child (`when` toggling true, a
/// `switch`/`match` branch swapping, an `Each` row inserting, a `presence`
/// entering) — has no upcoming `finish()` to size it. It must request its own
/// pass or it renders at default 0×0 `LayoutParams` and is invisible (the
/// "`when`-mounted camera widget never appears" bug).
///
/// - `is_portal_parent`: the parent is a portal content holder (always a
///   dynamic mount, regardless of attachment).
/// - `parent_attached_to_window`: the parent is already live in the window
///   hierarchy — Android's `View.isAttachedToWindow()` is the signal that the
///   initial `finish()` pass has run, so an insert now is a later dynamic
///   mount. A floating, mid-build parent is `false` here, so its inserts defer
///   to the upcoming `finish()` pass; scheduling against a partial tree would
///   compute and cache wrong sizes (the iOS mirror is `parent.window != nil` —
///   see `project_ios_insert_layout_discriminator`).
///
/// The coalescing flag in `imp::scheduler` (`LAYOUT_PASS_QUEUED`) collapses a
/// burst of sibling inserts in one runloop turn into a single pass, so a `true`
/// result is cheap even when a list mounts many rows at once.
// Consumed by `imp::insert` (android-only) and the tests below; on a host
// non-test lib build neither references it, hence the allow.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn insert_needs_layout_pass(
    is_portal_parent: bool,
    parent_attached_to_window: bool,
) -> bool {
    is_portal_parent || parent_attached_to_window
}

/// Android `ViewGroup.LayoutParams` sentinels. Named because `-1` and `-2`
/// in a size field are the two magic numbers this file exists to keep honest.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const MATCH_PARENT: i32 = -1;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const WRAP_CONTENT: i32 = -2;

/// What a style `width`/`height` becomes in `LayoutParams`, for every
/// `Length` that is not `Px` (which needs a live `View` for the dp→px
/// conversion and so is resolved by the caller).
///
/// `Full` maps to `WRAP_CONTENT`, with `Auto`. It is a corner-radius
/// concept — "half the shorter side, resolved when painted" — and carries
/// no meaning as a box dimension, so the framework's own shared mapping
/// in `runtime_layout` already folds the two together
/// (`FwLength::Auto | FwLength::Full => Dimension::Auto`). Android agreeing
/// is what keeps the same style tree the same size on every backend.
///
/// This exists as a function rather than a `match` at each call site
/// because it was two hand-rolled matches, and when `Length::Full` was
/// added they were the sites that did not learn about it — the Android
/// backend stopped compiling for `aarch64-linux-android` and stayed that
/// way. One arm-set, host-testable, is the fix that does not rot.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn non_px_layout_param(length: &runtime_shared::Length) -> Option<i32> {
    match length {
        runtime_shared::Length::Px(_) => None,
        runtime_shared::Length::Percent(_) => Some(MATCH_PARENT),
        runtime_shared::Length::Auto | runtime_shared::Length::Full => Some(WRAP_CONTENT),
    }
}

#[cfg(test)]
mod length_param_tests {
    use super::*;
    use runtime_shared::Length;

    /// Regression: `Length::Full` was added without these two matches
    /// learning about it, and `backend-android-mobile` stopped compiling
    /// for `aarch64-linux-android` — a whole backend dark, with the break
    /// invisible to `cargo test` because `imp/style.rs` is android-only.
    ///
    /// It folds in with `Auto` because that is what the framework's own
    /// shared mapping does (`runtime_layout`:
    /// `FwLength::Auto | FwLength::Full => Dimension::Auto`). `Full` is a
    /// corner-radius idea with no meaning as a box dimension.
    #[test]
    fn regression_full_sizes_like_auto() {
        assert_eq!(non_px_layout_param(&Length::Full), Some(WRAP_CONTENT));
        assert_eq!(
            non_px_layout_param(&Length::Full),
            non_px_layout_param(&Length::Auto),
            "Full and Auto must not diverge as dimensions"
        );
    }

    #[test]
    fn percent_fills_the_parent_and_px_defers_to_the_caller() {
        assert_eq!(non_px_layout_param(&Length::Percent(50.0)), Some(MATCH_PARENT));
        assert_eq!(
            non_px_layout_param(&Length::Px(12.0)),
            None,
            "Px needs a live View for dp->px, so the caller resolves it"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::insert_needs_layout_pass;

    /// Regression: a `when`/`Each`/`presence` child mounting into a live
    /// (window-attached) non-portal parent MUST schedule a layout pass. The
    /// pre-fix code only scheduled for portals, so this case returned `false`
    /// and the dynamically-mounted subtree stayed at 0×0 — the camera/record
    /// widgets in the whiteboard demo never appeared. Fails against the old
    /// portals-only behavior; passes after the fix.
    #[test]
    fn dynamic_mount_into_attached_nonportal_parent_schedules_pass() {
        assert!(insert_needs_layout_pass(false, true));
    }

    /// A mid-build insert into a floating (not-yet-attached) parent must NOT
    /// schedule — the upcoming `finish()` pass sizes it, and a pass against a
    /// partial tree would cache wrong sizes.
    #[test]
    fn mid_build_insert_into_floating_parent_defers() {
        assert!(!insert_needs_layout_pass(false, false));
    }

    /// Portals always schedule, regardless of attachment state at insert time.
    #[test]
    fn portal_parent_always_schedules() {
        assert!(insert_needs_layout_pass(true, false));
        assert!(insert_needs_layout_pass(true, true));
    }
}
