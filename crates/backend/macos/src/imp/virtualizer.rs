//! `Element::Virtualizer` — `NSCollectionView` with a flow layout
//! that drives real cell reuse. Mirrors the iOS UICollectionView
//! implementation in `crates/backend/ios/mobile/src/imp/virtualizer.rs`
//! with the AppKit-equivalent selectors.
//!
//! ## Architecture
//!
//! - Outer `NSScrollView` wraps an `NSCollectionView` configured with
//!   an `NSCollectionViewFlowLayout`. The framework's logical
//!   ScrollView (already an NSScrollView via `create_scroll_view`)
//!   is a sibling — virtualizer manages its own scroll container.
//! - A custom `NSObject` subclass [`VirtualizerDataSource`]
//!   implements `NSCollectionViewDataSource` + `NSCollectionView`-
//!   `DelegateFlowLayout` and routes every lifecycle event back to
//!   the `VirtualizerCallbacks` the framework handed us.
//! - A custom `NSCollectionViewItem` subclass [`VirtualizerItem`]
//!   hosts a single framework view as its `view`'s subview. On
//!   reuse / display-end we release the per-item Scope.
//!
//! ## Item-size strategy (matches iOS Phase-1)
//!
//! Supports `ItemSize::Known` — `callbacks.item_size(idx)` gives the
//! main-axis size; the cross-axis is the collection view's bounds
//! minus content insets. `ItemSize::Measured` is parked on the same
//! framework-core gap iOS hit: items live outside Taffy's tree, so
//! the framework's measure pass doesn't reach into the hosted
//! subtree. See iOS virtualizer.rs's docstring for the full reasoning.
//!
//! ## Why not just `NSTableView`?
//!
//! NSTableView has older API ergonomics + assumes row-based layout.
//! NSCollectionView is the modern AppKit equivalent of
//! UICollectionView and shares the bulk of the protocol surface,
//! making the iOS port straightforward.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol};
use objc2::{declare_class, mutability, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSCollectionViewFlowLayout, NSCollectionViewItem, NSScrollView, NSView,
};
use objc2_foundation::{
    CGFloat, CGRect, CGSize, MainThreadMarker, NSInteger, NSObject, NSString,
};
use runtime_shared::{VirtualizerCallbacks, VirtualLayout};

use super::MacosNode;

// =========================================================================
// Per-item mount state
// =========================================================================

#[derive(Clone)]
struct ItemMount {
    scope_id: u64,
    /// The framework-produced view we attached as a subview of
    /// the NSCollectionViewItem's `view`. Retained here so we can
    /// removeFromSuperview on reuse / teardown.
    child: Retained<NSView>,
}

/// Unmount hygiene: clear the host container's `hosted_child_key` so a
/// later `setFrameSize:` doesn't relayout a subtree that's being torn
/// down. Best-effort — a stale key is already benign (the layout pass
/// no-ops on unknown keys), this just avoids the wasted call.
fn clear_host_key(child: &NSView) {
    let superview: Option<Retained<NSView>> = unsafe { child.superview() };
    if let Some(sv) = superview {
        if let Some(host) = VirtualizerItemView::from_container(&sv) {
            host.ivars().hosted_child_key.set(0);
        }
    }
}

// =========================================================================
// VirtualizerDataSource — NSObject subclass.
// =========================================================================

pub(crate) struct VirtualizerDataSourceIvars {
    /// Framework-supplied callbacks. Wrapped in
    /// `Rc<RefCell<Option<_>>>` so `release` can drop them
    /// deterministically before the data source's GC turn — any
    /// AppKit event that fires after release sees `None` and
    /// bails cleanly.
    callbacks: Rc<RefCell<Option<VirtualizerCallbacks<MacosNode>>>>,
    /// Per-item-instance mount tracking. Keyed by the
    /// `NSCollectionViewItem`'s pointer; on reuse the same item
    /// gets handed back with a different index, so we use the
    /// pointer to detect "this item already hosts a mount we need
    /// to release first."
    mounts: Rc<RefCell<HashMap<usize, ItemMount>>>,
    /// Flips to `false` on `release()`. Every protocol entry
    /// checks this and short-circuits — guards against AppKit
    /// events queued past teardown.
    alive: Rc<RefCell<bool>>,
    /// Scroll axis + lane subdivision + gaps. `sizeForItemAt` reads
    /// this to size each cell: a list (one lane) fills the cross axis,
    /// a grid divides it into `lanes` columns. `AutoFit` resolves
    /// against the live collection-view bounds, so a window resize
    /// re-lanes the grid.
    layout: VirtualLayout,
}

declare_class!(
    pub(crate) struct VirtualizerDataSource;

    unsafe impl ClassType for VirtualizerDataSource {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMacVirtualizerDataSource";
    }

    impl DeclaredClass for VirtualizerDataSource {
        type Ivars = VirtualizerDataSourceIvars;
    }

    unsafe impl NSObjectProtocol for VirtualizerDataSource {}

    // ---- NSCollectionViewDataSource ----
    unsafe impl VirtualizerDataSource {
        #[method(numberOfSectionsInCollectionView:)]
        fn number_of_sections(&self, _cv: &NSObject) -> NSInteger {
            1
        }

        #[method(collectionView:numberOfItemsInSection:)]
        fn number_of_items(&self, _cv: &NSObject, _section: NSInteger) -> NSInteger {
            if !*self.ivars().alive.borrow() {
                return 0;
            }
            let cb_opt = self.ivars().callbacks.borrow();
            cb_opt
                .as_ref()
                .map(|c| (c.item_count)() as NSInteger)
                .unwrap_or(0)
        }

        #[method_id(collectionView:itemForRepresentedObjectAtIndexPath:)]
        fn item_for_index_path(
            &self,
            cv: &NSObject,
            index_path: &NSObject,
        ) -> Retained<NSObject> {
            // Dequeue + mount lives in a non-`declare_class!` helper so
            // the body can use early returns + question marks freely
            // (the macro's IdReturnValue conversion is finicky about
            // early returns in `Retained<_>` bodies).
            self.item_for_index_path_impl(cv, index_path)
        }
    }

    // ---- NSCollectionViewDelegate / DelegateFlowLayout ----
    unsafe impl VirtualizerDataSource {
        #[method(collectionView:didEndDisplayingItem:forRepresentedObjectAtIndexPath:)]
        fn did_end_displaying(
            &self,
            _cv: &NSObject,
            item: &NSObject,
            _index_path: &NSObject,
        ) {
            if !*self.ivars().alive.borrow() {
                return;
            }
            let item_ptr = item as *const NSObject as usize;
            let previous = self.ivars().mounts.borrow_mut().remove(&item_ptr);
            if let Some(prev) = previous {
                let release_fn = {
                    let cb_opt = self.ivars().callbacks.borrow();
                    cb_opt.as_ref().map(|c| c.release_item.clone())
                };
                clear_host_key(&prev.child);
                unsafe { prev.child.removeFromSuperview() };
                if let Some(release) = release_fn {
                    (release)(prev.scope_id);
                }
            }
        }

        #[method(collectionView:layout:sizeForItemAtIndexPath:)]
        fn size_for_item_at(
            &self,
            cv: &NSObject,
            _layout: &NSObject,
            index_path: &NSObject,
        ) -> CGSize {
            if !*self.ivars().alive.borrow() {
                return CGSize::new(0.0, 0.0);
            }
            let cb_opt = self.ivars().callbacks.borrow();
            let Some(cb) = cb_opt.as_ref() else {
                return CGSize::new(0.0, 0.0);
            };
            let item: NSInteger = unsafe { msg_send![index_path, item] };
            let idx = if item < 0 { 0usize } else { item as usize };
            let size_f = (cb.item_size)(idx) as CGFloat;

            let bounds: CGRect = unsafe { msg_send![cv, bounds] };
            let layout = self.ivars().layout;
            // Cross-axis extent divided into `lanes` columns minus
            // inter-lane gaps. One lane = list (cell fills cross axis).
            let usable_cross = if layout.axis.is_horizontal() {
                bounds.size.height.max(0.0)
            } else {
                bounds.size.width.max(0.0)
            };
            let lanes = layout.lanes.resolve(usable_cross as f32, layout.cross_spacing) as CGFloat;
            let lane_cross = if lanes >= 1.0 {
                ((usable_cross - (lanes - 1.0) * layout.cross_spacing as CGFloat) / lanes).max(0.0)
            } else {
                usable_cross
            };
            if layout.axis.is_horizontal() {
                CGSize::new(size_f, lane_cross)
            } else {
                CGSize::new(lane_cross, size_f)
            }
        }
    }
);

impl VirtualizerDataSource {
    fn item_for_index_path_impl(
        &self,
        cv: &NSObject,
        index_path: &NSObject,
    ) -> Retained<NSObject> {
        let identifier = item_reuse_identifier();
        // `makeItemWithIdentifier:forIndexPath:` is the
        // NSCollectionView equivalent of UIKit's
        // `dequeueReusableCellWithReuseIdentifier:forIndexPath:`.
        // Returns an NSCollectionViewItem*. Reuses an existing item
        // from the pool or constructs a fresh one (via the class
        // we registered with `registerClass:forItemWithIdentifier:`).
        let item: Retained<NSObject> = unsafe {
            msg_send_id![
                cv,
                makeItemWithIdentifier: &*identifier,
                forIndexPath: index_path
            ]
        };

        if !*self.ivars().alive.borrow() {
            return item;
        }

        let row_i: NSInteger = unsafe { msg_send![index_path, item] };
        let idx = if row_i < 0 { 0usize } else { row_i as usize };

        let item_ptr = &*item as *const NSObject as usize;
        let previous = self.ivars().mounts.borrow_mut().remove(&item_ptr);
        if let Some(prev) = previous {
            let release_fn = {
                let cb_opt = self.ivars().callbacks.borrow();
                cb_opt.as_ref().map(|c| c.release_item.clone())
            };
            clear_host_key(&prev.child);
            unsafe { prev.child.removeFromSuperview() };
            if let Some(release) = release_fn {
                (release)(prev.scope_id);
            }
        }

        let mount_fn = {
            let cb_opt = self.ivars().callbacks.borrow();
            cb_opt.as_ref().map(|c| c.mount_item.clone())
        };
        let Some(mount) = mount_fn else {
            return item;
        };
        let (node, scope_id) = (mount)(idx);
        let child_view = node.as_view();

        // NSCollectionViewItem's `view` is the item's containing
        // NSView. We add the framework-produced child as a
        // subview filling that container. Autoresizing mask flags:
        // width + height flexible (= 18 in NSAutoresizingMask bit
        // values — same numeric semantics as UIView's
        // UIViewAutoresizing).
        let container: Retained<NSView> = unsafe { msg_send_id![&item, view] };
        let bounds: CGRect = unsafe { msg_send![&container, bounds] };
        let _: () = unsafe { msg_send![child_view, setFrame: bounds] };
        let _: () = unsafe { msg_send![child_view, setAutoresizingMask: 18u64] };
        unsafe { container.addSubview(child_view) };

        // Hand the host view the subtree's layout key and lay the
        // subtree out right away if the cell already has its size
        // (the reuse path: same-sized cell gets a new mount, so no
        // `setFrameSize:` will fire). Fresh items are still 0×0 here
        // — their first `setFrameSize:` (when the flow layout applies
        // the item's attributes) runs the pass instead.
        let child_key = child_view as *const NSView as usize;
        if let Some(host) = VirtualizerItemView::from_container(&container) {
            host.ivars().hosted_child_key.set(child_key);
        }
        if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
            super::with_global_backend(|b| {
                b.layout_detached_root(child_key, bounds.size)
            });
        }

        let child_retained: Retained<NSView> = unsafe {
            Retained::retain(child_view as *const NSView as *mut NSView)
                .expect("retain mounted child NSView")
        };
        self.ivars().mounts.borrow_mut().insert(
            item_ptr,
            ItemMount { scope_id, child: child_retained },
        );

        item
    }

    fn new(
        mtm: MainThreadMarker,
        callbacks: VirtualizerCallbacks<MacosNode>,
        layout: VirtualLayout,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(VirtualizerDataSourceIvars {
            callbacks: Rc::new(RefCell::new(Some(callbacks))),
            mounts: Rc::new(RefCell::new(HashMap::new())),
            alive: Rc::new(RefCell::new(true)),
            layout,
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Tear down — flip `alive`, drain mounts, drop callbacks. After
    /// this returns the data source holds no framework references;
    /// safe to drop on the next GC turn.
    fn shutdown(&self) {
        *self.ivars().alive.borrow_mut() = false;
        let release_fn = {
            let cb_opt = self.ivars().callbacks.borrow();
            cb_opt.as_ref().map(|c| c.release_item.clone())
        };
        let mounts = std::mem::take(&mut *self.ivars().mounts.borrow_mut());
        for (_, mount) in mounts.into_iter() {
            clear_host_key(&mount.child);
            unsafe { mount.child.removeFromSuperview() };
            if let Some(ref release) = release_fn {
                (release)(mount.scope_id);
            }
        }
        *self.ivars().callbacks.borrow_mut() = None;
    }
}

// =========================================================================
// VirtualizerItem — a REAL `NSCollectionViewItem` subclass.
//
// It must be a genuine NSCollectionViewItem: `makeItemWithIdentifier:`
// instantiates the registered class and immediately sends it
// NSCollectionViewItem/NSViewController selectors (`view`, highlight
// state, …). A prior `Super = NSObject` version threw
// `unrecognized selector` inside the data-source callback — an ObjC
// exception crossing the non-unwinding `#[method]` boundary, i.e. a
// hard abort ("panic in a function that cannot unwind") the moment a
// `flat_list` rendered. iOS hit the identical bug and its fix is
// documented in `ios/mobile/src/imp/virtualizer.rs`; iOS could switch
// to the stock `UICollectionViewCell`, but AppKit has no nib-less
// stock item — `NSViewController.loadView` looks for a nib named
// after the class and throws when none exists. So we subclass and
// override `loadView` to install a plain code-created `NSView`.
// =========================================================================

declare_class!(
    pub(crate) struct VirtualizerItem;

    unsafe impl ClassType for VirtualizerItem {
        type Super = NSCollectionViewItem;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMacVirtualizerItem";
    }

    impl DeclaredClass for VirtualizerItem {}

    unsafe impl VirtualizerItem {
        // NSViewController's default `loadView` requires a nib; these
        // items are code-only containers, so hand it our host view.
        // The mounted framework child is added as a subview of this
        // view by `item_for_index_path_impl`.
        #[method(loadView)]
        fn load_view(&self) {
            let mtm = MainThreadMarker::from(self);
            let view = VirtualizerItemView::new(mtm);
            let _: () = unsafe { msg_send![self, setView: &*view] };
        }
    }
);

// =========================================================================
// VirtualizerItemView — the item's container NSView. Its job is to keep
// the hosted framework subtree LAID OUT: mounted item subtrees are
// Taffy orphans (built via `mount_item` outside the host root, inserted
// into the cell purely on the AppKit side), so the main
// `compute_and_apply_layout` pass — scoped to nodes reachable from
// `root_layout` — never frames them. Without this hook every view
// inside a cell stays 0×0: the list scrolled and recycled correctly
// but painted nothing. The cell's size arrives from
// NSCollectionViewFlowLayout as a `setFrameSize:` on this view, and we
// relayout the hosted subtree against it via `layout_detached_root`
// (the same subtree-scoped pass private-layer windows use).
// =========================================================================

pub(crate) struct VirtualizerItemViewIvars {
    /// `view_to_layout` key (view pointer) of the hosted framework
    /// subtree's root, or 0 while nothing is mounted. Set by
    /// `item_for_index_path_impl`, cleared on unmount. A stale key is
    /// benign — `layout_detached_root` no-ops when the key is gone.
    hosted_child_key: Cell<usize>,
}

declare_class!(
    pub(crate) struct VirtualizerItemView;

    unsafe impl ClassType for VirtualizerItemView {
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMacVirtualizerItemView";
    }

    impl DeclaredClass for VirtualizerItemView {
        type Ivars = VirtualizerItemViewIvars;
    }

    unsafe impl VirtualizerItemView {
        // Match the framework's FlippedView convention (top-left
        // origin) so the hosted subtree's Taffy frames land unflipped.
        #[method(isFlipped)]
        fn is_flipped(&self) -> bool {
            true
        }

        #[method(setFrameSize:)]
        fn set_frame_size(&self, size: CGSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            let key = self.ivars().hosted_child_key.get();
            if key != 0 && size.width > 0.0 && size.height > 0.0 {
                // `try_borrow_mut` inside — silently skipped if the
                // backend is mid-pass; the pass that resized us will
                // schedule/settle a consistent state on its next turn.
                super::with_global_backend(|b| b.layout_detached_root(key, size));
            }
        }
    }
);

// =========================================================================
// VirtualizerScrollView — the outer NSScrollView. Overrides `tile` to
// pin the collection view's CROSS axis to the clip view.
//
// NSClipView intentionally ignores its documentView's autoresizing
// mask (it owns document placement for scrolling), and NSCollectionView
// derives its content width FROM its own current bounds — so nothing
// re-narrows the CV when Taffy later shrinks the scroll view. The
// stale-wide items then get centered by the flow layout at a negative
// x origin (row content pushed off-view, list renders blank, AppKit
// logs "item width must be less than …"). `tile` runs on every scroll
// -view relayout; after super we clamp the document's cross axis to
// the clip's. Main axis stays content-driven (collectionViewContentSize)
// — pinning it would kill scrolling.
// =========================================================================

// =========================================================================
// VirtualizerFlowLayout — NSCollectionViewFlowLayout that re-queries
// item sizes when the CROSS axis changes.
//
// AppKit's flow layout (unlike UIKit's) does NOT invalidate itself on a
// bounds change, so after the framework's layout pass narrows the
// scroll view the layout keeps the item sizes it cached against the
// OLD width. Too-wide cached items get line-centered at a negative x
// origin (content off-view, list looks blank) and AppKit logs "the
// item width must be less than the width of the UICollectionView…"
// per relayout. Returning YES for cross-axis extent changes makes the
// layout re-run `sizeForItemAtIndexPath` against the new bounds —
// same-axis (scroll-position) bounds changes still return NO so
// scrolling doesn't thrash the layout.
// =========================================================================

declare_class!(
    pub(crate) struct VirtualizerFlowLayout;

    unsafe impl ClassType for VirtualizerFlowLayout {
        type Super = NSCollectionViewFlowLayout;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMacVirtualizerFlowLayout";
    }

    impl DeclaredClass for VirtualizerFlowLayout {}

    unsafe impl VirtualizerFlowLayout {
        #[method(shouldInvalidateLayoutForBoundsChange:)]
        fn should_invalidate_for_bounds_change(&self, new_bounds: CGRect) -> bool {
            let sup: bool = unsafe {
                msg_send![super(self), shouldInvalidateLayoutForBoundsChange: new_bounds]
            };
            // No early `return` — the macro only converts the tail
            // expression to the ObjC `Bool` return type.
            sup || self.cross_axis_changed(new_bounds)
        }

        // Saying "invalidate" is not enough: the flow layout CACHES the
        // delegate's per-item sizes, and a plain invalidation only
        // re-POSITIONS the cached sizes (that's how the stale-width
        // items kept re-centering at negative x through every resize).
        // The context's `invalidateFlowLayoutDelegateMetrics` is the
        // flag that flushes the size cache and re-runs
        // `sizeForItemAtIndexPath` — same contract as UIKit's
        // UICollectionViewFlowLayoutInvalidationContext.
        #[method_id(invalidationContextForBoundsChange:)]
        fn invalidation_context_for_bounds_change(
            &self,
            new_bounds: CGRect,
        ) -> Retained<NSObject> {
            let ctx: Retained<NSObject> = unsafe {
                msg_send_id![super(self), invalidationContextForBoundsChange: new_bounds]
            };
            if self.cross_axis_changed(new_bounds) {
                let _: () = unsafe {
                    msg_send![&ctx, setInvalidateFlowLayoutDelegateMetrics: true]
                };
            }
            ctx
        }
    }
);

impl VirtualizerFlowLayout {
    /// Did `new_bounds` change the CROSS-axis extent relative to the
    /// collection view's current bounds? Main-axis (scroll-position)
    /// changes return false so scrolling doesn't thrash the layout.
    fn cross_axis_changed(&self, new_bounds: CGRect) -> bool {
        let cv: *mut NSView = unsafe { msg_send![self, collectionView] };
        if cv.is_null() {
            return false;
        }
        let old: CGRect = unsafe { msg_send![cv, bounds] };
        // NSCollectionViewScrollDirection: 0 vertical, 1 horizontal.
        let dir: i64 = unsafe { msg_send![self, scrollDirection] };
        if dir == 1 {
            (old.size.height - new_bounds.size.height).abs() > 0.5
        } else {
            (old.size.width - new_bounds.size.width).abs() > 0.5
        }
    }
}

pub(crate) struct VirtualizerScrollViewIvars {
    /// Main-axis orientation, decides which axis gets pinned.
    horizontal: Cell<bool>,
}

declare_class!(
    pub(crate) struct VirtualizerScrollView;

    unsafe impl ClassType for VirtualizerScrollView {
        type Super = NSScrollView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMacVirtualizerScrollView";
    }

    impl DeclaredClass for VirtualizerScrollView {
        type Ivars = VirtualizerScrollViewIvars;
    }

    unsafe impl VirtualizerScrollView {
        #[method(tile)]
        fn tile(&self) {
            let _: () = unsafe { msg_send![super(self), tile] };
            let doc: *mut NSView = unsafe { msg_send![self, documentView] };
            if doc.is_null() {
                return;
            }
            let clip: Retained<NSView> = unsafe { msg_send_id![self, contentView] };
            let clip_bounds: CGRect = unsafe { msg_send![&clip, bounds] };
            let doc_frame: CGRect = unsafe { msg_send![doc, frame] };
            let horizontal = self.ivars().horizontal.get();
            let new_size = if horizontal {
                CGSize::new(doc_frame.size.width, clip_bounds.size.height)
            } else {
                CGSize::new(clip_bounds.size.width, doc_frame.size.height)
            };
            let differs = if horizontal {
                (doc_frame.size.height - new_size.height).abs() > 0.5
            } else {
                (doc_frame.size.width - new_size.width).abs() > 0.5
            };
            if differs && new_size.width > 0.0 && new_size.height >= 0.0 {
                let _: () = unsafe { msg_send![doc, setFrameSize: new_size] };
                // A direct `setFrameSize:` BYPASSES the layout's
                // bounds-change hooks (`shouldInvalidateLayoutForBounds
                // Change:` only fires for clip-driven visible-rect
                // changes), so invalidate explicitly — and with a
                // context that sets `invalidateFlowLayoutDelegateMetrics`,
                // because a bare `invalidateLayout` re-positions the
                // CACHED item sizes instead of re-querying
                // `sizeForItemAtIndexPath` at the new width.
                let layout: *mut AnyObject =
                    unsafe { msg_send![doc, collectionViewLayout] };
                if !layout.is_null() {
                    if let Some(ctx_cls) =
                        AnyClass::get("NSCollectionViewFlowLayoutInvalidationContext")
                    {
                        unsafe {
                            let ctx: *mut AnyObject = msg_send![ctx_cls, alloc];
                            let ctx: *mut AnyObject = msg_send![ctx, init];
                            let _: () = msg_send![
                                ctx,
                                setInvalidateFlowLayoutDelegateMetrics: true
                            ];
                            let _: () =
                                msg_send![layout, invalidateLayoutWithContext: ctx];
                            let _: () = msg_send![ctx, release];
                        }
                    }
                }
            }
        }
    }
);

impl VirtualizerScrollView {
    fn new(mtm: MainThreadMarker, horizontal: bool) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(VirtualizerScrollViewIvars {
            horizontal: Cell::new(horizontal),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

impl VirtualizerItemView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(VirtualizerItemViewIvars {
            hosted_child_key: Cell::new(0),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Downcast a container `NSView` back to this class. Items always
    /// host a `VirtualizerItemView` (installed by `loadView`), but be
    /// defensive: return `None` for anything else.
    fn from_container(container: &NSView) -> Option<&VirtualizerItemView> {
        if container.class() == Self::class() {
            let ptr = container as *const NSView as *const VirtualizerItemView;
            Some(unsafe { &*ptr })
        } else {
            None
        }
    }
}

// =========================================================================
// Public entry points.
// =========================================================================

pub(crate) struct VirtualizerInstance {
    pub(crate) data_source: Retained<VirtualizerDataSource>,
    #[allow(dead_code)]
    pub(crate) layout: Retained<NSObject>,
    #[allow(dead_code)]
    pub(crate) collection_view: Retained<NSView>,
}

/// Build an `NSScrollView`-wrapped `NSCollectionView` with a
/// `NSCollectionViewFlowLayout` and our custom data source.
/// Returns the outer scroll view as the mountable `NSView`.
pub(crate) fn create(
    mtm: MainThreadMarker,
    instances: &mut HashMap<usize, VirtualizerInstance>,
    callbacks: VirtualizerCallbacks<MacosNode>,
    _overscan: f32,
    virt_layout: VirtualLayout,
) -> Retained<NSView> {
    // ── Flow layout ────────────────────────────────────────────────
    // Our subclass — invalidates on cross-axis bounds changes so item
    // sizes re-resolve against the live width (see the type's doc).
    let layout_cls: &AnyClass = VirtualizerFlowLayout::class();
    let layout: Retained<NSObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![layout_cls, alloc];
        let inited: *mut AnyObject = msg_send![allocated, init];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("VirtualizerFlowLayout init returned nil")
    };
    // NSCollectionViewScrollDirection: 0 = vertical, 1 = horizontal.
    // Matches UIKit's UICollectionViewScrollDirection numeric values.
    let scroll_direction: i64 = if virt_layout.axis.is_horizontal() { 1 } else { 0 };
    let _: () = unsafe { msg_send![&layout, setScrollDirection: scroll_direction] };
    // Line spacing = gap between grid-rows (main axis); interitem
    // spacing = gap between lanes on a line (cross axis).
    let _: () = unsafe {
        msg_send![&layout, setMinimumLineSpacing: virt_layout.main_spacing as CGFloat]
    };
    let _: () = unsafe {
        msg_send![&layout, setMinimumInteritemSpacing: virt_layout.cross_spacing as CGFloat]
    };

    // ── Collection view ───────────────────────────────────────────
    let cv_cls: &AnyClass = AnyClass::get("NSCollectionView")
        .expect("NSCollectionView class not registered");
    let zero_rect = CGRect {
        origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: 0.0, height: 0.0 },
    };
    let cv: Retained<NSView> = unsafe {
        let allocated: *mut AnyObject = msg_send![cv_cls, alloc];
        let inited: *mut AnyObject = msg_send![allocated, initWithFrame: zero_rect];
        Retained::from_raw(inited.cast::<NSView>())
            .expect("NSCollectionView init returned nil")
    };
    let _: () = unsafe { msg_send![&cv, setCollectionViewLayout: &*layout] };

    // Register our item subclass against a stable reuse identifier.
    // NSCollectionView creates items lazily from this class as the
    // pool grows.
    let item_cls: &AnyClass = VirtualizerItem::class();
    let reuse_id = item_reuse_identifier();
    let _: () = unsafe {
        msg_send![
            &cv,
            registerClass: item_cls,
            forItemWithIdentifier: &*reuse_id
        ]
    };

    // Data source + delegate. NSCollectionView holds these weakly,
    // matching UIKit — we retain via the side-state `instances`
    // map until `release` fires.
    let data_source = VirtualizerDataSource::new(mtm, callbacks, virt_layout);
    let _: () = unsafe { msg_send![&cv, setDataSource: &*data_source] };
    let _: () = unsafe { msg_send![&cv, setDelegate: &*data_source] };

    // ── Scroll view ───────────────────────────────────────────────
    // NSCollectionView itself doesn't scroll — it relies on being
    // hosted in an NSScrollView whose documentView is the collection
    // view. Without this wrap, content overflowing the visible
    // bounds is clipped without any scroll affordance. Our subclass
    // additionally pins the CV's cross axis to the clip on `tile`
    // (see [`VirtualizerScrollView`]).
    let horizontal = virt_layout.axis.is_horizontal();
    let scroll_typed = VirtualizerScrollView::new(mtm, horizontal);
    let scroll: Retained<NSView> = unsafe {
        Retained::retain(&*scroll_typed as *const VirtualizerScrollView as *mut NSView)
            .expect("retain VirtualizerScrollView as NSView")
    };
    let _: () = unsafe {
        msg_send![&scroll, setHasVerticalScroller: !horizontal]
    };
    let _: () = unsafe {
        msg_send![&scroll, setHasHorizontalScroller: horizontal]
    };
    let _: () = unsafe { msg_send![&scroll, setAutohidesScrollers: true] };
    let _: () = unsafe { msg_send![&scroll, setDocumentView: &*cv] };

    let scroll_key = &*scroll as *const NSView as usize;
    instances.insert(
        scroll_key,
        VirtualizerInstance {
            data_source: data_source.clone(),
            layout,
            collection_view: cv,
        },
    );

    scroll
}

/// Tell the collection view to fully reload. Mirrors iOS's
/// `reloadData()` shape — every visible item gets the data-source
/// protocol re-run, and our `item_for_index_path_impl` re-mounts
/// fresh content. Future optimization: `performBatchUpdates` so
/// surviving items animate in place.
pub(crate) fn data_changed(scroll_view: &NSView) {
    // Walk: scroll_view → documentView → reloadData
    let doc_view: *mut NSView = unsafe { msg_send![scroll_view, documentView] };
    if doc_view.is_null() {
        return;
    }
    let _: () = unsafe { msg_send![doc_view, reloadData] };
}

/// Tear down — disconnect the data source so AppKit events stop,
/// drain mounted scopes, drop the instance. Matches iOS's pattern.
pub(crate) fn release(
    instances: &mut HashMap<usize, VirtualizerInstance>,
    scroll_view: &NSView,
) {
    let key = scroll_view as *const NSView as usize;
    let Some(instance) = instances.remove(&key) else {
        return;
    };
    let doc_view: *mut NSView =
        unsafe { msg_send![scroll_view, documentView] };
    if !doc_view.is_null() {
        let _: () = unsafe { msg_send![doc_view, setDataSource: std::ptr::null::<NSObject>()] };
        let _: () = unsafe { msg_send![doc_view, setDelegate: std::ptr::null::<NSObject>()] };
    }
    instance.data_source.shutdown();
    drop(instance);
}

/// Stable reuse identifier — same string across the process so
/// `registerClass:forItemWithIdentifier:` + `makeItemWithIdentifier:`
/// compare against the same NSString instance every time.
fn item_reuse_identifier() -> Retained<NSString> {
    NSString::from_str("IdealystMacVirtualizerItem")
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    //! Regression coverage for the macOS virtualizer (CLAUDE.md §8).
    //! Same constraints as the iOS twin's tests (see
    //! `ios/mobile/src/imp/virtualizer.rs`): NSCollectionView only
    //! renders meaningfully inside a live window + run loop, so cell
    //! recycling and pixel output are covered by running
    //! `websites/website`'s Primitives page (robot-verified); these
    //! tests pin the class-shape and invalidation properties that
    //! caused the actual bugs.
    use super::*;
    use runtime_shared::{Axis, Lanes, VirtualLayout, VirtualizerCallbacks};

    fn vlist() -> VirtualLayout {
        VirtualLayout { axis: Axis::Vertical, lanes: Lanes::Fixed(1), ..VirtualLayout::default() }
    }

    fn empty_callbacks() -> VirtualizerCallbacks<MacosNode> {
        VirtualizerCallbacks::<MacosNode> {
            item_count: Rc::new(|| 0usize),
            item_key: Rc::new(|i| i as u64),
            item_size: Rc::new(|_| 36.0_f32),
            measure_sizes: false,
            mount_item: Rc::new(|_| {
                let mtm = unsafe { MainThreadMarker::new_unchecked() };
                let view = VirtualizerItemView::new(mtm);
                let as_view: Retained<NSView> = unsafe {
                    Retained::retain(&*view as *const VirtualizerItemView as *mut NSView)
                        .expect("retain item view as NSView")
                };
                (MacosNode::View(as_view), 0u64)
            }),
            release_item: Rc::new(|_| {}),
            set_measured_size: Rc::new(|_, _| {}),
        }
    }

    /// The item class MUST be a real `NSCollectionViewItem` subclass.
    /// A prior `Super = NSObject` version blew up inside
    /// `makeItemWithIdentifier:` — an ObjC `unrecognized selector`
    /// exception crossing the non-unwinding `#[method]` boundary, so
    /// the whole app aborted ("panic in a function that cannot
    /// unwind") the moment any `flat_list` rendered.
    #[test]
    fn regression_virtualizer_item_is_a_real_nscollectionviewitem() {
        let item_cls = VirtualizerItem::class();
        let expected = NSCollectionViewItem::class();
        let mut cur = Some(item_cls);
        let mut found = false;
        while let Some(c) = cur {
            if c == expected {
                found = true;
                break;
            }
            cur = c.superclass();
        }
        assert!(found, "VirtualizerItem must inherit NSCollectionViewItem");
    }

    /// `loadView` must install a code-created view: NSViewController's
    /// default implementation looks for a nib named after the class
    /// and throws when none exists (AppKit has no nib-less stock item
    /// the way UIKit has a stock `UICollectionViewCell`).
    #[test]
    fn regression_virtualizer_item_load_view_is_nibless() {
        let item: Retained<NSObject> = unsafe {
            let allocated: *mut AnyObject =
                msg_send![VirtualizerItem::class(), alloc];
            let inited: *mut AnyObject = msg_send![allocated, init];
            Retained::from_raw(inited.cast::<NSObject>())
                .expect("VirtualizerItem init returned nil")
        };
        // Triggers loadView. Would raise (nib not found) before the fix.
        let view: Retained<NSView> = unsafe { msg_send_id![&item, view] };
        assert_eq!(
            view.class(),
            VirtualizerItemView::class(),
            "loadView must install the VirtualizerItemView host container"
        );
    }

    /// `create_virtualizer` must return a node instead of panicking
    /// (the `Backend` trait's default body is `unimplemented!()`).
    /// Also pins the side-state shape: instance registered under the
    /// scroll view's pointer, custom flow layout + data source wired.
    #[test]
    fn regression_macos_virtualizer_create_smoke() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut instances = HashMap::new();
        let scroll = create(mtm, &mut instances, empty_callbacks(), 1.0, vlist());

        let key = &*scroll as *const NSView as usize;
        assert!(instances.contains_key(&key), "instance keyed by scroll view ptr");
        let doc: *mut NSView = unsafe { msg_send![&scroll, documentView] };
        assert!(!doc.is_null(), "collection view installed as documentView");

        release(&mut instances, &scroll);
        assert!(instances.is_empty(), "release drains the instance map");
    }

    /// The flow layout must re-query item sizes when the CROSS axis
    /// changes and must NOT invalidate for main-axis (scroll) bounds
    /// changes. Before the fix the cached stale-width items were
    /// line-centered at negative x (content off-view, blank list).
    #[test]
    fn regression_flow_layout_invalidates_on_cross_axis_change_only() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut instances = HashMap::new();
        let scroll = create(mtm, &mut instances, empty_callbacks(), 1.0, vlist());
        let cv: *mut NSView = unsafe { msg_send![&scroll, documentView] };
        let layout: Retained<NSObject> = unsafe { msg_send_id![cv, collectionViewLayout] };
        assert_eq!(layout.class(), VirtualizerFlowLayout::class());

        // Give the CV a known bounds to diff against.
        let size = CGSize { width: 400.0, height: 200.0 };
        let _: () = unsafe { msg_send![cv, setFrameSize: size] };

        // Same-size scrolled bounds are deliberately NOT asserted: our
        // subclass ORs with `super`'s answer, and AppKit's default for
        // a window-less collection view is not part of our contract.
        let narrower = CGRect {
            origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: 300.0, height: 200.0 },
        };
        let resized: bool = unsafe {
            msg_send![&layout, shouldInvalidateLayoutForBoundsChange: narrower]
        };
        assert!(resized, "cross-axis width change must invalidate");

        // And the bounds-change context must flush the flow layout's
        // cached delegate sizes — a bare invalidation only re-positions
        // stale sizes.
        let ctx: Retained<NSObject> = unsafe {
            msg_send_id![&layout, invalidationContextForBoundsChange: narrower]
        };
        let flush: bool = unsafe { msg_send![&ctx, invalidateFlowLayoutDelegateMetrics] };
        assert!(flush, "cross-axis context must set invalidateFlowLayoutDelegateMetrics");

        release(&mut instances, &scroll);
    }
}
