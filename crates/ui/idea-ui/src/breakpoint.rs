//! Responsive breakpoints — categorical width buckets author code
//! switches layouts on.
//!
//! Built on top of [`runtime_core::viewport_size`]. The framework's
//! viewport signal updates on every pixel resize; this module
//! collapses that into a [`Breakpoint`] enum so subscribers only
//! re-fire when the *bucket* changes — dragging a window edge from
//! 1200 to 1000 doesn't trigger the layout switch until it actually
//! crosses the threshold.
//!
//! # Quick start
//!
//! ```ignore
//! use idea_ui::breakpoint::{current_breakpoint, Breakpoint};
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

use runtime_core::{memo, Signal, ViewportSize};

/// Categorical width bucket. Use this enum, not raw pixel
/// comparisons, so the breakpoint definition lives in one place.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Breakpoint {
    /// Below `sm_min` — phones in portrait, narrow embedded screens.
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

    const fn rank(self) -> u8 {
        match self {
            Self::Xs => 0,
            Self::Sm => 1,
            Self::Md => 2,
            Self::Lg => 3,
            Self::Xl => 4,
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
    /// and this matches the safe-area / viewport pattern in
    /// [`runtime_core`].
    static INSTALLED: OnceCell<Breakpoints> = const { OnceCell::new() };

    /// Memoized `Signal<Breakpoint>` derived from
    /// [`runtime_core::viewport_size`]. Lazily initialized on first
    /// `current_breakpoint()` call so apps that never read the hook
    /// don't pay for the memo effect.
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
/// value (yet — see [`runtime_core::viewport_size`] for the wired-up
/// list), the signal stays at `Breakpoint::Xs` (width 0).
pub fn current_breakpoint() -> Signal<Breakpoint> {
    MEMO.with(|cell| {
        *cell.get_or_init(|| {
            let table = breakpoints();
            memo(move || {
                let ViewportSize { width, .. } = runtime_core::viewport_size().get();
                table.classify(width)
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
    fn current_breakpoint_reflects_viewport_changes() {
        // Each cargo-test thread gets its own thread-local memo and
        // viewport signal, so this test is self-contained.
        runtime_core::set_viewport_size(ViewportSize::new(390.0, 800.0));
        let sig = current_breakpoint();
        assert_eq!(sig.get(), Breakpoint::Xs);

        runtime_core::set_viewport_size(ViewportSize::new(900.0, 800.0));
        assert_eq!(sig.get(), Breakpoint::Md);

        runtime_core::set_viewport_size(ViewportSize::new(1400.0, 800.0));
        assert_eq!(sig.get(), Breakpoint::Xl);
    }
}
