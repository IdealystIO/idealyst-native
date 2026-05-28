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

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol};
use objc2::{declare_class, mutability, ClassType, DeclaredClass};
use objc2_app_kit::NSView;
use objc2_foundation::{
    CGFloat, CGRect, CGSize, MainThreadMarker, NSInteger, NSObject, NSString,
};
use runtime_core::VirtualizerCallbacks;

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
    /// `true` for horizontal scrolling; swaps the main / cross
    /// axes in `sizeForItemAt`.
    horizontal: bool,
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
            if self.ivars().horizontal {
                CGSize::new(size_f, bounds.size.height.max(0.0))
            } else {
                CGSize::new(bounds.size.width.max(0.0), size_f)
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
        horizontal: bool,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(VirtualizerDataSourceIvars {
            callbacks: Rc::new(RefCell::new(Some(callbacks))),
            mounts: Rc::new(RefCell::new(HashMap::new())),
            alive: Rc::new(RefCell::new(true)),
            horizontal,
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
            unsafe { mount.child.removeFromSuperview() };
            if let Some(ref release) = release_fn {
                (release)(mount.scope_id);
            }
        }
        *self.ivars().callbacks.borrow_mut() = None;
    }
}

// =========================================================================
// VirtualizerItem — NSCollectionViewItem subclass. We don't override
// any methods today; the subclass exists solely to give us a stable
// class name for `registerClass:forItemWithIdentifier:`. Items are
// instantiated by NSCollectionView via `+alloc] init]` against this
// class whenever the reuse pool needs a fresh one.
//
// NSCollectionViewItem extends NSViewController in modern AppKit;
// declaring `Super = NSObject` and letting the runtime resolve
// dispatch against the actual class (registered via
// `registerClass:forItemWithIdentifier:`) keeps the objc2 binding
// surface minimal. UIKit's UICollectionViewCell takes the same
// trick in `imp/virtualizer.rs`.
// =========================================================================

declare_class!(
    pub(crate) struct VirtualizerItem;

    unsafe impl ClassType for VirtualizerItem {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMacVirtualizerItem";
    }

    impl DeclaredClass for VirtualizerItem {}
);

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
    horizontal: bool,
) -> Retained<NSView> {
    // ── Flow layout ────────────────────────────────────────────────
    let layout_cls: &AnyClass = AnyClass::get("NSCollectionViewFlowLayout")
        .expect("NSCollectionViewFlowLayout class not registered");
    let layout: Retained<NSObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![layout_cls, alloc];
        let inited: *mut AnyObject = msg_send![allocated, init];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("NSCollectionViewFlowLayout init returned nil")
    };
    // NSCollectionViewScrollDirection: 0 = vertical, 1 = horizontal.
    // Matches UIKit's UICollectionViewScrollDirection numeric values.
    let scroll_direction: i64 = if horizontal { 1 } else { 0 };
    let _: () = unsafe { msg_send![&layout, setScrollDirection: scroll_direction] };
    let _: () = unsafe { msg_send![&layout, setMinimumLineSpacing: 0.0 as CGFloat] };
    let _: () = unsafe { msg_send![&layout, setMinimumInteritemSpacing: 0.0 as CGFloat] };

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
    let data_source = VirtualizerDataSource::new(mtm, callbacks, horizontal);
    let _: () = unsafe { msg_send![&cv, setDataSource: &*data_source] };
    let _: () = unsafe { msg_send![&cv, setDelegate: &*data_source] };

    // ── Scroll view ───────────────────────────────────────────────
    // NSCollectionView itself doesn't scroll — it relies on being
    // hosted in an NSScrollView whose documentView is the collection
    // view. Without this wrap, content overflowing the visible
    // bounds is clipped without any scroll affordance.
    let scroll_cls: &AnyClass = AnyClass::get("NSScrollView")
        .expect("NSScrollView class not registered");
    let scroll: Retained<NSView> = unsafe {
        let allocated: *mut AnyObject = msg_send![scroll_cls, alloc];
        let inited: *mut AnyObject = msg_send![allocated, initWithFrame: zero_rect];
        Retained::from_raw(inited.cast::<NSView>())
            .expect("NSScrollView init returned nil")
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
