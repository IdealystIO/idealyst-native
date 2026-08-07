//! One-time diagnostics for style properties a backend silently
//! ignores.
//!
//! # Why this exists
//!
//! The expensive failure mode isn't "unsupported" — it's "unsupported
//! and *silent*". An author sets a property, the layout comes out
//! subtly wrong, and there is no warning, no `debug_assert`, and
//! nothing to grep for; the only way to find out is to read the
//! backend's lowering code and notice the property missing. That
//! diagnosis takes a day. A single line in the console takes five
//! seconds.
//!
//! # Dedup, and why the key is `&'static str`
//!
//! `apply_style` runs on every node on every restyle, so an ungated
//! warning would emit thousands of identical lines and bury itself.
//! Each message is emitted at most once per key per thread. The key is
//! a `&'static str` so the dedup set never allocates and the call site
//! is forced to name a *stable* condition (`"sticky.bottom"`) rather
//! than a per-node string.
//!
//! Key naming: `<feature>.<property>`, matching the phase-name
//! convention in `runtime_core::debug`.
//!
//! # Debug builds only
//!
//! The whole module compiles to nothing without `debug_assertions`.
//! Release builds shouldn't pay a set lookup per styled node for a
//! developer diagnostic, and CLAUDE.md §7 puts dev-only markers behind
//! the cfg rather than a runtime predicate. Call sites are written
//! unconditionally — the gate lives here, and the release shim inlines
//! to nothing.

#[cfg(debug_assertions)]
mod imp {
    use std::cell::RefCell;
    use std::collections::HashSet;

    thread_local! {
        static SEEN: RefCell<HashSet<&'static str>> = RefCell::new(HashSet::new());
    }

    /// Record `key` and report whether this was its FIRST sighting.
    /// Split out from [`super::warn_once`] so the dedup rule is
    /// testable without racing other tests on the process-global
    /// logger slot.
    pub fn mark_first_sighting(key: &'static str) -> bool {
        SEEN.with(|s| s.borrow_mut().insert(key))
    }

    /// Forget every recorded key. Tests only.
    pub fn reset_for_test() {
        SEEN.with(|s| s.borrow_mut().clear());
    }
}

#[cfg(not(debug_assertions))]
mod imp {
    #[inline(always)]
    pub fn mark_first_sighting(_key: &'static str) -> bool {
        false
    }
    #[inline(always)]
    pub fn reset_for_test() {}
}

#[doc(hidden)]
pub use imp::{mark_first_sighting, reset_for_test};

/// Warn — at most once per `key`, per thread — that a style property
/// the author set is being ignored.
///
/// `message` should say what was ignored, on which backend, and what
/// the observable consequence is. "Ignored" alone sends the reader
/// straight back to the source.
///
/// ```ignore
/// warn_once(
///     "sticky.bottom",
///     "position: Sticky with `bottom` — native backends pin only to \
///      leading edges (`top` / `left`), so this element will not pin.",
/// );
/// ```
#[inline]
pub fn warn_once(key: &'static str, message: &str) {
    if mark_first_sighting(key) {
        crate::log_warn!("[unsupported] {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure this module guards against is a FLOOD, not a
    /// silence. `apply_style` runs per node per restyle, so a warning
    /// that fires every time buries itself and is as useless as no
    /// warning at all.
    #[test]
    fn regression_repeated_warnings_would_flood_the_log() {
        reset_for_test();
        let emissions = (0..1000)
            .filter(|_| mark_first_sighting("flood.probe"))
            .count();
        assert_eq!(emissions, 1, "a keyed warning must emit exactly once");
    }

    /// Distinct keys must not suppress each other — otherwise the
    /// first unsupported property in a session would mask every other
    /// one, which is the original silence with extra steps.
    #[test]
    fn distinct_keys_do_not_suppress_each_other() {
        reset_for_test();
        assert!(mark_first_sighting("distinct.one"));
        assert!(mark_first_sighting("distinct.two"));
        assert!(!mark_first_sighting("distinct.one"));
    }
}
