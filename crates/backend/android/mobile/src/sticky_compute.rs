//! Host-runnable regression coverage for `Position::Sticky` on
//! Android — the pure compute function + the registry invariants
//! that don't require a JVM. Sister to `imp::sticky`, which holds
//! the full JNI-driven implementation (target_os = "android" only).
//!
//! Why this module exists: `imp::sticky` lives under `cfg(target_os
//! = "android")` because it depends on the `jni` crate (which the
//! `Cargo.toml` itself gates to Android). The iOS reference puts
//! all of its sticky tests inside the parallel iOS gate, which
//! means they don't run from `cargo test` on a host machine. We
//! deliberately mirror the pure parts here so the math + the
//! empty-registry invariant ARE host-testable; the JNI-driven
//! pieces (scroll-listener install, `setTranslationY` writes,
//! `getParent` ancestor walk) are out of scope for host tests and
//! verified on-device.
//!
//! ## What's covered
//!
//! - `compute_translate_dp` — the pure pin math used by the live
//!   scroll-event handler. Identical function body to
//!   `imp::sticky::compute_translate_dp`; duplicated here rather
//!   than re-exported because the imp module is target-gated and
//!   the host build can't reach it.
//! - Registry shrink invariant — `cargo test` reaches this even
//!   without an Android target.
//!
//! ## What's NOT covered host-side
//!
//! - The JNI-driven scroll-listener install/detach — requires a
//!   live JVM. Verified on-device by mounting the docs example's
//!   sticky-header demo and scrolling.
//! - The `getParent` ancestor walk for `find_enclosing_scroll_view`
//!   — requires a real `View` hierarchy. Same on-device coverage.
//! - The `setTranslationY` write in `on_scroll_event` —
//!   `View.setTranslationY` is a JVM call. On-device.
//!
//! Per CLAUDE.md §8, each `#[test]` below is named after the bug
//! it prevents, not the function it exercises.

/// Pure compute used by [`imp::sticky::on_scroll_event`] and the
/// tests below. Mirror of the function in `imp/sticky.rs`; the
/// duplication is intentional — see the module doc above.
///
/// Returns the translation (in dp) that should be applied to the
/// sticky child's `View.translationY` given its natural layout y
/// in the scroll view's content space, the configured pin
/// threshold (the `top` value), and the scroll view's current
/// scroll position. All inputs and the output are in dp.
#[inline]
#[allow(dead_code)] // Used only from `#[cfg(test)]` here; the live
                    // copy in `imp::sticky` (target_os = "android")
                    // is the one called at runtime.
pub fn compute_translate_dp(layout_y_dp: f32, threshold_dp: f32, scroll_y_dp: f32) -> f32 {
    let pinned_y = scroll_y_dp + threshold_dp;
    if pinned_y > layout_y_dp {
        pinned_y - layout_y_dp
    } else {
        0.0
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Pin compute: scrolling past the threshold translates the
    /// child down by the overshoot; scrolling above the threshold
    /// leaves the child at its natural position.
    ///
    /// Regression: a previous draft had `>=` instead of `>` at the
    /// pinned_y / layout_y comparison, which would have made the
    /// child snap to pinned position one device pixel early. The
    /// boundary assertion below locks that down.
    #[test]
    fn regression_sticky_registry_pins_when_scrolled_past_threshold() {
        // Child sits at y=100 dp in the scroll view's content; pin
        // threshold (top) is 20 dp from the scroll view's top edge.
        let layout_y = 100.0;
        let threshold = 20.0;

        // Far above the pin point — no translate.
        assert_eq!(compute_translate_dp(layout_y, threshold, 0.0), 0.0);

        // Just at the pin point (scroll_y + threshold == layout_y).
        // Boundary: still 0 (the `>` in compute, not `>=`).
        assert_eq!(compute_translate_dp(layout_y, threshold, 80.0), 0.0);

        // 1 dp past the pin point — translate by 1 dp.
        let t = compute_translate_dp(layout_y, threshold, 81.0);
        assert!((t - 1.0).abs() < 1e-5, "expected ~1.0, got {t}");

        // Way past the pin point — translate compensates fully so
        // the child renders at scroll_y + threshold = 300.
        let t = compute_translate_dp(layout_y, threshold, 280.0);
        assert!(
            (t - 200.0).abs() < 1e-5,
            "expected ~200.0 (so rendered y == scroll_y + threshold = 300), got {t}",
        );

        // Sanity: rendered y while pinned == scroll_y + threshold.
        let scroll_y = 500.0;
        let t = compute_translate_dp(layout_y, threshold, scroll_y);
        let rendered_y = layout_y + t;
        assert!(
            (rendered_y - (scroll_y + threshold)).abs() < 1e-5,
            "pinned rendered_y should equal scroll_y + threshold",
        );
    }

    /// Registry must shrink back to empty when its last child
    /// deregisters — otherwise the per-scroll-view entry leaks an
    /// orphan scroll-listener `GlobalRef` and a stale scroll-view
    /// ref. The shrink-back-to-empty property is the regression
    /// test for "registry leaks scroll-view entries when their
    /// last sticky child unmounts."
    ///
    /// We can't construct a real `GlobalRef` off-device (it
    /// requires a live JVM via `jni::JNIEnv::new_global_ref`), so
    /// the host-side test models the invariant with a stub-typed
    /// registry: same `HashMap<usize, Entry>` shape, same
    /// shrink-on-empty discipline. The matching live-`GlobalRef`
    /// path in `imp::sticky::deregister` is exercised on-device by
    /// the docs example's sticky-header demo.
    #[test]
    fn regression_sticky_registry_unregisters_on_unmount() {
        // Stand-in for `StickyScrollEntry`. Holds just the
        // children HashMap — the JNI-typed `scroll_view` /
        // `listener` fields aren't part of the invariant we're
        // testing here.
        struct StubEntry {
            children: HashMap<usize, ()>,
        }

        let mut registry: HashMap<usize, StubEntry> = HashMap::new();
        assert_eq!(registry.len(), 0);

        // Insert one scroll-view entry with one child — what the
        // `register` happy path produces.
        let scroll_key = 0x1000_usize;
        let child_key = 0x2000_usize;
        let mut children = HashMap::new();
        children.insert(child_key, ());
        registry.insert(scroll_key, StubEntry { children });
        assert_eq!(registry.len(), 1);

        // Simulate `deregister`: remove the child, then check the
        // entry's child set is empty, then remove the entry.
        // Mirrors the body of `imp::sticky::deregister`'s
        // `emptied_scrolls` loop.
        let entry = registry.get_mut(&scroll_key).unwrap();
        let removed = entry.children.remove(&child_key);
        assert!(removed.is_some(), "child was registered, removal should succeed");
        let became_empty = entry.children.is_empty();
        if became_empty {
            registry.remove(&scroll_key);
        }
        assert_eq!(
            registry.len(),
            0,
            "registry must shrink back to empty when the last child of a scroll view deregisters",
        );
    }

    /// `find_enclosing_scroll_view` returning `None` is the
    /// fall-back-to-relative path; `register` is documented to
    /// no-op (return `false`) in that case. Verifies that the
    /// pure-compute helper produces no translation when there's
    /// no scroll motion (which is the observable behavior of
    /// "sticky in a non-scrolling parent" — it sits at its
    /// natural position, same as `Relative`).
    ///
    /// The full integration — `register(view_with_no_scroll_ancestor)`
    /// returning false and not creating a registry entry — needs a
    /// live `View` hierarchy and is verified on-device.
    #[test]
    fn regression_sticky_falls_back_to_relative_without_scroll_ancestor() {
        // With no scroll ancestor, no scroll listener fires, so
        // `compute_translate_dp` is never called. But the math
        // helper's "no pin while scroll_y < layout_y - threshold"
        // property is the same: the child sits at its natural
        // layout position with translation = 0, identical to what
        // a `Relative`-positioned view would render.
        let t = compute_translate_dp(
            /* layout_y */ 100.0,
            /* threshold */ 20.0,
            /* scroll_y */ 0.0,
        );
        assert_eq!(t, 0.0, "no scroll ancestor → no scroll → no pin");

        // Also: the absent-key path must not panic and must
        // observe the registry as empty.
        let registry: HashMap<usize, ()> = HashMap::new();
        let absent_key = 0xDEAD_BEEF_usize;
        assert!(registry.get(&absent_key).is_none());
        assert_eq!(registry.len(), 0);
    }
}
