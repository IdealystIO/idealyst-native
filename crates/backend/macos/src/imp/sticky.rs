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
    /// Pin threshold in points, from `StyleRules.top`.
    pub(crate) threshold_top: f32,
    /// Natural y of the child in the scroll view's content space.
    /// Refreshed after every layout pass by `refresh_layout_positions`.
    pub(crate) natural_y: f32,
    /// Last pin translate applied to the frame. Used to shift the
    /// frame back on deregister without needing layout access.
    pub(crate) last_translate: f32,
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

/// Pure pin compute — identical to the iOS module's (the duplicated
/// pure-fn-with-tests pattern `private_layer_hittest` also follows).
/// Translation that keeps the child rendered at `scroll_y +
/// threshold_top` once its natural top scrolls past the threshold.
#[inline]
pub(crate) fn compute_translate(natural_y: f32, threshold_top: f32, scroll_y: f32) -> f32 {
    let pinned_y = scroll_y + threshold_top;
    if pinned_y > natural_y {
        pinned_y - natural_y
    } else {
        0.0
    }
}

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
    threshold_top: f32,
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
            threshold_top,
            natural_y: 0.0,
            last_translate: 0.0,
        },
    );

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

/// Shift a deregistering child's frame back by whatever pin offset it
/// carries, so a Sticky → Relative transition doesn't strand the view
/// below its natural position. No layout access needed — the offset is
/// tracked per child.
fn remove_pin_offset(child: &StickyChild) {
    if child.last_translate == 0.0 {
        return;
    }
    let cur: CGRect = unsafe { msg_send![&*child.view, frame] };
    let origin = CGPoint {
        x: cur.origin.x,
        y: cur.origin.y - child.last_translate as f64,
    };
    let _: () = unsafe { msg_send![&*child.view, setFrameOrigin: origin] };
}

/// Current scroll offset of the scroll view's clip view — the y the
/// content has scrolled, in points (the document view is flipped, so
/// y grows downward exactly like web/iOS).
fn scroll_offset_y(scroll_view: &NSView) -> f32 {
    let clip: Option<Retained<NSView>> = unsafe { msg_send_id![scroll_view, contentView] };
    let Some(clip) = clip else { return 0.0 };
    let bounds: CGRect = unsafe { msg_send![&clip, bounds] };
    bounds.origin.y as f32
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
    let scroll_y = scroll_offset_y(&entry.scroll_view);
    for (child_key, child) in entry.children.iter_mut() {
        // Parent-relative Taffy frame — the same coordinates the
        // layout pass applies with `setFrame:`.
        let Some((_, node)) = backend.view_to_layout.get(child_key) else {
            continue;
        };
        let natural = backend.layout.frame_of(*node);
        let translate = compute_translate(child.natural_y, child.threshold_top, scroll_y);
        let target_y = natural.y as f64 + translate as f64;
        let cur: CGRect = unsafe { msg_send![&*child.view, frame] };
        child.last_translate = translate;
        if (cur.origin.y - target_y).abs() < STICKY_EPSILON {
            continue;
        }
        let origin = CGPoint { x: natural.x as f64, y: target_y };
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
            let Some(natural_y) =
                compute_natural_y_in_scroll(*child_key, *scroll_key, layout, view_to_layout)
            else {
                continue;
            };
            child.natural_y = natural_y;
        }
    }
}

/// Sum Taffy frame y values from `child_key` up to (but not
/// including) `scroll_key`. `None` when the chain can't be traced.
fn compute_natural_y_in_scroll(
    child_key: usize,
    scroll_key: usize,
    layout: &runtime_layout::LayoutTree,
    view_to_layout: &HashMap<usize, (Retained<NSView>, runtime_layout::LayoutNode)>,
) -> Option<f32> {
    let (_, child_node) = view_to_layout.get(&child_key)?;
    let (_, scroll_node) = view_to_layout.get(&scroll_key)?;

    let mut sum_y = 0.0_f32;
    let mut cursor = *child_node;
    let mut steps = 0;
    while cursor != *scroll_node {
        sum_y += layout.frame_of(cursor).y;
        let parent = layout.parent_of(cursor)?;
        cursor = parent;
        steps += 1;
        if steps > 256 {
            return None;
        }
    }
    Some(sum_y)
}

// =========================================================================
// Tests — pure compute, host-runnable (macOS host). Mirrors the iOS
// module's regression names per CLAUDE.md §8.
// =========================================================================

#[cfg(test)]
mod tests {
    use super::compute_translate;

    /// The "TOC scrolls away with the content on macOS" regression:
    /// scrolling past the threshold must translate the child down by
    /// the overshoot so it renders pinned at `scroll_y + threshold`.
    #[test]
    fn regression_macos_sticky_pins_when_scrolled_past_threshold() {
        let natural_y = 100.0;
        let threshold = 32.0;

        // Above the pin point — child stays at its natural position
        // (this was ALL macOS did before the fix, at every offset).
        assert_eq!(compute_translate(natural_y, threshold, 0.0), 0.0);
        assert_eq!(compute_translate(natural_y, threshold, 68.0), 0.0);

        // Past the pin point — the rendered y tracks the viewport.
        let scroll_y = 500.0;
        let t = compute_translate(natural_y, threshold, scroll_y);
        assert!(
            ((natural_y + t) - (scroll_y + threshold)).abs() < 1e-5,
            "pinned rendered y must equal scroll_y + threshold",
        );
    }

    /// Scrolling back up un-pins: translate returns to zero.
    #[test]
    fn regression_macos_sticky_unpins_on_scroll_back() {
        let t = compute_translate(100.0, 32.0, 500.0);
        assert!(t > 0.0);
        assert_eq!(compute_translate(100.0, 32.0, 0.0), 0.0);
    }
}
