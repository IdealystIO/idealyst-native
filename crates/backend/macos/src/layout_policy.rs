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

/// Content bounding box (max right edge, max bottom edge) of a scroll view's
/// Taffy subtree, projected into the scroll view's content coordinate space.
///
/// The macOS backend parents a scroll view's children directly under the OUTER
/// scroll-view Taffy node (mirroring iOS's single-`UIScrollView` model), so the
/// layout pass sizes them — but the inner `NSScrollView` documentView is a
/// native-only container Taffy never positions. This computes the size the
/// documentView must take so AppKit can scroll: without it the documentView
/// stays 0×0 and clips every (correctly laid-out) child to nothing — the macOS
/// "scroll page renders blank" bug.
///
/// `roots` are the scroll node's direct Taffy children. `frame_of(n)` returns
/// `(x, y, w, h)` relative to `n`'s parent; `children_of(n)` returns `n`'s
/// children. The walk descends the FULL subtree, not just the direct children:
/// authors routinely set `min_height: 100%` on a page's outermost container,
/// which Taffy clamps to the scroll view's bounds, while a Spacer-pushed footer
/// (or any overflowing grandchild) sits past that clamped frame. Stopping at
/// direct children would under-report the content height and the tail wouldn't
/// scroll into view. Mirrors the iOS backend's `contentSize` sync.
///
/// Returns `(0.0, 0.0)` for an empty subtree.
pub(crate) fn scroll_content_bbox<N: Copy>(
    roots: &[N],
    frame_of: impl Fn(N) -> (f32, f32, f32, f32),
    children_of: impl Fn(N) -> Vec<N>,
) -> (f32, f32) {
    let mut max_x = 0.0_f32;
    let mut max_y = 0.0_f32;
    // (node, parent_origin_x, parent_origin_y) — accumulate the running origin
    // while descending so a deep descendant's frame projects into content space.
    let mut stack: Vec<(N, f32, f32)> = roots.iter().map(|&n| (n, 0.0, 0.0)).collect();
    while let Some((node, origin_x, origin_y)) = stack.pop() {
        let (fx, fy, fw, fh) = frame_of(node);
        let nx = origin_x + fx;
        let ny = origin_y + fy;
        max_x = max_x.max(nx + fw);
        max_y = max_y.max(ny + fh);
        for child in children_of(node) {
            stack.push((child, nx, ny));
        }
    }
    (max_x, max_y)
}

/// The size an `NSScrollView`'s documentView must take, given its content's
/// bounding box (`content`, from [`scroll_content_bbox`]) and the scroll view's
/// own clip size (`clip`).
///
/// The documentView fills *at least* the clip so short content still paints
/// edge-to-edge (a flipped documentView shorter than the clip would otherwise
/// leave a gap), and grows *past* the clip on either axis to make the overflow
/// scrollable. This is the pure decision behind `sync_scroll_document_views`'s
/// `setFrame:` — the AppKit `documentView`/`bounds` plumbing needs the main
/// thread and a live window, so the sizing math lives here where `cargo test`
/// can pin it (mirroring how the rest of this module factors the AppKit-bound
/// decisions out). It's the macOS analogue of the iOS `contentSize` value.
///
/// CRUCIAL: the per-axis `max` is what makes a **top-level** scroll view paint.
/// A scroll view that fills the whole window has its bbox equal to the viewport
/// when content is short (e.g. `(800, 40)` for a 40pt-tall page in an 800×600
/// window); without clamping to the clip the documentView would be 40pt tall and
/// the bottom of the window would be a blank documentView gap. With the clamp the
/// documentView is `800×600` — full-bleed — and the laid-out children show. The
/// macOS "top-level scroll page renders blank" bug is a 0×0 documentView, which
/// this returns ONLY when the clip is also 0×0 (no usable bounds yet); any real
/// clip yields a real size. See [`scroll_document_view_is_degenerate`] for the
/// guard that warns if a 0×0 documentView ever slips through with real children.
pub(crate) fn scroll_document_view_size(content: (f32, f32), clip: (f32, f32)) -> (f32, f32) {
    (content.0.max(clip.0), content.1.max(clip.1))
}

/// Whether a scroll view's documentView is about to be sized `0×0` *despite*
/// having a non-empty content bounding box — i.e. the "renders blank" regression
/// is live. Returns `true` only when the chosen documentView size is degenerate
/// on either axis while the content bbox reports real children. A legitimately
/// empty scroll view (no children → `content == (0, 0)`) is NOT flagged.
///
/// `sync_scroll_document_views` calls this and logs a loud warning when it trips,
/// so the next time the top-level-scroll-blank bug reappears it surfaces in the
/// console at the exact layout pass that caused it, instead of as an
/// inexplicably empty window. `clip` is the scroll view's own bounds: if BOTH
/// the clip and the content are zero the scroll view simply has no usable bounds
/// yet (pre-first-paint) — that's not the bug, so don't warn.
pub(crate) fn scroll_document_view_is_degenerate(
    content: (f32, f32),
    doc: (f32, f32),
    clip: (f32, f32),
) -> bool {
    let has_real_content = content.0 > 0.0 && content.1 > 0.0;
    let doc_degenerate = doc.0 <= 0.0 || doc.1 <= 0.0;
    let clip_has_bounds = clip.0 > 0.0 || clip.1 > 0.0;
    has_real_content && doc_degenerate && clip_has_bounds
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny tree fixture: each node is an index into `frames`/`kids`.
    struct Tree {
        frames: Vec<(f32, f32, f32, f32)>,
        kids: Vec<Vec<usize>>,
    }
    impl Tree {
        fn bbox(&self, roots: &[usize]) -> (f32, f32) {
            scroll_content_bbox(
                roots,
                |n| self.frames[n],
                |n| self.kids[n].clone(),
            )
        }
    }

    #[test]
    fn empty_scroll_has_zero_content() {
        let t = Tree { frames: vec![], kids: vec![] };
        assert_eq!(t.bbox(&[]), (0.0, 0.0));
    }

    #[test]
    fn single_child_drives_content_size() {
        // One content view filling the column, shorter than the viewport.
        let t = Tree { frames: vec![(0.0, 0.0, 744.0, 500.0)], kids: vec![vec![]] };
        assert_eq!(t.bbox(&[0]), (744.0, 500.0));
    }

    #[test]
    fn deep_descendant_past_clamped_parent_extends_content() {
        // Regression: the macOS scroll-page-blank fix. A page container (node 0)
        // is clamped to the scroll view's 744×768 bounds (min_height: 100%), but
        // a footer (node 1) sits at local y=900 — past the clamped parent. A
        // direct-children-only walk would report height 768 and the footer would
        // never scroll into view; the deep walk projects it to y=900 → 940.
        let t = Tree {
            frames: vec![(0.0, 0.0, 744.0, 768.0), (0.0, 900.0, 200.0, 40.0)],
            kids: vec![vec![1], vec![]],
        };
        assert_eq!(t.bbox(&[0]), (744.0, 940.0));
    }

    #[test]
    fn horizontal_overflow_extends_width() {
        // Two siblings under a row; the second starts at x=744 and is 300 wide,
        // so content width is 1044 (scrolls horizontally).
        let t = Tree {
            frames: vec![
                (0.0, 0.0, 1044.0, 200.0), // row container
                (0.0, 0.0, 744.0, 200.0),
                (744.0, 0.0, 300.0, 200.0),
            ],
            kids: vec![vec![1, 2], vec![], vec![]],
        };
        assert_eq!(t.bbox(&[0]), (1044.0, 200.0));
    }

    #[test]
    fn nested_origins_accumulate() {
        // child at (0, 100) is only 40 tall (bottom 140); its grandchild at
        // local (10, 20) size 50×30 projects to (10, 120) with bottom 150 —
        // overflowing the child. The bbox bottom must follow the grandchild
        // (150), not stop at the child (140).
        let t = Tree {
            frames: vec![
                (0.0, 100.0, 744.0, 40.0), // child, bottom 140
                (10.0, 20.0, 50.0, 30.0),  // grandchild overflows to y=150
            ],
            kids: vec![vec![1], vec![]],
        };
        assert_eq!(t.bbox(&[0]), (744.0, 150.0));
    }

    // Regression: the macOS "top-level scroll_view makes the whole window
    // blank" bug. A scroll view that fills the window and wraps short content
    // must size its documentView to the FULL clip (so it paints edge-to-edge),
    // never to the smaller content height (which left a blank documentView gap).
    // And it must NEVER be 0×0 when there are laid-out children — that's the
    // exact "renders blank" symptom.

    #[test]
    fn top_level_scroll_short_content_fills_the_clip() {
        // 800×600 window, a 40pt-tall page laid out by Taffy → content bbox
        // (800, 40). The documentView must be 800×600 (full-bleed), NOT 800×40,
        // so the bottom of the window isn't a blank documentView gap and the
        // page (which IS laid out — see the `LayoutTree` probe) shows.
        let doc = scroll_document_view_size((800.0, 40.0), (800.0, 600.0));
        assert_eq!(doc, (800.0, 600.0));
        // ...and the guard does NOT trip: real children, real (non-zero) doc.
        assert!(!scroll_document_view_is_degenerate(
            (800.0, 40.0),
            doc,
            (800.0, 600.0),
        ));
    }

    #[test]
    fn top_level_scroll_tall_content_grows_past_the_clip() {
        // A 1500pt-tall page in the same window → documentView grows to 1500 so
        // the overflow scrolls; width stays the clip width.
        let doc = scroll_document_view_size((800.0, 1500.0), (800.0, 600.0));
        assert_eq!(doc, (800.0, 1500.0));
        assert!(!scroll_document_view_is_degenerate(
            (800.0, 1500.0),
            doc,
            (800.0, 600.0),
        ));
    }

    #[test]
    fn empty_scroll_view_is_not_flagged_as_blank() {
        // No children → content (0, 0). The documentView is the clip size and
        // the guard stays quiet (an empty scroll view is legitimately empty, not
        // the regression).
        let doc = scroll_document_view_size((0.0, 0.0), (800.0, 600.0));
        assert_eq!(doc, (800.0, 600.0));
        assert!(!scroll_document_view_is_degenerate(
            (0.0, 0.0),
            doc,
            (800.0, 600.0),
        ));
    }

    #[test]
    fn pre_paint_zero_clip_is_not_flagged() {
        // Before the first paint the scroll view has no bounds yet (clip 0×0);
        // even with content the guard must stay quiet — that's a not-yet-laid-out
        // state, not the blank-render bug.
        assert!(!scroll_document_view_is_degenerate(
            (800.0, 40.0),
            (800.0, 40.0),
            (0.0, 0.0),
        ));
    }

    #[test]
    fn zero_doc_with_real_children_and_clip_is_the_blank_bug() {
        // The exact regression: real laid-out children (content 800×40) but the
        // documentView somehow ends up 0×0 while the clip has real bounds. The
        // guard MUST flag this so it surfaces immediately.
        assert!(scroll_document_view_is_degenerate(
            (800.0, 40.0),
            (0.0, 0.0),
            (800.0, 600.0),
        ));
        // Degenerate on a single axis counts too (e.g. width collapsed).
        assert!(scroll_document_view_is_degenerate(
            (800.0, 40.0),
            (0.0, 40.0),
            (800.0, 600.0),
        ));
    }

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
