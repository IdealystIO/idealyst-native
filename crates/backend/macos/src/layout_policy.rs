//! Pure, host-testable decisions behind the macOS backend's post-mount
//! layout-pass scheduling. The objc2 / AppKit machinery that consumes these
//! (`schedule_layout_pass`, `run_layout_pass_global`) needs a live window and
//! the main thread, so the *logic* is factored here where a plain `cargo test`
//! can pin it.
//!
//! ## The bug these guard
//!
//! `Backend::finish` runs exactly once, at mount (the walker calls it after the
//! build). Reactive Effects that fire later — e.g. a `when`/reactive-style
//! toggle that grows a collapsed `0×0` box to its real size — call
//! `apply_style` directly and push the new size into Taffy, but nothing
//! re-invokes the layout pass, so the NSView keeps its stale frame. The
//! whiteboard-demo recording preview (a box sized only after the Record button
//! is pressed) stayed invisible for exactly this reason. `apply_style` now
//! schedules a coalesced pass when the change can affect layout AND the view is
//! already in a window (so we don't fire N redundant passes during the initial
//! build, when views aren't attached yet).

use std::cell::Cell;

/// Claim the coalescing slot for a deferred layout pass. Returns `true` iff the
/// caller should actually post the microtask — i.e. the flag was previously
/// clear. Subsequent callers (same batch, flag already set) get `false` and
/// drop their post; the one queued pass picks up every mutation they made. The
/// pass clears the flag on entry via [`release_coalesced_pass`].
pub(crate) fn claim_coalesced_pass(queued: &Cell<bool>) -> bool {
    !queued.replace(true)
}

/// Clear the coalescing slot. Called at the *start* of the deferred pass so any
/// `schedule_layout_pass` that arrives while the pass runs re-arms and fires
/// again afterward (it reflects post-layout state this pass couldn't capture).
pub(crate) fn release_coalesced_pass(queued: &Cell<bool>) {
    queued.set(false);
}

/// Whether a reactive `apply_style` should schedule a layout pass. Gate on the
/// view being attached to a window: during the initial build, views are
/// floating (not yet parented into the host window) and the upcoming `finish`
/// already lays them out, so scheduling then is pure waste. After mount, an
/// attached view whose style changed is exactly the case `finish` won't catch.
pub(crate) fn reactive_change_needs_layout_pass(view_attached_to_window: bool) -> bool {
    view_attached_to_window
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_claim_posts_subsequent_drop() {
        let flag = Cell::new(false);
        // First scheduler in a batch claims the slot and posts.
        assert!(claim_coalesced_pass(&flag), "first claim must post");
        // Every later scheduler in the same batch is coalesced away.
        assert!(!claim_coalesced_pass(&flag), "second claim must drop");
        assert!(!claim_coalesced_pass(&flag), "third claim must drop");
    }

    #[test]
    fn release_rearms_the_slot() {
        let flag = Cell::new(false);
        assert!(claim_coalesced_pass(&flag));
        // The pass runs and clears the slot on entry...
        release_coalesced_pass(&flag);
        // ...so a change made *after* that point schedules a fresh pass.
        assert!(
            claim_coalesced_pass(&flag),
            "a claim after release must post again"
        );
    }

    #[test]
    fn detached_views_do_not_schedule() {
        // Mid-build (floating view) → finish() will lay it out; don't schedule.
        assert!(!reactive_change_needs_layout_pass(false));
        // Post-mount (in a window) → finish() already ran; we must schedule.
        assert!(reactive_change_needs_layout_pass(true));
    }
}
