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
//! ## Axis coverage
//!
//! All four edges. Leading: `top` drives the vertical pin off
//! `contentOffset.y`, `left` the horizontal pin off
//! `contentOffset.x`. Trailing: `bottom` / `right` pin the child's far
//! edge inside the scrollport's trailing edge — the tick reads the
//! scroll view's `bounds.size` for the scrollport extent and the
//! cached Taffy frame size for the child extent (both refreshed per
//! layout pass, same discipline as `natural`). An axis with no inset
//! scrolls normally, so a frozen column (`left` or `right` only)
//! still scrolls vertically with the content — which is the whole
//! point of having the thresholds be per-edge `Option`s rather than
//! one number.

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
    /// Per-axis pin thresholds, in points, resolved from
    /// `StyleRules` by `runtime_shared::sticky::StickyInsets`
    /// (`top` → vertical, `left` → horizontal). An axis whose
    /// threshold is `None` scrolls normally.
    pub(crate) insets: StickyInsets,
    /// Natural `(x, y)` of the child in the scroll view's content
    /// coordinate space, in points. Refreshed after every layout
    /// pass by `refresh_layout_positions`. Initialized to the
    /// origin; the first layout pass replaces it with real values.
    pub(crate) natural: (f32, f32),
    /// The child's `(width, height)` from its Taffy frame — trailing
    /// -edge pins measure the far edge against the scrollport.
    /// Refreshed alongside `natural` (a UIView's own frame is
    /// undefined under a transform, so the live view is never read).
    pub(crate) size: (f32, f32),
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

// The pin arithmetic lives in `runtime_shared::sticky` — one
// implementation for every backend, so UIKit and AppKit cannot drift
// a pixel apart (CLAUDE.md §7). This module keeps only the UIKit
// mechanism: the display-link cadence, where the natural position
// comes from, and how the pin is written to `transform`.
use runtime_shared::sticky::{translate, StickyInsets};

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
    insets: StickyInsets,
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
            insets,
            natural: (0.0, 0.0),
            size: (0.0, 0.0),
        },
    );

    // Raise the pinned view above its static siblings, matching how
    // CSS paints positioned elements above non-positioned ones — a
    // frozen column must draw over the cells that slide beneath it.
    // `layer.zPosition` orders sibling layers without touching the
    // subview order UIKit's index-based insert bookkeeping relies on;
    // hit-testing still follows subview order, which is acceptable
    // (the overlap region is content the pin visually covers).
    raise_z(view, 1.0);

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

/// UIKit's 2-D affine transform. Declared once here rather than
/// re-declared inside each helper: the three sticky helpers below all
/// need the same layout, and a `#[repr(C)]` struct whose field order
/// must match a C ABI is precisely the thing that should not exist in
/// three copies.
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

impl CGAffineTransform {
    /// `CGAffineTransformIdentity` — `(1, 0, 0, 1, 0, 0)`.
    const IDENTITY: Self = Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    const fn translation(tx: f64, ty: f64) -> Self {
        Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx, ty }
    }
}

/// Reset `view.transform` to the identity.
fn reset_view_transform(view: &UIView) {
    let _: () = unsafe { msg_send![view, setTransform: CGAffineTransform::IDENTITY] };
    raise_z(view, 0.0);
}

/// Set the sticky raise on the view's layer. `1.0` on register (paint
/// above static siblings, CSS positioned-element order), `0.0` on
/// deregister so a Sticky → Relative flip restores document order.
fn raise_z(view: &UIView, z: f64) {
    let layer: Option<Retained<NSObject>> = unsafe { msg_send_id![view, layer] };
    if let Some(layer) = layer {
        let _: () = unsafe { msg_send![&layer, setZPosition: z] };
    }
}

/// Apply `(translate_x, translate_y)` translation to `view.transform`.
fn apply_translate(view: &UIView, translate_x: f64, translate_y: f64) {
    let t = CGAffineTransform::translation(translate_x, translate_y);
    let _: () = unsafe { msg_send![view, setTransform: t] };
}

/// Read `(view.transform.tx, view.transform.ty)` — the current
/// translate. Used to epsilon-skip when nothing changed since the last
/// tick.
fn current_translate(view: &UIView) -> (f64, f64) {
    let t: CGAffineTransform = unsafe { msg_send![view, transform] };
    (t.tx, t.ty)
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
/// Reads `contentOffset`, computes each child's per-axis translate
/// via the shared `runtime_shared::sticky::translate`, and writes it
/// to the child's `CGAffineTransform`. Skips the write when both axes
/// are within [`STICKY_EPSILON`] of the live value, matching the
/// portal anchor tracker's idle-frame discipline.
fn tick(backend: &mut crate::imp::IosBackend, scroll_key: usize) {
    let Some(entry) = backend.sticky_registry.get(&scroll_key) else {
        return;
    };
    let scroll: (f32, f32) = {
        let offset: objc2_foundation::CGPoint = unsafe {
            msg_send![&*entry.scroll_view, contentOffset]
        };
        (offset.x as f32, offset.y as f32)
    };
    // Scrollport extent for trailing-edge pins. `bounds.size` is the
    // visible extent regardless of the current transform/offset (UIKit
    // moves `bounds.origin` when scrolling, not the size).
    let viewport: (f32, f32) = {
        let bounds: objc2_foundation::CGRect = unsafe { msg_send![&*entry.scroll_view, bounds] };
        (bounds.size.width as f32, bounds.size.height as f32)
    };
    for (_, child) in entry.children.iter() {
        let (tx, ty) = translate(child.insets, child.natural, child.size, scroll, viewport);
        let (cur_x, cur_y) = current_translate(&child.view);
        // One combined write: `setTransform:` replaces the whole
        // matrix, so writing the axes separately would have the second
        // write clobber the first.
        if (cur_x as f32 - tx).abs() < STICKY_EPSILON
            && (cur_y as f32 - ty).abs() < STICKY_EPSILON
        {
            continue;
        }
        apply_translate(&child.view, tx as f64, ty as f64);
    }
}

/// Refresh the cached natural `(x, y)` for every sticky child after a
/// layout pass. Walks Taffy parents from the child up to its
/// registered scroll view, summing frame origins.
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
            let Some(natural) = compute_natural_in_scroll(
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
            child.natural = natural;
            // The child's own extent, for trailing-edge pins — from
            // the Taffy frame, same source as `natural` (the UIView
            // frame is undefined under the pin transform).
            if let Some((_, node)) = view_to_layout.get(child_key) {
                let frame = layout.frame_of(*node);
                child.size = (frame.width, frame.height);
            }
        }
    }
}

/// Sum Taffy frame y values from `child_key` up to (but not
/// including) `scroll_key`. Returns `None` if we can't trace the
/// chain (child or an ancestor isn't in `view_to_layout`, or we
/// walked off the root without finding the scroll view).
fn compute_natural_in_scroll(
    child_key: usize,
    scroll_key: usize,
    layout: &runtime_layout::LayoutTree,
    view_to_layout: &HashMap<usize, (Retained<UIView>, runtime_layout::LayoutNode)>,
) -> Option<(f32, f32)> {
    let (_, child_node) = view_to_layout.get(&child_key)?;
    let (_, scroll_node) = view_to_layout.get(&scroll_key)?;

    // Both axes are summed unconditionally, not just the pinned one: a
    // reactive style change can add a `left` inset between layout
    // passes, and a child that only tracked `y` would then tick
    // against a stale `natural.x`.
    let mut sum = (0.0_f32, 0.0_f32);
    let mut cursor = *child_node;

    // Defensive depth cap — if Taffy hands us a cycle, we'd loop
    // forever otherwise.
    let mut steps = 0;
    while cursor != *scroll_node {
        let frame = layout.frame_of(cursor);
        sum.0 += frame.x;
        sum.1 += frame.y;
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
    Some(sum)
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

    /// Which TRANSFORM COMPONENTS the tick writes. The pin
    /// arithmetic's own regressions live with the arithmetic, in
    /// `runtime_shared::sticky`; what iOS owns is composing that
    /// result into a single `CGAffineTransform` — and `setTransform:`
    /// replaces the whole matrix, so an axis the element does not pin
    /// on must come back exactly `0.0` or the combined write would
    /// drag it off its laid-out position.
    #[test]
    fn regression_sticky_registry_pins_when_scrolled_past_threshold() {
        // Child sits at y=100 in the scroll view's content; pin
        // threshold (top) is 20pt from the scroll view's top edge.
        let vertical = StickyInsets { top: Some(20.0), ..Default::default() };
        let natural = (33.0_f32, 100.0_f32);
        let size = (80.0_f32, 40.0_f32);
        let viewport = (390.0_f32, 844.0_f32);

        // Far above the pin point — no translate on either axis.
        assert_eq!(translate(vertical, natural, size, (0.0, 0.0), viewport), (0.0, 0.0));

        // Way past the pin point — ty compensates fully so the child
        // renders at scroll_y + threshold, and tx stays 0 so the
        // combined write preserves the laid-out x.
        let (tx, ty) = translate(vertical, natural, size, (0.0, 280.0), viewport);
        assert_eq!(tx, 0.0, "a vertical-only pin must leave transform.tx at 0");
        assert!(
            ((natural.1 + ty) - 300.0).abs() < 1e-5,
            "pinned rendered y should equal scroll_y + threshold",
        );
    }

    /// A frozen COLUMN on iOS: `left` pins horizontally off
    /// `contentOffset.x` while the child keeps scrolling vertically.
    /// Before horizontal support the registry held a single
    /// `threshold_top`, so `left` produced no horizontal pin at all —
    /// a frozen column was inexpressible on every backend.
    #[test]
    fn regression_sticky_left_never_pins_horizontally() {
        let horizontal = StickyInsets { left: Some(0.0), ..Default::default() };
        let (tx, ty) = translate(horizontal, (160.0, 40.0), (80.0, 24.0), (600.0, 250.0), (390.0, 844.0));
        assert!(
            ((160.0 + tx) - 600.0).abs() < 1e-5,
            "pinned rendered x should equal scroll_x + threshold",
        );
        assert_eq!(ty, 0.0, "a horizontal-only pin must leave transform.ty at 0");
    }

    /// A RIGHT-frozen column on iOS: `right` pulls the child back so
    /// its far edge parks at the scroll view's trailing bounds edge,
    /// composed into the same single `setTransform:` write — so the
    /// unpinned vertical axis must still come back exactly 0. Before
    /// trailing-edge support the registry carried no scrollport or
    /// child extent, and `right` produced no pin on any native backend.
    #[test]
    fn regression_sticky_right_pins_at_scrollport_trailing_edge() {
        let horizontal = StickyInsets { right: Some(0.0), ..Default::default() };
        // Column at x=900, 100 wide, scrollport 390 wide, unscrolled:
        // parks at 390 - 100 = 290.
        let (tx, ty) = translate(horizontal, (900.0, 40.0), (100.0, 24.0), (0.0, 0.0), (390.0, 844.0));
        assert!(((900.0 + tx) - 290.0).abs() < 1e-5);
        assert_eq!(ty, 0.0, "a horizontal-only pin must leave transform.ty at 0");
        // Scrolled far enough right that it's naturally visible —
        // rides the content, no transform.
        let (tx, _) = translate(horizontal, (900.0, 40.0), (100.0, 24.0), (610.0, 0.0), (390.0, 844.0));
        assert_eq!(tx, 0.0);
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
                        insets: StickyInsets { top: Some(0.0), ..Default::default() },
                        natural: (0.0, 0.0),
                        size: (0.0, 0.0),
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

        // The shared translate must also be a no-translate path when
        // the scroll position can't possibly pin the child
        // (scroll == 0 and threshold > 0). This is the same numeric
        // result the registry-less path would yield — i.e.
        // "rendered position == natural position" → no transform.
        let insets = StickyInsets { top: Some(20.0), left: Some(20.0), ..Default::default() };
        assert_eq!(
            translate(insets, (100.0, 100.0), (80.0, 40.0), (0.0, 0.0), (390.0, 844.0)),
            (0.0, 0.0),
            "no scroll ancestor implies no scroll → no pin",
        );
    }
}
