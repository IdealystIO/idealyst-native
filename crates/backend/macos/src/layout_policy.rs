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

/// Whether a `Backend::insert` should schedule a layout pass. Same gate as a
/// reactive style change: during the initial build the parent isn't in a window
/// yet and `finish` lays the whole tree out, so scheduling is waste; but a
/// POST-mount insert into a window-attached parent (a `presence`/`when` mount —
/// the Settings/Preview screens) is exactly what `finish` never revisits, so it
/// must kick a pass or the new subtree stays unsized (0×0) and invisible.
pub(crate) fn insert_needs_layout_pass(parent_attached_to_window: bool) -> bool {
    parent_attached_to_window
}

/// Minimum per-axis delta (in points) before a `setFrameSize:` on the host
/// resize observer counts as a real resize. AppKit's autoresize math can emit
/// sub-pixel jitter for a nominally-unchanged size; reacting to it would fire
/// redundant layout passes.
pub(crate) const RESIZE_EPSILON: f32 = 0.5;

/// What the macOS host resize observer should do for a `setFrameSize:`, given
/// the size it last reacted to (`last`) and the incoming size (`next`).
///
/// `Backend::finish` lays out once at mount; a raw window resize produces no
/// reactive render, so the observer is the only thing that re-runs layout.
/// This is the pure decision behind that observer's objc2 method — the AppKit
/// `NSView`/`setFrameSize:` plumbing needs the main thread and a live window,
/// so the logic lives here where `cargo test` can pin it. Mirrors the iOS
/// `LayoutObserverView` dedupe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResizeReaction {
    /// Mirror the new size into the reactive `viewport_size()` signal.
    pub mirror_viewport: bool,
    /// Kick a coalesced layout pass so every frame is recomputed.
    pub schedule_pass: bool,
}

/// Decide how the resize observer reacts. `last` is the size it last reacted to
/// — `(0.0, 0.0)` means it has never seen a real bounds (the host had no size
/// at mount). The observer's `last_size` is otherwise pre-seeded to the host's
/// mount-time bounds, so the first call here that differs is a genuine resize.
///
/// - Unchanged within [`RESIZE_EPSILON`] → do nothing (the redundant
///   `setFrameSize:` AppKit emits, or the seeded initial `setFrame:`).
/// - Changed, but `last` was `(0, 0)` → mirror the viewport (author code may
///   want the first real size) but DON'T schedule a pass: `finish` already ran
///   the mount layout against these same bounds.
/// - Changed from a real prior size → mirror AND schedule: this is the actual
///   window resize `finish` never revisits.
pub(crate) fn resize_observer_reaction(last: (f32, f32), next: (f32, f32)) -> ResizeReaction {
    let changed =
        (last.0 - next.0).abs() > RESIZE_EPSILON || (last.1 - next.1).abs() > RESIZE_EPSILON;
    if !changed {
        return ResizeReaction { mirror_viewport: false, schedule_pass: false };
    }
    let had_real_size = last.0 != 0.0 || last.1 != 0.0;
    ResizeReaction { mirror_viewport: true, schedule_pass: had_real_size }
}

/// Whether a `PrivateLayer` overlay child window needs its frame rewritten to
/// match the host's content area. `addChildWindow:ordered:` tracks the parent
/// window's *moves* (origin) but NOT its *size*, so on a host resize the
/// overlay keeps its old size and the toolbar inside it lays out against stale
/// bounds. We rewrite the overlay frame only when the size actually drifts (a
/// real host resize) so we don't fight AppKit's per-frame child-window move
/// tracking on every layout pass. `current` is the overlay's current size,
/// `host_content` the host window's current content-rect size.
pub(crate) fn detached_overlay_needs_resize(current: (f32, f32), host_content: (f32, f32)) -> bool {
    (current.0 - host_content.0).abs() > RESIZE_EPSILON
        || (current.1 - host_content.1).abs() > RESIZE_EPSILON
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

    #[test]
    fn post_mount_insert_into_attached_parent_schedules() {
        // Regression: a `presence`/`when` mount AFTER the initial render (the
        // whiteboard Settings/Preview screens) inserts into a window-attached
        // parent. Without a scheduled pass the new subtree stays 0×0 and
        // invisible — the macOS "can't open settings / see the preview" bug.
        assert!(insert_needs_layout_pass(true));
        // During the initial build the parent isn't in a window yet; `finish`
        // lays it out, so scheduling then is pure waste.
        assert!(!insert_needs_layout_pass(false));
    }

    // Regression: a window resize must recompute element positions. Before the
    // observer existed, `finish` laid out once at mount and a raw resize left
    // every frame stale. These pin the observer's react/skip decision.

    #[test]
    fn resize_from_real_size_schedules_a_pass() {
        // Drag the window from 800×600 to 1024×768 → mirror viewport AND
        // re-run layout. This is the case `finish` never revisits.
        let r = resize_observer_reaction((800.0, 600.0), (1024.0, 768.0));
        assert_eq!(
            r,
            ResizeReaction { mirror_viewport: true, schedule_pass: true },
        );
    }

    #[test]
    fn unchanged_size_is_a_noop() {
        // AppKit re-emits `setFrameSize:` for the same size (and the seeded
        // initial `setFrame:` in set_host_root matches the seed); neither must
        // fire a pass.
        let r = resize_observer_reaction((1024.0, 768.0), (1024.0, 768.0));
        assert_eq!(
            r,
            ResizeReaction { mirror_viewport: false, schedule_pass: false },
        );
    }

    #[test]
    fn subpixel_jitter_is_ignored() {
        // Autoresize math can nudge the size by a fraction of a point; that
        // isn't a resize.
        let r = resize_observer_reaction((1024.0, 768.0), (1024.2, 767.9));
        assert_eq!(
            r,
            ResizeReaction { mirror_viewport: false, schedule_pass: false },
        );
    }

    #[test]
    fn first_real_size_after_zero_mirrors_but_skips_pass() {
        // Host had no bounds at mount (seed 0×0); the first real fill mirrors
        // the viewport for author code but skips the pass — `finish` already
        // laid out against these bounds.
        let r = resize_observer_reaction((0.0, 0.0), (1024.0, 768.0));
        assert_eq!(
            r,
            ResizeReaction { mirror_viewport: true, schedule_pass: false },
        );
    }

    // Regression: a PrivateLayer overlay (screen-recorder toolbar) is a child
    // window that tracks the host's moves but not its size, so a host resize
    // left the overlay — and the toolbar laid out inside it — at the old size.

    #[test]
    fn overlay_resizes_when_host_content_grows() {
        // Host window grew from 800×600 to 1200×800 content → overlay must
        // follow so the toolbar re-lays-out against the real drawable area.
        assert!(detached_overlay_needs_resize((800.0, 600.0), (1200.0, 800.0)));
    }

    #[test]
    fn overlay_skips_when_size_matches() {
        // Steady state (only a move, which AppKit's child-window tracking
        // already handles) → don't rewrite the frame and fight that tracking.
        assert!(!detached_overlay_needs_resize((1200.0, 800.0), (1200.0, 800.0)));
        // Sub-pixel drift isn't a resize either.
        assert!(!detached_overlay_needs_resize((1200.0, 800.0), (1200.3, 799.8)));
    }
}
