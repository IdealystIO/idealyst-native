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
                // `with_backend` has returned, so its borrow is gone —
                // one of the two seams where queued mounts may run.
                drain_pending();
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

/// Everything one sync touches, with no backend borrow among them.
///
/// The split this type exists for is the whole point of the module's
/// deferral: the ONLY thing a sync needs the backend for is finding the
/// instance in `virtual_grid_registry`. Once the handles are cloned out,
/// the diff is pure UIKit plus framework `Rc`s.
#[derive(Clone)]
struct GridHandles {
    scroll: Retained<UIScrollView>,
    callbacks: Rc<RefCell<Option<GridCallbacks<IosNode>>>>,
    metrics: Rc<RefCell<GridMetrics>>,
    mounted: Rc<RefCell<HashMap<(usize, usize), MountedCell>>>,
    last_window: Rc<RefCell<Option<GridWindow>>>,
}

/// A grid's cells, taken out of the registry, waiting to be dropped.
struct TeardownJob {
    cells: Vec<MountedCell>,
    release_cell: Option<Rc<dyn Fn(u64)>>,
}

enum PendingJob {
    Sync(usize, GridHandles),
    Teardown(TeardownJob),
}

thread_local! {
    /// Work that must run with NO backend borrow held.
    ///
    /// # Why this queue exists
    ///
    /// `mount_cell` realizes a subtree and `release_cell` drops one, and
    /// both re-enter the backend through `create_*` / scope cleanups —
    /// `handlers::view::mount_view` opens with `backend.borrow_mut()`.
    /// Every path that reaches a sync is already holding that borrow:
    /// the layout pass runs inside `drain_queued_layout_pass`'s `RefMut`,
    /// `virtual_grid_data_changed` is dispatched through
    /// `backend.borrow_mut()`, and the scroll delegate goes through
    /// `with_backend`. Cloning the instance's `Rc`s — which this module
    /// used to do, with a comment saying it ended the borrow — does not
    /// end it: the borrow belongs to the CALLER.
    ///
    /// So the borrow-free half is queued here and drained at the two
    /// seams where the borrow is provably gone (see `drain_pending`).
    /// The cost is that cells mount one pass later than the layout that
    /// discovered them; they mark the tree dirty on mount, so the
    /// following pass frames them. That was already true of the first
    /// fill, which `create` deliberately deferred for this same reason.
    ///
    /// Found 2026-09-14: with `GridOps` wired up on iOS for the first
    /// time, the first schedule grid to mount aborted the app with
    /// `RefCell already borrowed` inside `virtual_grid::mount_cell`.
    static PENDING: RefCell<Vec<PendingJob>> = const { RefCell::new(Vec::new()) };

    /// Mounted cells' view pointers → the box this engine framed them
    /// at, keyed the way `layout_for_view` keys `view_to_layout`.
    ///
    /// # Why the layout pass needs to be told
    ///
    /// A mounted cell's root view is registered with Taffy like any
    /// other view, and it has no Taffy PARENT — the cell is positioned
    /// by this engine in the scroller's content space, not by the
    /// layout tree. That makes every cell a Taffy ROOT, and
    /// `run_layout_pass_global` does two things to a root that are
    /// exactly wrong for a cell: it computes it against the VIEWPORT,
    /// and it then writes the resulting frame over whatever the view
    /// had. A 40×44 cell came back 40×956 at the origin — every cell in
    /// the grid stacked in one column, which is what the first working
    /// mount on iOS actually looked like.
    ///
    /// So the pass consults this map: a root in here is computed against
    /// its own box instead of the viewport, and its frame is left alone
    /// (its children still get theirs, relative to it). The cell's
    /// SUBTREE still lays out properly — it just lays out inside the
    /// cell rather than inside the screen.
    static CELL_BOXES: RefCell<HashMap<usize, (f32, f32)>> = RefCell::new(HashMap::new());
}

/// The box a mounted grid cell was framed at, or `None` for any view
/// that is not a grid cell root. Consulted by `run_layout_pass_global`.
pub(crate) fn cell_box(view_key: usize) -> Option<(f32, f32)> {
    CELL_BOXES.with(|m| m.borrow().get(&view_key).copied())
}

/// Queue a sync, replacing any already queued for the same grid — the
/// diff is computed at drain time, so the later one subsumes the
/// earlier and running both would only repeat an unchanged window.
fn queue_sync(key: usize, handles: GridHandles) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if let Some(slot) = p.iter_mut().find(
            |j| matches!(j, PendingJob::Sync(k, _) if *k == key),
        ) {
            *slot = PendingJob::Sync(key, handles);
        } else {
            p.push(PendingJob::Sync(key, handles));
        }
    });
    // Guarantee a drain even if nothing else schedules one. Cheap: the
    // pass is coalesced and drops itself if the backend is gone.
    // NO `schedule_layout_pass()` here, and the omission is
    // load-bearing. `sync_all` queues EVERY registered grid on every
    // layout pass, so a schedule from this function makes each pass
    // arm the next one and the app spins the main thread forever —
    // measured at 25,488 passes in 90s, a constant view count, taps
    // and the robot bridge both dead. The queue does not need it: the
    // pass that queued this drains it on the way out, and the mounts
    // that drain performs dirty the tree themselves, which schedules
    // the pass that frames them. That settles, because a re-queued
    // sync whose window has not moved returns without mounting.
}

/// Run the queued mounts, releases and teardowns.
///
/// **Call only where the backend borrow is provably released.** Today
/// that is `drain_queued_layout_pass`, immediately after the `RefMut`
/// falls out of scope, and the scroll delegate once `with_backend` has
/// returned. Calling it under a borrow puts back the exact abort the
/// queue exists to prevent.
///
/// Loops rather than draining once: a teardown's scope cleanups can
/// queue another grid's sync. Bounded, because each pass either empties
/// the queue or the source of the refills is a bug worth stopping on
/// rather than spinning in.
pub(crate) fn drain_pending() {
    for _ in 0..8 {
        let jobs: Vec<PendingJob> = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
        if jobs.is_empty() {
            return;
        }
        for job in jobs {
            match job {
                PendingJob::Sync(_, handles) => sync_now(&handles),
                PendingJob::Teardown(job) => teardown_now(job),
            }
        }
    }
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
    // The one queueing path with no layout pass already behind it, so
    // it arms the drain itself. Safe where `queue_sync` is not: this
    // fires on a real data change, not once per pass.
    crate::imp::schedule_layout_pass();
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
    // The registry lookup is the ONLY part that needs the backend, and
    // the caller is holding its borrow. Clone the handles and hand the
    // rest to the queue — `mount_cell` re-enters the backend, so it
    // cannot run from here. See `PENDING`.
    queue_sync(
        key,
        GridHandles {
            scroll: inst.scroll_view.clone(),
            callbacks: inst.callbacks.clone(),
            metrics: inst.metrics.clone(),
            mounted: inst.mounted.clone(),
            last_window: inst.last_window.clone(),
        },
    );
}

/// One grid's re-window, with no backend borrow held. The body of the
/// old `sync`; see [`PENDING`] for why it is reached through a queue.
fn sync_now(h: &GridHandles) {
    // Cloned / reborrowed one at a time rather than destructured: the
    // `msg_send!` sites want a `&Retained<_>`, which a `&GridHandles`
    // field is not.
    let scroll = h.scroll.clone();
    let callbacks = &h.callbacks;
    let metrics = &h.metrics;
    let mounted = &h.mounted;
    let last_window = &h.last_window;

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
            CELL_BOXES.with(|m| {
                m.borrow_mut()
                    .remove(&(&*cell.view as *const UIView as usize))
            });
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
        // Tell the layout pass this root is a cell, not a screen — see
        // `CELL_BOXES`. Recorded BEFORE the view is attached, so the
        // pass a mount schedules cannot see it un-registered.
        CELL_BOXES.with(|m| {
            m.borrow_mut()
                .insert(view as *const UIView as usize, (w, h))
        });
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

/// Drop a torn-down grid's cells, with no backend borrow held.
///
/// Separated from [`release`] for the same reason as [`sync_now`]:
/// dropping a cell scope runs its cleanups, and a cleanup that touches
/// the backend re-enters the borrow the caller is holding. That is the
/// two-axis form of the virtualizer teardown bug (FRAMEWORK-NOTES
/// Wave-19).
fn teardown_now(job: TeardownJob) {
    for cell in job.cells {
        CELL_BOXES.with(|m| {
            m.borrow_mut()
                .remove(&(&*cell.view as *const UIView as usize))
        });
        unsafe { cell.view.removeFromSuperview() };
        if let Some(release) = job.release_cell.as_ref() {
            crate::imp::ffi_guard::guard_ffi("virtual_grid::release (teardown)", || {
                release(cell.scope_id)
            });
        }
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
    let cells: Vec<MountedCell> = inst.mounted.borrow_mut().drain().map(|(_, v)| v).collect();
    // Queued, not run: this is called under the caller's backend
    // borrow, and dropping a cell scope re-enters it. Any sync still
    // queued for this grid runs first and finds `callbacks` already
    // `None`, so it mounts nothing into a half-freed instance.
    PENDING.with(|p| {
        p.borrow_mut()
            .push(PendingJob::Teardown(TeardownJob { cells, release_cell }))
    });
    crate::imp::schedule_layout_pass();
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
