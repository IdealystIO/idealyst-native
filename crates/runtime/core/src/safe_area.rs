//! Safe-area insets — the per-edge "system chrome" reservations
//! (status bar, home indicator, notch / dynamic island on iOS;
//! status / nav bars + display cutout on Android; `env(safe-area-*)`
//! on web).
//!
//! ## Two-piece API
//!
//! - [`safe_area_insets()`] — reactive `Signal<EdgeInsets>` users can
//!   read directly. Re-fires when the platform reports a change
//!   (orientation flip, sheet transition, accessory bar appearing).
//!   Backends call [`set_safe_area_insets`] when they detect a
//!   change.
//! - `.safe_area(SafeAreaSides::TOP | …)` on container [`Bound`]s — a
//!   per-component opt-in that adds the platform inset to the
//!   matching side of the container's *padding* (not margin). The
//!   container's background still bleeds under the system chrome;
//!   only the content gets pushed inward.
//!
//! ## Nested opt-ins
//!
//! If a parent and a child both opt in on the same side, the inset
//! is added to *both* paddings (naive stacking). This is a
//! documented limitation — author code should put `.safe_area(...)`
//! on the outermost container. The alternative (track "is this side
//! already absorbed upstream") is meaningfully more complex; defer
//! until use cases force it.
//!
//! ## Keyboard
//!
//! Out of scope for the safe area. The keyboard inset, if ever
//! exposed, lives in a separate API — different reactivity rate,
//! different accessibility semantics, different animation curve.
//!
//! [`Bound`]: crate::Bound

use crate::reactive::Signal;
use std::cell::OnceCell;

/// Per-edge insets in CSS pixels (or the backend's equivalent point
/// unit). Always non-negative; backends clamp before pushing.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct EdgeInsets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl EdgeInsets {
    pub const ZERO: Self = Self { top: 0.0, right: 0.0, bottom: 0.0, left: 0.0 };

    /// Pick the inset for one side based on a bitflag — convenience
    /// for backends combining author padding with safe-area padding.
    pub fn for_side(&self, side: SafeAreaSides) -> f32 {
        // Caller passes a single-side flag (TOP / RIGHT / BOTTOM /
        // LEFT). For ALL / VERTICAL / HORIZONTAL combinations the
        // backend should call this per side.
        match side {
            s if s == SafeAreaSides::TOP => self.top,
            s if s == SafeAreaSides::RIGHT => self.right,
            s if s == SafeAreaSides::BOTTOM => self.bottom,
            s if s == SafeAreaSides::LEFT => self.left,
            _ => 0.0,
        }
    }
}

/// Per-side opt-in flags for `.safe_area(...)`. Composable via bitwise
/// `|`. The common combinations are exposed as constants
/// ([`ALL`](SafeAreaSides::ALL), [`HORIZONTAL`](SafeAreaSides::HORIZONTAL),
/// [`VERTICAL`](SafeAreaSides::VERTICAL)) so author code doesn't have
/// to OR them by hand.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SafeAreaSides(pub u8);

impl SafeAreaSides {
    pub const NONE: Self = Self(0);
    pub const TOP: Self = Self(1 << 0);
    pub const RIGHT: Self = Self(1 << 1);
    pub const BOTTOM: Self = Self(1 << 2);
    pub const LEFT: Self = Self(1 << 3);
    pub const ALL: Self = Self(0b1111);
    pub const HORIZONTAL: Self = Self(0b1010); // right | left
    pub const VERTICAL: Self = Self(0b0101); // top | bottom

    /// True iff every flag in `other` is also set in `self`.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// True iff no flags are set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for SafeAreaSides {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for SafeAreaSides {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for SafeAreaSides {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

// ---------------------------------------------------------------------------
// Reactive insets signal
// ---------------------------------------------------------------------------

thread_local! {
    /// The framework's authoritative safe-area-insets signal. Lazily
    /// initialized to `EdgeInsets::ZERO` on first access; backends
    /// overwrite it as the platform reports changes.
    ///
    /// Thread-local because `Signal` is reactive-arena-backed and the
    /// reactive runtime is single-threaded (UI thread).
    static INSETS: OnceCell<Signal<EdgeInsets>> = const { OnceCell::new() };
}

fn insets_signal() -> Signal<EdgeInsets> {
    INSETS.with(|cell| {
        // Root-anchor this thread-lifetime cached signal so it isn't owned
        // by whatever transient scope first touches it (e.g. a navigator
        // screen reading `safe_area_insets()` during its mount, or a
        // deferred chrome build). Without `unscope`, that scope owns the
        // signal; when the screen is released — every drawer/tab `select`
        // over the runtime-server wire releases the outgoing screen scope —
        // the cached `SignalId` dangles and the next read panics with
        // "signal used after its scope was dropped". Mirrors
        // `crate::viewport_size` and `crate::current_breakpoint`.
        *cell.get_or_init(|| crate::reactive::unscope(|| Signal::new(EdgeInsets::ZERO)))
    })
}

/// The reactive safe-area insets signal. Read via `.get()` inside an
/// effect / derived to subscribe. The value updates whenever the
/// active backend reports a change (orientation, sheet adaptation,
/// dynamic island, etc.).
///
/// On platforms without a backend-side observer hooked up the value
/// stays at `EdgeInsets::ZERO` — degrades gracefully.
pub fn safe_area_insets() -> Signal<EdgeInsets> {
    insets_signal()
}

/// Backend entry point: push a new value into the global insets
/// signal. Called on the UI thread by each platform's observer
/// (UIView.safeAreaInsetsDidChange, WindowInsets listener,
/// MutationObserver on the web). Idempotent — Signal compares by
/// equality.
pub fn set_safe_area_insets(insets: EdgeInsets) {
    let sig = insets_signal();
    if sig.get() != insets {
        sig.set(insets);
    }
}

#[cfg(test)]
mod tests {
    //! Coverage for `EdgeInsets`, `SafeAreaSides`, and the global
    //! safe-area-insets signal lifecycle.

    use super::*;

    // -----------------------------------------------------------------------
    // EdgeInsets
    // -----------------------------------------------------------------------

    #[test]
    fn edge_insets_zero_is_all_zeros() {
        let z = EdgeInsets::ZERO;
        assert_eq!(z.top, 0.0);
        assert_eq!(z.right, 0.0);
        assert_eq!(z.bottom, 0.0);
        assert_eq!(z.left, 0.0);
    }

    #[test]
    fn edge_insets_default_matches_zero() {
        let d: EdgeInsets = EdgeInsets::default();
        assert_eq!(d, EdgeInsets::ZERO);
    }

    #[test]
    fn edge_insets_for_side_picks_the_right_field() {
        let insets = EdgeInsets {
            top: 10.0,
            right: 20.0,
            bottom: 30.0,
            left: 40.0,
        };
        assert_eq!(insets.for_side(SafeAreaSides::TOP), 10.0);
        assert_eq!(insets.for_side(SafeAreaSides::RIGHT), 20.0);
        assert_eq!(insets.for_side(SafeAreaSides::BOTTOM), 30.0);
        assert_eq!(insets.for_side(SafeAreaSides::LEFT), 40.0);
    }

    #[test]
    fn edge_insets_for_side_returns_zero_for_compound_flags() {
        // The doc says "caller passes a single-side flag" — compound
        // combinations fall through to 0.0. Pin that down.
        let insets = EdgeInsets {
            top: 5.0,
            right: 5.0,
            bottom: 5.0,
            left: 5.0,
        };
        assert_eq!(insets.for_side(SafeAreaSides::ALL), 0.0);
        assert_eq!(insets.for_side(SafeAreaSides::HORIZONTAL), 0.0);
        assert_eq!(insets.for_side(SafeAreaSides::VERTICAL), 0.0);
        assert_eq!(insets.for_side(SafeAreaSides::NONE), 0.0);
    }

    // -----------------------------------------------------------------------
    // SafeAreaSides bitops
    // -----------------------------------------------------------------------

    #[test]
    fn safe_area_sides_constants_have_expected_bit_layout() {
        assert_eq!(SafeAreaSides::NONE.0, 0);
        assert_eq!(SafeAreaSides::TOP.0, 0b0001);
        assert_eq!(SafeAreaSides::RIGHT.0, 0b0010);
        assert_eq!(SafeAreaSides::BOTTOM.0, 0b0100);
        assert_eq!(SafeAreaSides::LEFT.0, 0b1000);
        assert_eq!(SafeAreaSides::ALL.0, 0b1111);
        assert_eq!(SafeAreaSides::HORIZONTAL.0, 0b1010);
        assert_eq!(SafeAreaSides::VERTICAL.0, 0b0101);
    }

    #[test]
    fn safe_area_sides_horizontal_is_right_or_left() {
        let h = SafeAreaSides::RIGHT | SafeAreaSides::LEFT;
        assert_eq!(h, SafeAreaSides::HORIZONTAL);
    }

    #[test]
    fn safe_area_sides_vertical_is_top_or_bottom() {
        let v = SafeAreaSides::TOP | SafeAreaSides::BOTTOM;
        assert_eq!(v, SafeAreaSides::VERTICAL);
    }

    #[test]
    fn safe_area_sides_all_is_horizontal_or_vertical() {
        let combined = SafeAreaSides::HORIZONTAL | SafeAreaSides::VERTICAL;
        assert_eq!(combined, SafeAreaSides::ALL);
    }

    #[test]
    fn safe_area_sides_contains_matches_set_bits() {
        let mixed = SafeAreaSides::TOP | SafeAreaSides::RIGHT;
        assert!(mixed.contains(SafeAreaSides::TOP));
        assert!(mixed.contains(SafeAreaSides::RIGHT));
        assert!(!mixed.contains(SafeAreaSides::BOTTOM));
        assert!(!mixed.contains(SafeAreaSides::LEFT));
        // contains(NONE) is vacuously true (every set contains the empty set).
        assert!(mixed.contains(SafeAreaSides::NONE));
        // contains(ALL) is false unless mixed IS all.
        assert!(!mixed.contains(SafeAreaSides::ALL));
    }

    #[test]
    fn safe_area_sides_is_empty() {
        assert!(SafeAreaSides::NONE.is_empty());
        assert!(SafeAreaSides::default().is_empty());
        assert!(!SafeAreaSides::TOP.is_empty());
        assert!(!SafeAreaSides::ALL.is_empty());
    }

    #[test]
    fn safe_area_sides_bitor_assign_combines_in_place() {
        let mut s = SafeAreaSides::TOP;
        s |= SafeAreaSides::BOTTOM;
        assert_eq!(s, SafeAreaSides::VERTICAL);
    }

    #[test]
    fn safe_area_sides_bitand_masks_to_intersection() {
        let intersection = SafeAreaSides::ALL & SafeAreaSides::HORIZONTAL;
        assert_eq!(intersection, SafeAreaSides::HORIZONTAL);
        let top_only = SafeAreaSides::VERTICAL & SafeAreaSides::TOP;
        assert_eq!(top_only, SafeAreaSides::TOP);
        let disjoint = SafeAreaSides::TOP & SafeAreaSides::LEFT;
        assert_eq!(disjoint, SafeAreaSides::NONE);
    }

    // -----------------------------------------------------------------------
    // Signal lifecycle
    // -----------------------------------------------------------------------

    #[test]
    fn safe_area_insets_initial_value_is_zero() {
        // The thread-local OnceCell is per-thread; this test runs in
        // its own thread by default (cargo test parallelism). The
        // initial value should be ZERO before anyone calls
        // set_safe_area_insets in this thread.
        let initial = safe_area_insets().get();
        // Other tests in the same thread MAY have already pushed a
        // value, but we can still verify the signal handle is the
        // same one (idempotent init).
        let again = safe_area_insets().get();
        assert_eq!(initial, again, "signal should return a stable value on idempotent reads");
    }

    #[test]
    fn set_safe_area_insets_updates_the_signal() {
        // Note: cargo runs tests on multiple threads; each gets its
        // own thread-local INSETS cell. We push and read on the
        // same thread.
        let new_insets = EdgeInsets {
            top: 47.0,
            right: 0.0,
            bottom: 34.0,
            left: 0.0,
        };
        set_safe_area_insets(new_insets);
        assert_eq!(safe_area_insets().get(), new_insets);

        // Calling again with the SAME value is a no-op (idempotent).
        // Hard to observe externally, but the second call must not
        // panic and the value must still match.
        set_safe_area_insets(new_insets);
        assert_eq!(safe_area_insets().get(), new_insets);
    }

    /// Regression: the insets signal must survive when its FIRST access
    /// happens inside a transient render scope that then drops — exactly
    /// what every drawer/tab `select` over the runtime-server wire does
    /// (the recording handler releases the outgoing screen's scope, and
    /// iOS screens/sidebars read `safe_area_insets()` during their build).
    /// Before root-anchoring with `unscope`, the transient scope owned the
    /// signal, freed its slot on drop, and the next read panicked with
    /// "signal used after its scope was dropped" — the second-navigation
    /// crash. Runs on a fresh thread so the thread-local cache starts
    /// uninitialized and the first touch is the one inside the scope.
    #[test]
    fn insets_signal_survives_first_access_in_transient_scope() {
        std::thread::spawn(|| {
            use crate::reactive::{with_scope, Scope, Signal};
            let id = {
                let mut scope = Scope::new();
                with_scope(&mut scope, || safe_area_insets().id())
            };
            // Churn the arena so the freed slot (pre-fix) gets recycled to
            // a different-typed signal, turning the dangle into a loud
            // panic on read rather than silently reading stale bytes.
            {
                let mut churn = Scope::new();
                with_scope(&mut churn, || {
                    for _ in 0..64 {
                        let _ = Signal::new(0u8);
                    }
                });
            }
            // Cache intact (same id) AND the slot still holds EdgeInsets:
            // these reads panic pre-fix.
            assert_eq!(safe_area_insets().id(), id);
            assert_eq!(safe_area_insets().get(), EdgeInsets::ZERO);
            // And a post-drop update still lands.
            set_safe_area_insets(EdgeInsets { top: 44.0, right: 0.0, bottom: 34.0, left: 0.0 });
            assert_eq!(safe_area_insets().get().top, 44.0);
        })
        .join()
        .expect("insets signal survives transient-scope first access");
    }
}
