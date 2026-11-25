//! The virtualizer primitive — `NSCollectionView` with a flow layout
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
//! ## Item-size strategy
//!
//! `callbacks.item_size(idx)` gives every cell's main-axis size; the
//! cross axis is the collection view's bounds divided into lanes.
//!
//! Each row is a detached Taffy root laid out at its cell's size
//! ([`layout_row_in`]) on two paths: when the cell is sized or reused
//! (`setFrameSize:` / mount), and from the backend's layout pass for any
//! mounted row whose Taffy root is dirty
//! ([`super::MacosBackend::relayout_dirty_virtualizer_rows`]) — so a reactive
//! change inside a mounted row reaches its views.
//!
//! - `ItemSize::Known` (`measure_sizes == false`): that size is final.
//!   Rows are never measured.
//! - `ItemSize::Measured` (`measure_sizes == true`): `item_size` starts
//!   as the estimate. Every time a row is laid out (both paths) its
//!   natural main-axis size is measured at the cell's cross size
//!   (`LayoutTree::measure_main` — `compute` alone would just force the
//!   row into the estimated box). A deferred drain reports each size that
//!   differs from `item_size(idx)` by more than [`MEASURE_EPSILON`]
//!   through `set_measured_size` (the vocabulary keys it by item key, so
//!   it survives recycling and reorders), then one flow-layout
//!   invalidation re-queries every
//!   `sizeForItemAtIndexPath`: the row's slot, the offsets after it,
//!   the content size and the visible range all follow. The re-layout
//!   measures the same size again, `item_size` already returns it, and
//!   nothing more is reported — that is what makes it converge.
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
    NSCollectionViewFlowLayout, NSCollectionViewItem, NSEvent, NSScrollView, NSView,
};
use objc2_foundation::{
    CGFloat, CGRect, CGSize, MainThreadMarker, NSEdgeInsets, NSInteger, NSObject, NSString,
};
use runtime_shared::primitives::virtualizer::ItemKey;
use runtime_shared::primitives::virtualizer_diff::{diff_keys, KeyedUpdate, MAX_BATCH_OPS};
use runtime_shared::{StyleRules, VirtualizerCallbacks, VirtualLayout};

use super::MacosNode;

// =========================================================================
// Per-item mount state
// =========================================================================

#[derive(Clone)]
struct ItemMount {
    scope_id: u64,
    /// The item key the row was mounted for. A batched data change
    /// moves the row without remounting it, so the row's index is
    /// re-derived from this key afterwards ([`VirtualizerDataSource::reindex_mounts`]).
    key: ItemKey,
    /// The framework-produced view we attached as a subview of
    /// the NSCollectionViewItem's `view`. Retained here so we can
    /// removeFromSuperview on reuse / teardown.
    child: Retained<NSView>,
}

/// Unmount hygiene: clear the host container's `hosted_child_key` (and
/// its measuring slot) so a later `setFrameSize:` doesn't relayout or
/// measure a subtree that's being torn down — a released row must not
/// report a size for a scope that no longer exists.
fn clear_host_key(child: &NSView) {
    let superview: Option<Retained<NSView>> = unsafe { child.superview() };
    if let Some(sv) = superview {
        if let Some(host) = VirtualizerItemView::from_container(&sv) {
            host.ivars().hosted_child_key.set(0);
            *host.ivars().measure.borrow_mut() = None;
        }
    }
}

// =========================================================================
// Measured item sizes (`ItemSize::Measured`). See the module docs.
// =========================================================================

/// A measured size within this many points of the size the list already
/// uses is not reported — sub-point float noise must not re-run the flow
/// layout (and re-measure, and re-report) forever.
const MEASURE_EPSILON: f32 = 0.5;

/// Per-list measuring state, shared by the data source and every item
/// view currently hosting one of its rows. Only built when
/// `callbacks.measure_sizes` — a Known list has none, so it never
/// measures.
struct RowMeasure {
    horizontal: bool,
    item_size: Rc<dyn Fn(usize) -> f32>,
    set_measured_size: Rc<dyn Fn(u64, f32)>,
    /// Weak: item views (owned by the collection view) hold this struct,
    /// so a strong reference would be a retain cycle.
    collection_view: objc2::rc::Weak<NSView>,
    /// Measurements waiting for the next drain: `(scope_id, idx, size)`.
    pending: RefCell<Vec<(u64, usize, f32)>>,
    /// A drain is scheduled and has not run yet.
    relayout_queued: Cell<bool>,
    /// The data source's `alive` flag: false after `release`.
    alive: Rc<RefCell<bool>>,
}

impl RowMeasure {
    /// Queue row `idx`'s measured main-axis size. One deferred drain per
    /// turn, however many rows measured: rows are measured with the
    /// backend borrowed (inside its layout pass, or a cell's
    /// `setFrameSize:` while the collection view applies layout
    /// attributes), where neither calling framework closures nor
    /// invalidating the flow layout mid-pass is safe.
    fn queue_report(self: &Rc<Self>, scope_id: u64, idx: usize, measured: f32) {
        self.pending.borrow_mut().push((scope_id, idx, measured));
        if self.relayout_queued.replace(true) {
            return;
        }
        let this = self.clone();
        runtime_shared::schedule_microtask(move || this.drain());
    }

    /// Report every queued size that differs from the size the list uses
    /// for its row, then invalidate the flow layout once if any did.
    fn drain(&self) {
        self.relayout_queued.set(false);
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        if !*self.alive.borrow() {
            return;
        }
        let mut changed = false;
        for (scope_id, idx, measured) in pending {
            if (measured - (self.item_size)(idx)).abs() <= MEASURE_EPSILON {
                continue;
            }
            // Stored by item key on the vocabulary side; a scope released
            // since the measurement is ignored there.
            (self.set_measured_size)(scope_id, measured);
            changed = true;
        }
        if changed {
            if let Some(cv) = self.collection_view.load() {
                invalidate_item_sizes(&cv);
            }
        }
    }
}

/// What an item view needs to measure the row it hosts.
#[derive(Clone)]
struct RowSlot {
    list: Rc<RowMeasure>,
    scope_id: u64,
    idx: usize,
}

/// Invalidate `cv`'s flow layout so it re-queries
/// `sizeForItemAtIndexPath` for every item. The context's
/// `invalidateFlowLayoutDelegateMetrics` is required: a bare
/// `invalidateLayout` re-positions the flow layout's CACHED item sizes
/// instead of asking for new ones.
fn invalidate_item_sizes(cv: &NSView) {
    let layout: *mut AnyObject = unsafe { msg_send![cv, collectionViewLayout] };
    if layout.is_null() {
        return;
    }
    if let Some(ctx_cls) = AnyClass::get("NSCollectionViewFlowLayoutInvalidationContext") {
        unsafe {
            let ctx: *mut AnyObject = msg_send![ctx_cls, alloc];
            let ctx: *mut AnyObject = msg_send![ctx, init];
            let _: () = msg_send![ctx, setInvalidateFlowLayoutDelegateMetrics: true];
            let _: () = msg_send![layout, invalidateLayoutWithContext: ctx];
            let _: () = msg_send![ctx, release];
        }
    }
}

impl super::MacosBackend {
    /// The natural main-axis size of the detached root registered under
    /// `root_key` at cross size `cross` (see `LayoutTree::measure_main`).
    /// `None` when the key isn't registered. Leaves measuring frames
    /// behind — `layout_detached_root` must run after it.
    fn measure_detached_main(&mut self, root_key: usize, horizontal: bool, cross: f32) -> Option<f32> {
        let (_, node) = self.view_to_layout.get(&root_key)?;
        let node = *node;
        Some(self.layout.measure_main(node, horizontal, cross))
    }
}

/// Lay the row hosted by `host` out at the cell size `size` and, in a
/// Measured list, measure it first and queue its natural size.
fn layout_row_in(b: &mut super::MacosBackend, host: &VirtualizerItemView, size: CGSize) {
    let key = host.ivars().hosted_child_key.get();
    if key == 0 || size.width <= 0.0 || size.height <= 0.0 {
        return;
    }
    let slot = host.ivars().measure.borrow().clone();
    let mut measured = None;
    if let Some(slot) = &slot {
        let cross = if slot.list.horizontal { size.height } else { size.width };
        measured = b.measure_detached_main(key, slot.list.horizontal, cross as f32);
    }
    // After the measurement: it leaves frames for a content-sized box
    // behind, and the cell's box is the one that renders.
    b.layout_detached_root(key, size);
    if let (Some(slot), Some(measured)) = (slot, measured) {
        slot.list.queue_report(slot.scope_id, slot.idx, measured);
    }
}

/// [`layout_row_in`] through the global backend handle — for AppKit
/// callbacks (`setFrameSize:`, the data source's mount). `try_borrow_mut`
/// inside: silently skipped if the backend is mid-pass; that pass's
/// [`super::MacosBackend::relayout_dirty_virtualizer_rows`] picks the row up.
fn layout_hosted_row(host: &VirtualizerItemView, size: CGSize) {
    super::with_global_backend(|b| layout_row_in(b, host, size));
}

impl super::MacosBackend {
    /// Lay out again every mounted row whose Taffy root is dirty, at its
    /// cell's size, re-measuring it in a Measured list. Called from the
    /// backend's layout pass: rows are detached Taffy roots the host-root
    /// compute never reaches, so without this a reactive change inside a
    /// mounted row (a restyle, a text change, a child appearing) left the
    /// row's views at their mount-time frames and its slot at its
    /// mount-time size. Clean rows are skipped, so a pass that touched no
    /// row costs one dirty check per mounted row.
    pub(crate) fn relayout_dirty_virtualizer_rows(&mut self) {
        let mut cells: Vec<Retained<NSView>> = Vec::new();
        for instance in self.virtualizer_instances.values() {
            let ivars = instance.data_source.ivars();
            if !*ivars.alive.borrow() {
                continue;
            }
            for mount in ivars.mounts.borrow().values() {
                if let Some(cell) = unsafe { mount.child.superview() } {
                    cells.push(cell);
                }
            }
        }
        for cell in cells {
            let Some(host) = VirtualizerItemView::from_container(&cell) else {
                continue;
            };
            let key = host.ivars().hosted_child_key.get();
            let Some((_, node)) = self.view_to_layout.get(&key) else {
                continue;
            };
            if !self.layout.is_dirty(*node) {
                continue;
            }
            let bounds: CGRect = unsafe { msg_send![&cell, bounds] };
            layout_row_in(self, host, bounds.size);
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
    /// Measuring state for an `ItemSize::Measured` list; `None` for a
    /// Known list, which never measures. See the module docs.
    measure: Option<Rc<RowMeasure>>,
    /// The item keys the collection view was last given, in order —
    /// `None` until it first asks for its item count. This, not the live
    /// `item_count`, is what `numberOfItemsInSection` answers: a batch
    /// update is checked against the count the collection view had
    /// before it (old count − deletes + inserts must equal the count
    /// after, or AppKit raises), so the data source must not show a
    /// change before the batch that applies it. See [`data_changed`].
    known_keys: Rc<RefCell<Option<Vec<ItemKey>>>>,
    /// A data change is waiting for its deferred apply.
    pending: Cell<bool>,
    /// A row was mounted while a change was pending. Its index path is
    /// the collection view's old one but its content came from the new
    /// data, so a keyed batch would keep a wrong row: the apply reloads.
    mounted_while_pending: Cell<bool>,
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
            let mut known = self.ivars().known_keys.borrow_mut();
            if known.is_none() {
                // First load: the collection view starts from the live data.
                let cb_opt = self.ivars().callbacks.borrow();
                let Some(cb) = cb_opt.as_ref() else {
                    return 0;
                };
                *known = Some(live_keys(cb));
            }
            known.as_ref().map_or(0, |k| k.len() as NSInteger)
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
                // The row's scope is released on a later turn, not here:
                // AppKit ends displaying items synchronously from whatever
                // made it lay out (a `remove_child` taking the collection
                // view out of the window, a screenshot's forced display),
                // and those run under the backend's `borrow_mut()`. The
                // release tears down the row's styles, which borrow the
                // backend again ("RefCell already borrowed", an abort).
                // The row's view is already out of the tree.
                if let Some(release) = release_fn {
                    let scope_id = prev.scope_id;
                    runtime_shared::scheduling::schedule_microtask(move || (release)(scope_id));
                }
            }
        }

        #[method(collectionView:layout:sizeForItemAtIndexPath:)]
        fn size_for_item_at(
            &self,
            cv: &NSObject,
            flow_layout: &NSObject,
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
            // Between a data change and its deferred apply AppKit still
            // has the old count, and a layout in that gap asks about rows
            // the data no longer has.
            if idx >= (cb.item_count)() {
                return CGSize::new(0.0, 0.0);
            }
            let size_f = (cb.item_size)(idx) as CGFloat;

            let bounds: CGRect = unsafe { msg_send![cv, bounds] };
            // The list's own padding (see [`sync_padding`]): the cross
            // axis the rows share is the bounds less its two cross sides.
            let inset: NSEdgeInsets = unsafe { msg_send![flow_layout, sectionInset] };
            let layout = self.ivars().layout;
            // Cross-axis extent divided into `lanes` columns minus
            // inter-lane gaps. One lane = list (cell fills cross axis).
            let usable_cross = if layout.axis.is_horizontal() {
                (bounds.size.height - inset.top - inset.bottom).max(0.0)
            } else {
                (bounds.size.width - inset.left - inset.right).max(0.0)
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

    // ---- Deferred data-change apply (see [`data_changed`]) ----
    unsafe impl VirtualizerDataSource {
        #[method(idealystApplyDataChange:)]
        fn apply_data_change_sel(&self, cv: &NSView) {
            self.apply_data_change(cv);
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
            cb_opt
                .as_ref()
                .map(|c| (c.mount_item.clone(), c.item_key.clone(), (c.item_count)()))
        };
        let Some((mount, key_fn, count)) = mount_fn else {
            return item;
        };
        // A stale row of a change not applied yet (see
        // `size_for_item_at`): an empty item, gone with the apply.
        if idx >= count {
            return item;
        }
        if self.ivars().pending.get() {
            self.ivars().mounted_while_pending.set(true);
        }
        let key = key_fn(idx);
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
        // The row stacks by its item root's z (see "Row stacking"); a
        // reused item drops the previous item's.
        sync_item_z(&container);

        // Hand the host view the subtree's layout key and lay the
        // subtree out right away if the cell already has its size
        // (the reuse path: same-sized cell gets a new mount, so no
        // `setFrameSize:` will fire). Fresh items are still 0×0 here
        // — their first `setFrameSize:` (when the flow layout applies
        // the item's attributes) runs the pass instead.
        let child_key = child_view as *const NSView as usize;
        if let Some(host) = VirtualizerItemView::from_container(&container) {
            host.ivars().hosted_child_key.set(child_key);
            *host.ivars().measure.borrow_mut() = self
                .ivars()
                .measure
                .as_ref()
                .map(|list| RowSlot { list: list.clone(), scope_id, idx });
            layout_hosted_row(host, bounds.size);
        }

        let child_retained: Retained<NSView> = unsafe {
            Retained::retain(child_view as *const NSView as *mut NSView)
                .expect("retain mounted child NSView")
        };
        self.ivars().mounts.borrow_mut().insert(
            item_ptr,
            ItemMount { scope_id, key, child: child_retained },
        );

        item
    }

    fn new(
        mtm: MainThreadMarker,
        callbacks: VirtualizerCallbacks<MacosNode>,
        layout: VirtualLayout,
        collection_view: &NSView,
    ) -> Retained<Self> {
        let alive = Rc::new(RefCell::new(true));
        let measure = callbacks.measure_sizes.then(|| {
            Rc::new(RowMeasure {
                horizontal: layout.axis.is_horizontal(),
                item_size: callbacks.item_size.clone(),
                set_measured_size: callbacks.set_measured_size.clone(),
                collection_view: objc2::rc::Weak::new(collection_view),
                pending: RefCell::new(Vec::new()),
                relayout_queued: Cell::new(false),
                alive: alive.clone(),
            })
        });
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(VirtualizerDataSourceIvars {
            callbacks: Rc::new(RefCell::new(Some(callbacks))),
            mounts: Rc::new(RefCell::new(HashMap::new())),
            alive,
            layout,
            measure,
            known_keys: Rc::new(RefCell::new(None)),
            pending: Cell::new(false),
            mounted_while_pending: Cell::new(false),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Bring the collection view from the keys it knows to the live data:
    /// one `performBatchUpdates` of the keyed diff ([`diff_keys`]), or a
    /// `reloadData` when the diff is unusable. Runs on a run-loop turn of
    /// its own ([`data_changed`]), never under the backend's borrow: both
    /// paths end displaying rows and mount new ones.
    ///
    /// Survivors keep their item, view and row scope: AppKit moves a
    /// batch's untouched items without asking the data source again. A
    /// survivor whose item changed (`item_changed`) is deleted and
    /// inserted, which releases its row and mounts it from the new item.
    ///
    /// The scroll offset is kept across both paths, clamped to the new
    /// content: a reload otherwise lands the clip back at the origin, so a
    /// `scroll_to` made before the apply ran was lost.
    fn apply_data_change(&self, cv: &NSView) {
        self.ivars().pending.set(false);
        let mounted_in_gap = self.ivars().mounted_while_pending.replace(false);
        if !*self.ivars().alive.borrow() {
            return;
        }
        let (new_keys, changed) = {
            let cb_opt = self.ivars().callbacks.borrow();
            let Some(cb) = cb_opt.as_ref() else {
                return;
            };
            (live_keys(cb), cb.item_changed.clone())
        };
        let old_keys = self.ivars().known_keys.borrow().clone();
        // Never loaded: the collection view reads the live data when it
        // first asks.
        let Some(old_keys) = old_keys else {
            return;
        };
        let update = if mounted_in_gap {
            None
        } else {
            diff_keys(
                &old_keys,
                &new_keys,
                |i| changed.as_ref().is_some_and(|f| f(i)),
                MAX_BATCH_OPS,
            )
        };
        if update.as_ref().is_some_and(KeyedUpdate::is_empty) {
            // Same keys, same order, no stale row: nothing to tell AppKit.
            *self.ivars().known_keys.borrow_mut() = Some(new_keys);
            return;
        }
        let offset = ScrollOffset::save(cv);
        match update {
            Some(update) => self.perform_batch(cv, new_keys.clone(), update),
            None => {
                *self.ivars().known_keys.borrow_mut() = Some(new_keys.clone());
                let _: () = unsafe { msg_send![cv, reloadData] };
            }
        }
        self.reindex_mounts(&new_keys);
        if let Some(offset) = offset {
            offset.restore(cv);
        }
    }

    /// Apply `update` as one non-animated batch. The known keys switch to
    /// `new_keys` INSIDE the updates block, which AppKit runs
    /// synchronously: it reads the item count before the block (old) and
    /// after it (new) and checks the ops against both.
    fn perform_batch(&self, cv: &NSView, new_keys: Vec<ItemKey>, update: KeyedUpdate) {
        let paths = |idxs: &mut dyn Iterator<Item = usize>| -> Retained<AnyObject> {
            unsafe {
                let set: Retained<AnyObject> = msg_send_id![objc2::class!(NSMutableSet), set];
                for i in idxs {
                    let path = index_path(i);
                    let _: () = msg_send![&set, addObject: &*path];
                }
                set
            }
        };
        let deletes = paths(&mut update.deletes.iter().copied());
        let inserts = paths(&mut update.inserts.iter().copied());
        let moves: Vec<(Retained<AnyObject>, Retained<AnyObject>)> =
            update.moves.iter().map(|&(o, n)| (index_path(o), index_path(n))).collect();
        let pending_keys = RefCell::new(Some(new_keys));
        let known = self.ivars().known_keys.clone();
        // SAFETY: `cv` is a live collection view borrowed by this frame.
        let cv_owned: Retained<NSView> =
            unsafe { Retained::retain(cv as *const NSView as *mut NSView) }.expect("collection view");
        let block = block2::RcBlock::new(move || {
            let cv = &*cv_owned;
            if let Some(keys) = pending_keys.borrow_mut().take() {
                *known.borrow_mut() = Some(keys);
            }
            unsafe {
                let _: () = msg_send![cv, deleteItemsAtIndexPaths: &*deletes];
                let _: () = msg_send![cv, insertItemsAtIndexPaths: &*inserts];
                for (from, to) in &moves {
                    let _: () = msg_send![cv, moveItemAtIndexPath: &**from, toIndexPath: &**to];
                }
            }
        });
        // Not through `animator`: a direct call applies without
        // animation, as `reloadData` does and as web does.
        let _: () = unsafe {
            msg_send![
                cv,
                performBatchUpdates: &*block,
                completionHandler: Option::<&block2::Block<dyn Fn(objc2::runtime::Bool)>>::None
            ]
        };
    }

    /// Point every mounted row's measuring slot at its key's index in
    /// `keys`. A batch moves rows without remounting them, and a slot's
    /// index is what the measure drain compares sizes against.
    fn reindex_mounts(&self, keys: &[ItemKey]) {
        if self.ivars().measure.is_none() {
            return;
        }
        let index: HashMap<ItemKey, usize> = keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();
        for mount in self.ivars().mounts.borrow().values() {
            let Some(&idx) = index.get(&mount.key) else {
                continue;
            };
            let Some(cell) = (unsafe { mount.child.superview() }) else {
                continue;
            };
            if let Some(host) = VirtualizerItemView::from_container(&cell) {
                if let Some(slot) = host.ivars().measure.borrow_mut().as_mut() {
                    slot.idx = idx;
                }
            }
        }
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
    /// The hosted row's measuring slot in a Measured list; `None` in a
    /// Known list and while nothing is mounted.
    measure: RefCell<Option<RowSlot>>,
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
            layout_hosted_row(self, size);
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
    /// Scroll chaining to the enclosing scroll view (`scroll_chain`).
    chain: Cell<crate::imp::scroll_chain::ScrollChain>,
    /// The clip's cross-axis extent the item sizes were last queried at
    /// (see `tile`); `-1` before the first.
    sized_cross: Cell<f64>,
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
            // The cross extent the items must be sized to. Compared with the
            // one they WERE sized to, not only with the document's frame: the
            // document can reach the clip's size by another path (AppKit
            // resizing it with the clip on a window resize) that never
            // re-queries the item sizes, and the flow layout then centres
            // the old, shorter items in the taller document (a horizontal
            // list's lanes sat 117pt down after the window grew).
            let cross = if horizontal { clip_bounds.size.height } else { clip_bounds.size.width };
            let stale = (self.ivars().sized_cross.get() - cross).abs() > 0.5;
            if (differs || stale) && new_size.width > 0.0 && new_size.height >= 0.0 {
                self.ivars().sized_cross.set(cross);
                if differs {
                    let _: () = unsafe { msg_send![doc, setFrameSize: new_size] };
                }
                // A direct `setFrameSize:` BYPASSES the layout's
                // bounds-change hooks (`shouldInvalidateLayoutForBounds
                // Change:` only fires for clip-driven visible-rect
                // changes), so invalidate explicitly — and with a
                // context that sets `invalidateFlowLayoutDelegateMetrics`,
                // so the item sizes are re-queried at the new width.
                invalidate_item_sizes(unsafe { &*doc });
            }
        }

        // A gesture along the axis the list cannot scroll (a horizontal pan
        // over a vertical list) goes to the enclosing scroll view — see
        // `scroll_chain`.
        #[method(scrollWheel:)]
        fn scroll_wheel(&self, event: &NSEvent) {
            if !crate::imp::scroll_chain::scroll_wheel(self, &self.ivars().chain, event) {
                let _: () = unsafe { msg_send![super(self), scrollWheel: event] };
            }
        }

        // Opt back into responsive scrolling, which AppKit otherwise turns
        // off for a scroll view overriding `scrollWheel:` (`scroll_chain`).
        #[method(isCompatibleWithResponsiveScrolling)]
        fn is_compatible_with_responsive_scrolling() -> bool {
            true
        }
    }
);

impl VirtualizerScrollView {
    fn new(mtm: MainThreadMarker, horizontal: bool) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(VirtualizerScrollViewIvars {
            horizontal: Cell::new(horizontal),
            chain: Cell::default(),
            sized_cross: Cell::new(-1.0),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// `view`'s chaining state, when `view` is a `VirtualizerScrollView`.
    pub(crate) fn chain_of(view: &NSView) -> Option<&Cell<crate::imp::scroll_chain::ScrollChain>> {
        let is: bool = unsafe { msg_send![view, isKindOfClass: Self::class()] };
        if !is {
            return None;
        }
        let typed = unsafe { &*(view as *const NSView as *const VirtualizerScrollView) };
        Some(&typed.ivars().chain)
    }
}

impl VirtualizerItemView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(VirtualizerItemViewIvars {
            hosted_child_key: Cell::new(0),
            measure: RefCell::new(None),
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
// Row stacking — `z_index` on a virtualized item.
//
// On web a virtualized list's rows are plain siblings in one container,
// so a `z_index` on an item's root orders that ROW against its
// neighbours, for paint and for the pointer. Here every row sits inside
// its own item view (`VirtualizerItemView`), and the item views are the
// collection view's subviews: the item root's `zPosition` (written by
// `apply_style` / `AnimProp::ZIndex`) only orders it inside its own item
// view. So the ITEM VIEW carries its hosted item root's z — written when
// the item mounts and when the item's z changes later
// (`sync_item_z_for_child`). Not via the layout attributes' `zIndex`: the
// z belongs to the mounted item's style, which the layout computes
// attributes for BEFORE the item mounts, so feeding it through the layout
// would need a relayout after every mount. A reused item re-reads its NEW
// item's z, so a raised row never leaks its z into the row that reuses
// its view.
//
// Paint then follows Core Animation's sibling sort; clicks follow it
// because the collection view is an `IdealystMacCollectionView`, whose
// `hitTest:` walks the item views in paint order
// (`view::z_ordered_hit_test`) where AppKit's stock walk goes by subview
// index.
// =========================================================================

declare_class!(
    /// The virtualizer's collection view: its `hitTest:` walks the item
    /// views in `z_index` paint order. See "Row stacking".
    pub(crate) struct IdealystMacCollectionView;

    unsafe impl ClassType for IdealystMacCollectionView {
        type Super = objc2_app_kit::NSCollectionView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMacCollectionView";
    }

    impl DeclaredClass for IdealystMacCollectionView {}

    unsafe impl IdealystMacCollectionView {
        #[method_id(hitTest:)]
        fn hit_test(&self, point: objc2_foundation::CGPoint) -> Option<Retained<NSView>> {
            match super::view::z_ordered_hit_test(self, point) {
                Some(resolved) => resolved,
                None => unsafe { msg_send_id![super(self), hitTest: point] },
            }
        }
    }
);

/// Give the item view `container` its hosted item root's `zPosition` (0
/// when it hosts nothing). See "Row stacking".
fn sync_item_z(container: &NSView) {
    let subviews: Retained<objc2_foundation::NSArray<NSView>> =
        unsafe { msg_send_id![container, subviews] };
    let z = subviews
        .iter()
        .next()
        .map(|child| {
            let layer: *mut AnyObject = unsafe { msg_send![child, layer] };
            if layer.is_null() {
                0.0
            } else {
                let z: CGFloat = unsafe { msg_send![layer, zPosition] };
                z
            }
        })
        .unwrap_or(0.0);
    if z != 0.0 {
        let _: () = unsafe { msg_send![container, setWantsLayer: true] };
    }
    let layer: *mut AnyObject = unsafe { msg_send![container, layer] };
    if !layer.is_null() {
        let _: () = unsafe { msg_send![layer, setZPosition: z] };
    }
}

/// After `view`'s own `zPosition` changed: when `view` is a virtualized
/// item's root (hosted in a `VirtualizerItemView`), carry the new z to
/// the item view. No-op for every other view.
pub(crate) fn sync_item_z_for_child(view: &NSView) {
    let container: Option<Retained<NSView>> = unsafe { msg_send_id![view, superview] };
    if let Some(container) = container {
        if VirtualizerItemView::from_container(&container).is_some() {
            sync_item_z(&container);
        }
    }
}

// =========================================================================
// Public entry points.
// =========================================================================

/// The live data's keys, in order.
fn live_keys(cb: &VirtualizerCallbacks<MacosNode>) -> Vec<ItemKey> {
    (0..(cb.item_count)()).map(|i| (cb.item_key)(i)).collect()
}

/// `NSIndexPath` for item `i` of the list's one section.
fn index_path(i: usize) -> Retained<AnyObject> {
    unsafe {
        msg_send_id![
            objc2::class!(NSIndexPath),
            indexPathForItem: i as NSInteger,
            inSection: 0 as NSInteger
        ]
    }
}

/// A collection view's scroll offset (its clip view's bounds origin),
/// saved across a data-change apply.
struct ScrollOffset {
    scroll: Retained<NSView>,
    clip: Retained<NSView>,
    origin: objc2_foundation::CGPoint,
}

impl ScrollOffset {
    fn save(cv: &NSView) -> Option<Self> {
        let scroll: Option<Retained<NSView>> = unsafe { msg_send_id![cv, enclosingScrollView] };
        let scroll = scroll?;
        let clip: Retained<NSView> = unsafe { msg_send_id![&scroll, contentView] };
        let bounds: CGRect = unsafe { msg_send![&clip, bounds] };
        Some(Self { scroll, clip, origin: bounds.origin })
    }

    /// Lay the collection view out at its new content, then put the clip
    /// back at the saved origin, clamped to the new scroll range.
    fn restore(self, cv: &NSView) {
        let _: () = unsafe { msg_send![&self.scroll, layoutSubtreeIfNeeded] };
        let _: () = unsafe { msg_send![cv, layoutSubtreeIfNeeded] };
        let doc: CGRect = unsafe { msg_send![cv, frame] };
        let bounds: CGRect = unsafe { msg_send![&self.clip, bounds] };
        let max_x = (doc.size.width - bounds.size.width).max(0.0);
        let max_y = (doc.size.height - bounds.size.height).max(0.0);
        let target = objc2_foundation::CGPoint {
            x: self.origin.x.clamp(0.0, max_x),
            y: self.origin.y.clamp(0.0, max_y),
        };
        if (target.x - bounds.origin.x).abs() < 0.5 && (target.y - bounds.origin.y).abs() < 0.5 {
            return;
        }
        let _: () = unsafe { msg_send![&self.clip, scrollToPoint: target] };
        let _: () = unsafe { msg_send![&self.scroll, reflectScrolledClipView: &*self.clip] };
    }
}

pub(crate) struct VirtualizerInstance {
    pub(crate) data_source: Retained<VirtualizerDataSource>,
    /// The flow layout; its `sectionInset` carries the list's padding.
    pub(crate) layout: Retained<NSObject>,
    pub(crate) collection_view: Retained<NSView>,
    /// `NSViewBoundsDidChangeNotification` target for the author's
    /// `on_scroll`, when one was supplied. `None` otherwise — no
    /// observer is installed at all, so a virtualizer without
    /// `.on_scroll(..)` costs nothing per scroll event.
    pub(crate) scroll_observer: Option<Retained<NSObject>>,
}

/// Build an `NSScrollView`-wrapped `NSCollectionView` with a
/// `NSCollectionViewFlowLayout` and our custom data source.
/// Returns the outer scroll view as the mountable `NSView`.
pub(crate) fn create(
    mtm: MainThreadMarker,
    instances: &mut HashMap<usize, VirtualizerInstance>,
    mut callbacks: VirtualizerCallbacks<MacosNode>,
    _overscan: f32,
    virt_layout: VirtualLayout,
) -> Retained<NSView> {
    // Taken out before the bundle moves into the data source — the
    // observer watches the outer scroll view, not the collection view,
    // so it has nothing to do with row supply.
    let on_scroll = callbacks.on_scroll.take();
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
    let cv_cls: &AnyClass = IdealystMacCollectionView::class();
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
    let data_source = VirtualizerDataSource::new(mtm, callbacks, virt_layout, &cv);
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
    // Overlay scrollers (`NSScrollerStyleOverlay` = 1), as `create_scroll_view`
    // sets: the legacy style the user's "Show scroll bars" setting can pick
    // reserves a ~15pt gutter in the clip, so a horizontal list lost that much
    // height to its scroller, and its items' cross size no longer matched the
    // laid-out box.
    let _: () = unsafe { msg_send![&scroll, setScrollerStyle: 1isize] };
    // Transparent, like a web scroller: the author's style paints any
    // fill. AppKit's defaults are opaque on all three layers (the
    // scroll view and its clip draw the window background, the
    // collection view `controlBackgroundColor`), which covered whatever
    // the parent painted beneath the row.
    let _: () = unsafe { msg_send![&scroll, setDrawsBackground: false] };
    let clip: *mut NSObject = unsafe { msg_send![&scroll, contentView] };
    if !clip.is_null() {
        let _: () = unsafe { msg_send![clip, setDrawsBackground: false] };
    }
    unsafe {
        let clear: *mut AnyObject = msg_send![objc2::class!(NSColor), clearColor];
        let colors: *mut AnyObject = msg_send![objc2::class!(NSArray), arrayWithObject: clear];
        let _: () = msg_send![&cv, setBackgroundColors: colors];
    }
    let _: () = unsafe { msg_send![&scroll, setDocumentView: &*cv] };

    // Author scroll observer on the SAME clip-view notification channel
    // `create_scroll_view` uses, so a virtualizer and a scroll_view
    // report byte-identical offsets for the same gesture. The target is
    // owned by the instance rather than the backend's global
    // `callback_targets`: a virtualizer's `release` is deterministic
    // (the surrounding scope drops), so the observer can and should die
    // with it instead of accumulating for the process lifetime.
    let scroll_observer = on_scroll
        .and_then(|cb| crate::imp::callbacks::install_scroll_observer(mtm, &scroll, cb));

    let scroll_key = &*scroll as *const NSView as usize;
    instances.insert(
        scroll_key,
        VirtualizerInstance {
            data_source: data_source.clone(),
            layout,
            collection_view: cv,
            scroll_observer,
        },
    );

    scroll
}

/// Schedule the collection view's update to the list's new data. The
/// update itself is [`VirtualizerDataSource::apply_data_change`]: a keyed
/// `performBatchUpdates` that keeps every unchanged surviving row mounted,
/// or a `reloadData` when the diff is unusable.
pub(crate) fn data_changed(scroll_view: &NSView) {
    let doc_view: *mut NSView = unsafe { msg_send![scroll_view, documentView] };
    if doc_view.is_null() {
        return;
    }
    let ds: *mut AnyObject = unsafe { msg_send![doc_view, dataSource] };
    if ds.is_null() {
        return;
    }
    // SAFETY: non-null, checked just above.
    let is_ours = unsafe { (*ds).class() } == VirtualizerDataSource::class();
    if !is_ours {
        return;
    }
    // SAFETY: the class check above.
    let data_source: &VirtualizerDataSource = unsafe { &*(ds as *const VirtualizerDataSource) };
    data_source.ivars().pending.set(true);
    // On a later run-loop turn, not now: this runs under the backend's
    // `borrow_mut()` (the data effect), and applying the change ends
    // displaying rows synchronously and mounts new ones, both of which
    // borrow the backend again ("RefCell already borrowed", an abort).
    // The `virtualizer_data_changed` contract asks for the deferral; a
    // pending apply is replaced, so a burst of changes applies once,
    // against the data as it stands then. Released before it runs, the
    // data source is dead and the apply is a no-op. In the common modes:
    // the default mode alone does not run while the user scrolls or
    // resizes the window.
    extern "C" {
        static NSRunLoopCommonModes: &'static NSString;
    }
    let apply = objc2::sel!(idealystApplyDataChange:);
    unsafe {
        let modes: Retained<AnyObject> =
            msg_send_id![objc2::class!(NSArray), arrayWithObject: NSRunLoopCommonModes];
        let _: () = msg_send![
            objc2::class!(NSObject),
            cancelPreviousPerformRequestsWithTarget: ds,
            selector: apply,
            object: doc_view
        ];
        let _: () = msg_send![
            ds,
            performSelector: apply,
            withObject: doc_view,
            afterDelay: 0.0 as CGFloat,
            inModes: &*modes
        ];
    }
}

/// The list's own `padding_*` as flow-layout section insets. Unset
/// padding is 0; a percent length is 0, as for every other padding the
/// backend realizes outside Taffy (`text_style::length_to_px`).
fn padding_insets(style: &StyleRules) -> NSEdgeInsets {
    let resolve = |t: &Option<runtime_shared::Tokenized<runtime_shared::Length>>| {
        t.as_ref()
            .map(|tok| super::text_style::length_to_px(&tok.resolve()))
            .unwrap_or(0.0)
    };
    NSEdgeInsets {
        top: resolve(&style.padding_top),
        left: resolve(&style.padding_left),
        bottom: resolve(&style.padding_bottom),
        right: resolve(&style.padding_right),
    }
}

/// Realize the virtualizer's own `padding_*` as its flow layout's
/// `sectionInset`. Rows are cells of the collection view, not Taffy
/// children of the list's node, so Taffy's padding on that node reaches
/// no row. The section inset gives the web result: a web virtualizer is a
/// padded scroll container whose rows fill its content box, so the rows
/// sit inside the padding on all four sides and the main-axis padding is
/// part of the scroll extent (the flow layout adds the section's insets
/// to `collectionViewContentSize`). `sizeForItemAtIndexPath` subtracts
/// the cross-axis sides before dividing into lanes. No-op when `view` is
/// not a virtualizer or the insets are unchanged.
pub(crate) fn sync_padding(
    instances: &HashMap<usize, VirtualizerInstance>,
    scroll_view: &NSView,
    style: &StyleRules,
) {
    let key = scroll_view as *const NSView as usize;
    let Some(instance) = instances.get(&key) else {
        return;
    };
    let next = padding_insets(style);
    let current: NSEdgeInsets = unsafe { msg_send![&instance.layout, sectionInset] };
    if current == next {
        return;
    }
    let _: () = unsafe { msg_send![&instance.layout, setSectionInset: next] };
    // The cached item sizes were sized against the old cross extent.
    invalidate_item_sizes(&instance.collection_view);
}

/// Tear down — disconnect the data source so AppKit events stop,
/// drain mounted scopes, drop the instance. Matches iOS's pattern.
pub(crate) fn release(
    instances: &mut HashMap<usize, VirtualizerInstance>,
    scroll_view: &NSView,
) {
    let key = scroll_view as *const NSView as usize;
    let Some(mut instance) = instances.remove(&key) else {
        return;
    };
    // Detach the scroll observer BEFORE anything else. The
    // notification center holds a non-owning reference, so a bounds
    // change delivered after the per-item scopes drain below would
    // call the author's closure against freed signal state — the same
    // use-after-free class the JS-side `_released` guard prevents on
    // web.
    if let Some(target) = instance.scroll_observer.take() {
        let center: *mut objc2::runtime::AnyObject =
            unsafe { msg_send![objc2::class!(NSNotificationCenter), defaultCenter] };
        let _: () = unsafe { msg_send![center, removeObserver: &*target] };
    }
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
    //! caused the actual bugs. Measured item sizes run against a real
    //! window in `tests/virtualizer_measured.rs`.
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
            on_scroll: None,
            item_changed: None,
        }
    }

    /// Callbacks over a shared key list (`item_key(i) == keys[i]`), no rows.
    fn keyed_callbacks(keys: Rc<RefCell<Vec<u64>>>) -> VirtualizerCallbacks<MacosNode> {
        let count_keys = keys.clone();
        VirtualizerCallbacks::<MacosNode> {
            item_count: Rc::new(move || count_keys.borrow().len()),
            item_key: Rc::new(move |i| keys.borrow()[i]),
            ..empty_callbacks()
        }
    }

    fn data_source_of(scroll: &NSView) -> Retained<VirtualizerDataSource> {
        let cv: *mut NSView = unsafe { msg_send![scroll, documentView] };
        let ds: *mut AnyObject = unsafe { msg_send![cv, dataSource] };
        unsafe { Retained::retain(ds as *mut VirtualizerDataSource) }.expect("data source")
    }

    fn known(ds: &VirtualizerDataSource) -> Option<Vec<u64>> {
        ds.ivars().known_keys.borrow().clone()
    }

    /// The batch-update count rule: between a data change and its apply
    /// the collection view must keep answering the count it was last
    /// given (AppKit checks old count − deletes + inserts against the
    /// count after the batch and raises on a mismatch — an abort across
    /// the ObjC frame). The apply then moves it to the new keys, through
    /// a keyed batch here (one key moved, one deleted, one inserted).
    #[test]
    fn regression_item_count_holds_until_the_keyed_apply() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut instances = HashMap::new();
        let keys = Rc::new(RefCell::new(vec![10u64, 11, 12]));
        let scroll = create(mtm, &mut instances, keyed_callbacks(keys.clone()), 1.0, vlist());
        let cv: Retained<NSView> = unsafe { msg_send_id![&scroll, documentView] };
        let ds = data_source_of(&scroll);
        let count = |ds: &VirtualizerDataSource| -> NSInteger {
            unsafe { msg_send![ds, collectionView: &*cv, numberOfItemsInSection: 0 as NSInteger] }
        };

        assert_eq!(known(&ds), None, "nothing known before the first load");
        assert_eq!(count(&ds), 3, "the first load reads the live data");
        assert_eq!(known(&ds), Some(vec![10, 11, 12]));

        *keys.borrow_mut() = vec![12, 10, 13, 14];
        data_changed(&scroll);
        assert!(ds.ivars().pending.get(), "the apply is deferred");
        assert_eq!(count(&ds), 3, "the old count holds until the apply");

        ds.apply_data_change(&cv);
        assert!(!ds.ivars().pending.get());
        assert_eq!(known(&ds), Some(vec![12, 10, 13, 14]), "the batch moved the known keys");
        assert_eq!(count(&ds), 4);
        let cv_count: NSInteger = unsafe { msg_send![&cv, numberOfItemsInSection: 0 as NSInteger] };
        assert_eq!(cv_count, 4, "the collection view took the batch");

        // A full replacement has no survivor: the reload path.
        *keys.borrow_mut() = vec![1, 2];
        ds.apply_data_change(&cv);
        assert_eq!(known(&ds), Some(vec![1, 2]), "the reload moved the known keys");
        release(&mut instances, &scroll);
    }

    /// A change applied after the data source was released is a no-op.
    #[test]
    fn an_apply_after_release_is_a_no_op() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut instances = HashMap::new();
        let keys = Rc::new(RefCell::new(vec![1u64, 2]));
        let scroll = create(mtm, &mut instances, keyed_callbacks(keys.clone()), 1.0, vlist());
        let cv: Retained<NSView> = unsafe { msg_send_id![&scroll, documentView] };
        let ds = data_source_of(&scroll);
        let _: NSInteger = unsafe { msg_send![&*ds, collectionView: &*cv, numberOfItemsInSection: 0 as NSInteger] };
        data_changed(&scroll);
        release(&mut instances, &scroll);
        *keys.borrow_mut() = vec![2, 1];
        ds.apply_data_change(&cv);
        assert_eq!(known(&ds), Some(vec![1, 2]), "a dead list applies nothing");
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

    /// The list's padding lands on the flow layout's section inset, and a
    /// restyle without padding (a variant swap) takes it off again.
    #[test]
    fn virtualizer_padding_follows_the_list_style() {
        use runtime_shared::{Length, StyleRules};
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut instances = HashMap::new();
        let scroll = create(mtm, &mut instances, empty_callbacks(), 1.0, vlist());
        let inset = |instances: &HashMap<usize, VirtualizerInstance>| -> NSEdgeInsets {
            let layout = &instances.values().next().expect("instance").layout;
            unsafe { msg_send![layout, sectionInset] }
        };
        let px = |v: f32| Some(Length::Px(v).into());
        let padded = StyleRules {
            padding_top: px(1.0),
            padding_left: px(2.0),
            padding_bottom: px(3.0),
            padding_right: px(4.0),
            ..Default::default()
        };
        sync_padding(&instances, &scroll, &padded);
        assert_eq!(
            inset(&instances),
            NSEdgeInsets { top: 1.0, left: 2.0, bottom: 3.0, right: 4.0 },
        );
        sync_padding(&instances, &scroll, &StyleRules::default());
        assert_eq!(
            inset(&instances),
            NSEdgeInsets { top: 0.0, left: 0.0, bottom: 0.0, right: 0.0 },
        );
        release(&mut instances, &scroll);
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
