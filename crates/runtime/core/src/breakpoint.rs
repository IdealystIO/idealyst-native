//! Responsive breakpoints — categorical width buckets author code
//! switches layouts on.
//!
//! Built on top of [`crate::viewport_size`]. The framework's viewport
//! signal updates on every pixel resize; this module collapses that
//! into a [`Breakpoint`] enum so subscribers only re-fire when the
//! *bucket* changes — dragging a window edge from 1200 to 1000 doesn't
//! trigger the layout switch until it actually crosses the threshold.
//!
//! # Why this lives in `runtime-core`
//!
//! Breakpoints are not a UI-kit convenience — they're a primitive of
//! the style system. The `Backend` trait, the build walker, and the
//! `css` crate all need to reason about breakpoints (to emit
//! `@media (min-width: …)` rules on web, to merge the active bucket's
//! overlay reactively on native). None of those layers can depend on
//! `idea-ui`, so the breakpoint definition has to sit at the same level
//! as `viewport_size`, which it's derived from. `idea-ui` re-exports
//! the whole module so `idea_ui::breakpoint::*` keeps working.
//!
//! # Quick start
//!
//! ```ignore
//! use runtime_core::breakpoint::{current_breakpoint, Breakpoint};
//!
//! effect!({
//!     match current_breakpoint().get() {
//!         Breakpoint::Xs | Breakpoint::Sm => { /* mobile layout */ }
//!         Breakpoint::Md => { /* tablet layout */ }
//!         Breakpoint::Lg | Breakpoint::Xl => { /* desktop layout */ }
//!     }
//! });
//! ```
//!
//! Most author code shouldn't read the signal directly — it should
//! declare `breakpoint md { … }` overlays in a `stylesheet!` and let
//! the framework realize them (CSS `@media` on web, reactive merge on
//! native). The signal is the escape hatch for genuinely imperative
//! layout switches.
//!
//! # Thresholds
//!
//! Defaults match the tailwind / common-design-system scale (`sm` at
//! 640 dp, `md` at 768, `lg` at 1024, `xl` at 1280). Apps that want a
//! different scale call [`install_breakpoints`] once at startup
//! before mounting.
//!
//! Each threshold is the *lower* bound of its bucket: width `>=
//! sm_min` and `< md_min` is `Breakpoint::Sm`. Width below the
//! smallest threshold is `Breakpoint::Xs`.

use std::cell::OnceCell;

use crate::{memo, viewport_size, Signal, ViewportSize};

/// Categorical width bucket. Use this enum, not raw pixel
/// comparisons, so the breakpoint definition lives in one place.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Breakpoint {
    /// Below `sm_min` — phones in portrait, narrow embedded screens.
    /// This is the **mobile-first base**: a stylesheet's base rules are
    /// the `Xs` layout, and there is no `__bp_xs` overlay axis (an `Xs`
    /// override would be the base itself).
    Xs,
    /// `[sm_min, md_min)` — large phones / phablets.
    Sm,
    /// `[md_min, lg_min)` — tablets / split-screen.
    Md,
    /// `[lg_min, xl_min)` — laptops / standard desktop.
    Lg,
    /// `>= xl_min` — wide desktop / ultrawide.
    Xl,
}

impl Breakpoint {
    /// True if this breakpoint is *at least* `other`. Convenience for
    /// "show this on Md or larger" style checks. `Breakpoint::Lg.is_at_least(Breakpoint::Md)` is `true`.
    pub const fn is_at_least(self, other: Self) -> bool {
        self.rank() >= other.rank()
    }

    /// Ordinal rank, ascending by width (`Xs` = 0 … `Xl` = 4). Used to
    /// layer overlays in mobile-first min-width order: at a given
    /// active breakpoint, every overlay whose rank is `<=` the active
    /// rank applies, lowest first, so higher breakpoints win on
    /// conflicting properties (matching how stacked `@media (min-width)`
    /// rules cascade by source order).
    pub const fn rank(self) -> u8 {
        match self {
            Self::Xs => 0,
            Self::Sm => 1,
            Self::Md => 2,
            Self::Lg => 3,
            Self::Xl => 4,
        }
    }

    /// The reserved variant-axis name for this breakpoint's overlay, or
    /// `None` for `Xs` (which is the base, not an overlay). A
    /// `stylesheet!`'s `breakpoint md { … }` block becomes a
    /// `.variant("__bp_md", "on", …)` overlay; the `__bp_` namespace
    /// keeps these out of the author variant namespace, exactly like
    /// `__state_*` does for interaction states.
    pub const fn axis_name(self) -> Option<&'static str> {
        match self {
            Self::Xs => None,
            Self::Sm => Some("__bp_sm"),
            Self::Md => Some("__bp_md"),
            Self::Lg => Some("__bp_lg"),
            Self::Xl => Some("__bp_xl"),
        }
    }

    /// Inverse of [`Self::axis_name`]: map a `__bp_*` axis name back to
    /// its breakpoint, or `None` if the axis isn't a breakpoint
    /// overlay. The style system uses this to recognize which declared
    /// variant axes are breakpoint overlays.
    pub fn from_axis_name(axis: &str) -> Option<Self> {
        match axis {
            "__bp_sm" => Some(Self::Sm),
            "__bp_md" => Some(Self::Md),
            "__bp_lg" => Some(Self::Lg),
            "__bp_xl" => Some(Self::Xl),
            _ => None,
        }
    }
}

/// Threshold table. Each field is the *lower* bound (in dp) of the
/// matching bucket. Values should be monotonically increasing — the
/// classifier doesn't enforce that, but a non-monotonic table will
/// shadow buckets in the middle.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Breakpoints {
    pub sm_min: f32,
    pub md_min: f32,
    pub lg_min: f32,
    pub xl_min: f32,
}

impl Breakpoints {
    /// Tailwind-style defaults. Familiar to most web designers and
    /// roughly aligned with typical phone / tablet / laptop / desktop
    /// boundaries.
    pub const DEFAULT: Self = Self {
        sm_min: 640.0,
        md_min: 768.0,
        lg_min: 1024.0,
        xl_min: 1280.0,
    };

    pub const fn classify(self, width: f32) -> Breakpoint {
        if width >= self.xl_min {
            Breakpoint::Xl
        } else if width >= self.lg_min {
            Breakpoint::Lg
        } else if width >= self.md_min {
            Breakpoint::Md
        } else if width >= self.sm_min {
            Breakpoint::Sm
        } else {
            Breakpoint::Xs
        }
    }

    /// The lower-bound threshold (dp) for a breakpoint's overlay, or
    /// `None` for `Xs` (the base, which has no `min-width` query). The
    /// `css` crate uses this to emit `@media (min-width: <px>px)` rules
    /// that match the same bucket boundaries the native classifier uses,
    /// so a `breakpoint md { … }` overlay activates at *exactly* the
    /// same width on web as on native.
    pub const fn min_width(self, bp: Breakpoint) -> Option<f32> {
        match bp {
            Breakpoint::Xs => None,
            Breakpoint::Sm => Some(self.sm_min),
            Breakpoint::Md => Some(self.md_min),
            Breakpoint::Lg => Some(self.lg_min),
            Breakpoint::Xl => Some(self.xl_min),
        }
    }
}

impl Default for Breakpoints {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// ---------------------------------------------------------------------------
// Installed breakpoint table + memoized signal
// ---------------------------------------------------------------------------

thread_local! {
    /// App-supplied threshold table. `None` => use [`Breakpoints::DEFAULT`].
    /// Thread-local because the reactive runtime is single-threaded
    /// and this matches the safe-area / viewport pattern in this crate.
    static INSTALLED: OnceCell<Breakpoints> = const { OnceCell::new() };

    /// Memoized `Signal<Breakpoint>` derived from [`crate::viewport_size`].
    /// Lazily initialized on first `current_breakpoint()` call so apps
    /// that never read the hook don't pay for the memo effect.
    static MEMO: OnceCell<Signal<Breakpoint>> = const { OnceCell::new() };
}

/// Install a custom breakpoint table. Idempotent — first call wins.
/// Call once at app startup *before* mounting (or before the first
/// `current_breakpoint()` read; whichever is earlier), since the
/// derived signal captures the table by value on first read.
///
/// Returns `Ok(())` on first install, `Err(prev)` if a table was
/// already installed (the existing value is unchanged). Most apps
/// can ignore the result.
pub fn install_breakpoints(table: Breakpoints) -> Result<(), Breakpoints> {
    INSTALLED.with(|cell| cell.set(table))
}

/// Read the active threshold table — either the installed one or
/// [`Breakpoints::DEFAULT`].
pub fn breakpoints() -> Breakpoints {
    INSTALLED.with(|cell| cell.get().copied().unwrap_or(Breakpoints::DEFAULT))
}

/// Reactive current-breakpoint signal. Re-fires only when the
/// classified bucket changes, not on every pixel resize.
///
/// Read inside an effect / derived / `signal_class!`:
///
/// ```ignore
/// effect!({
///     let bp = current_breakpoint().get();
///     // … switch layout on `bp`
/// });
/// ```
///
/// On platforms where the active backend doesn't push a viewport
/// value (yet — see [`crate::viewport_size`] for the wired-up list),
/// the signal stays at `Breakpoint::Xs` (width 0).
pub fn current_breakpoint() -> Signal<Breakpoint> {
    MEMO.with(|cell| {
        *cell.get_or_init(|| {
            let table = breakpoints();
            // Root-anchor this thread-lifetime cached memo so it isn't
            // owned by whatever transient scope first touches it (e.g. an
            // SSR deferred chrome build) — otherwise the cached signal id
            // dangles when that scope drops and its slot recycles.
            crate::unscope(move || {
                memo(move || {
                    let ViewportSize { width, .. } = viewport_size().get();
                    table.classify(width)
                })
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_tailwind_scale() {
        let b = Breakpoints::DEFAULT;
        assert_eq!(b.sm_min, 640.0);
        assert_eq!(b.md_min, 768.0);
        assert_eq!(b.lg_min, 1024.0);
        assert_eq!(b.xl_min, 1280.0);
    }

    #[test]
    fn classify_assigns_buckets_at_boundaries() {
        let b = Breakpoints::DEFAULT;
        assert_eq!(b.classify(0.0), Breakpoint::Xs);
        assert_eq!(b.classify(639.9), Breakpoint::Xs);
        assert_eq!(b.classify(640.0), Breakpoint::Sm);
        assert_eq!(b.classify(767.9), Breakpoint::Sm);
        assert_eq!(b.classify(768.0), Breakpoint::Md);
        assert_eq!(b.classify(1023.9), Breakpoint::Md);
        assert_eq!(b.classify(1024.0), Breakpoint::Lg);
        assert_eq!(b.classify(1279.9), Breakpoint::Lg);
        assert_eq!(b.classify(1280.0), Breakpoint::Xl);
        assert_eq!(b.classify(9_999.0), Breakpoint::Xl);
    }

    #[test]
    fn is_at_least_compares_ranks_in_order() {
        assert!(Breakpoint::Lg.is_at_least(Breakpoint::Md));
        assert!(Breakpoint::Lg.is_at_least(Breakpoint::Lg));
        assert!(!Breakpoint::Sm.is_at_least(Breakpoint::Md));
        assert!(Breakpoint::Xl.is_at_least(Breakpoint::Xs));
        assert!(!Breakpoint::Xs.is_at_least(Breakpoint::Xl));
    }

    #[test]
    fn axis_name_roundtrips_for_overlay_buckets() {
        for bp in [Breakpoint::Sm, Breakpoint::Md, Breakpoint::Lg, Breakpoint::Xl] {
            let axis = bp.axis_name().expect("overlay bucket has an axis name");
            assert_eq!(Breakpoint::from_axis_name(axis), Some(bp));
        }
        // Xs is the base, not an overlay axis.
        assert_eq!(Breakpoint::Xs.axis_name(), None);
        assert_eq!(Breakpoint::from_axis_name("__bp_xs"), None);
        assert_eq!(Breakpoint::from_axis_name("tone"), None);
    }

    #[test]
    fn min_width_matches_classifier_thresholds() {
        let b = Breakpoints::DEFAULT;
        assert_eq!(b.min_width(Breakpoint::Xs), None);
        assert_eq!(b.min_width(Breakpoint::Sm), Some(640.0));
        assert_eq!(b.min_width(Breakpoint::Md), Some(768.0));
        assert_eq!(b.min_width(Breakpoint::Lg), Some(1024.0));
        assert_eq!(b.min_width(Breakpoint::Xl), Some(1280.0));
    }

    #[test]
    fn current_breakpoint_reflects_viewport_changes() {
        // Each cargo-test thread gets its own thread-local memo and
        // viewport signal, so this test is self-contained.
        crate::set_viewport_size(ViewportSize::new(390.0, 800.0));
        let sig = current_breakpoint();
        assert_eq!(sig.get(), Breakpoint::Xs);

        crate::set_viewport_size(ViewportSize::new(900.0, 800.0));
        assert_eq!(sig.get(), Breakpoint::Md);

        crate::set_viewport_size(ViewportSize::new(1400.0, 800.0));
        assert_eq!(sig.get(), Breakpoint::Xl);
    }
}
