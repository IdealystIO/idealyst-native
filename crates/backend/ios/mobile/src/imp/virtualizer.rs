//! `Element::Virtualizer` — `UICollectionView` with a flow layout
//! that drives real cell recycling. Supports vertical and horizontal
//! single-section lists.
//!
//! Architecture:
//!
//! - The collection view is plain `UICollectionView` (UIKit), built
//!   with a `UICollectionViewFlowLayout` instance. Scroll direction
//!   is set from the `horizontal` flag handed to
//!   `Backend::create_virtualizer`.
//! - A custom `NSObject` subclass [`VirtualizerDataSource`] implements
//!   `UICollectionViewDataSource` + `UICollectionViewDelegateFlowLayout`
//!   and trampolines every lifecycle event back to the
//!   `VirtualizerCallbacks` the framework handed us. It also tracks
//!   the orientation so `sizeForItemAt` returns axis-correct sizes.
//! - A stock `UICollectionViewCell` hosts a single child UIView produced
//!   by `callbacks.mount_item(idx)`. On reuse / display-end, the cell's
//!   host child is removed and `callbacks.release_item(scope_id)` fires so
//!   the per-item Scope drops (freeing every Signal / Effect nested inside
//!   the item).
//!
//! ## Cell hosting
//!
//! Each cell carries a single hosted subview pinned to the cell's
//! `contentView` via the autoresizing mask. The hosted view is the
//! framework-produced node returned by `callbacks.mount_item(idx)`.
//! UICollectionView recycles cells aggressively; on every
//! `cellForItemAt`, if the cell still has a previously-hosted view,
//! we first release that item's scope and detach the old subview,
//! then mount a fresh one.
//!
//! ## Item-size strategy
//!
//! Supports `ItemSize::Known` — the data source returns
//! `callbacks.item_size(idx)` as the cell's main-axis size (height in
//! vertical mode, width in horizontal mode). The cross-axis dimension
//! is filled from the collection view's bounds minus content inset.
//!
//! `ItemSize::Measured` is still parked, but the blocker now is a
//! framework-core gap rather than an iOS gap: cells live outside the
//! framework's Taffy layout tree (UICollectionViewLayout owns cell
//! sizing), so the hosted subtree never gets a Taffy measure pass
//! and has no intrinsic-size we can read back via
//! `systemLayoutSizeFittingSize:`. Implementing this needs the
//! framework to expose a measure-only pass over a detached subtree;
//! once that lands, the cell can override
//! `preferredLayoutAttributesFittingAttributes:` to surface the
//! measured size and fire `callbacks.set_measured_size`.
//!
//! ## Sections
//!
//! Multi-section lists (sticky headers, section insets, grouped data)
//! also block on a framework-core gap: `VirtualizerCallbacks` is flat
//! — `item_count` returns the global item count and `item_key` /
//! `item_size` / `mount_item` are keyed by a flat `usize` index. The
//! UICollectionView side trivially supports sections; the missing
//! piece is a section-aware `VirtualizerCallbacks` shape.
//!
//! ## Ownership / safety
//!
//! - UIKit holds the dataSource + delegate as **weak** references.
//!   We retain the `VirtualizerDataSource` in a side map keyed by the
//!   collection view's pointer so it outlives the collection view; the
//!   map entry is removed in `release_virtualizer`.
//! - The `VirtualizerCallbacks` live inside the data source's ivars
//!   (an `Rc`-wrapped struct) — they're freed when the data source
//!   drops, which happens at `release_virtualizer` time. This is what
//!   prevents queued UIKit callbacks from firing into a freed Signal
//!   slot after the framework scope has dropped.
//! - The map of `scope_id -> cell_ptr -> child_view` lives on the data
//!   source so we can release the right scope on reuse / teardown.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::{VirtualizerCallbacks, VirtualLayout};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{
    CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSInteger, NSObject as NSObjectFoundation,
};
use objc2_ui_kit::{UIView, UIViewAutoresizing};

use super::IosNode;

// =========================================================================
// Per-cell state — what scope_id is currently mounted in this cell, and
// the hosted child UIView (so we can detach it on reuse / teardown).
// =========================================================================

#[derive(Clone)]
struct CellMount {
    scope_id: u64,
    child: Retained<UIView>,
}

// =========================================================================
// VirtualizerDataSource — NSObject subclass implementing
// UICollectionViewDataSource + UICollectionViewDelegateFlowLayout.
// =========================================================================

pub(crate) struct VirtualizerDataSourceIvars {
    /// The `VirtualizerCallbacks` bundle the framework handed us.
    /// Wrapped in `Rc<RefCell<Option<_>>>` so `release_virtualizer`
    /// can drop them deterministically (taking the option) before
    /// the data source itself is freed — that way any UIKit event
    /// queued for the next runloop turn sees `None` and bails
    /// cleanly instead of reaching into a freed framework Scope.
    callbacks: Rc<RefCell<Option<VirtualizerCallbacks<IosNode>>>>,
    /// Map from cell pointer to its current mount. UIKit reuses
    /// cells (the same `UICollectionViewCell` instance gets handed out
    /// for different indices over time), so we key by the cell's
    /// own address to know which scope to release on reuse.
    mounts: Rc<RefCell<HashMap<usize, CellMount>>>,
    /// `false` once `release_virtualizer` has fired. Guards every
    /// callback path so queued UIKit events firing after teardown
    /// no-op cleanly. Tracked separately from `callbacks` being
    /// `None` because UIKit also calls `numberOfItemsInSection` on
    /// every `reloadData()`, and we need a single check at the top
    /// of each entry point.
    alive: Rc<RefCell<bool>>,
    /// Scroll axis + lane subdivision + gaps. `sizeForItemAt` reads
    /// this to compute each cell's cross-axis size: a list (one lane)
    /// fills the cross axis, a grid divides it into `lanes` columns.
    /// Stored on the data source (not just on the flow layout) so the
    /// delegate's size-callback has it available without reaching back
    /// into the layout. `AutoFit` resolves against the live container
    /// bounds inside `sizeForItemAt`, so a rotation re-lanes the grid.
    layout: VirtualLayout,
}

declare_class!(
    pub(crate) struct VirtualizerDataSource;

    unsafe impl ClassType for VirtualizerDataSource {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystVirtualizerDataSource";
    }

    impl DeclaredClass for VirtualizerDataSource {
        type Ivars = VirtualizerDataSourceIvars;
    }

    unsafe impl NSObjectProtocol for VirtualizerDataSource {}

    // ---- UICollectionViewDataSource ----
    unsafe impl VirtualizerDataSource {
        #[method(numberOfSectionsInCollectionView:)]
        fn number_of_sections(&self, _cv: &NSObject) -> NSInteger {
            // Phase-1: single section. Sections support is phase-2.
            1
        }

        #[method(collectionView:numberOfItemsInSection:)]
        fn number_of_items(&self, _cv: &NSObject, _section: NSInteger) -> NSInteger {
            if !*self.ivars().alive.borrow() {
                return 0;
            }
            let cb_opt = self.ivars().callbacks.borrow();
            let Some(cb) = cb_opt.as_ref() else {
                return 0;
            };
            // Guard the framework callback — this is an extern "C" IMP.
            crate::imp::ffi_guard::guard_ffi(
                "VirtualizerDataSource::numberOfItems",
                || (cb.item_count)(),
            ) as NSInteger
        }

        #[method_id(collectionView:cellForItemAtIndexPath:)]
        fn cell_for_item_at(
            &self,
            cv: &NSObject,
            index_path: &NSObject,
        ) -> Retained<NSObject> {
            // Dequeue + mount in a helper so the method body has a
            // single tail expression (the macro's IdReturnValue
            // shim doesn't gracefully handle early `return`s in a
            // body that produces a `Retained<_>`).
            //
            // Guard the mount path: cellForItem is an extern "C" IMP and
            // mounting runs the full framework build (Taffy, reactive
            // scope, RefCell borrows) — the most panic-prone callback
            // here. A panic must abort, not unwind into UICollectionView.
            crate::imp::ffi_guard::guard_ffi(
                "VirtualizerDataSource::cellForItem",
                || self.cell_for_item_impl(cv, index_path),
            )
        }
    }

    // ---- UICollectionViewDelegate ----
    unsafe impl VirtualizerDataSource {
        #[method(collectionView:didEndDisplayingCell:forItemAtIndexPath:)]
        fn did_end_displaying_cell(
            &self,
            _cv: &NSObject,
            cell: &NSObject,
            _index_path: &NSObject,
        ) {
            // Cell scrolled out of the visible window. UIKit will
            // either drop it (low memory) or hand it back via
            // `cellForItemAt` for another index; either way the
            // currently mounted item won't be visible again under
            // its current scope. Release the per-item Scope now so
            // the data signals it owns are freed promptly instead
            // of waiting for the next `cellForItemAt`.
            if !*self.ivars().alive.borrow() {
                return;
            }
            let cell_ptr = cell as *const NSObject as usize;
            let previous = self.ivars().mounts.borrow_mut().remove(&cell_ptr);
            if let Some(prev) = previous {
                let release_fn = {
                    let cb_opt = self.ivars().callbacks.borrow();
                    cb_opt.as_ref().map(|c| c.release_item.clone())
                };
                unsafe { prev.child.removeFromSuperview() };
                if let Some(release) = release_fn {
                    // Guard the framework teardown callback (extern "C" IMP).
                    crate::imp::ffi_guard::guard_ffi(
                        "VirtualizerDataSource::didEndDisplaying",
                        || (release)(prev.scope_id),
                    );
                }
            }
        }
    }

    // ---- UICollectionViewDelegateFlowLayout ----
    unsafe impl VirtualizerDataSource {
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
            let row: NSInteger = unsafe { msg_send![index_path, row] };
            let idx = if row < 0 { 0usize } else { row as usize };
            // Guard the framework size callback (extern "C" IMP).
            let size_f = crate::imp::ffi_guard::guard_ffi(
                "VirtualizerDataSource::sizeForItem",
                || (cb.item_size)(idx),
            ) as CGFloat;
            // Read the collection view's bounds; subtract content
            // insets so cells don't get clipped under nav bars / safe
            // area when the user has set `contentInset`.
            let bounds: CGRect = unsafe { msg_send![cv, bounds] };
            let insets: UIEdgeInsets = unsafe { msg_send![cv, contentInset] };
            let layout = self.ivars().layout;
            // Cross-axis extent available to lanes (perpendicular to
            // scroll), then split into `lanes` columns minus gaps. With
            // one lane this is the old list behavior (cell fills the
            // cross axis); with N it's a uniform grid. The flow layout
            // wraps N cells per line because each is `lane_cross` wide.
            let (usable_cross, lane_cross) = {
                let usable = if layout.axis.is_horizontal() {
                    (bounds.size.height - insets.top - insets.bottom).max(0.0)
                } else {
                    (bounds.size.width - insets.left - insets.right).max(0.0)
                };
                let lanes = layout.lanes.resolve(usable as f32, layout.cross_spacing) as CGFloat;
                let lane = if lanes >= 1.0 {
                    ((usable - (lanes - 1.0) * layout.cross_spacing as CGFloat) / lanes).max(0.0)
                } else {
                    usable
                };
                (usable, lane)
            };
            let _ = usable_cross;
            if layout.axis.is_horizontal() {
                // Horizontal flow → user `item_size` is the cell's
                // width (main axis); cross-axis is the lane height.
                CGSize::new(size_f, lane_cross)
            } else {
                // Vertical flow → user `item_size` is the cell's
                // height (main axis); cross-axis is the lane width.
                CGSize::new(lane_cross, size_f)
            }
        }
    }
);

// `UIEdgeInsets` mirrored locally because `objc2-ui-kit` exposes it
// via the foundation crate's `UIEdgeInsets` type only when extra
// features are enabled — and we don't need any other UIKit primitives
// from this struct here, so duplicating it keeps the feature list
// minimal.
#[repr(C)]
#[derive(Clone, Copy)]
struct UIEdgeInsets {
    top: CGFloat,
    left: CGFloat,
    bottom: CGFloat,
    right: CGFloat,
}
unsafe impl objc2::Encode for UIEdgeInsets {
    const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
        "UIEdgeInsets",
        &[
            <CGFloat as objc2::Encode>::ENCODING,
            <CGFloat as objc2::Encode>::ENCODING,
            <CGFloat as objc2::Encode>::ENCODING,
            <CGFloat as objc2::Encode>::ENCODING,
        ],
    );
}
unsafe impl objc2::RefEncode for UIEdgeInsets {
    const ENCODING_REF: objc2::Encoding =
        objc2::Encoding::Pointer(&<Self as objc2::Encode>::ENCODING);
}

impl VirtualizerDataSource {
    /// Dequeue + mount helper. Lives outside the `declare_class!`
    /// block so it can use early `return`s + question marks without
    /// fighting the macro's `IdReturnValue` conversion.
    fn cell_for_item_impl(
        &self,
        cv: &NSObject,
        index_path: &NSObject,
    ) -> Retained<NSObject> {
        let identifier = cell_reuse_identifier();
        let cell: Retained<NSObject> = unsafe {
            msg_send_id![
                cv,
                dequeueReusableCellWithReuseIdentifier: &*identifier,
                forIndexPath: index_path
            ]
        };

        if !*self.ivars().alive.borrow() {
            return cell;
        }

        let row: NSInteger = unsafe { msg_send![index_path, row] };
        let idx = if row < 0 { 0usize } else { row as usize };

        // Cell pointer is our key into the mounts map. Same cell
        // instance reused later for a different index will hit the
        // same key — we use that to release the previous mount before
        // installing the new one.
        let cell_ptr = &*cell as *const NSObject as usize;

        // If this cell currently hosts a different item, tear down
        // that mount first. `release_item` drops the framework's
        // per-item Scope (freeing every Signal / Effect inside it);
        // we then detach the now-stale UIView. We clone the
        // release_fn out of the RefCell before invoking it so a
        // re-entrant borrow doesn't panic.
        let previous = self.ivars().mounts.borrow_mut().remove(&cell_ptr);
        if let Some(prev) = previous {
            let release_fn = {
                let cb_opt = self.ivars().callbacks.borrow();
                cb_opt.as_ref().map(|c| c.release_item.clone())
            };
            unsafe { prev.child.removeFromSuperview() };
            if let Some(release) = release_fn {
                (release)(prev.scope_id);
            }
        }

        // Mount the fresh item. The framework's mount_item builds
        // the subtree inside a new per-item Scope, returns both the
        // native node and the scope id.
        let mount_fn = {
            let cb_opt = self.ivars().callbacks.borrow();
            cb_opt.as_ref().map(|c| c.mount_item.clone())
        };
        let Some(mount) = mount_fn else {
            return cell;
        };
        let (node, scope_id) = (mount)(idx);
        let child_view = node.as_view();

        // Pin the new child inside the cell's contentView so it fills
        // the available cell bounds. Autoresizing-mask path (not Auto
        // Layout) for two reasons: (1) the framework's Taffy-driven
        // layout doesn't touch cells (they're inside
        // UICollectionView's private layout flow), and (2)
        // autoresizing-mask is cheaper and matches the cell's own
        // resize cycle.
        let content_view: Retained<UIView> =
            unsafe { msg_send_id![&cell, contentView] };
        let bounds: CGRect = unsafe { msg_send![&content_view, bounds] };
        let _: () = unsafe { msg_send![child_view, setFrame: bounds] };
        // flexibleWidth | flexibleHeight = 0x12 — the cell's
        // contentView gets resized by UIKit on every layout pass;
        // the autoresizing mask keeps the child filling it.
        let mask: UIViewAutoresizing = UIViewAutoresizing::from_bits_truncate(0x12);
        let _: () = unsafe { msg_send![child_view, setAutoresizingMask: mask] };
        unsafe { content_view.addSubview(child_view) };

        // Retain the child so the cell-mount map owns it even after
        // the caller's IosNode (which is itself a Retained) is dropped.
        let child_retained: Retained<UIView> = unsafe {
            Retained::retain(child_view as *const UIView as *mut UIView)
                .expect("retain mounted child UIView")
        };
        self.ivars().mounts.borrow_mut().insert(
            cell_ptr,
            CellMount {
                scope_id,
                child: child_retained,
            },
        );

        cell
    }

    fn new(
        mtm: MainThreadMarker,
        callbacks: VirtualizerCallbacks<IosNode>,
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

    /// Tear down — flips the alive flag so further UIKit callbacks
    /// short-circuit, releases every still-mounted scope, and drops
    /// the framework callbacks bundle. After this, the data source
    /// holds no references into framework-owned state and can be
    /// safely dropped on the next ObjC autorelease drain.
    fn shutdown(&self) {
        *self.ivars().alive.borrow_mut() = false;
        // Drain mounts + release every scope. Take the release_fn
        // out of the option first so it survives the callbacks
        // drop on the same path.
        let release_fn = {
            let cb_opt = self.ivars().callbacks.borrow();
            cb_opt.as_ref().map(|c| c.release_item.clone())
        };
        let mounts = std::mem::take(&mut *self.ivars().mounts.borrow_mut());
        for (_cell_ptr, mount) in mounts.into_iter() {
            unsafe { mount.child.removeFromSuperview() };
            if let Some(ref release) = release_fn {
                (release)(mount.scope_id);
            }
        }
        // Drop the callbacks bundle — frees the Rc<dyn Fn> closures
        // and, transitively, any framework state they captured (data
        // signals, item-key closures, etc.). Doing this AFTER the
        // mounts-drain ensures `release_item` was reachable while we
        // needed it.
        *self.ivars().callbacks.borrow_mut() = None;
    }
}

// Cells are stock `UICollectionViewCell`s (registered in `create`). We host
// the user's view inside the dequeued cell's `contentView` and override no
// cell behavior, so no custom subclass is needed. (A prior `NSObject`-super
// subclass tripped `registerClass:forCellWithReuseIdentifier:`'s assertion
// that the class be a `UICollectionViewCell` subclass — a hard crash that
// meant `flat_list` never actually ran on iOS.)

// =========================================================================
// Public entry points (called from imp::mod's Backend impl).
// =========================================================================

/// Side map keyed by the collection view's pointer: holds the
/// `VirtualizerDataSource` (UIKit dataSource is a *weak* ref, we need
/// to keep it retained) plus the layout instance (for completeness;
/// not currently mutated after create).
pub(crate) struct VirtualizerInstance {
    pub(crate) data_source: Retained<VirtualizerDataSource>,
    /// Held to keep alive — UIKit doesn't strongly retain assigned
    /// layouts in all cases, and the flow layout has no other owner.
    #[allow(dead_code)]
    pub(crate) layout: Retained<NSObject>,
}

/// Build a UICollectionView with a vertical flow layout, register our
/// cell class, wire up data source + delegate. Returns the collection
/// view wrapped in `IosNode::View`.
pub(crate) fn create(
    mtm: MainThreadMarker,
    instances: &mut HashMap<usize, VirtualizerInstance>,
    callbacks: VirtualizerCallbacks<IosNode>,
    _overscan: f32,
    layout: VirtualLayout,
) -> Retained<UIView> {
    // `overscan` is parked: UICollectionView's built-in cell prefetch
    // (default-on since iOS 10) already overscans implicitly; exposing
    // an exact-count knob would require either a custom
    // UICollectionViewLayout subclass or fiddling with
    // `isPrefetchingEnabled` heuristics. Revisit if a list shows up
    // that the framework wants more aggressive prefetch on.
    let _ = _overscan;

    // 1) Build the flow layout. We tune zero spacing between items
    //    and sections so the user's `item_size` is exactly the
    //    rendered row/column pitch.
    let layout_cls = objc2::class!(UICollectionViewFlowLayout);
    let flow_layout: Retained<NSObject> = unsafe {
        msg_send_id![msg_send_id![layout_cls, alloc], init]
    };
    // UICollectionViewScrollDirection: 0 = vertical, 1 = horizontal.
    let scroll_direction: i64 = if layout.axis.is_horizontal() { 1 } else { 0 };
    let _: () = unsafe { msg_send![&flow_layout, setScrollDirection: scroll_direction] };
    // `minimumLineSpacing` is the gap between successive grid-rows
    // along the scroll axis; `minimumInteritemSpacing` is the gap
    // between lanes (cells on the same line). For a list (one lane)
    // only line spacing applies.
    let _: () = unsafe {
        msg_send![&flow_layout, setMinimumLineSpacing: layout.main_spacing as CGFloat]
    };
    let _: () = unsafe {
        msg_send![&flow_layout, setMinimumInteritemSpacing: layout.cross_spacing as CGFloat]
    };

    // 2) Build the collection view. `initWithFrame:collectionViewLayout:`
    //    needs a CGRect frame and the flow layout we just built; Taffy
    //    rewrites the frame on the next layout pass.
    let cv_cls = objc2::class!(UICollectionView);
    let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
    let cv: Retained<UIView> = unsafe {
        msg_send_id![
            msg_send_id![cv_cls, alloc],
            initWithFrame: frame,
            collectionViewLayout: &*flow_layout
        ]
    };

    // Default background is black on iOS — set clear so the
    // virtualizer doesn't paint over its parent's background while
    // cells are sparse.
    let clear: Retained<NSObject> = unsafe {
        msg_send_id![objc2::class!(UIColor), clearColor]
    };
    let _: () = unsafe { msg_send![&cv, setBackgroundColor: &*clear] };

    // 3) Register the stock `UICollectionViewCell` class against a stable
    //    reuse identifier. We host the user's view inside the dequeued cell's
    //    `contentView` (see `cell_for_item_impl`) and need no custom cell
    //    behavior, so the built-in class is exactly right — and crucially,
    //    `registerClass:forCellWithReuseIdentifier:` ASSERTS the class is a
    //    `UICollectionViewCell` subclass, so registering an `NSObject`
    //    subclass crashes at register time (UICollectionView.m assertion).
    let cell_cls: &objc2::runtime::AnyClass = objc2::class!(UICollectionViewCell);
    let reuse_id = cell_reuse_identifier();
    let _: () = unsafe {
        msg_send![
            &cv,
            registerClass: cell_cls,
            forCellWithReuseIdentifier: &*reuse_id
        ]
    };

    // 4) Build the data source + delegate. UIKit holds these weakly,
    //    so we keep the Retained ref in `instances` keyed by the
    //    collection view's pointer.
    let data_source = VirtualizerDataSource::new(mtm, callbacks, layout);
    let _: () = unsafe { msg_send![&cv, setDataSource: &*data_source] };
    let _: () = unsafe { msg_send![&cv, setDelegate: &*data_source] };

    // 5) Stash the instance side-state. The pointer used as the key
    //    is stable across the collection view's lifetime — same
    //    convention used by `navigator_instances` / `portal_instances`.
    let cv_key = &*cv as *const UIView as usize;
    instances.insert(
        cv_key,
        VirtualizerInstance {
            data_source: data_source.clone(),
            layout: flow_layout,
        },
    );

    cv
}

/// Force a full reload. Phase-1 uses `reloadData` for every data-changed
/// notification; this is correct (UIKit re-queries item_count + sizes
/// + cellForItem) but potentially expensive on very large lists.
/// Phase-2: switch to `performBatchUpdates` + diff against the previous
/// key set so surviving items animate in place.
pub(crate) fn data_changed(view: &UIView) {
    let _: () = unsafe { msg_send![view, reloadData] };
}

/// Tear down the data source's mounts + drop the framework callbacks
/// bundle. The collection view itself is dropped when the framework
/// drops the `IosNode::View`; we just need to make sure no UIKit
/// callback fires into a freed Scope afterwards.
pub(crate) fn release(
    instances: &mut HashMap<usize, VirtualizerInstance>,
    view: &UIView,
) {
    let key = view as *const UIView as usize;
    let Some(instance) = instances.remove(&key) else {
        return;
    };
    // Detach UIKit's references to the data source so any queued
    // event drains as a no-op. UIKit holds weak refs but `setDataSource:nil`
    // also tells UIKit to stop pulling cells; same for setDelegate.
    let _: () = unsafe { msg_send![view, setDataSource: std::ptr::null::<NSObject>()] };
    let _: () = unsafe { msg_send![view, setDelegate: std::ptr::null::<NSObject>()] };
    // Run the in-data-source shutdown to release every mounted scope
    // and drop the callbacks bundle. After this returns, the data
    // source holds no framework references.
    instance.data_source.shutdown();
    // `instance` drops here — its Retained<VirtualizerDataSource> goes
    // away, releasing the ObjC retain count we held against UIKit's
    // weak ref.
    drop(instance);
}

/// Stable, process-wide reuse identifier string. Static `NSString*` so
/// the registerClass + dequeueReusable calls compare against the same
/// string instance every time.
fn cell_reuse_identifier() -> Retained<objc2_foundation::NSString> {
    objc2_foundation::NSString::from_str("IdealystVirtualizerCell")
}

// Re-export NSObjectFoundation so this file's parent module sees the
// type alias even when objc2-foundation hasn't enabled every NSObject
// feature. Currently unused outside this file; kept for symmetry with
// other backend imp/* modules.
#[allow(dead_code)]
type _ForceImportNSObject = NSObjectFoundation;

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    //! Regression coverage for `create_virtualizer`. Per CLAUDE.md §8,
    //! every bug fix lands with a regression test that fails before
    //! the fix and passes after. The fix here is: `create_virtualizer`
    //! used to delegate to the framework's default `unimplemented!()`,
    //! so the smoke test is simply "does calling it with a minimal
    //! callbacks struct return a node without panicking?"
    //!
    //! ## Why this isn't a tighter test
    //!
    //! UICollectionView only renders meaningfully when attached to a
    //! UIWindow + a view hierarchy with real bounds. Driving cells
    //! through the dequeue/mount/release cycle from a unit test would
    //! require either:
    //!   1. A live UIApplication + UIWindow + run loop, which is
    //!      what a UI test target (XCTest / EarlGrey) is for — not
    //!      reachable from `cargo test`.
    //!   2. Manually calling the data-source methods on a synthesized
    //!      NSIndexPath, which exercises the trampoline but not the
    //!      actual UICollectionView layout flow we depend on.
    //!
    //! The runtime-core walker tests cover `VirtualizerCallbacks`
    //! plumbing on a stub backend; this test covers the iOS-specific
    //! "method exists + returns something" property. Cell recycling
    //! behavior is gated to manual on-device QA against the
    //! `examples/welcome` flat-list page until we wire up a UI test
    //! target.
    use super::*;
    use runtime_core::{Axis, Lanes, VirtualizerCallbacks, VirtualLayout};

    fn vlist(horizontal: bool) -> VirtualLayout {
        VirtualLayout {
            axis: if horizontal { Axis::Horizontal } else { Axis::Vertical },
            ..VirtualLayout::default()
        }
    }

    /// Empty `VirtualizerCallbacks` for tests that exercise the
    /// construction/teardown path. UIKit won't call `cellForItemAt`
    /// when `item_count` returns 0, so `mount_item` is only invoked
    /// in the unreachable path.
    fn empty_callbacks() -> VirtualizerCallbacks<IosNode> {
        VirtualizerCallbacks::<IosNode> {
            item_count: Rc::new(|| 0usize),
            item_key: Rc::new(|i| i as u64),
            item_size: Rc::new(|_| 44.0_f32),
            measure_sizes: false,
            mount_item: Rc::new(|_| {
                let mtm = unsafe { MainThreadMarker::new_unchecked() };
                let view = unsafe { UIView::new(mtm) };
                (IosNode::View(view), 0u64)
            }),
            release_item: Rc::new(|_| {}),
            set_measured_size: Rc::new(|_, _| {}),
        }
    }

    /// `create_virtualizer` must return a node instead of panicking
    /// (the framework's default impl `unimplemented!()`s, which is
    /// the bug we just fixed). This test verifies the iOS backend's
    /// `create` entry point can be called with a minimal callbacks
    /// struct and produces a real `UIView` (the UICollectionView).
    #[test]
    fn regression_ios_virtualizer_does_not_unimplemented_panic() {
        // Cargo's iOS test runner spawns the test process on the
        // main thread by default; `new_unchecked` is therefore safe.
        // If a future test harness moves us off the main thread, the
        // first `UICollectionView::alloc` call would crash with an
        // NSInternalInconsistencyException anyway, surfacing the
        // misuse loudly.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };

        let mut instances = HashMap::new();
        let view = create(mtm, &mut instances, empty_callbacks(), 1.0, vlist(false));

        // The view must be a real UIView (UICollectionView is-a
        // UIView), and our side-state map must have exactly one
        // entry keyed by the view's pointer.
        let key = &*view as *const UIView as usize;
        assert!(
            instances.contains_key(&key),
            "create() must register the data source in the side map so UIKit's weak \
             delegate reference doesn't dangle"
        );

        // Smoke-test the teardown path. After `release`, the side
        // map must no longer hold the instance — that's what frees
        // the data source and the framework callbacks bundle it owns.
        release(&mut instances, &view);
        assert!(
            !instances.contains_key(&key),
            "release() must remove the instance from the side map so the data source \
             drops and its captured framework callbacks are freed"
        );
    }

    /// Phase-2 regression: when `horizontal = true`, the flow layout
    /// must be configured with `scrollDirection = horizontal (1)` and
    /// the data source must record the orientation so its
    /// `sizeForItemAt` returns axis-swapped sizes. The bug before this
    /// landed was that the `horizontal` parameter was parked
    /// (`let _ = _horizontal;`), so author-side `Virtualizer { horizontal:
    /// true }` rendered identically to the vertical default.
    #[test]
    fn regression_ios_virtualizer_horizontal_sets_scroll_direction() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut instances = HashMap::new();
        let view = create(mtm, &mut instances, empty_callbacks(), 1.0, vlist(true));
        let key = &*view as *const UIView as usize;
        let instance = instances
            .get(&key)
            .expect("create() registers the instance");

        // Read the flow layout's `scrollDirection`. 1 = horizontal.
        let direction: NSInteger =
            unsafe { msg_send![&instance.layout, scrollDirection] };
        assert_eq!(
            direction, 1,
            "horizontal=true must set the flow layout's scrollDirection to 1 \
             (UICollectionViewScrollDirectionHorizontal)"
        );

        // And the data source must remember the orientation so the
        // size-callback reads the correct axis.
        assert!(
            instance.data_source.ivars().layout.axis.is_horizontal(),
            "horizontal=true must set the data source's layout axis so \
             `sizeForItemAt` returns axis-swapped sizes"
        );

        release(&mut instances, &view);
    }

    /// Grid regression: a `lanes > 1` layout must propagate the lane
    /// count + inter-lane spacing onto the flow layout
    /// (`minimumInteritemSpacing`) and the data source so
    /// `sizeForItemAt` divides the cross axis into columns. Without a
    /// live window we can't assert the resolved cell width, but we can
    /// assert the layout descriptor reached the data source and the
    /// flow layout's spacing was configured.
    #[test]
    fn regression_ios_virtualizer_grid_lanes_configures_layout() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut instances = HashMap::new();
        let layout = VirtualLayout {
            axis: Axis::Vertical,
            lanes: Lanes::Fixed(3),
            main_spacing: 8.0,
            cross_spacing: 12.0,
        };
        let view = create(mtm, &mut instances, empty_callbacks(), 1.0, layout);
        let key = &*view as *const UIView as usize;
        let instance = instances.get(&key).expect("instance registered");

        // Inter-item (cross) spacing on the flow layout drives the gap
        // between lanes on a line.
        let interitem: CGFloat =
            unsafe { msg_send![&instance.layout, minimumInteritemSpacing] };
        assert!(
            (interitem - 12.0).abs() < 0.01,
            "cross_spacing must set minimumInteritemSpacing (got {interitem})"
        );
        let line: CGFloat = unsafe { msg_send![&instance.layout, minimumLineSpacing] };
        assert!(
            (line - 8.0).abs() < 0.01,
            "main_spacing must set minimumLineSpacing (got {line})"
        );

        // The data source carries the lane count for `sizeForItemAt`.
        assert_eq!(
            instance.data_source.ivars().layout.lanes,
            Lanes::Fixed(3),
            "grid lane count must reach the data source"
        );

        release(&mut instances, &view);
    }
}
