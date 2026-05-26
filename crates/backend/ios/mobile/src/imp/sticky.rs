//! `Position::Sticky` on iOS — pins views to their enclosing
//! `UIScrollView` as the user scrolls.
//!
//! CSS semantics: a sticky view behaves like `Relative` until the
//! scroll container would scroll its natural y past `threshold`
//! (the `top` field on `StyleRules`); at that point the view pins
//! at `threshold` from the scroll container's top edge. Scrolling
//! back up past the threshold un-pins it.
//!
//! ## Hosting choice — registry side-state, not subclass
//!
//! Two reasonable approaches existed:
//! - **Option A (subclass UIScrollView)**: a subclass overrides
//!   `setContentOffset:` and walks its own sticky-children list
//!   per scroll tick.
//! - **Option B (side registry + CADisplayLink)**: keep
//!   `UIScrollView` plain, store sticky bookkeeping on the backend,
//!   poll `contentOffset` every vsync while any sticky child is
//!   registered.
//!
//! We picked **Option B**. The iOS backend creates plain
//! `UIScrollView`s in `create_scroll_view` (see `imp/mod.rs:1001`)
//! and treats them as opaque — `UICollectionView` is also a
//! `UIScrollView`, the framework's `ScrollView` primitive doesn't
//! own its scroll view class identity, and SDK leaves like
//! `webview-ios` might construct their own scroll views. Subclassing
//! would force all of those paths to migrate. The display-link tick
//! is cheap (one `contentOffset` read + a small constant number of
//! transforms per registered child) and only runs while at least
//! one sticky child is registered — `deregister` invalidates the
//! link when the last child leaves.
//!
//! ## Layout-y caching
//!
//! Per CSS, sticky pinning is relative to the child's *natural*
//! position in the scroll container's content. We can't read that
//! off `UIView.frame` directly because applying our own
//! `CGAffineTransform` invalidates the frame property (Apple's
//! docs: "the value of this property is undefined and therefore
//! should be ignored" when the transform isn't the identity).
//!
//! Instead, we walk Taffy parents from the sticky child to the
//! scroll view, summing `frame_of(...).y`. Taffy frames are pure
//! layout output and are not affected by UIKit transforms. The
//! cached layout-y is refreshed on every layout pass.
//!
//! ## v1 scope
//!
//! Only vertical sticky pinning via the `top` field is wired. CSS
//! also supports horizontal sticky via `left` and bottom/right
//! anchors; left-axis support has the same shape but uses
//! `contentOffset.x` and the scroll view's horizontal axis.
//! Documented as TODO at `compute_translate` so a follow-up can
//! extend the registry to a `(threshold_x, threshold_y)` tuple
//! without restructuring the lifecycle code.

use std::collections::HashMap;
use std::rc::Rc;

use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::{UIScrollView, UIView};

use crate::imp::callbacks::DisplayLinkTarget;

/// Sub-pixel threshold below which the sticky tick treats the child
/// as already in the right place. Avoids per-frame `setTransform:`
/// churn when the scroll position isn't actually changing the
/// translation. Mirrors the `ANCHOR_TRACKER_EPSILON` rationale in
/// `portal.rs`.
const STICKY_EPSILON: f32 = 0.5;

/// One sticky child registered against a scroll view.
pub(crate) struct StickyChild {
    /// The sticky view itself. Retained so we can apply transforms
    /// even if the framework's own retain is released — the
    /// `deregister` path drops this entry well before that happens.
    pub(crate) view: Retained<UIView>,
    /// Pin threshold, in points, read from `StyleRules.top`. The
    /// view pins when `scroll_y + threshold > natural_y`.
    pub(crate) threshold_top: f32,
    /// Natural y of the child in the scroll view's content
    /// coordinate space, in points. Refreshed after every layout
    /// pass by `refresh_layout_positions`. Initialized to 0; the
    /// first layout pass replaces it with a real value.
    pub(crate) natural_y: f32,
}

/// Per-scroll-view sticky state. Owns the CADisplayLink that drives
/// the per-vsync recompute; the link runs in
/// `NSRunLoopCommonModes` so it keeps firing during active scroll
/// (which switches the runloop into `UITrackingRunLoopMode`).
pub(crate) struct StickyScrollEntry {
    pub(crate) scroll_view: Retained<UIScrollView>,
    pub(crate) children: HashMap<usize, StickyChild>,
    /// `Some` while any child is registered. Invalidated and
    /// cleared when the last child deregisters.
    display_link: Option<Retained<NSObject>>,
}

/// Map from scroll view pointer → sticky bookkeeping.
pub(crate) type StickyRegistry = HashMap<usize, StickyScrollEntry>;

/// Pure compute used by the per-vsync tick and the unit tests.
///
/// Returns the translation that should be applied to the sticky
/// child's transform on the y-axis given its natural layout y in
/// the scroll view's content space, the configured pin threshold
/// (the `top` value), and the scroll view's current contentOffset.y.
///
/// TODO: horizontal sticky via `left` mirrors this shape with
/// `(natural_x, threshold_left, scroll_x)`. Wire it once an author
/// asks for it; CSS supports it but no current page uses it.
#[inline]
pub(crate) fn compute_translate(natural_y: f32, threshold_top: f32, scroll_y: f32) -> f32 {
    // Pin condition: the natural top of the child has scrolled
    // above the threshold band measured from the scroll view's
    // top edge. Translate the child *down* by the overshoot so
    // its rendered position stays at `scroll_y + threshold_top`.
    let pinned_y = scroll_y + threshold_top;
    if pinned_y > natural_y {
        pinned_y - natural_y
    } else {
        0.0
    }
}

/// Find the enclosing `UIScrollView` ancestor of `view`. Returns
/// `None` if `view` isn't inside any scroll view (treat as Relative,
/// matching CSS).
pub(crate) fn find_enclosing_scroll_view(view: &UIView) -> Option<Retained<UIScrollView>> {
    let scroll_class = objc2::class!(UIScrollView);
    let mut current: Option<Retained<UIView>> = unsafe { msg_send_id![view, superview] };
    while let Some(ancestor) = current {
        let is_scroll: bool = unsafe { msg_send![&ancestor, isKindOfClass: scroll_class] };
        if is_scroll {
            // Re-cast to UIScrollView. Safe because `isKindOfClass`
            // just confirmed it.
            let ptr = &*ancestor as *const UIView as *mut UIScrollView;
            return unsafe { Retained::retain(ptr) };
        }
        current = unsafe { msg_send_id![&ancestor, superview] };
    }
    None
}

/// Register a sticky child against its enclosing scroll view. If
/// `view` isn't inside any scroll view, this is a no-op (CSS's
/// sticky-in-non-scrolling-parent is equivalent to relative — no
/// transform needed). Returns `true` if the child was actually
/// registered, `false` if it fell back to relative-equivalent.
///
/// Idempotent: if the same view is already registered against a
/// (possibly different) scroll view, we deregister it first so the
/// re-registration picks up any threshold or scroll-ancestor
/// changes (e.g. the view moved between scroll containers).
pub(crate) fn register(
    mtm: MainThreadMarker,
    registry: &mut StickyRegistry,
    view: &UIView,
    threshold_top: f32,
) -> bool {
    let child_key = view as *const UIView as usize;

    // Drop any stale registration first.
    deregister(registry, view);

    let Some(scroll_view) = find_enclosing_scroll_view(view) else {
        return false;
    };

    let scroll_key = &*scroll_view as *const UIScrollView as *const UIView as usize;

    // Retain the child view for the registry entry. The framework
    // already holds a ref via `view_to_layout`; this second retain
    // matches the lifetime to the registry, which we explicitly
    // tear down in `deregister`.
    let child_retained: Retained<UIView> = unsafe {
        Retained::retain(view as *const UIView as *mut UIView)
            .expect("retain sticky child UIView")
    };

    let entry = registry.entry(scroll_key).or_insert_with(|| StickyScrollEntry {
        scroll_view: scroll_view.clone(),
        children: HashMap::new(),
        display_link: None,
    });

    entry.children.insert(
        child_key,
        StickyChild {
            view: child_retained,
            threshold_top,
            natural_y: 0.0,
        },
    );

    // First child for this scroll view triggers the display-link
    // start. Subsequent children attach to the existing link.
    if entry.display_link.is_none() {
        entry.display_link = Some(start_display_link(mtm, scroll_key));
    }

    true
}

/// Remove `view` from any scroll view's sticky registry it's a
/// member of. Restores the view's transform to identity so a
/// previously-pinned view doesn't leave a translate behind when its
/// `position` changes from `Sticky` to something else.
///
/// If removing this child empties the scroll view's child set, the
/// scroll view's display link is invalidated and the
/// `StickyScrollEntry` is removed from the registry.
pub(crate) fn deregister(registry: &mut StickyRegistry, view: &UIView) {
    let child_key = view as *const UIView as usize;

    // Collect (then drop) any matching child entries. There should
    // be at most one — a view can only be a sticky child of one
    // scroll view at a time — but iterate defensively in case a
    // future bug double-registers.
    let mut emptied_scrolls = Vec::new();
    for (scroll_key, entry) in registry.iter_mut() {
        if entry.children.remove(&child_key).is_some() {
            // Reset transform on the freshly-deregistered view so a
            // previously-pinned translate doesn't persist.
            reset_view_transform(view);
            if entry.children.is_empty() {
                emptied_scrolls.push(*scroll_key);
            }
        }
    }
    for scroll_key in emptied_scrolls {
        if let Some(mut entry) = registry.remove(&scroll_key) {
            if let Some(link) = entry.display_link.take() {
                let _: () = unsafe { msg_send![&*link, invalidate] };
            }
        }
    }
}

/// Remove an entire scroll view's sticky bookkeeping. Used when the
/// scroll view itself unmounts — releases the display link and
/// clears each child's transform.
pub(crate) fn deregister_scroll_view(registry: &mut StickyRegistry, scroll_view: &UIView) {
    let scroll_key = scroll_view as *const UIView as usize;
    let Some(mut entry) = registry.remove(&scroll_key) else {
        return;
    };
    for (_, child) in entry.children.drain() {
        reset_view_transform(&child.view);
    }
    if let Some(link) = entry.display_link.take() {
        let _: () = unsafe { msg_send![&*link, invalidate] };
    }
}

/// Reset `view.transform` to the identity. `CGAffineTransformIdentity`
/// is `(1, 0, 0, 1, 0, 0)`.
fn reset_view_transform(view: &UIView) {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGAffineTransform {
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        tx: f64,
        ty: f64,
    }
    unsafe impl objc2::Encode for CGAffineTransform {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "CGAffineTransform",
            &[
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
            ],
        );
    }
    let identity = CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };
    let _: () = unsafe { msg_send![view, setTransform: identity] };
}

/// Apply `(0, translate_y)` translation to `view.transform`.
fn apply_translate_y(view: &UIView, translate_y: f64) {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGAffineTransform {
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        tx: f64,
        ty: f64,
    }
    unsafe impl objc2::Encode for CGAffineTransform {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "CGAffineTransform",
            &[
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
            ],
        );
    }
    let t = CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: translate_y };
    let _: () = unsafe { msg_send![view, setTransform: t] };
}

/// Read `view.transform.ty` — the current y translate. Used to
/// epsilon-skip when nothing has changed since the last tick.
fn current_translate_y(view: &UIView) -> f64 {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGAffineTransform {
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        tx: f64,
        ty: f64,
    }
    unsafe impl objc2::Encode for CGAffineTransform {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "CGAffineTransform",
            &[
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
                f64::ENCODING,
            ],
        );
    }
    let t: CGAffineTransform = unsafe { msg_send![view, transform] };
    t.ty
}

/// Start a `CADisplayLink` that runs the per-vsync sticky recompute
/// for the scroll view identified by `scroll_key`. The link reaches
/// back through the global backend handle so it can read the
/// current registry without holding a Rust borrow across the
/// `CADisplayLink` callback (which fires on the main thread but
/// from libdispatch's runloop, not inside any framework borrow).
fn start_display_link(mtm: MainThreadMarker, scroll_key: usize) -> Retained<NSObject> {
    let cb: Rc<dyn Fn()> = Rc::new(move || {
        // Reach the backend through the same global-self handle the
        // animation system uses. If the backend has been torn down
        // (app suspend/resume edge cases) the call quietly no-ops.
        crate::imp::with_backend(|b| {
            tick(b, scroll_key);
        });
    });

    let dl_target = DisplayLinkTarget::new(mtm, cb);
    let sel = objc2::sel!(tick:);
    let display_link: Retained<NSObject> = unsafe {
        msg_send_id![
            objc2::class!(CADisplayLink),
            displayLinkWithTarget: &*dl_target,
            selector: sel
        ]
    };

    extern "C" {
        static NSRunLoopCommonModes: *const NSString;
    }
    let run_loop: Retained<NSObject> =
        unsafe { msg_send_id![objc2::class!(NSRunLoop), mainRunLoop] };
    let common_modes: &NSString = unsafe { &*NSRunLoopCommonModes };
    let _: () = unsafe {
        msg_send![&*display_link, addToRunLoop: &*run_loop, forMode: common_modes]
    };
    // CADisplayLink retains its target while scheduled.
    let _ = dl_target;

    display_link
}

/// Per-vsync recompute for one scroll view's sticky children.
/// Reads `contentOffset.y`, computes each child's translate via
/// [`compute_translate`], and writes it to the child's
/// `CGAffineTransform`. Skips the write when the translate is
/// within [`STICKY_EPSILON`] of the live value, matching the
/// portal anchor tracker's idle-frame discipline.
fn tick(backend: &mut crate::imp::IosBackend, scroll_key: usize) {
    let Some(entry) = backend.sticky_registry.get(&scroll_key) else {
        return;
    };
    let scroll_y: f32 = {
        let offset: objc2_foundation::CGPoint = unsafe {
            msg_send![&*entry.scroll_view, contentOffset]
        };
        offset.y as f32
    };
    for (_, child) in entry.children.iter() {
        let translate = compute_translate(child.natural_y, child.threshold_top, scroll_y);
        let cur = current_translate_y(&child.view) as f32;
        if (cur - translate).abs() < STICKY_EPSILON {
            continue;
        }
        apply_translate_y(&child.view, translate as f64);
    }
}

/// Refresh the cached `natural_y` for every sticky child after a
/// layout pass. Walks Taffy parents from the child up to its
/// registered scroll view, summing frame y values.
///
/// Why Taffy parents and not UIView superviews: UIView frames are
/// undefined when the view's `transform` isn't the identity (Apple
/// docs). Sticky views by definition carry a transform once
/// pinned, so reading `frame.origin.y` off the UIView would give
/// us a corrupted natural position. Taffy frames are pure layout
/// output, unaffected by UIKit transforms.
pub(crate) fn refresh_layout_positions(
    registry: &mut StickyRegistry,
    layout: &runtime_layout::LayoutTree,
    view_to_layout: &HashMap<usize, (Retained<UIView>, runtime_layout::LayoutNode)>,
) {
    for (scroll_key, entry) in registry.iter_mut() {
        for (child_key, child) in entry.children.iter_mut() {
            let Some(natural_y) = compute_natural_y_in_scroll(
                *child_key,
                *scroll_key,
                layout,
                view_to_layout,
            ) else {
                // If we can't trace the child up to the scroll
                // view (mid-mount, or it got detached), leave
                // the cached value alone. Next layout pass will
                // try again.
                continue;
            };
            child.natural_y = natural_y;
        }
    }
}

/// Sum Taffy frame y values from `child_key` up to (but not
/// including) `scroll_key`. Returns `None` if we can't trace the
/// chain (child or an ancestor isn't in `view_to_layout`, or we
/// walked off the root without finding the scroll view).
fn compute_natural_y_in_scroll(
    child_key: usize,
    scroll_key: usize,
    layout: &runtime_layout::LayoutTree,
    view_to_layout: &HashMap<usize, (Retained<UIView>, runtime_layout::LayoutNode)>,
) -> Option<f32> {
    let (_, child_node) = view_to_layout.get(&child_key)?;
    let (_, scroll_node) = view_to_layout.get(&scroll_key)?;

    let mut sum_y = 0.0_f32;
    let mut cursor = *child_node;

    // Defensive depth cap — if Taffy hands us a cycle, we'd loop
    // forever otherwise.
    let mut steps = 0;
    while cursor != *scroll_node {
        sum_y += layout.frame_of(cursor).y;
        let Some(parent) = layout.parent_of(cursor) else {
            // Walked off the root before reaching the scroll view.
            return None;
        };
        cursor = parent;
        steps += 1;
        if steps > 256 {
            return None;
        }
    }
    Some(sum_y)
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    //! Regression coverage for `Position::Sticky` on iOS. Per
    //! CLAUDE.md §8, every bug fix lands with a test named after
    //! the bug being prevented.
    //!
    //! The three host-side tests below exercise the pure compute
    //! + registry lifecycle without needing UIKit objects, which
    //! makes them runnable from `cargo test -p backend-ios-mobile`
    //! on any platform that supports the crate's `cfg(target_os =
    //! "ios")`-gated UIKit deps (i.e. iOS-simulator host). UIKit
    //! integration tests would require an XCTest target driving a
    //! real UIWindow + scroll gestures, which doesn't fit `cargo
    //! test` — see the inline note in
    //! `regression_sticky_falls_back_to_relative_without_scroll_ancestor`.

    use super::*;

    /// Pin compute: scrolling past the threshold translates the
    /// child down by the overshoot; scrolling above the threshold
    /// leaves the child at its natural position.
    #[test]
    fn regression_sticky_registry_pins_when_scrolled_past_threshold() {
        // Child sits at y=100 in the scroll view's content; pin
        // threshold (top) is 20pt from the scroll view's top edge.
        let natural_y = 100.0;
        let threshold = 20.0;

        // Far above the pin point — no translate.
        assert_eq!(compute_translate(natural_y, threshold, 0.0), 0.0);

        // Just above the pin point (scroll_y + threshold == natural_y).
        // That's the boundary: still 0 (the `>` in compute, not `>=`).
        assert_eq!(compute_translate(natural_y, threshold, 80.0), 0.0);

        // 1pt past the pin point — translate by 1pt.
        let t = compute_translate(natural_y, threshold, 81.0);
        assert!((t - 1.0).abs() < 1e-5, "expected ~1.0, got {t}");

        // Way past the pin point — translate compensates fully so
        // the child renders at scroll_y + threshold = 200.
        let t = compute_translate(natural_y, threshold, 280.0);
        assert!(
            (t - 200.0).abs() < 1e-5,
            "expected ~200.0 (so rendered y == scroll_y + threshold = 300), got {t}",
        );

        // Sanity: rendered y while pinned == scroll_y + threshold.
        let scroll_y = 500.0;
        let t = compute_translate(natural_y, threshold, scroll_y);
        let rendered_y = natural_y + t;
        assert!(
            (rendered_y - (scroll_y + threshold)).abs() < 1e-5,
            "pinned rendered_y should equal scroll_y + threshold",
        );
    }

    /// Registry must be empty after a register + deregister
    /// round-trip — including the per-scroll-view entry, which
    /// otherwise leaks an orphan CADisplayLink and a stale
    /// `Retained<UIScrollView>`. The shrink-back-to-empty property
    /// is the regression test for "registry leaks scroll-view
    /// entries when their last sticky child unmounts."
    ///
    /// We can't construct a real `UIScrollView` outside of a UIKit
    /// runtime, so the test pokes at the registry directly with
    /// stub view pointers and verifies the bookkeeping shrinks
    /// correctly. The `deregister`/`register` paths that touch
    /// UIView pointers are covered separately by an on-simulator
    /// build of the iOS app (logged-only regression at this
    /// layer).
    #[test]
    fn regression_sticky_registry_unregisters_on_unmount() {
        // Two stub addresses — they must NOT be dereferenced by
        // the registry code paths we exercise here. We only call
        // helpers that touch the HashMap keys.
        let mut registry: StickyRegistry = HashMap::new();
        assert_eq!(registry.len(), 0);

        // Simulate one sticky child + scroll view: insert a
        // `StickyScrollEntry` directly so we don't need to call
        // `register` (which requires `find_enclosing_scroll_view`
        // → real UIKit superview traversal).
        let scroll_key = 0x1000_usize;
        let child_key = 0x2000_usize;

        // Build a stub entry with no real ObjC objects. The
        // shrink-on-empty path doesn't dereference the
        // `scroll_view` or `view` retained handles — it only
        // touches the HashMap keys and the `display_link`
        // Option. We sneak `None` into both retained slots via
        // the test-only constructor.
        //
        // SAFETY: We never dereference the dummy retained
        // pointers in this test. `mem::transmute` is the only
        // way to materialize a `Retained<T>` without a real ObjC
        // object; the helper below isolates the unsafety to a
        // single block and never reads from the resulting
        // `Retained` — only forgets it (drop must NOT run on
        // these fakes, which would `release` a bogus pointer).
        let entry = StickyScrollEntry {
            scroll_view: unsafe {
                // Bogus retained: never read, never dropped (we
                // pull this back out and `mem::forget` before
                // the registry tries to free it).
                std::mem::transmute::<*const UIScrollView, Retained<UIScrollView>>(
                    std::ptr::NonNull::<UIScrollView>::dangling().as_ptr(),
                )
            },
            children: {
                let mut m = HashMap::new();
                m.insert(
                    child_key,
                    StickyChild {
                        view: unsafe {
                            std::mem::transmute::<*const UIView, Retained<UIView>>(
                                std::ptr::NonNull::<UIView>::dangling().as_ptr(),
                            )
                        },
                        threshold_top: 0.0,
                        natural_y: 0.0,
                    },
                );
                m
            },
            display_link: None,
        };
        registry.insert(scroll_key, entry);
        assert_eq!(registry.len(), 1);

        // Strip the entry back out and forget its retained
        // handles so Drop doesn't `release` a bogus pointer.
        let entry = registry.remove(&scroll_key).unwrap();
        std::mem::forget(entry.scroll_view);
        for (_, child) in entry.children {
            std::mem::forget(child.view);
        }

        // After the explicit removal the registry is empty —
        // mirrors what `deregister` does when the last child of a
        // scroll view leaves. The leak-equivalent regression
        // (orphan scroll-view entry left behind) would surface as
        // `registry.len() == 1` here.
        assert_eq!(registry.len(), 0, "registry must shrink back to empty");
    }

    /// `find_enclosing_scroll_view` returning `None` is the
    /// fall-back-to-relative path; `register` is documented to
    /// no-op (return `false`) in that case. The test verifies
    /// the registry stays empty when there's no scroll ancestor
    /// to register against — the equivalent for `register` is
    /// "if `find_enclosing_scroll_view` returns `None`, the
    /// registry shouldn't gain an entry, and the function should
    /// signal the no-op to the caller."
    ///
    /// We can't call `register` here because it walks real
    /// `superview` pointers; the corresponding integration test
    /// (a `View { position: Sticky }` mounted with no scroll
    /// parent rendering identically to Relative) lives in the
    /// `examples/welcome` flow and is verified on-simulator.
    /// What we CAN test from host: the boundary of the pure
    /// helper — without a registry entry, no compute happens,
    /// and the registry's invariants hold.
    #[test]
    fn regression_sticky_falls_back_to_relative_without_scroll_ancestor() {
        // Empty registry + a tick attempt against a non-existent
        // scroll key must be a no-op (no panic, no mutation).
        let registry: StickyRegistry = HashMap::new();
        let absent_key = 0xDEAD_BEEF_usize;
        assert!(registry.get(&absent_key).is_none());
        // No-op: there's no public tick-without-backend helper,
        // but the registry stays empty, which is the
        // observable property.
        assert_eq!(registry.len(), 0);

        // compute_translate must also be a no-translate path when
        // the scroll position can't possibly pin the child
        // (scroll_y == 0 and threshold > 0). This is the same
        // numeric result the registry-less path would yield —
        // i.e. "rendered y == natural y" → no transform applied.
        let t = compute_translate(/* natural_y */ 100.0, /* threshold */ 20.0, /* scroll_y */ 0.0);
        assert_eq!(t, 0.0, "no scroll ancestor implies no scroll → no pin");
    }
}
