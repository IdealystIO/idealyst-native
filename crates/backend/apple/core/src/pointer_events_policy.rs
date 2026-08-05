//! CSS `pointer-events` hit-test verdict, shared by the UIKit and AppKit
//! backends. Pure logic — no objc — so the regression coverage runs from
//! any host (the `private_layer_hittest` precedent).
//!
//! Background: `StyleRules.pointer_events` is emitted as real CSS on
//! web/SSR, but the native backends historically dropped it. That broke
//! the overlay click-through composition (`overlay().click_through(true)`
//! → root `pointer-events: none`, interactive children opt back in with
//! `Auto`) and, worse, any always-mounted full-window scrim held inert by
//! `PointerEvents::None` — idea-ui-nav's `AppShell` drawer scrim is
//! exactly that, so on macOS EVERY real mouse click in an AppShell app
//! landed on the invisible scrim and content links never fired (robot
//! clicks, which bypass hit-testing, kept working — which is how the bug
//! hid).
//!
//! Mechanism per platform, converging on this verdict (CLAUDE.md §7):
//! - **AppKit**: `FlippedView.hitTest:` runs the default (frame-based)
//!   resolution first, then consults [`none_hit_stands`] when its own
//!   style said `None` — returning `nil` lets AppKit resolve whatever is
//!   visually behind the subtree.
//! - **UIKit**: `IdealystTouchView.hitTest:withEvent:` does the same;
//!   returning `nil` makes UIKit continue with siblings underneath.
//!
//! CSS semantics being modeled: `pointer-events` inherits, so the
//! effective value at the hit view is the NEAREST explicit value walking
//! ancestors from the hit upward. A `none` root turns its subtree
//! hit-transparent; a descendant with explicit `auto` re-enables itself
//! and its own subtree; a further-nested explicit `none` disables again.
//! Every framework host view applies the verdict in its own hit-test, so
//! by the time a `none` root sees a resolved descendant, any nested
//! `none` host below it has already declined — the chain walk here only
//! needs the nearest-explicit rule.

use runtime_shared::PointerEvents;

/// Should a hit that the platform's default hit-test resolved inside a
/// `pointer-events: none` view's subtree stand?
///
/// - `hit_is_self`: the resolved view IS the `none` view itself → never.
/// - `chain`: each host view's *explicit* `pointer_events` (unset =
///   `None` in the `Option` sense), ordered from the hit view upward to
///   (excluding) the `none` root. Non-framework views (native controls,
///   labels) contribute `None`.
pub fn none_hit_stands(hit_is_self: bool, chain: &[Option<PointerEvents>]) -> bool {
    if hit_is_self {
        return false;
    }
    for pe in chain {
        match pe {
            // Nearest explicit value wins (CSS inheritance).
            Some(PointerEvents::Auto) => return true,
            Some(PointerEvents::None) => return false,
            None => {}
        }
    }
    // No explicit re-enable anywhere below the root: the subtree
    // inherits the root's `none`.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The AppShell-scrim regression: a full-window `pressable` with
    /// `pointer_events: None` and no children must decline its own hit,
    /// so the click resolves to the content underneath.
    #[test]
    fn regression_none_scrim_declines_its_own_hit() {
        assert!(!none_hit_stands(true, &[]));
    }

    /// The overlay click-through composition: portal root is `none`, a
    /// toast card opts back in with explicit `Auto` — a hit on (or
    /// inside) the card stands.
    #[test]
    fn auto_descendant_reenables_subtree() {
        // hit directly on the Auto card
        assert!(none_hit_stands(false, &[Some(PointerEvents::Auto)]));
        // hit on an unset label INSIDE the Auto card
        assert!(none_hit_stands(false, &[None, Some(PointerEvents::Auto)]));
    }

    /// A descendant with no explicit value inherits the root's `none`.
    #[test]
    fn unset_descendants_inherit_none() {
        assert!(!none_hit_stands(false, &[]));
        assert!(!none_hit_stands(false, &[None, None]));
    }

    /// Nearest explicit value wins: a nested explicit `none` below an
    /// `auto` disables again for its own subtree.
    #[test]
    fn nearest_explicit_wins() {
        // hit → [none, auto] : the closer `none` shadows the outer auto
        assert!(!none_hit_stands(
            false,
            &[Some(PointerEvents::None), Some(PointerEvents::Auto)]
        ));
        // hit → [auto, none] : the closer `auto` re-enables
        assert!(none_hit_stands(
            false,
            &[Some(PointerEvents::Auto), Some(PointerEvents::None)]
        ));
    }
}
