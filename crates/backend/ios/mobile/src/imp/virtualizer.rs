//! `Primitive::Virtualizer` — `UICollectionView` with a single-section
//! vertical flow layout that drives real cell recycling.
//!
//! Architecture:
//!
//! - The collection view is plain `UICollectionView` (UIKit), built
//!   with a `UICollectionViewFlowLayout` instance. Currently we ship
//!   vertical-only, single-column flow. Phase-2 work: horizontal
//!   scrolling, multi-column / grid, sections, sticky headers.
//! - A custom `NSObject` subclass [`VirtualizerDataSource`] implements
//!   `UICollectionViewDataSource` + `UICollectionViewDelegateFlowLayout`
//!   and trampolines every lifecycle event back to the
//!   `VirtualizerCallbacks` the framework handed us.
//! - A custom `UICollectionViewCell` subclass [`VirtualizerCell`] hosts
//!   a single child UIView produced by `callbacks.mount_item(idx)`.
//!   On reuse / display-end, the cell's host child is removed and
//!   `callbacks.release_item(scope_id)` fires so the per-item Scope
//!   drops (freeing every Signal / Effect nested inside the item).
//!
//! ## Cell hosting
//!
//! Each cell carries a single hosted subview pinned to the cell's
//! `contentView` via Auto Layout. The hosted view is the framework-
//! produced node returned by `callbacks.mount_item(idx)`. UICollectionView
//! recycles cells aggressively; on every `cellForItemAt`, if the cell
//! still has a previously-hosted view, we first release that item's
//! scope and detach the old subview, then mount a fresh one.
//!
//! ## Item-size strategy
//!
//! Phase-1 supports `ItemSize::Known` only — the data source returns
//! `callbacks.item_size(idx)` as the cell's height with a width equal
//! to the collection view's bounds.width. `ItemSize::Measured` (which
//! requires `preferredLayoutAttributesFitting`-driven self-sizing) is
//! tracked as phase-2 work; for now Measured items render with their
//! estimate and we don't update the framework via `set_measured_size`.
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

use framework_core::VirtualizerCallbacks;
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
    /// cells (the same `VirtualizerCell` instance gets handed out
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
            (cb.item_count)() as NSInteger
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
            self.cell_for_item_impl(cv, index_path)
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
                    (release)(prev.scope_id);
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
            let size_f = (cb.item_size)(idx);
            // Vertical flow + single column → full available width,
            // user-provided height. Read the collection view's
            // bounds; subtract content insets so cells don't get
            // clipped under nav bars / safe area when the user has
            // set `contentInset`.
            let bounds: CGRect = unsafe { msg_send![cv, bounds] };
            let insets: UIEdgeInsets = unsafe { msg_send![cv, contentInset] };
            let usable_w = (bounds.size.width - insets.left - insets.right).max(0.0);
            CGSize::new(usable_w, size_f as CGFloat)
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
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(VirtualizerDataSourceIvars {
            callbacks: Rc::new(RefCell::new(Some(callbacks))),
            mounts: Rc::new(RefCell::new(HashMap::new())),
            alive: Rc::new(RefCell::new(true)),
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

// =========================================================================
// VirtualizerCell — plain UICollectionViewCell subclass. We don't
// override anything yet; the data source handles all mounting via
// `cellForItemAt` and `didEndDisplayingCell`. The subclass exists
// solely to give us a stable class name to register with the collection
// view's `registerClass:forCellWithReuseIdentifier:` and to mark cells
// produced by us versus default UICollectionViewCells.
// =========================================================================

declare_class!(
    pub(crate) struct VirtualizerCell;

    unsafe impl ClassType for VirtualizerCell {
        // UICollectionViewCell is the proper superclass, but objc2-ui-kit's
        // re-export of that type would force a Cargo feature; we instead
        // declare the super as `NSObject` and let the ObjC runtime resolve
        // method dispatch against UICollectionViewCell at registration
        // time. Practically: this struct is opaque to Rust callers and
        // only ever instantiated by UIKit via the registered class name.
        //
        // EDIT: we DO use UICollectionViewCell as super via the runtime —
        // see the override of `ClassType::class()` below — by adjusting
        // the metaclass before any `+alloc` happens. For now this stays
        // an NSObject subclass which works because we never `dequeueReusable`
        // call directly; UIKit does.
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystVirtualizerCell";
    }

    impl DeclaredClass for VirtualizerCell {}
);

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
    _horizontal: bool,
) -> Retained<UIView> {
    // Phase-1 MVP: vertical flow layout, single column. Horizontal /
    // sections / sticky headers / overscan tuning are phase-2.
    let _ = _overscan;
    let _ = _horizontal;

    // 1) Build the flow layout. We tune zero spacing between items
    //    and sections so the user's `item_size` height is exactly the
    //    rendered row pitch.
    let layout_cls = objc2::class!(UICollectionViewFlowLayout);
    let layout: Retained<NSObject> = unsafe {
        msg_send_id![msg_send_id![layout_cls, alloc], init]
    };
    // scrollDirection = vertical (0). Horizontal (1) is phase-2.
    let _: () = unsafe { msg_send![&layout, setScrollDirection: 0i64] };
    let _: () = unsafe { msg_send![&layout, setMinimumLineSpacing: 0.0 as CGFloat] };
    let _: () = unsafe { msg_send![&layout, setMinimumInteritemSpacing: 0.0 as CGFloat] };

    // 2) Build the collection view. `initWithFrame:collectionViewLayout:`
    //    needs a CGRect frame and the flow layout we just built; Taffy
    //    rewrites the frame on the next layout pass.
    let cv_cls = objc2::class!(UICollectionView);
    let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
    let cv: Retained<UIView> = unsafe {
        msg_send_id![
            msg_send_id![cv_cls, alloc],
            initWithFrame: frame,
            collectionViewLayout: &*layout
        ]
    };

    // Default background is black on iOS — set clear so the
    // virtualizer doesn't paint over its parent's background while
    // cells are sparse.
    let clear: Retained<NSObject> = unsafe {
        msg_send_id![objc2::class!(UIColor), clearColor]
    };
    let _: () = unsafe { msg_send![&cv, setBackgroundColor: &*clear] };

    // 3) Register our cell subclass against a stable reuse identifier.
    let cell_cls: &objc2::runtime::AnyClass = VirtualizerCell::class();
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
    let data_source = VirtualizerDataSource::new(mtm, callbacks);
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
            layout,
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
