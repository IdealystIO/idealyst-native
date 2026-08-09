//! `virtual_grid` on iOS — a two-axis `UIScrollView` whose visible
//! cells are windowed by the framework.
//!
//! # Why not `UICollectionView`
//!
//! The 1-D [`virtualizer`](super::virtualizer) wraps a
//! `UICollectionView` because UIKit's flow layout gives it native cell
//! recycling for free. That doesn't carry over here: a flow layout
//! scrolls one direction, so a two-axis grid would need a custom
//! `UICollectionViewLayout` subclass — several hundred lines of
//! `layoutAttributesForElements(in:)` re-deriving a visible-rect
//! search that `runtime_shared::primitives::virtual_grid::GridMetrics`
//! already performs, and that the web engine already uses.
//!
//! Re-deriving it is precisely how implementations drift (four copies
//! of the sticky pin math, before `runtime_shared::sticky`). So this
//! backend takes the same architecture as web instead: one scroller
//! with a content extent, cells absolutely positioned inside it, and
//! ONE shared windowing algorithm deciding which cells exist. UIKit
//! diverges in mechanism (`UIScrollView` + `setFrame:` vs a `<div>` +
//! `style.left`); the observable behavior converges (CLAUDE.md §7).
//!
//! The recycling that `UICollectionView` would have provided is
//! already the framework's own contract: `mount_cell` / `release_cell`
//! create and drop per-cell ownership scopes, so a cell leaving the
//! window frees its subtree either way. What UIKit would add is a
//! *view* pool, and with a viewport-bounded mounted set (tens of
//! cells, not thousands) that is not where the cost is.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::msg_send;
use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
use objc2_ui_kit::{UIScrollView, UIView};
use runtime_shared::primitives::virtual_grid::{GridCallbacks, GridMetrics, GridWindow};

use super::IosNode;

/// One mounted cell: the native view plus the framework scope id we
/// must hand back to `release_cell`.
struct MountedCell {
    view: Retained<UIView>,
    scope_id: u64,
}

pub(crate) struct VirtualGridInstance {
    scroll_view: Retained<UIScrollView>,
    callbacks: Rc<RefCell<Option<GridCallbacks<IosNode>>>>,
    metrics: Rc<RefCell<GridMetrics>>,
    /// Mounted cells keyed by `(col, row)` — the slot the window diff
    /// adds and removes, matching the web shim's `_slot`.
    mounted: Rc<RefCell<HashMap<(usize, usize), MountedCell>>>,
    /// Last applied window, so an unchanged one skips the diff
    /// entirely. `None` forces a full re-diff (mount, data change,
    /// bounds change).
    last_window: Rc<RefCell<Option<GridWindow>>>,
    /// Delegate target retained for the grid's lifetime; UIKit holds
    /// the delegate weakly.
    _delegate: Option<Retained<objc2::runtime::NSObject>>,
}

pub(crate) type GridRegistry = HashMap<usize, VirtualGridInstance>;

/// Build the scroller. Cells are NOT mounted here — `create_*` runs
/// under `backend.borrow_mut()` and `mount_cell` re-enters it, the
/// same constraint every backend's virtualizer documents. The first
/// fill happens on the first `sync` (driven by the layout pass, which
/// is also when the viewport size is first known).
pub(crate) fn create(
    mtm: MainThreadMarker,
    registry: &mut GridRegistry,
    callbacks: GridCallbacks<IosNode>,
    _overscan: f32,
) -> Retained<UIView> {
    let scroll = unsafe { UIScrollView::new(mtm) };
    // Both axes bounce, so the grid feels alive even when the content
    // happens to fit one direction — matching `create_scroll_view`'s
    // reasoning, applied to both axes because both scroll here.
    let _: () = unsafe { msg_send![&scroll, setAlwaysBounceHorizontal: true] };
    let _: () = unsafe { msg_send![&scroll, setAlwaysBounceVertical: true] };
    // Same rationale as `create_scroll_view`: the framework's
    // interactive leaves aren't `UIControl`s, so a delayed touch reads
    // as an unpressable cell.
    let _: () = unsafe { msg_send![&scroll, setDelaysContentTouches: false] };

    let metrics = Rc::new(RefCell::new(build_metrics(&callbacks)));
    let callbacks = Rc::new(RefCell::new(Some(callbacks)));

    let key = &*scroll as *const UIScrollView as usize;

    // Scroll delegate: re-window on every scroll, and forward the
    // author's `on_scroll` if there is one. Installed unconditionally
    // because the WINDOWING needs it — unlike the virtualizer, where
    // the observer exists only for the author's callback.
    let delegate = {
        let cb = callbacks.borrow();
        let author = cb.as_ref().and_then(|c| c.on_scroll.clone());
        drop(cb);
        let target = crate::imp::callbacks::ScrollDelegate::new(
            mtm,
            Rc::new(move |x: f32, y: f32| {
                crate::imp::with_backend(|b| sync(b, key));
                if let Some(f) = author.as_ref() {
                    f(x, y);
                }
            }),
        );
        let _: () = unsafe { msg_send![&scroll, setDelegate: &*target] };
        Some(unsafe { Retained::cast::<objc2::runtime::NSObject>(target) })
    };

    registry.insert(
        key,
        VirtualGridInstance {
            scroll_view: scroll.clone(),
            callbacks,
            metrics,
            mounted: Rc::new(RefCell::new(HashMap::new())),
            last_window: Rc::new(RefCell::new(None)),
            _delegate: delegate,
        },
    );

    unsafe { Retained::cast::<UIView>(scroll) }
}

fn build_metrics(cb: &GridCallbacks<IosNode>) -> GridMetrics {
    GridMetrics::build(
        (cb.col_count)(),
        (cb.row_count)(),
        &*cb.col_width,
        &*cb.row_height,
    )
}

/// Counts or sizes changed: rebuild metrics, drop the cached window so
/// the next `sync` re-diffs from scratch, and re-sync now.
pub(crate) fn data_changed(backend: &mut crate::imp::IosBackend, node: &IosNode) {
    let key = node.as_view() as *const UIView as usize;
    {
        let Some(inst) = backend.virtual_grid_registry.get(&key) else {
            return;
        };
        let Some(cb) = inst.callbacks.borrow().as_ref().map(build_metrics) else {
            return;
        };
        *inst.metrics.borrow_mut() = cb;
        *inst.last_window.borrow_mut() = None;
    }
    sync(backend, key);
}

/// Re-window: recompute the visible rect, drop cells that left it,
/// mount cells that entered it, and keep `contentSize` in step.
///
/// Called from the scroll delegate and from the layout pass (the
/// viewport size is only known after layout). Cheap when nothing
/// changed — an unchanged window returns after the `contentSize`
/// write.
pub(crate) fn sync(backend: &mut crate::imp::IosBackend, key: usize) {
    let Some(inst) = backend.virtual_grid_registry.get(&key) else {
        return;
    };
    // Clone the Rc handles so the backend borrow ends before
    // `mount_cell` runs — it realizes a subtree, which re-enters the
    // backend through `create_*`.
    let scroll = inst.scroll_view.clone();
    let callbacks = inst.callbacks.clone();
    let metrics = inst.metrics.clone();
    let mounted = inst.mounted.clone();
    let last_window = inst.last_window.clone();

    let (content_w, content_h) = metrics.borrow().content_size();
    let cur: CGSize = unsafe { msg_send![&scroll, contentSize] };
    if (cur.width - content_w as f64).abs() > 0.5 || (cur.height - content_h as f64).abs() > 0.5 {
        let size = CGSize::new(content_w as f64, content_h as f64);
        let _: () = unsafe { msg_send![&scroll, setContentSize: size] };
    }

    let offset: CGPoint = unsafe { msg_send![&scroll, contentOffset] };
    let bounds: CGRect = unsafe { msg_send![&scroll, bounds] };
    // A zero-sized viewport means layout hasn't run yet; windowing
    // against it would mount nothing and then cache that empty window
    // as "current". Bail without caching so the next pass retries.
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return;
    }

    let overscan = 1.0;
    let window = metrics.borrow().visible_window(
        (offset.x as f32, offset.y as f32),
        (bounds.size.width as f32, bounds.size.height as f32),
        overscan,
    );

    if *last_window.borrow() == Some(window) {
        return;
    }
    *last_window.borrow_mut() = Some(window);

    // Drop cells outside the new window first, so their scopes are
    // freed before the new ones allocate.
    let leaving: Vec<(usize, usize)> = mounted
        .borrow()
        .keys()
        .copied()
        .filter(|(c, r)| !window.contains(*c, *r))
        .collect();
    for slot in leaving {
        let cell = mounted.borrow_mut().remove(&slot);
        if let Some(cell) = cell {
            unsafe { cell.view.removeFromSuperview() };
            let release = callbacks
                .borrow()
                .as_ref()
                .map(|c| c.release_cell.clone());
            if let Some(release) = release {
                crate::imp::ffi_guard::guard_ffi("virtual_grid::release_cell", || {
                    release(cell.scope_id)
                });
            }
        }
    }

    // Mount cells that entered.
    for (col, row) in window.cells() {
        if mounted.borrow().contains_key(&(col, row)) {
            continue;
        }
        let mount = callbacks.borrow().as_ref().map(|c| c.mount_cell.clone());
        let Some(mount) = mount else { break };
        let (node, scope_id) =
            crate::imp::ffi_guard::guard_ffi("virtual_grid::mount_cell", || mount(col, row));
        let view = node.as_view();
        let (x, y) = metrics.borrow().cell_origin(col, row);
        let (w, h) = metrics.borrow().cell_size(col, row);
        let frame = CGRect {
            origin: CGPoint { x: x as f64, y: y as f64 },
            size: CGSize::new(w as f64, h as f64),
        };
        let _: () = unsafe { msg_send![view, setFrame: frame] };
        unsafe { scroll.addSubview(view) };
        mounted.borrow_mut().insert(
            (col, row),
            MountedCell {
                view: unsafe {
                    Retained::retain(view as *const UIView as *mut UIView)
                        .expect("retain grid cell")
                },
                scope_id,
            },
        );
    }
}

/// Re-window every registered grid. Called from the layout pass, which
/// is when a grid first learns its viewport size and when a resize
/// changes it.
pub(crate) fn sync_all(backend: &mut crate::imp::IosBackend) {
    let keys: Vec<usize> = backend.virtual_grid_registry.keys().copied().collect();
    for key in keys {
        // The cached window is keyed to a viewport size, so a layout
        // pass must invalidate it — otherwise a resize that leaves the
        // window's INDICES unchanged would skip the cell re-frame.
        if let Some(inst) = backend.virtual_grid_registry.get(&key) {
            *inst.last_window.borrow_mut() = None;
        }
        sync(backend, key);
    }
}

/// Tear down: detach the delegate so queued scroll events stop, then
/// drain every mounted cell's scope. Order matters — a scroll event
/// delivered mid-drain would call `mount_cell` against a half-freed
/// registry entry.
pub(crate) fn release(backend: &mut crate::imp::IosBackend, node: &IosNode) {
    let key = node.as_view() as *const UIView as usize;
    let Some(inst) = backend.virtual_grid_registry.remove(&key) else {
        return;
    };
    let _: () = unsafe { msg_send![&inst.scroll_view, setDelegate: std::ptr::null::<UIView>()] };

    // Take the callbacks OUT before draining so a late event sees
    // `None` and bails instead of reaching into freed framework state
    // — the same guard the 1-D data source uses.
    let cbs = inst.callbacks.borrow_mut().take();
    let release_cell = cbs.as_ref().map(|c| c.release_cell.clone());
    let drained: Vec<MountedCell> = inst.mounted.borrow_mut().drain().map(|(_, v)| v).collect();
    for cell in drained {
        unsafe { cell.view.removeFromSuperview() };
        if let Some(release) = release_cell.as_ref() {
            crate::imp::ffi_guard::guard_ffi("virtual_grid::release (teardown)", || {
                release(cell.scope_id)
            });
        }
    }
}

/// Imperative handle: the node IS the scroller, so offsets are plain
/// `contentOffset` reads/writes — the same surface `IosScrollViewOps`
/// and `IosVirtualizerOps` use, so all three report identically.
pub(crate) struct IosVirtualGridOps;

impl runtime_shared::primitives::virtual_grid::VirtualGridOps for IosVirtualGridOps {
    fn scroll_to_cell(&self, node: &dyn std::any::Any, col: usize, row: usize) {
        let Some(n) = node.downcast_ref::<IosNode>() else {
            return;
        };
        let key = n.as_view() as *const UIView as usize;
        // The origin comes from the LIVE metrics — column widths may
        // have changed since mount.
        let origin = crate::imp::with_backend(|b| {
            b.virtual_grid_registry
                .get(&key)
                .map(|i| i.metrics.borrow().cell_origin(col, row))
        })
        .flatten();
        let Some((x, y)) = origin else { return };
        set_offset(n, x, y);
    }

    fn scroll_offset(&self, node: &dyn std::any::Any) -> (f32, f32) {
        let Some(n) = node.downcast_ref::<IosNode>() else {
            return (0.0, 0.0);
        };
        let offset: CGPoint = unsafe { msg_send![n.as_view(), contentOffset] };
        (offset.x as f32, offset.y as f32)
    }

    fn scroll_to(&self, node: &dyn std::any::Any, x: f32, y: f32) {
        if let Some(n) = node.downcast_ref::<IosNode>() {
            set_offset(n, x, y);
        }
    }
}

fn set_offset(node: &IosNode, x: f32, y: f32) {
    let offset = CGPoint { x: x as f64, y: y as f64 };
    let _: () = unsafe { msg_send![node.as_view(), setContentOffset: offset, animated: false] };
}

pub(crate) static IOS_VIRTUAL_GRID_OPS: IosVirtualGridOps = IosVirtualGridOps;

pub(crate) fn make_handle(
    node: &IosNode,
) -> runtime_shared::primitives::virtual_grid::VirtualGridHandle {
    runtime_shared::primitives::virtual_grid::VirtualGridHandle::new(
        Rc::new(node.clone()),
        &IOS_VIRTUAL_GRID_OPS,
    )
}
