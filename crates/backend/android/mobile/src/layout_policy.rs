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
