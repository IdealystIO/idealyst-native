//! Host-runnable regression coverage for `Position::Sticky` on
//! Android — the axis/write policy and the registry invariants that
//! don't require a JVM. Sister to `imp::sticky`, which holds the full
//! JNI-driven implementation (target_os = "android" only).
//!
//! Why this module exists: `imp::sticky` lives under `cfg(target_os
//! = "android")` because it depends on the `jni` crate (which the
//! `Cargo.toml` itself gates to Android). The iOS reference puts
//! all of its sticky tests inside the parallel iOS gate, which
//! means they don't run from `cargo test` on a host machine. We
//! deliberately mirror the pure parts here so the axis policy + the
//! empty-registry invariant ARE host-testable; the JNI-driven
//! pieces (scroll-listener install, `setTranslationX/Y` writes,
//! `getParent` ancestor walk) are out of scope for host tests and
//! verified on-device.
//!
//! ## What's covered
//!
//! - Which axis the tick writes, via the shared
//!   `runtime_shared::sticky::translate`. Android has two
//!   INDEPENDENT translation setters (unlike UIKit's single
//!   `setTransform:`), so "the unpinned axis stays exactly 0" is an
//!   Android-specific correctness property, not just restated math.
//!   The math's own regressions live once, with the math.
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
//! - The `setTranslationX/Y` writes in `on_scroll_event` — both are
//!   JVM calls. On-device.
//!
//! Per CLAUDE.md §8, each `#[test]` below is named after the bug
//! it prevents, not the function it exercises.

// The pin arithmetic no longer lives here (nor in `imp::sticky`): it
// is `runtime_shared::sticky`, shared by every backend and tested
// there. This module keeps the host-runnable ANDROID-specific
// invariants — the registry's shrink-on-empty discipline and the
// per-axis write policy — which is what it was always for.
use runtime_shared::sticky::{translate, StickyInsets};

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Which AXIS the tick writes. The pin arithmetic's own
    /// regressions (including the `>` vs `>=` boundary) live with the
    /// arithmetic in `runtime_shared::sticky`; what Android owns is
    /// that `setTranslationX` and `setTranslationY` are INDEPENDENT
    /// setters, so an axis the element does not pin on must come back
    /// exactly `0.0` — `write_translate` epsilon-gates each axis
    /// separately and would otherwise leave a stale translation on
    /// the free axis.
    #[test]
    fn regression_sticky_registry_pins_when_scrolled_past_threshold() {
        // Child sits at y=100 dp in the scroll view's content; pin
        // threshold (top) is 20 dp from the scroll view's top edge.
        let vertical = StickyInsets { top: Some(20.0), ..Default::default() };
        let natural = (12.0_f32, 100.0_f32);
        let size = (80.0_f32, 40.0_f32);
        let viewport = (411.0_f32, 731.0_f32);

        // Far above the pin point — no translate on either axis.
        assert_eq!(translate(vertical, natural, size, (0.0, 0.0), viewport), (0.0, 0.0));

        // Way past the pin point: y compensates fully so the child
        // renders at scroll_y + threshold = 300 dp, and x stays 0 so
        // `setTranslationX` is never written.
        let (dx, dy) = translate(vertical, natural, size, (0.0, 280.0), viewport);
        assert_eq!(dx, 0.0, "a vertical-only pin must not write translationX");
        assert!(
            ((natural.1 + dy) - 300.0).abs() < 1e-5,
            "pinned rendered y should equal scroll_y + threshold",
        );
    }

    /// A frozen COLUMN on Android: `left` pins off `getScrollX()`
    /// while the child scrolls freely vertically. Before horizontal
    /// support the registry carried a single `threshold_top`, so
    /// `left` wrote nothing at all.
    #[test]
    fn regression_sticky_left_never_pins_horizontally() {
        let horizontal = StickyInsets { left: Some(0.0), ..Default::default() };
        let (dx, dy) = translate(horizontal, (160.0, 40.0), (80.0, 24.0), (600.0, 250.0), (411.0, 731.0));
        assert!(
            ((160.0 + dx) - 600.0).abs() < 1e-5,
            "pinned rendered x should equal scroll_x + threshold",
        );
        assert_eq!(dy, 0.0, "a horizontal-only pin must not write translationY");
    }

    /// A RIGHT-frozen column on Android: `right` pulls the child back
    /// so its far edge parks at the scrollport's trailing edge —
    /// written through `setTranslationX` only, so the free vertical
    /// axis must come back exactly 0 or `write_translate` would leave
    /// a stale `translationY` behind. Before trailing-edge support
    /// `right` wrote nothing on any native backend.
    #[test]
    fn regression_sticky_right_pins_at_scrollport_trailing_edge() {
        let horizontal = StickyInsets { right: Some(0.0), ..Default::default() };
        // Column at x=900 dp, 100 dp wide, 411 dp scrollport,
        // unscrolled: parks at 411 - 100 = 311.
        let (dx, dy) = translate(horizontal, (900.0, 40.0), (100.0, 24.0), (0.0, 0.0), (411.0, 731.0));
        assert!(((900.0 + dx) - 311.0).abs() < 1e-5);
        assert_eq!(dy, 0.0, "a horizontal-only pin must not write translationY");
        // Scrolled far enough right — rides the content again.
        let (dx, _) = translate(horizontal, (900.0, 40.0), (100.0, 24.0), (589.0, 0.0), (411.0, 731.0));
        assert_eq!(dx, 0.0);
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
        // With no scroll ancestor, no scroll listener fires, so the
        // shared translate is never called. But its "no pin while the
        // content hasn't scrolled past the threshold" property is the
        // same on both axes: the child sits at its natural layout
        // position with translation = 0, identical to what a
        // `Relative`-positioned view would render.
        let insets = StickyInsets { top: Some(20.0), left: Some(20.0), ..Default::default() };
        assert_eq!(
            translate(insets, (100.0, 100.0), (80.0, 40.0), (0.0, 0.0), (411.0, 731.0)),
            (0.0, 0.0),
            "no scroll ancestor → no scroll → no pin",
        );

        // Also: the absent-key path must not panic and must
        // observe the registry as empty.
        let registry: HashMap<usize, ()> = HashMap::new();
        let absent_key = 0xDEAD_BEEF_usize;
        assert!(registry.get(&absent_key).is_none());
        assert_eq!(registry.len(), 0);
    }
}
