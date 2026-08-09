//! `Position::Sticky` on macOS — pins views to their enclosing
//! `NSScrollView` as the user scrolls. The AppKit mirror of the iOS
//! implementation in `backend-ios-mobile/src/imp/sticky.rs`; read that
//! module's header for the CSS semantics and the registry-side-state
//! design rationale (Rule #7: mechanism diverges, behavior converges).
//!
//! macOS divergences in MECHANISM only:
//!
//! - **Scroll signal**: iOS polls `contentOffset` on a `CADisplayLink`.
//!   AppKit has a push channel — `NSViewBoundsDidChangeNotification` on
//!   the scroll view's clip view (the same channel `create_scroll_view`
//!   already uses for `on_scroll`) — so we observe instead of polling:
//!   zero work while the user isn't scrolling.
//! - **Pin write**: iOS applies a `CGAffineTransform`; here we move the
//!   view's FRAME (`setFrameOrigin:` = Taffy y + pin translate). A
//!   layer-transform pin was tried first and text inside the pinned
//!   subtree went blank while scrolled: AppKit manages scroll-view
//!   drawing by frame, and views whose frames sit outside the visible /
//!   prepared content rect get their draw skipped and their backing
//!   stores purged — layer properties (borders, backgrounds) survive
//!   but `drawRect`-drawn label text does not, so a transform can move
//!   a view into view without AppKit ever drawing it. Frame moves keep
//!   AppKit's draw management, hit-testing, and cursor rects all
//!   natively correct with no compensation anywhere.
//! - **Natural position**: same Taffy-parent walk as iOS for the
//!   content-space `natural_y` the pin compute needs; the write targets
//!   the child's parent-relative Taffy frame, the same coordinate the
//!   layout pass itself applies. The frame-apply pass diffs against
//!   TAFFY values (not the live frame), so a pinned origin persists
//!   until layout genuinely changes — and the layout-pass tail retick
//!   re-pins immediately when it does.

use std::collections::HashMap;

use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::{CGPoint, CGRect, MainThreadMarker, NSObject};

/// Sub-pixel threshold below which the tick skips the frame write.
/// Mirrors the iOS module's epsilon rationale.
const STICKY_EPSILON: f64 = 0.5;

/// One sticky child registered against a scroll view.
pub(crate) struct StickyChild {
    /// The sticky view. Retained for the registry's lifetime; dropped
    /// (and its pin offset removed) on `deregister`.
    pub(crate) view: Retained<NSView>,
    /// Per-axis pin thresholds in points, resolved from `StyleRules`
    /// by `runtime_shared::sticky::StickyInsets`.
    pub(crate) insets: runtime_shared::sticky::StickyInsets,
    /// Natural `(x, y)` of the child in the scroll view's content
    /// space. Refreshed after every layout pass by
    /// `refresh_layout_positions`.
    pub(crate) natural: (f32, f32),
    /// Last pin translate applied to the frame, per axis. Used to
    /// shift the frame back on deregister without needing layout
    /// access.
    pub(crate) last_translate: (f32, f32),
}

/// Per-scroll-view sticky state. Owns the bounds-change observer
/// target; removed from the notification center when the last child
/// deregisters.
pub(crate) struct StickyScrollEntry {
    pub(crate) scroll_view: Retained<NSView>,
    pub(crate) children: HashMap<usize, StickyChild>,
    observer: Option<Retained<NSObject>>,
}

/// Map from scroll view pointer → sticky bookkeeping.
pub(crate) type StickyRegistry = HashMap<usize, StickyScrollEntry>;

// The pin arithmetic itself lives in `runtime_shared::sticky` — one
// implementation shared by every backend, so "AppKit pins a pixel
// differently from UIKit" cannot happen (CLAUDE.md §7). This module
// keeps only the AppKit mechanism: where the natural position comes
// from, and how the pin is written.
use runtime_shared::sticky::{translate, StickyInsets};

/// Find the enclosing `NSScrollView` ancestor of `view`. The native
/// parent chain runs child → documentView → NSClipView →
/// NSScrollView, so a plain superview walk with an `isKindOfClass:`
/// check finds it. `None` = no scroll ancestor → sticky degrades to
/// relative, matching CSS.
pub(crate) fn find_enclosing_scroll_view(view: &NSView) -> Option<Retained<NSView>> {
    let scroll_class = objc2::class!(NSScrollView);
    let mut current: Option<Retained<NSView>> = unsafe { msg_send_id![view, superview] };
    while let Some(ancestor) = current {
        let is_scroll: bool = unsafe { msg_send![&ancestor, isKindOfClass: scroll_class] };
        if is_scroll {
            return Some(ancestor);
        }
        current = unsafe { msg_send_id![&ancestor, superview] };
    }
    None
}

/// Register a sticky child against its enclosing scroll view.
/// Returns `false` (caller records in `pending_sticky`) when there is
/// no scroll ancestor yet — first-mount ordering, same as iOS.
/// Idempotent via the leading `deregister`.
pub(crate) fn register(
    mtm: MainThreadMarker,
    registry: &mut StickyRegistry,
    view: &NSView,
    insets: StickyInsets,
) -> bool {
    let child_key = view as *const NSView as usize;
    deregister(registry, view);

    let Some(scroll_view) = find_enclosing_scroll_view(view) else {
        return false;
    };
    let scroll_key = &*scroll_view as *const NSView as usize;

    let child_retained: Retained<NSView> = unsafe {
        Retained::retain(view as *const NSView as *mut NSView).expect("retain sticky child")
    };

    let entry = registry.entry(scroll_key).or_insert_with(|| StickyScrollEntry {
        scroll_view: scroll_view.clone(),
        children: HashMap::new(),
        observer: None,
    });
    entry.children.insert(
        child_key,
        StickyChild {
            view: child_retained,
            insets,
            natural: (0.0, 0.0),
            last_translate: (0.0, 0.0),
        },
    );

    // Raise the pinned view above its static siblings, matching how
    // CSS paints positioned elements above non-positioned ones — a
    // frozen column must draw over the cells that slide beneath it.
    // Views are layer-backed at creation (`setWantsLayer:` in
    // `create_view`), so `zPosition` orders sibling layers; it does
    // NOT change AppKit hit-test order (subview order), which is
    // acceptable: the overlap region is content the pin visually
    // covers, and interactive children keep working outside it.
    raise_z(view, 1.0);

    // First child for this scroll view installs the bounds-change
    // observer (the same clipView notification channel `on_scroll`
    // uses). The observer callback reaches the backend through the
    // global-self handle — same shape as the animation system. If it
    // fires while the backend is borrowed (a layout pass moving the
    // clip bounds), it safely no-ops; the layout-pass tail retick
    // covers that case.
    if entry.observer.is_none() {
        let clip_view: Option<Retained<NSView>> =
            unsafe { msg_send_id![&scroll_view, contentView] };
        if let Some(clip_view) = clip_view {
            let _: () = unsafe { msg_send![&clip_view, setPostsBoundsChangedNotifications: true] };
            let target = crate::imp::callbacks::ScrollObserverTarget::new(
                mtm,
                std::rc::Rc::new(move |_x: f32, _y: f32| {
                    crate::imp::with_backend(|b| tick_scroll_view(b, scroll_key));
                }),
            );
            let center: *mut objc2::runtime::AnyObject =
                unsafe { msg_send![objc2::class!(NSNotificationCenter), defaultCenter] };
            let name = objc2_foundation::NSString::from_str("NSViewBoundsDidChangeNotification");
            let sel = objc2::sel!(boundsDidChange:);
            let _: () = unsafe {
                msg_send![
                    center,
                    addObserver: &*target,
                    selector: sel,
                    name: &*name,
                    object: &*clip_view,
                ]
            };
            entry.observer = Some(unsafe { Retained::cast::<NSObject>(target) });
        }
    }
    true
}

/// Remove `view` from any scroll view's registry, shifting its frame
/// back by the applied pin offset. Empties + tears down the
/// per-scroll-view entry (and its notification observer) when the
/// last child leaves.
pub(crate) fn deregister(registry: &mut StickyRegistry, view: &NSView) {
    let child_key = view as *const NSView as usize;
    let mut emptied = Vec::new();
    for (scroll_key, entry) in registry.iter_mut() {
        if let Some(child) = entry.children.remove(&child_key) {
            remove_pin_offset(&child);
            if entry.children.is_empty() {
                emptied.push(*scroll_key);
            }
        }
    }
    for scroll_key in emptied {
        if let Some(mut entry) = registry.remove(&scroll_key) {
            remove_observer(entry.observer.take());
        }
    }
}

/// Tear down an entire scroll view's sticky bookkeeping — used when
/// the scroll view itself unmounts (screen swap removes the subtree,
/// so per-child deregisters never run).
pub(crate) fn deregister_scroll_view(registry: &mut StickyRegistry, scroll_view: &NSView) {
    let scroll_key = scroll_view as *const NSView as usize;
    let Some(mut entry) = registry.remove(&scroll_key) else {
        return;
    };
    for (_, child) in entry.children.drain() {
        remove_pin_offset(&child);
    }
    remove_observer(entry.observer.take());
}

fn remove_observer(observer: Option<Retained<NSObject>>) {
    if let Some(observer) = observer {
        let center: *mut objc2::runtime::AnyObject =
            unsafe { msg_send![objc2::class!(NSNotificationCenter), defaultCenter] };
        let _: () = unsafe { msg_send![center, removeObserver: &*observer] };
    }
}

/// Set the sticky raise on a view's backing layer. `1.0` on register
/// (paint above static siblings, CSS positioned-element order), `0.0`
/// on deregister so a Sticky → Relative flip restores document order.
fn raise_z(view: &NSView, z: f64) {
    let layer: Option<Retained<NSObject>> = unsafe { msg_send_id![view, layer] };
    if let Some(layer) = layer {
        let _: () = unsafe { msg_send![&layer, setZPosition: z] };
    }
}

/// Shift a deregistering child's frame back by whatever pin offset it
/// carries, so a Sticky → Relative transition doesn't strand the view
/// below its natural position. No layout access needed — the offset is
/// tracked per child.
fn remove_pin_offset(child: &StickyChild) {
    raise_z(&child.view, 0.0);
    let (dx, dy) = child.last_translate;
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    let cur: CGRect = unsafe { msg_send![&*child.view, frame] };
    let origin = CGPoint {
        x: cur.origin.x - dx as f64,
        y: cur.origin.y - dy as f64,
    };
    let _: () = unsafe { msg_send![&*child.view, setFrameOrigin: origin] };
}

/// Current scroll offset AND scrollport extent of the scroll view's
/// clip view, in points (the document view is flipped, so y grows
/// downward exactly like web/iOS; x grows rightward on every backend).
/// The extent is the clip view's bounds size — the visible scrollport,
/// which trailing-edge (`bottom` / `right`) pins measure against.
fn scroll_geometry(scroll_view: &NSView) -> ((f32, f32), (f32, f32)) {
    let clip: Option<Retained<NSView>> = unsafe { msg_send_id![scroll_view, contentView] };
    let Some(clip) = clip else { return ((0.0, 0.0), (0.0, 0.0)) };
    let bounds: CGRect = unsafe { msg_send![&clip, bounds] };
    (
        (bounds.origin.x as f32, bounds.origin.y as f32),
        (bounds.size.width as f32, bounds.size.height as f32),
    )
}

/// Recompute + apply the pin for one scroll view's sticky children.
/// Fired by the bounds-change observer per scroll event and by the
/// layout pass after frames move. The write is ABSOLUTE — Taffy
/// frame y + translate — so it self-heals regardless of whether the
/// frame-apply pass rewrote the natural frame since the last tick.
pub(crate) fn tick_scroll_view(backend: &mut crate::imp::MacosBackend, scroll_key: usize) {
    let Some(entry) = backend.sticky_registry.get_mut(&scroll_key) else {
        return;
    };
    let (scroll, viewport) = scroll_geometry(&entry.scroll_view);
    for (child_key, child) in entry.children.iter_mut() {
        // Parent-relative Taffy frame — the same coordinates the
        // layout pass applies with `setFrame:`.
        let Some((_, node)) = backend.view_to_layout.get(child_key) else {
            continue;
        };
        let frame = backend.layout.frame_of(*node);
        // The child's extent comes from the same Taffy frame the pin
        // composes onto — trailing-edge pins measure the far edge.
        let size = (frame.width, frame.height);
        let (dx, dy) = translate(child.insets, child.natural, size, scroll, viewport);
        let target_x = frame.x as f64 + dx as f64;
        let target_y = frame.y as f64 + dy as f64;
        let cur: CGRect = unsafe { msg_send![&*child.view, frame] };
        child.last_translate = (dx, dy);
        if (cur.origin.x - target_x).abs() < STICKY_EPSILON
            && (cur.origin.y - target_y).abs() < STICKY_EPSILON
        {
            continue;
        }
        let origin = CGPoint { x: target_x, y: target_y };
        let _: () = unsafe { msg_send![&*child.view, setFrameOrigin: origin] };
    }
}

/// Tick every registered scroll view — called from the layout pass so
/// pins re-apply against fresh natural positions.
pub(crate) fn tick_all(backend: &mut crate::imp::MacosBackend) {
    let keys: Vec<usize> = backend.sticky_registry.keys().copied().collect();
    for key in keys {
        tick_scroll_view(backend, key);
    }
}

/// Refresh the cached `natural_y` for every sticky child after a
/// layout pass — the same Taffy-parent frame-summing walk as iOS
/// (frames come from the layout tree, never from the possibly-pinned
/// live NSView).
pub(crate) fn refresh_layout_positions(
    registry: &mut StickyRegistry,
    layout: &runtime_layout::LayoutTree,
    view_to_layout: &HashMap<usize, (Retained<NSView>, runtime_layout::LayoutNode)>,
) {
    for (scroll_key, entry) in registry.iter_mut() {
        for (child_key, child) in entry.children.iter_mut() {
            let Some(natural) =
                compute_natural_in_scroll(*child_key, *scroll_key, layout, view_to_layout)
            else {
                continue;
            };
            child.natural = natural;
        }
    }
}

/// Sum Taffy frame origins from `child_key` up to (but not including)
/// `scroll_key`, on both axes. `None` when the chain can't be traced.
///
/// Both components are summed unconditionally rather than only the
/// pinned axis: `refresh_layout_positions` runs once per layout pass
/// for every registered child, and branching per axis here would mean
/// a child that later gains a `left` inset (a reactive style change)
/// would tick against a stale `natural.x` until the next layout.
fn compute_natural_in_scroll(
    child_key: usize,
    scroll_key: usize,
    layout: &runtime_layout::LayoutTree,
    view_to_layout: &HashMap<usize, (Retained<NSView>, runtime_layout::LayoutNode)>,
) -> Option<(f32, f32)> {
    let (_, child_node) = view_to_layout.get(&child_key)?;
    let (_, scroll_node) = view_to_layout.get(&scroll_key)?;

    let mut sum = (0.0_f32, 0.0_f32);
    let mut cursor = *child_node;
    let mut steps = 0;
    while cursor != *scroll_node {
        let frame = layout.frame_of(cursor);
        sum.0 += frame.x;
        sum.1 += frame.y;
        let parent = layout.parent_of(cursor)?;
        cursor = parent;
        steps += 1;
        if steps > 256 {
            return None;
        }
    }
    Some(sum)
}

// =========================================================================
// Tests
//
// The pin arithmetic's own regressions now live with the arithmetic,
// in `runtime_shared::sticky` — one suite instead of a copy per
// backend. What stays here is the AppKit-specific consequence of that
// math: which frame origin components the tick writes.
// =========================================================================

#[cfg(test)]
mod tests {
    use runtime_shared::sticky::{translate, StickyInsets};

    /// The "TOC scrolls away with the content on macOS" regression, at
    /// the layer this module owns: `tick_scroll_view` composes the
    /// shared translate onto the Taffy frame origin, so a vertical pin
    /// must move `origin.y` and leave `origin.x` on its laid-out value.
    #[test]
    fn regression_macos_sticky_pins_when_scrolled_past_threshold() {
        let insets = StickyInsets {
            top: Some(32.0),
            ..Default::default()
        };
        let frame = (12.0_f32, 100.0_f32);
        let size = (80.0_f32, 40.0_f32);
        let viewport = (600.0_f32, 800.0_f32);

        // Above the pin point — child stays at its natural position
        // (this was ALL macOS did before the fix, at every offset).
        assert_eq!(translate(insets, frame, size, (0.0, 0.0), viewport), (0.0, 0.0));
        assert_eq!(translate(insets, frame, size, (0.0, 68.0), viewport), (0.0, 0.0));

        // Past the pin point — the rendered y tracks the viewport and
        // x is untouched, so `setFrameOrigin:` keeps the laid-out x.
        let (dx, dy) = translate(insets, frame, size, (0.0, 500.0), viewport);
        assert_eq!(dx, 0.0, "a vertical pin must not move the frame's x");
        assert!(
            ((frame.1 + dy) - 532.0).abs() < 1e-5,
            "pinned rendered y must equal scroll_y + threshold",
        );
    }

    /// A frozen column on macOS: `left` must move `origin.x` while the
    /// child keeps scrolling vertically. Before horizontal support the
    /// registry carried a single `threshold_top`, so this produced no
    /// horizontal movement at all.
    #[test]
    fn regression_macos_sticky_left_never_pins_horizontally() {
        let insets = StickyInsets {
            left: Some(0.0),
            ..Default::default()
        };
        let (dx, dy) = translate(insets, (100.0, 40.0), (80.0, 24.0), (400.0, 900.0), (600.0, 800.0));
        assert!(((100.0 + dx) - 400.0).abs() < 1e-5);
        assert_eq!(dy, 0.0, "a horizontal pin must not move the frame's y");
    }

    /// A RIGHT-frozen column on macOS: `right` pulls `origin.x` back so
    /// the cell's far edge parks at the clip view's trailing edge, and
    /// releases once the content scrolls it into natural view. Before
    /// trailing-edge support the tick had no scrollport/child extents
    /// to measure against, so `right` produced no pin on any native
    /// backend.
    #[test]
    fn regression_macos_sticky_right_pins_at_clipview_trailing_edge() {
        let insets = StickyInsets {
            right: Some(0.0),
            ..Default::default()
        };
        // Column at x=900, 100 wide, clip view 600 wide, unscrolled:
        // parks at 600 - 100 = 500.
        let (dx, dy) = translate(insets, (900.0, 0.0), (100.0, 40.0), (0.0, 0.0), (600.0, 800.0));
        assert!(((900.0 + dx) - 500.0).abs() < 1e-5);
        assert_eq!(dy, 0.0, "a horizontal pin must not move the frame's y");
        // Scrolled far enough right — rides the content again.
        let (dx, _) = translate(insets, (900.0, 0.0), (100.0, 40.0), (400.0, 0.0), (600.0, 800.0));
        assert_eq!(dx, 0.0);
    }
}
