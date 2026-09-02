//! Pure decision logic for draining the coalesced layout pass.
//!
//! Un-gated (compiles on any host) so the regression tests run from
//! any platform — same pattern as `splice_policy` / `portal_policy` /
//! `transform_transition_policy`. The half that actually borrows the
//! backend and runs Taffy lives in `imp::drain_queued_layout_pass`.
//!
//! # Why this exists
//!
//! A queued layout pass has two racing drain points — the pre-commit
//! runloop observer (early, same turn) and the main-queue backstop
//! (late, guaranteed). Each has to decide the same four-way question
//! from the same three facts, and the interesting case is the one that
//! used to be wrong: when the backend is already mutably borrowed, the
//! drain must **keep the pass queued and re-post**, not swallow it.
//! Dropping it stranded the tree at whatever positions the last
//! completed pass left, with nothing scheduled to fix it — the layout
//! stayed wrong until some unrelated event happened to schedule
//! another pass.

#![cfg_attr(not(target_os = "ios"), allow(dead_code))]

/// What a drain call should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Drain {
    /// Nothing queued — the other drain point already handled it.
    Nothing,
    /// Queued, but there is no backend to run it against (runtime-server
    /// mode drives layout itself; or the backend has been dropped).
    /// Clear the flag: retrying can never succeed.
    Abandon,
    /// Queued and runnable — clear the flag, then run the pass.
    Run,
    /// Queued but the backend is mid-borrow. Leave the flag SET and
    /// post a fresh backstop so the next turn tries again.
    Retry,
}

/// Decide from the three facts a drain site can observe.
pub(crate) fn decide(queued: bool, has_backend: bool, borrow_available: bool) -> Drain {
    if !queued {
        return Drain::Nothing;
    }
    if !has_backend {
        return Drain::Abandon;
    }
    if !borrow_available {
        return Drain::Retry;
    }
    Drain::Run
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The common case: something queued a pass, the backend is there
    /// and free.
    #[test]
    fn queued_and_free_runs() {
        assert_eq!(decide(true, true, true), Drain::Run);
    }

    /// Both drain points fire every turn; the loser must do nothing
    /// rather than run a second, redundant full-tree pass.
    #[test]
    fn nothing_queued_is_a_no_op() {
        assert_eq!(decide(false, true, true), Drain::Nothing);
        assert_eq!(decide(false, false, false), Drain::Nothing);
    }

    /// No backend to run against — clearing the flag is right because
    /// no number of retries would find one.
    #[test]
    fn missing_backend_abandons_rather_than_spinning() {
        assert_eq!(decide(true, false, true), Drain::Abandon);
        assert_eq!(decide(true, false, false), Drain::Abandon);
    }

    /// The regression this module exists for. A pass that arrives while
    /// the backend is borrowed must survive to be retried; the old code
    /// cleared the flag first and returned, losing it outright.
    #[test]
    fn borrowed_backend_retries_instead_of_dropping_the_pass() {
        assert_eq!(decide(true, true, false), Drain::Retry);
        assert_ne!(
            decide(true, true, false),
            Drain::Nothing,
            "a dropped pass leaves the tree at stale positions with nothing scheduled to fix it"
        );
    }

    /// Only `Run` and `Abandon` may clear the queued flag: the pass is
    /// either done or impossible. `Retry` keeping it set is what makes
    /// the retry happen at all.
    #[test]
    fn only_run_and_abandon_clear_the_queue() {
        let clears = |d: Drain| matches!(d, Drain::Run | Drain::Abandon);
        assert!(clears(decide(true, true, true)));
        assert!(clears(decide(true, false, true)));
        assert!(!clears(decide(true, true, false)), "Retry must stay queued");
    }
}
