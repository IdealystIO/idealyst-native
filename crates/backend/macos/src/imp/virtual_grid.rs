//! `virtual_grid` on macOS — a two-axis `NSScrollView` whose visible
//! cells are windowed by the framework.
//!
//! AppKit twin of `backend-ios-mobile/src/imp/virtual_grid.rs`; read
//! that module's header for why this backend does NOT wrap a
//! collection view (short version: a two-axis collection layout would
//! mean re-deriving the visible-rect search that
//! `runtime_shared::primitives::virtual_grid::GridMetrics` already
//! owns, and re-derivation is how backends drift apart).
//!
//! macOS divergences in MECHANISM only:
//!
//! - **Scroll signal**: iOS gets `scrollViewDidScroll:` from a
//!   delegate. `NSScrollView` has no delegate, so this rides the same
//!   `NSViewBoundsDidChangeNotification` channel on the clip view that
//!   `create_scroll_view` and `sticky` already use — installed through
//!   the shared `callbacks::install_scroll_observer`, so all three
//!   report offsets identically.
//! - **Content extent**: iOS sets `contentSize` on the scroller;
//!   AppKit's scrollable extent is its documentView's FRAME, so the
//!   engine resizes a flipped document view instead.
//! - **Flipped coordinates**: the document view is a
//!   `ScrollDocumentView` (`isFlipped == true`), so cell origins are
//!   top-left and the y arithmetic matches web/iOS with no sign flip.
//!   Without it every cell would be positioned bottom-up and the grid
//!   would render upside down.
//!
//! # Frozen panes
//!
//! AppKit holds frozen cells still itself — the mechanism `NSTableView`'s
//! header uses — so no cell moves while the grid scrolls:
//!
//! - a cell frozen on ONE axis (a header cell, a pinned column's cell) is a
//!   floating subview of the scroll view (`addFloatingSubview:forAxis:`),
//!   which keeps its document-space frame but stays fixed on that axis;
//! - a cell frozen on BOTH (a corner) is a plain subview of the scroll view,
//!   above the clip view, fixed on both.
//!
//! Re-placing frozen cells from the scroll notification instead trails the
//! scroll: AppKit scrolls asynchronously (responsive scrolling, which the
//! document view keeps on — see `ScrollDocumentView`) and tells the main
//! thread afterwards, so headers rode along with the rows and snapped back.
//! In every one of those coordinate spaces a cell's frame is
//! `GridMetrics::display_origin` at a ZERO scroll offset: the scrolling axis
//! keeps document coordinates, the fixed one is the viewport's.
//!
//! The containers also give the z-order: floating subviews draw (and
//! hit-test) above the document, scroll-view subviews above both.
//!
//! # Cell reuse
//!
//! With a [`GridRecycling`] (a grid built with a reusable renderer), a cell
//! that leaves the window is PARKED — kept, views and all, off in its old
//! position — under its (reuse class, bands), and a cell that enters takes a
//! parked one of the same key: it is rebound to the new position, placed and
//! laid out, instead of mounted. Rebinds are batched and flushed once
//! (`newcore::flush_now`) before the batch is laid out, so a reused cell
//! never draws its old row in its new place. Bands are part of the key
//! because a frozen cell lives in another container (module docs above).
//! Whatever is still parked once a sync completes is released.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_app_kit::NSView;
use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSObject};
use runtime_shared::primitives::virtual_grid::{
    Band, CellKey, FrozenPanes, GridCallbacks, GridMetrics, GridRecycling, PaneWindow, Scrollport,
};

use super::MacosNode;

struct MountedCell {
    view: Retained<NSView>,
    scope_id: u64,
    /// The content key this cell was mounted for. `cell_key` is
    /// content-addressed precisely so a pooled cell cannot survive a
    /// data change with stale contents. Keying `mounted` by `(col, row)`
    /// alone and skipping any filled slot would leave a cell whose
    /// CONTENT changed showing its old view until it scrolled out of the
    /// window.
    key: CellKey,
    /// `(column band, row band)` it was inserted for — decides its
    /// subview order and whether a scroll re-frames it.
    bands: (Band, Band),
}

pub(crate) struct VirtualGridInstance {
    scroll_view: Retained<NSView>,
    document_view: Retained<NSView>,
    callbacks: Rc<RefCell<Option<GridCallbacks<MacosNode>>>>,
    metrics: Rc<RefCell<GridMetrics>>,
    mounted: Rc<RefCell<HashMap<(usize, usize), MountedCell>>>,
    last_window: Rc<RefCell<Option<PaneWindow>>>,
    frozen: Rc<std::cell::Cell<FrozenPanes>>,
    /// `VirtualGridHandle::keep_row_mounted`'s row.
    kept_row: Rc<std::cell::Cell<Option<usize>>>,
    /// Buffer outside the visible window, in viewport extents — the
    /// author's `.overscan(..)`.
    overscan: f32,
    incomplete: Rc<std::cell::Cell<bool>>,
    follow_up: Rc<std::cell::Cell<bool>>,
    retiring: Rc<RefCell<Vec<MountedCell>>>,
    recycling: Rc<RefCell<Option<GridRecycling>>>,
    pool: Rc<RefCell<HashMap<(u64, (Band, Band)), Vec<MountedCell>>>>,
    /// Bounds-change observer; the notification center holds it
    /// non-owningly, so the instance must.
    observer: Option<Retained<NSObject>>,
    /// The thickness a scroller takes from the box right now — what the
    /// intrinsic-size measure_fn reads (`grid_intrinsic_measure_live`).
    /// `0` under overlay scrollers, the legacy scroller's width under the
    /// legacy style. Kept current by [`scroller_style_changed`].
    gutter: Rc<std::cell::Cell<f32>>,
    /// Observer for the scroller-style preference change; same ownership
    /// as `observer`.
    style_observer: Option<Retained<NSObject>>,
}

pub(crate) type GridRegistry = HashMap<usize, VirtualGridInstance>;

pub(crate) fn create(
    mtm: MainThreadMarker,
    registry: &mut GridRegistry,
    callbacks: GridCallbacks<MacosNode>,
    overscan: f32,
) -> Retained<NSView> {
    let document_view: Retained<NSView> =
        Retained::into_super(crate::imp::view::ScrollDocumentView::new(mtm));

    let scroll: Retained<NSView> = unsafe {
        let allocated: *mut objc2::runtime::AnyObject =
            msg_send![objc2::class!(NSScrollView), alloc];
        let zero = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize::new(0.0, 0.0),
        };
        let inited: *mut objc2::runtime::AnyObject = msg_send![allocated, initWithFrame: zero];
        Retained::from_raw(inited.cast::<NSView>()).expect("NSScrollView init returned nil")
    };
    // Both axes scroll here, so both scrollers exist — unlike
    // `create_scroll_view`, which enables exactly one.
    let _: () = unsafe { msg_send![&scroll, setHasVerticalScroller: true] };
    let _: () = unsafe { msg_send![&scroll, setHasHorizontalScroller: true] };
    let _: () = unsafe { msg_send![&scroll, setAutohidesScrollers: true] };
    // Overlay scrollers (NSScrollerStyleOverlay = 1) reserve no width,
    // so a cell's full column width stays visible instead of sliding
    // under a legacy scroller's gutter — the same reasoning
    // `create_scroll_view` documents at length.
    let _: () = unsafe { msg_send![&scroll, setScrollerStyle: 1isize] };
    let _: () = unsafe { msg_send![&scroll, setDrawsBackground: false] };
    let clip: *mut NSObject = unsafe { msg_send![&scroll, contentView] };
    if !clip.is_null() {
        let _: () = unsafe { msg_send![clip, setDrawsBackground: false] };
    }
    let _: () = unsafe { msg_send![&scroll, setDocumentView: &*document_view] };

    let metrics = Rc::new(RefCell::new(build_metrics(&callbacks)));
    let author_on_scroll = callbacks.on_scroll.clone();
    let callbacks = Rc::new(RefCell::new(Some(callbacks)));

    let key = &*scroll as *const NSView as usize;

    // Re-window on every scroll, then forward the author's callback.
    // Installed unconditionally: the WINDOWING needs the signal even
    // when the author asked for nothing.
    let observer = crate::imp::callbacks::install_scroll_observer(
        mtm,
        &scroll,
        Rc::new(move |x: f32, y: f32| {
            crate::imp::with_backend(|b| sync(b, key));
            // `with_backend` has returned, so its borrow is gone — one
            // of the seams where queued mounts may run.
            drain_pending();
            if let Some(f) = author_on_scroll.as_ref() {
                f(x, y);
            }
        }),
    );

    // "Show scroll bars" changed (or a mouse was plugged in under
    // "Automatic"): AppKit restyles every scroll view — this one too,
    // despite the overlay style forced above — and a LEGACY scroller then
    // takes its thickness from the clip view. Re-measure, or a grid
    // fitted to its rows loses the last one under the horizontal bar.
    let style_observer = crate::imp::callbacks::observe_notification(
        mtm,
        "NSPreferredScrollerStyleDidChangeNotification",
        std::ptr::null_mut(),
        Rc::new(move || crate::imp::with_backend(|b| scroller_style_changed(b, key))),
    );

    registry.insert(
        key,
        VirtualGridInstance {
            scroll_view: scroll.clone(),
            document_view,
            callbacks,
            metrics,
            mounted: Rc::new(RefCell::new(HashMap::new())),
            last_window: Rc::new(RefCell::new(None)),
            frozen: Rc::new(std::cell::Cell::new(FrozenPanes::NONE)),
            kept_row: Rc::new(std::cell::Cell::new(None)),
            overscan,
            incomplete: Rc::new(std::cell::Cell::new(false)),
            follow_up: Rc::new(std::cell::Cell::new(false)),
            retiring: Rc::new(RefCell::new(Vec::new())),
            recycling: Rc::new(RefCell::new(None)),
            pool: Rc::new(RefCell::new(HashMap::new())),
            observer,
            gutter: Rc::new(std::cell::Cell::new(scroller_gutter(&scroll))),
            style_observer: Some(style_observer),
        },
    );

    scroll
}

fn build_metrics(cb: &GridCallbacks<MacosNode>) -> GridMetrics {
    GridMetrics::build(
        (cb.col_count)(),
        (cb.row_count)(),
        &*cb.col_width,
        &*cb.row_height,
    )
}

/// The grid's live metrics cell — the one [`data_changed`] rebuilds —
/// for the intrinsic-size measure_fn `create_virtual_grid_impl`
/// installs (`runtime_layout::grid_intrinsic_measure`).
pub(crate) fn live_metrics(registry: &GridRegistry, view: &NSView) -> Option<Rc<RefCell<GridMetrics>>> {
    registry
        .get(&(view as *const NSView as usize))
        .map(|inst| inst.metrics.clone())
}

/// The grid's live gutter cell, for the measure_fn
/// `create_virtual_grid_impl` installs.
pub(crate) fn live_gutter(registry: &GridRegistry, view: &NSView) -> Option<Rc<std::cell::Cell<f32>>> {
    registry
        .get(&(view as *const NSView as usize))
        .map(|inst| inst.gutter.clone())
}

/// The room one scroller takes from the box under `scroll`'s CURRENT
/// style: `0` for overlay scrollers, which float over the content, and
/// the legacy scroller's width for its control size otherwise — exactly
/// what `NSScrollView` tiles out of the clip view (pinned by
/// `regression_fit_content_grid_keeps_its_last_row_under_legacy_scrollers`).
fn scroller_gutter(scroll: &NSView) -> f32 {
    let style: isize = unsafe { msg_send![scroll, scrollerStyle] };
    // NSScrollerStyleLegacy = 0, NSScrollerStyleOverlay = 1.
    if style != 0 {
        return 0.0;
    }
    let scroller: *mut objc2::runtime::AnyObject = unsafe { msg_send![scroll, horizontalScroller] };
    let control_size: usize = if scroller.is_null() { 0 } else { unsafe { msg_send![scroller, controlSize] } };
    let width: f64 = unsafe {
        msg_send![objc2::class!(NSScroller), scrollerWidthForControlSize: control_size, scrollerStyle: 0isize]
    };
    width as f32
}

/// The scroller style of grid `key` may have changed: re-read the gutter
/// and, if it moved, dirty the node so the next pass re-measures — the
/// same re-measure a data change triggers.
pub(crate) fn scroller_style_changed(backend: &mut crate::imp::MacosBackend, key: usize) {
    let Some(inst) = backend.virtual_grid_registry.get(&key) else {
        return;
    };
    let gutter = scroller_gutter(&inst.scroll_view);
    if inst.gutter.replace(gutter) == gutter {
        return;
    }
    let view = inst.scroll_view.clone();
    let layout = backend.layout_for_view(&view);
    backend.layout.mark_dirty(layout);
    crate::imp::schedule_layout_pass();
}

pub(crate) fn data_changed(backend: &mut crate::imp::MacosBackend, node: &MacosNode) {
    data_changed_in(&mut backend.virtual_grid_registry, node);
    // New counts/sizes are a new intrinsic size: flag the node so the
    // pass armed below re-runs its measure_fn, or a fit-to-content grid
    // keeps its old (often empty → 0) height after an async load.
    if let MacosNode::View(view) = node {
        let layout = backend.layout_for_view(view);
        backend.layout.mark_dirty(layout);
    }
    // The one queueing path with no layout pass already behind it, so
    // it arms the drain itself. Safe where `queue_sync` is not: this
    // fires on a real data change, not once per pass.
    crate::imp::schedule_layout_pass();
}

/// [`data_changed`] against the registry alone, minus the layout pass
/// it arms — the half the tests below drive.
fn data_changed_in(registry: &mut GridRegistry, node: &MacosNode) {
    let MacosNode::View(view) = node else { return };
    let key = &**view as *const NSView as usize;
    {
        let Some(inst) = registry.get(&key) else {
            return;
        };
        let Some(m) = inst.callbacks.borrow().as_ref().map(build_metrics) else {
            return;
        };
        *inst.metrics.borrow_mut() = m;
        *inst.last_window.borrow_mut() = None;
    }
    sync_in(registry, key);
}

/// Set the frozen panes and re-window: the bands join the mounted set,
/// and cells already mounted are re-inserted at their band's z-order.
pub(crate) fn set_frozen(backend: &mut crate::imp::MacosBackend, node: &MacosNode, frozen: FrozenPanes) {
    set_frozen_in(&mut backend.virtual_grid_registry, node, frozen);
    crate::imp::schedule_layout_pass();
}

fn set_frozen_in(registry: &mut GridRegistry, node: &MacosNode, frozen: FrozenPanes) {
    let MacosNode::View(view) = node else { return };
    let key = &**view as *const NSView as usize;
    let Some(inst) = registry.get(&key) else { return };
    inst.frozen.set(frozen);
    *inst.last_window.borrow_mut() = None;
    sync_in(registry, key);
}

/// Keep row `row` mounted outside the window (`VirtualGridHandle::
/// keep_row_mounted`) and re-window.
fn set_kept_row_in(registry: &GridRegistry, key: usize, row: Option<usize>) {
    let Some(inst) = registry.get(&key) else { return };
    if inst.kept_row.replace(row) == row {
        return;
    }
    *inst.last_window.borrow_mut() = None;
    sync_in(registry, key);
}

/// Reuse cells from now on (module docs, "Cell reuse").
pub(crate) fn set_recycling(backend: &mut crate::imp::MacosBackend, node: &MacosNode, recycling: GridRecycling) {
    set_recycling_in(&backend.virtual_grid_registry, node, recycling);
}

fn set_recycling_in(registry: &GridRegistry, node: &MacosNode, recycling: GridRecycling) {
    let MacosNode::View(view) = node else { return };
    let key = &**view as *const NSView as usize;
    if let Some(inst) = registry.get(&key) {
        *inst.recycling.borrow_mut() = Some(recycling);
    }
}

/// Everything one sync touches, with no backend borrow among them.
///
/// The ONLY thing a sync needs the backend for is finding the instance
/// in `virtual_grid_registry`; once these are cloned out, the diff is
/// pure AppKit plus framework `Rc`s.
#[derive(Clone)]
struct GridHandles {
    scroll: Retained<NSView>,
    document: Retained<NSView>,
    callbacks: Rc<RefCell<Option<GridCallbacks<MacosNode>>>>,
    metrics: Rc<RefCell<GridMetrics>>,
    mounted: Rc<RefCell<HashMap<(usize, usize), MountedCell>>>,
    last_window: Rc<RefCell<Option<PaneWindow>>>,
    frozen: Rc<std::cell::Cell<FrozenPanes>>,
    kept_row: Rc<std::cell::Cell<Option<usize>>>,
    overscan: f32,
    /// The registry key, for the follow-up sync that finishes the overscan.
    key: usize,
    /// The last sync left overscan cells unmounted (see [`OVERSCAN_BUDGET`]).
    incomplete: Rc<std::cell::Cell<bool>>,
    /// A follow-up sync is already scheduled.
    follow_up: Rc<std::cell::Cell<bool>>,
    /// Cells that left the window: detached at once, released (their
    /// subtree and scope dropped) within the budget — see [`retire`].
    retiring: Rc<RefCell<Vec<MountedCell>>>,
    /// The reuse bundle, when the grid reuses cells (module docs, "Cell
    /// reuse").
    recycling: Rc<RefCell<Option<GridRecycling>>>,
    /// Parked cells, by (reuse class, bands).
    pool: Rc<RefCell<HashMap<(u64, (Band, Band)), Vec<MountedCell>>>>,
}

/// How long one sync may spend on work that can wait — mounting OVERSCAN
/// cells (the ones just outside the viewport) and releasing cells that left
/// the window — before it hands the rest to the next run-loop turn. Cells in
/// view always mount at once. A fast flick brings several rows in and sends
/// as many out; each table cell is a small styled view tree, and building the
/// buffer and tearing down the leavers in the same scroll callback stalled
/// the frame the scroll was drawing. Half of a 120 Hz frame.
const OVERSCAN_BUDGET: std::time::Duration = std::time::Duration::from_millis(4);

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
    /// `mount_cell` realizes a subtree and `release_cell` drops one, and
    /// both re-enter the backend through `create_*` / scope cleanups —
    /// `handlers::view::mount_view` opens with `backend.borrow_mut()`.
    /// Every path into a sync already holds that borrow: the layout pass
    /// runs inside `with_global_backend`, `virtual_grid_data_changed` is
    /// dispatched through `backend.borrow_mut()`, and the bounds
    /// observer goes through `with_backend`. Cloning the instance's
    /// `Rc`s does not end it: the borrow is the CALLER's.
    ///
    /// So the borrow-free half is queued here and drained where the
    /// borrow is provably gone. Cells mount one pass later than the
    /// layout that discovered them and mark the tree dirty on mount, so
    /// the following pass frames them — the first fill, which `create`
    /// defers, works the same way. Mounting synchronously aborts with
    /// `RefCell already borrowed` inside `virtual_grid::mount_cell`.
    static PENDING: RefCell<Vec<PendingJob>> = const { RefCell::new(Vec::new()) };
}

/// Queue a sync, replacing any already queued for the same grid — the
/// diff is computed at drain time, so the later subsumes the earlier.
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
/// **Call only where the backend borrow is provably released**, or this
/// puts back the abort the queue exists to prevent. Loops because a
/// teardown's scope cleanups can queue another grid's sync.
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

/// Drop a torn-down grid's cells, with no backend borrow held — the
/// two-axis form of the virtualizer teardown hazard.
fn teardown_now(job: TeardownJob) {
    for cell in job.cells {
        let _: () = unsafe { msg_send![&cell.view, removeFromSuperview] };
        if let Some(release) = job.release_cell.as_ref() {
            release(cell.scope_id);
        }
    }
}

/// Re-window one grid: resize the document view to the content
/// extent, then diff the visible cell rect.
pub(crate) fn sync(backend: &mut crate::imp::MacosBackend, key: usize) {
    sync_in(&backend.virtual_grid_registry, key);
}

/// [`sync`] against the registry alone — the only part of the backend
/// a sync needs. Split out so the tests below can drive the queue
/// without a `MacosBackend`.
fn sync_in(registry: &GridRegistry, key: usize) {
    let Some(inst) = registry.get(&key) else {
        return;
    };
    // The registry lookup is the only part needing the backend, and the
    // caller holds its borrow — hand the rest to the queue. See `PENDING`.
    queue_sync(
        key,
        GridHandles {
            scroll: inst.scroll_view.clone(),
            document: inst.document_view.clone(),
            callbacks: inst.callbacks.clone(),
            metrics: inst.metrics.clone(),
            mounted: inst.mounted.clone(),
            last_window: inst.last_window.clone(),
            frozen: inst.frozen.clone(),
            kept_row: inst.kept_row.clone(),
            overscan: inst.overscan,
            key,
            incomplete: inst.incomplete.clone(),
            follow_up: inst.follow_up.clone(),
            retiring: inst.retiring.clone(),
            recycling: inst.recycling.clone(),
            pool: inst.pool.clone(),
        },
    );
}

/// The borrow-free half of a sync: runs from the drain, with no backend
/// borrow held.
fn sync_now(h: &GridHandles) {
    let t0 = std::time::Instant::now();
    let scroll = h.scroll.clone();
    let document = h.document.clone();
    let callbacks = &h.callbacks;
    let metrics = &h.metrics;
    let mounted = &h.mounted;
    let last_window = &h.last_window;

    let (content_w, content_h) = metrics.borrow().content_size();
    let doc_frame: CGRect = unsafe { msg_send![&document, frame] };
    if (doc_frame.size.width - content_w as f64).abs() > 0.5
        || (doc_frame.size.height - content_h as f64).abs() > 0.5
    {
        // AppKit's scrollable extent IS the documentView's frame —
        // there is no `contentSize` to set.
        let size = CGSize::new(content_w as f64, content_h as f64);
        let _: () = unsafe { msg_send![&document, setFrameSize: size] };
    }

    let clip: Option<Retained<NSView>> = unsafe { msg_send_id![&scroll, contentView] };
    let Some(clip) = clip else { return };
    let mut clip_bounds: CGRect = unsafe { msg_send![&clip, bounds] };
    if clip_bounds.size.width <= 0.0 || clip_bounds.size.height <= 0.0 {
        // The layout pass framed the scroller, but NSScrollView sizes its
        // clip view lazily (`tile`, on its next layout/display), and this
        // sync runs right after that pass. Reading the clip here saw 0×0,
        // bailed, and the grid stayed EMPTY until some unrelated later pass
        // happened to re-sync it — on some launches never. Tile now.
        let frame: CGRect = unsafe { msg_send![&scroll, frame] };
        if frame.size.width > 0.0 && frame.size.height > 0.0 {
            let _: () = unsafe { msg_send![&scroll, tile] };
            clip_bounds = unsafe { msg_send![&clip, bounds] };
        }
    }
    if clip_bounds.size.width <= 0.0 || clip_bounds.size.height <= 0.0 {
        // Layout hasn't run. Bail WITHOUT caching, so the next pass
        // retries instead of remembering an empty window as current.
        return;
    }

    let overscan = h.overscan;
    let frozen = h.frozen.get();
    let scroll_at = (clip_bounds.origin.x as f32, clip_bounds.origin.y as f32);
    let viewport = (clip_bounds.size.width as f32, clip_bounds.size.height as f32);
    let window = metrics.borrow().pane_window(scroll_at, viewport, overscan, frozen).keeping_row(h.kept_row.get());

    // The cells actually in view: these mount in this sync whatever it
    // costs; the rest of `window` (the overscan) within `OVERSCAN_BUDGET`.
    let in_view = metrics.borrow().pane_window(scroll_at, viewport, 0.0, frozen);
    if *last_window.borrow() == Some(window) && !h.incomplete.get() {
        finish_turn(h, t0, false);
        return;
    }
    // A window that was invalidated (a data change, a layout pass) may
    // have moved every surviving cell: a width or height changed.
    let reframe_all = last_window.borrow().is_none();
    *last_window.borrow_mut() = Some(window);

    let leaving: Vec<(usize, usize)> = mounted
        .borrow()
        .iter()
        .filter(|((c, r), cell)| !window.contains(*c, *r) || cell.bands != window.bands(*c, *r))
        .map(|(slot, _)| *slot)
        .collect();
    let recycling = h.recycling.borrow().clone();
    for slot in leaving {
        let cell = mounted.borrow_mut().remove(&slot);
        if let Some(cell) = cell {
            match &recycling {
                // Parked under the class of the position it LEFT, which is
                // the structure it was built with.
                Some(r) => {
                    let class = (r.reuse_class)(slot.0, slot.1);
                    h.pool.borrow_mut().entry((class, cell.bands)).or_default().push(cell);
                }
                None => retire(h, cell),
            }
        }
    }
    // Cells rebound in this sync, laid out after one flush.
    let mut rebound: Vec<(Retained<NSView>, usize, usize)> = Vec::new();

    // Mount cells that entered — and re-mount any whose CONTENT changed
    // under them, which a positional diff cannot see.
    let mut deferred = false;
    for (col, row) in window.cells() {
        let key_now = callbacks
            .borrow()
            .as_ref()
            .map(|c| (c.cell_key)(col, row));
        let mounted_key = mounted.borrow().get(&(col, row)).map(|m| m.key);
        match (mounted_key, key_now) {
            // Live cell, unchanged content: leave it alone (re-framed
            // when its geometry may have moved). This is the common case
            // and the whole point of the pool.
            (Some(old), Some(new)) if old == new => {
                if reframe_all {
                    let view = mounted.borrow().get(&(col, row)).map(|c| c.view.clone());
                    if let Some(view) = view {
                        layout_cell(&view, &metrics.borrow(), col, row, scroll_at, viewport, frozen);
                    }
                }
                continue;
            }
            // A reusable cell's content follows its data reactively: a new
            // key needs no rebuild (`virtual_grid_recycled`'s contract).
            (Some(_), Some(new)) if recycling.is_some() => {
                if let Some(cell) = mounted.borrow_mut().get_mut(&(col, row)) {
                    cell.key = new;
                }
                continue;
            }
            // Live cell whose content moved on: drop it so the mount
            // below rebuilds it. Releasing FIRST keeps the scope count
            // flat rather than doubling for a frame.
            (Some(_), _) => {
                let stale = mounted.borrow_mut().remove(&(col, row));
                if let Some(stale) = stale {
                    retire(h, stale);
                }
            }
            (None, _) => {}
        }
        if !in_view.contains(col, row) && t0.elapsed() >= OVERSCAN_BUDGET {
            deferred = true;
            continue;
        }
        // Reuse a parked cell of this position's class and bands: rebind it
        // (staged; flushed once below) and move it here.
        let bands = window.bands(col, row);
        if let Some(r) = &recycling {
            let parked = h.pool.borrow_mut().get_mut(&((r.reuse_class)(col, row), bands)).and_then(Vec::pop);
            if let Some(mut cell) = parked {
                (r.rebind_cell)(cell.scope_id, col, row);
                place(&cell.view, &metrics.borrow(), col, row, scroll_at, viewport, frozen);
                rebound.push((cell.view.clone(), col, row));
                cell.key = key_now.unwrap_or_default();
                mounted.borrow_mut().insert((col, row), cell);
                continue;
            }
        }
        let mount = callbacks.borrow().as_ref().map(|c| c.mount_cell.clone());
        let Some(mount) = mount else { break };
        let (node, scope_id) = mount(col, row);
        let MacosNode::View(view) = &node else { continue };
        place(view, &metrics.borrow(), col, row, scroll_at, viewport, frozen);
        insert_cell(&scroll, &document, view, bands);
        // Lay the new cell's contents out now: this runs from the drain,
        // after the pass that discovered the cell, and nothing guarantees
        // another pass before it draws (see `layout_cells`).
        layout_cell(view, &metrics.borrow(), col, row, scroll_at, viewport, frozen);
        mounted.borrow_mut().insert(
            (col, row),
            MountedCell {
                view: view.clone(),
                scope_id,
                key: key_now.unwrap_or_default(),
                bands,
            },
        );
    }
    // The rebinds land in one flush, then the rebound cells are laid out
    // against their new content — all before this frame draws.
    if !rebound.is_empty() {
        crate::newcore::flush_now();
        for (view, col, row) in rebound {
            layout_cell(&view, &metrics.borrow(), col, row, scroll_at, viewport, frozen);
        }
    }
    // Parked cells nothing claimed once the window is complete: release
    // them. Kept while overscan is still pending, which will want them.
    if !deferred {
        let leftovers: Vec<MountedCell> = h.pool.borrow_mut().drain().flat_map(|(_, v)| v).collect();
        for cell in leftovers {
            retire(h, cell);
        }
    }
    h.incomplete.set(deferred);
    finish_turn(h, t0, deferred);
}

/// Take a cell off screen now; release it within the budget ([`finish_turn`]).
/// Detaching keeps its subtree assembled, which `release_subtree` needs.
fn retire(h: &GridHandles, cell: MountedCell) {
    let _: () = unsafe { msg_send![&cell.view, removeFromSuperview] };
    h.retiring.borrow_mut().push(cell);
}

/// End of a sync: release retiring cells until the budget is spent, then, if
/// overscan mounts or releases are left, schedule a follow-up sync on the
/// next run-loop turn — after this frame has drawn.
fn finish_turn(h: &GridHandles, t0: std::time::Instant, deferred: bool) {
    let release = h.callbacks.borrow().as_ref().map(|c| c.release_cell.clone());
    loop {
        if t0.elapsed() >= OVERSCAN_BUDGET {
            break;
        }
        let Some(cell) = h.retiring.borrow_mut().pop() else { break };
        if let Some(release) = release.as_ref() {
            release(cell.scope_id);
        }
    }
    let more = deferred || !h.retiring.borrow().is_empty();
    if more && !h.follow_up.replace(true) {
        let (key, follow_up) = (h.key, h.follow_up.clone());
        runtime_shared::scheduling::after_ms_detached(0, move || {
            follow_up.set(false);
            crate::imp::with_backend(|b| sync(b, key));
            drain_pending();
        });
    }
}

/// Lay a cell's subtree out against its box (`layout_detached_in_place`,
/// which keeps the root where the grid put it), then frame the root.
/// Borrow-free callers only: it takes the backend.
#[allow(clippy::too_many_arguments)]
fn layout_cell(
    view: &NSView,
    m: &GridMetrics,
    col: usize,
    row: usize,
    scroll: (f32, f32),
    viewport: (f32, f32),
    frozen: FrozenPanes,
) {
    let (w, h) = m.cell_size(col, row);
    let key = view as *const NSView as usize;
    crate::imp::with_backend(|b| b.layout_detached_in_place(key, CGSize::new(w as f64, h as f64)));
    place(view, m, col, row, scroll, viewport, frozen);
}

/// Frame a cell where it draws: its origin, or for a frozen cell the
/// sticky position at this scroll (`GridMetrics::display_origin`). Skips
/// an unchanged frame — this runs per scroll for every frozen cell.
fn place(
    view: &NSView,
    m: &GridMetrics,
    col: usize,
    row: usize,
    scroll: (f32, f32),
    viewport: (f32, f32),
    frozen: FrozenPanes,
) {
    // Zero scroll: AppKit applies the scroll on the scrolling axes and holds
    // the frozen ones (module docs, "Frozen panes").
    let _ = scroll;
    let (x, y) = m.display_origin(col, row, (0.0, 0.0), viewport, frozen);
    let (w, h) = m.cell_size(col, row);
    let frame = CGRect {
        origin: CGPoint { x: x as f64, y: y as f64 },
        size: CGSize::new(w as f64, h as f64),
    };
    let now: CGRect = unsafe { msg_send![view, frame] };
    if now != frame {
        // Implicit layer actions off: a cell moving to its slot jumps there.
        unsafe {
            let _: () = msg_send![objc2::class!(CATransaction), begin];
            let _: () = msg_send![objc2::class!(CATransaction), setDisableActions: true];
            let _: () = msg_send![view, setFrame: frame];
            let _: () = msg_send![objc2::class!(CATransaction), commit];
        }
    }
}

/// Parent a cell by its bands (module docs, "Frozen panes"): the document
/// for a scrolling cell, a floating subview fixed on the frozen axis for an
/// edge cell, the scroll view itself for a corner.
fn insert_cell(scroll: &NSView, document: &NSView, view: &NSView, bands: (Band, Band)) {
    // NSEventGestureAxis: Horizontal = 1, Vertical = 2.
    match bands {
        (Band::Scrolling, Band::Scrolling) => unsafe {
            let _: () = msg_send![document, addSubview: view];
        },
        (Band::Scrolling, _) => unsafe {
            let _: () = msg_send![scroll, addFloatingSubview: view, forAxis: 2isize];
        },
        (_, Band::Scrolling) => unsafe {
            let _: () = msg_send![scroll, addFloatingSubview: view, forAxis: 1isize];
        },
        _ => unsafe {
            let _: () = msg_send![scroll, addSubview: view];
        },
    }
}

/// Re-window every registered grid — called from the layout pass,
/// which is when a grid first learns its viewport size.
pub(crate) fn sync_all(backend: &mut crate::imp::MacosBackend) {
    let keys: Vec<usize> = backend.virtual_grid_registry.keys().copied().collect();
    for key in keys {
        if let Some(inst) = backend.virtual_grid_registry.get(&key) {
            // The cached window is keyed to a viewport size, so a
            // layout pass must invalidate it: a resize that leaves the
            // window's INDICES unchanged still needs cells re-framed.
            *inst.last_window.borrow_mut() = None;
        }
        sync(backend, key);
    }
}

/// Lay out every mounted cell's SUBTREE against the cell's box. A cell root
/// is a Taffy root with no parent — this engine places it in the document
/// view — so the host pass never reaches it, and without this everything
/// inside a cell (a table cell's padding box and its text) stays 0×0: the
/// cell frames draw, empty. `layout_detached_in_place` frames the subtree
/// and leaves each root where the grid put it.
/// iOS and Android do the same through their layout pass's `cell_box` hook.
pub(crate) fn layout_cells(backend: &mut crate::imp::MacosBackend) {
    struct Cell {
        key: usize,
        view: Retained<NSView>,
        slot: (usize, usize),
        size: CGSize,
    }
    struct Grid {
        cells: Vec<Cell>,
        metrics: Rc<RefCell<GridMetrics>>,
        scroll: Retained<NSView>,
        frozen: FrozenPanes,
    }
    let grids: Vec<Grid> = backend
        .virtual_grid_registry
        .values()
        .map(|inst| {
            let m = inst.metrics.borrow();
            Grid {
                cells: inst
                    .mounted
                    .borrow()
                    .iter()
                    // A cell whose subtree nothing touched since it was laid
                    // out keeps its frames: re-laying out every mounted cell
                    // on every pass cost hundreds of ms per scroll.
                    .filter(|(_, cell)| {
                        let key = &*cell.view as *const NSView as usize;
                        backend.view_to_layout.get(&key).is_some_and(|(_, n)| backend.layout.is_dirty(*n))
                    })
                    .map(|(slot, cell)| {
                        let (w, h) = m.cell_size(slot.0, slot.1);
                        Cell {
                            key: &*cell.view as *const NSView as usize,
                            view: cell.view.clone(),
                            slot: *slot,
                            size: CGSize::new(w as f64, h as f64),
                        }
                    })
                    .collect(),
                metrics: inst.metrics.clone(),
                scroll: inst.scroll_view.clone(),
                frozen: inst.frozen.get(),
            }
        })
        .collect();
    for g in grids {
        let (scroll_at, viewport) = clip_offset_and_size(&g.scroll);
        for c in g.cells {
            backend.layout_detached_in_place(c.key, c.size);
            place(&c.view, &g.metrics.borrow(), c.slot.0, c.slot.1, scroll_at, viewport, g.frozen);
        }
    }
}

pub(crate) fn release(backend: &mut crate::imp::MacosBackend, node: &MacosNode) {
    release_in(&mut backend.virtual_grid_registry, node);
    crate::imp::schedule_layout_pass();
}

/// [`release`] against the registry alone, minus the layout pass it
/// arms — the half the tests below drive.
fn release_in(registry: &mut GridRegistry, node: &MacosNode) {
    let MacosNode::View(view) = node else { return };
    let key = &**view as *const NSView as usize;
    let Some(mut inst) = registry.remove(&key) else {
        return;
    };
    // Detach the observer FIRST — a bounds change delivered mid-drain
    // would re-enter `sync` against a half-freed instance.
    if let Some(target) = inst.observer.take() {
        let center: *mut objc2::runtime::AnyObject =
            unsafe { msg_send![objc2::class!(NSNotificationCenter), defaultCenter] };
        let _: () = unsafe { msg_send![center, removeObserver: &*target] };
    }
    if let Some(target) = inst.style_observer.take() {
        crate::imp::callbacks::remove_observer(&target);
    }
    let cbs = inst.callbacks.borrow_mut().take();
    let release_cell = cbs.as_ref().map(|c| c.release_cell.clone());
    let mut cells: Vec<MountedCell> = inst.mounted.borrow_mut().drain().map(|(_, v)| v).collect();
    // Cells already off screen but not yet released go with the rest.
    cells.append(&mut inst.retiring.borrow_mut());
    cells.extend(inst.pool.borrow_mut().drain().flat_map(|(_, v)| v));
    // Queued, not run: this runs under the caller's backend borrow, and
    // dropping a cell scope re-enters it. Any sync still queued for this
    // grid runs first and finds `callbacks` already `None`.
    PENDING.with(|p| {
        p.borrow_mut()
            .push(PendingJob::Teardown(TeardownJob { cells, release_cell }))
    });
}

// =========================================================================
// Imperative handle
// =========================================================================

pub(crate) struct MacosVirtualGridOps;

impl runtime_shared::primitives::virtual_grid::VirtualGridOps for MacosVirtualGridOps {
    fn scroll_to_cell(&self, node: &dyn std::any::Any, col: usize, row: usize) {
        let Some(MacosNode::View(view)) = node.downcast_ref::<MacosNode>() else {
            return;
        };
        let key = &**view as *const NSView as usize;
        // `with_backend` returns `()` on this backend, so the lookup
        // is captured out through a Cell rather than returned.
        // Past the frozen header and columns, so the cell doesn't land
        // beneath them (`GridMetrics::scroll_target`).
        let origin: std::cell::Cell<Option<(f32, f32)>> = std::cell::Cell::new(None);
        crate::imp::with_backend(|b| {
            if let Some(i) = b.virtual_grid_registry.get(&key) {
                let (current, viewport) = clip_offset_and_size(view);
                origin.set(Some(i.metrics.borrow().scroll_target(col, row, current, viewport, i.frozen.get())));
            }
        });
        let Some((x, y)) = origin.get() else { return };
        scroll_clip_to(view, x, y);
    }

    fn keep_row_mounted(&self, node: &dyn std::any::Any, row: Option<usize>) {
        let Some(MacosNode::View(view)) = node.downcast_ref::<MacosNode>() else {
            return;
        };
        let key = &**view as *const NSView as usize;
        crate::imp::with_backend(|b| set_kept_row_in(&b.virtual_grid_registry, key, row));
        // The queued sync drains with the pass this arms.
        crate::imp::schedule_layout_pass();
    }

    fn scroll_offset(&self, node: &dyn std::any::Any) -> (f32, f32) {
        let Some(MacosNode::View(view)) = node.downcast_ref::<MacosNode>() else {
            return (0.0, 0.0);
        };
        let clip: Option<Retained<NSView>> = unsafe { msg_send_id![&**view, contentView] };
        let Some(clip) = clip else { return (0.0, 0.0) };
        let bounds: CGRect = unsafe { msg_send![&clip, bounds] };
        (bounds.origin.x as f32, bounds.origin.y as f32)
    }

    fn scroll_to(&self, node: &dyn std::any::Any, x: f32, y: f32) {
        if let Some(MacosNode::View(view)) = node.downcast_ref::<MacosNode>() {
            scroll_clip_to(view, x, y);
        }
    }

    fn scrollport(&self, node: &dyn std::any::Any) -> Option<Scrollport> {
        let Some(MacosNode::View(view)) = node.downcast_ref::<MacosNode>() else {
            return None;
        };
        scrollport_of(view)
    }

    /// Delivered off the clip view's frame notification — see
    /// `install_clip_frame_observer` for why that, and not the layout
    /// pass's `on_layout` hook, is the trigger.
    fn subscribe_scrollport(
        &self,
        node: &dyn std::any::Any,
        callback: Box<dyn Fn(Scrollport)>,
    ) -> runtime_shared::LayoutSubscription {
        let Some(MacosNode::View(view)) = node.downcast_ref::<MacosNode>() else {
            return runtime_shared::LayoutSubscription::noop();
        };
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let scroll = view.clone();
        let observer = crate::imp::callbacks::install_clip_frame_observer(
            mtm,
            view,
            Rc::new(move || {
                if let Some(port) = scrollport_of(&scroll) {
                    callback(port);
                    // A notification is an author-code entry point; commit
                    // what the callback staged.
                    backend_apple_core::dispatch_hook::fire_dispatch_hook();
                }
            }),
        );
        let Some(observer) = observer else {
            return runtime_shared::LayoutSubscription::noop();
        };
        runtime_shared::LayoutSubscription::new(move || {
            crate::imp::callbacks::remove_observer(&observer);
            drop(observer);
        })
    }
}

/// The grid's scrollport: its clip view's frame, which `NSScrollView`
/// tiles to leave room for LEGACY scrollers and stretches under OVERLAY
/// ones. A scroller's thickness is what the clip view lost on that axis
/// (`0` for overlay scrollers). `None` before layout.
///
/// `tile` first: the clip frame is only re-derived when the scroll view
/// tiles, and a read straight after a document-size change (a scroller
/// that should now auto-show) would otherwise see the previous layout.
/// Tiling is idempotent, so a read that changes nothing costs nothing.
fn scrollport_of(scroll: &NSView) -> Option<Scrollport> {
    let _: () = unsafe { msg_send![scroll, tile] };
    let frame: CGRect = unsafe { msg_send![scroll, frame] };
    if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
        return None;
    }
    let clip: Option<Retained<NSView>> = unsafe { msg_send_id![scroll, contentView] };
    let clip_frame: CGRect = unsafe { msg_send![&clip?, frame] };
    let (w, h) = (clip_frame.size.width as f32, clip_frame.size.height as f32);
    Some(Scrollport {
        width: w,
        height: h,
        scrollbar_width: (frame.size.width as f32 - w).max(0.0),
        scrollbar_height: (frame.size.height as f32 - h).max(0.0),
    })
}

/// The clip view's scroll offset and visible size.
fn clip_offset_and_size(scroll: &NSView) -> ((f32, f32), (f32, f32)) {
    let clip: Option<Retained<NSView>> = unsafe { msg_send_id![scroll, contentView] };
    let Some(clip) = clip else { return ((0.0, 0.0), (0.0, 0.0)) };
    let b: CGRect = unsafe { msg_send![&clip, bounds] };
    ((b.origin.x as f32, b.origin.y as f32), (b.size.width as f32, b.size.height as f32))
}

fn scroll_clip_to(scroll: &Retained<NSView>, x: f32, y: f32) {
    let clip: Option<Retained<NSView>> = unsafe { msg_send_id![&**scroll, contentView] };
    let Some(clip) = clip else { return };
    let point = CGPoint { x: x as f64, y: y as f64 };
    let _: () = unsafe { msg_send![&clip, scrollToPoint: point] };
    // Without `reflectScrolledClipView:` the clip moves but the
    // scrollers and document don't redraw against it.
    let _: () = unsafe { msg_send![&**scroll, reflectScrolledClipView: &*clip] };
}

pub(crate) static MACOS_VIRTUAL_GRID_OPS: MacosVirtualGridOps = MacosVirtualGridOps;

pub(crate) fn make_handle(
    node: &MacosNode,
) -> runtime_shared::primitives::virtual_grid::VirtualGridHandle {
    runtime_shared::primitives::virtual_grid::VirtualGridHandle::new(
        Rc::new(node.clone()) as Rc<dyn std::any::Any>,
        &MACOS_VIRTUAL_GRID_OPS,
    )
}

#[cfg(test)]
mod pending_tests {
    //! The borrow discipline, against a REAL grid: `create` takes the
    //! registry rather than the backend, so the whole mount path runs
    //! in the host test binary with real AppKit views and no
    //! `MacosBackend`. (`with_backend` is a no-op with no global self
    //! installed, so the bounds observer `create` installs is inert.)
    use super::*;
    use std::cell::Cell;

    /// A grid whose cells count their mounts and releases. Every cell
    /// is 40×40; the scroller is framed 200×200 after creation.
    struct Fixture {
        registry: GridRegistry,
        node: MacosNode,
        key: usize,
        mounts: Rc<Cell<usize>>,
        releases: Rc<Cell<usize>>,
        /// Bump to change cell `(0, 0)`'s content key — what a data
        /// change that rewrote one cell looks like to the engine.
        origin_version: Rc<Cell<u64>>,
    }

    fn grid(cols: usize, rows: usize) -> Fixture {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mounts = Rc::new(Cell::new(0usize));
        let releases = Rc::new(Cell::new(0usize));
        let origin_version = Rc::new(Cell::new(0u64));
        let m = mounts.clone();
        let r = releases.clone();
        let v = origin_version.clone();
        let callbacks = GridCallbacks::<MacosNode> {
            col_count: Rc::new(move || cols),
            row_count: Rc::new(move || rows),
            col_width: Rc::new(|_| 40.0),
            row_height: Rc::new(|_| 40.0),
            cell_key: Rc::new(move |c, r| {
                let content = if (c, r) == (0, 0) { v.get() } else { 0 };
                (c * 1000 + r) as u64 * 100 + content
            }),
            mount_cell: Rc::new(move |c, r| {
                m.set(m.get() + 1);
                std::thread::sleep(MOUNT_DELAY.with(|d| d.get()));
                let view: Retained<NSView> =
                    Retained::into_super(crate::imp::view::FlippedView::new(mtm));
                (MacosNode::View(view), (c * 1000 + r) as u64)
            }),
            release_cell: Rc::new(move |_| r.set(r.get() + 1)),
            on_scroll: None,
        };
        let mut registry = GridRegistry::new();
        let scroll = create(mtm, &mut registry, callbacks, 1.0);
        let _: () = unsafe {
            msg_send![&scroll, setFrame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(200.0, 200.0))]
        };
        let key = &*scroll as *const NSView as usize;
        Fixture {
            registry,
            node: MacosNode::View(scroll),
            key,
            mounts,
            releases,
            origin_version,
        }
    }

    thread_local! {
        /// How long each fixture cell takes to mount — what a heavy cell
        /// (a table cell's styled view tree) looks like to the engine.
        static MOUNT_DELAY: Cell<std::time::Duration> = const { Cell::new(std::time::Duration::ZERO) };
    }

    /// A fast flick brings several rows in at once. Mounting the overscan
    /// buffer in the same scroll callback as the rows in view stalled the
    /// frame being drawn: the cells IN VIEW must mount at once, the
    /// overscan within `OVERSCAN_BUDGET`, and the rest on a later turn.
    #[test]
    fn regression_a_flick_mounts_the_whole_overscan_in_one_frame() {
        drain_pending();
        let f = grid(5, 400); // 40x40 cells, 200x200 viewport, overscan 1.0
        MOUNT_DELAY.with(|d| d.set(std::time::Duration::from_millis(1)));
        sync_in(&f.registry, f.key);
        drain_pending();
        let inst = f.registry.get(&f.key).unwrap();
        // In view: rows 0..=5 (a partial 6th at 200) x 5 columns.
        for r in 0..5 {
            for c in 0..5 {
                assert!(inst.mounted.borrow().contains_key(&(c, r)), "in-view cell ({c},{r}) mounts now");
            }
        }
        let first = f.mounts.get();
        assert!(first < 5 * 11, "the overscan was deferred, not mounted with the view: {first} mounts");
        assert!(inst.incomplete.get(), "the sync knows it left work");
        // The follow-up turns (the scheduled sync, run by hand here) finish it.
        for _ in 0..20 {
            if !inst.incomplete.get() {
                break;
            }
            sync_in(&f.registry, f.key);
            drain_pending();
        }
        MOUNT_DELAY.with(|d| d.set(std::time::Duration::ZERO));
        assert!(!inst.incomplete.get(), "the overscan finishes");
        assert_eq!(f.mounts.get(), 5 * 11, "rows 0..=10 (the view plus a viewport below) x 5 columns, each once");
    }

    /// A flick that replaces the whole screen also sends a screen of cells
    /// out, and releasing them (subtree and scope teardown) in the same
    /// scroll callback was ~40% of the worst frames. They leave the screen
    /// at once — detached in that sync — and are released within the
    /// budget, the rest on later turns.
    #[test]
    fn regression_cells_that_scroll_away_are_torn_down_in_the_scroll_frame() {
        drain_pending();
        let f = grid(5, 400);
        sync_in(&f.registry, f.key);
        drain_pending();
        let inst = f.registry.get(&f.key).unwrap();
        while inst.incomplete.get() {
            sync_in(&f.registry, f.key);
            drain_pending();
        }
        let old: Vec<Retained<NSView>> = inst.mounted.borrow().values().map(|c| c.view.clone()).collect();
        let released_before = f.releases.get();

        // Jump far; slow cells use up the budget on the cells in view.
        MOUNT_DELAY.with(|d| d.set(std::time::Duration::from_millis(1)));
        let MacosNode::View(scroll) = f.node.clone() else { unreachable!() };
        scroll_clip_to(&scroll, 0.0, 8000.0);
        sync_in(&f.registry, f.key);
        drain_pending();
        MOUNT_DELAY.with(|d| d.set(std::time::Duration::ZERO));

        for v in &old {
            let sup: *mut NSView = unsafe { msg_send![&**v, superview] };
            assert!(sup.is_null(), "a cell that left the window is off screen in the same sync");
        }
        assert!(
            f.releases.get() - released_before < old.len(),
            "the release of a screenful of leavers waited for a later turn"
        );
        for _ in 0..50 {
            if inst.retiring.borrow().is_empty() && !inst.incomplete.get() {
                break;
            }
            sync_in(&f.registry, f.key);
            drain_pending();
        }
        assert_eq!(f.releases.get() - released_before, old.len(), "every leaver is released");
    }

    /// REGRESSION: cells flashed through the header. Laying a cell's
    /// contents out wrote the cell's own frame at the Taffy origin — the
    /// grid's top-left, under the header — before the grid moved it back to
    /// its slot. Laying out in place keeps the cell where the grid put it.
    #[test]
    fn regression_laying_out_a_cell_moves_it_under_the_header() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = crate::imp::MacosBackend::new(mtm);
        let cell: Retained<NSView> = Retained::into_super(crate::imp::view::FlippedView::new(mtm));
        let _ = backend.layout_for_view(&cell);
        let slot = CGRect::new(CGPoint::new(120.0, 880.0), CGSize::new(40.0, 40.0));
        let _: () = unsafe { msg_send![&cell, setFrame: slot] };
        let key = &*cell as *const NSView as usize;
        backend.layout_detached_in_place(key, CGSize::new(40.0, 40.0));
        let after: CGRect = unsafe { msg_send![&cell, frame] };
        assert_eq!(after, slot, "the cell stays in its slot");
    }

    /// `keep_row_mounted`: with cell reuse on, a jump far away would park
    /// the kept row's cells and hand them to other rows — the view a drag
    /// started in would be moved and rebound mid-gesture. Kept, the row
    /// keeps its own views at its own slot; let go, it leaves like any row.
    #[test]
    fn a_kept_row_keeps_its_views_through_a_jump() {
        drain_pending();
        let f = grid(5, 400); // 40x40 cells, 200x200 viewport
        set_recycling_in(
            &f.registry,
            &f.node,
            GridRecycling { reuse_class: Rc::new(|c, _| c as u64), rebind_cell: Rc::new(|_, _, _| {}) },
        );
        sync_in(&f.registry, f.key);
        drain_pending();
        let inst = f.registry.get(&f.key).unwrap();
        while inst.incomplete.get() {
            sync_in(&f.registry, f.key);
            drain_pending();
        }
        let views_of_row = |r: usize| -> Vec<usize> {
            (0..5)
                .filter_map(|c| inst.mounted.borrow().get(&(c, r)).map(|m| &*m.view as *const NSView as usize))
                .collect()
        };
        let held = views_of_row(2);
        assert_eq!(held.len(), 5);

        set_kept_row_in(&f.registry, f.key, Some(2));
        drain_pending();
        let MacosNode::View(scroll) = f.node.clone() else { unreachable!() };
        scroll_clip_to(&scroll, 0.0, 8000.0);
        sync_in(&f.registry, f.key);
        drain_pending();
        while inst.incomplete.get() {
            sync_in(&f.registry, f.key);
            drain_pending();
        }
        assert_eq!(views_of_row(2), held, "the kept row's own views, never parked or rebound");
        let cell = inst.mounted.borrow().get(&(1, 2)).map(|m| m.view.clone()).unwrap();
        let frame: CGRect = unsafe { msg_send![&*cell, frame] };
        assert_eq!((frame.origin.x, frame.origin.y), (40.0, 80.0), "still at its own slot");
        assert!(inst.mounted.borrow().contains_key(&(1, 200)), "the window followed the jump");

        set_kept_row_in(&f.registry, f.key, None);
        drain_pending();
        assert!(views_of_row(2).is_empty(), "let go, the row leaves with the window");
    }

    /// Cell reuse: a jump that replaces the whole window reuses every cell
    /// that left — each entering position takes a parked cell of its class,
    /// rebound to the new position and framed there — builds only what the
    /// old window couldn't supply, and leaves nothing parked.
    #[test]
    fn a_jump_reuses_the_cells_that_scrolled_away() {
        drain_pending();
        let f = grid(5, 400); // 40x40 cells, 200x200 viewport
        let rebinds: Rc<RefCell<Vec<(u64, usize, usize)>>> = Rc::new(RefCell::new(Vec::new()));
        let log = rebinds.clone();
        set_recycling_in(
            &f.registry,
            &f.node,
            GridRecycling {
                reuse_class: Rc::new(|c, _| c as u64),
                rebind_cell: Rc::new(move |id, c, r| log.borrow_mut().push((id, c, r))),
            },
        );
        sync_in(&f.registry, f.key);
        drain_pending();
        let inst = f.registry.get(&f.key).unwrap();
        while inst.incomplete.get() {
            sync_in(&f.registry, f.key);
            drain_pending();
        }
        let built = f.mounts.get();
        let views_before: std::collections::HashSet<usize> =
            inst.mounted.borrow().values().map(|c| &*c.view as *const NSView as usize).collect();

        let MacosNode::View(scroll) = f.node.clone() else { unreachable!() };
        scroll_clip_to(&scroll, 0.0, 8000.0);
        sync_in(&f.registry, f.key);
        drain_pending();
        while inst.incomplete.get() {
            sync_in(&f.registry, f.key);
            drain_pending();
        }

        // At the top the window is the view plus overscan BELOW (55 cells);
        // mid-grid it has overscan on both sides (80). Every parked cell is
        // reused; only the shortfall is built.
        let window_now = inst.mounted.borrow().len();
        assert_eq!(f.mounts.get() - built, window_now - views_before.len(), "only the shortfall is built");
        assert!(!rebinds.borrow().is_empty());
        for &(_, c, r) in rebinds.borrow().iter() {
            let cell = inst.mounted.borrow().get(&(c, r)).map(|m| m.view.clone()).expect("rebound cell is mounted there");
            let frame: CGRect = unsafe { msg_send![&*cell, frame] };
            assert_eq!((frame.origin.x, frame.origin.y), (c as f64 * 40.0, r as f64 * 40.0), "framed at its new slot");
        }
        let views_after: std::collections::HashSet<usize> =
            inst.mounted.borrow().values().map(|c| &*c.view as *const NSView as usize).collect();
        assert!(views_before.is_subset(&views_after), "every old view is reused, now showing another row");
        assert!(inst.pool.borrow().values().all(Vec::is_empty), "nothing left parked");
        assert_eq!(f.releases.get(), 0, "nothing released");
    }

    fn pending_len() -> usize {
        PENDING.with(|p| p.borrow().len())
    }

    /// Regression: mounting cells inline in `sync`, under the caller's
    /// backend borrow, aborts with `RefCell already borrowed`. A sync must mount
    /// NOTHING itself; the cells appear when the borrow-free drain runs.
    #[test]
    fn regression_sync_mounts_nothing_until_the_borrow_free_drain() {
        drain_pending();
        let f = grid(3, 3);
        sync_in(&f.registry, f.key);
        assert_eq!(f.mounts.get(), 0, "sync mounted under the caller's borrow");
        assert_eq!(pending_len(), 1);
        drain_pending();
        assert_eq!(f.mounts.get(), 9, "every cell of a 3×3 in a 200×200 viewport");
        assert_eq!(pending_len(), 0);
    }

    /// Regression: dropping every cell scope inline in `release` aborts
    /// the same way. The scopes go when the
    /// drain runs, and the cells leave the document with them.
    #[test]
    fn regression_release_defers_cell_scope_drops_until_the_drain() {
        drain_pending();
        let mut f = grid(2, 2);
        sync_in(&f.registry, f.key);
        drain_pending();
        assert_eq!(f.mounts.get(), 4);
        release_in(&mut f.registry, &f.node);
        assert_eq!(f.releases.get(), 0, "release dropped scopes under the caller's borrow");
        assert!(f.registry.is_empty(), "the instance leaves the registry at once");
        drain_pending();
        assert_eq!(f.releases.get(), 4);
    }

    /// Regression (CrewForge 10-05: "`VirtualGridHandle` exposes no
    /// scrollport client size or scrollbar thickness"). The handle reads
    /// the clip view: under the grid's default OVERLAY scrollers the
    /// scrollport is the whole frame, and once the "always show scroll
    /// bars" preference turns the scrollers LEGACY, the horizontal bar the
    /// 4000-wide content needs takes its thickness from the height — with
    /// the grid's frame unchanged, which is why `on_scrollport` must follow
    /// the clip view rather than the layout pass.
    #[test]
    fn regression_handle_reports_the_clip_view_as_the_scrollport() {
        // Scheduled deliveries buffer on this thread and drain below —
        // deterministic whether or not another test installed the
        // main-queue scheduler (whose queue no test thread drains).
        backend_apple_core::scheduler::install_scheduler();
        backend_apple_core::scheduler::begin_mount_buffering();

        let f = grid(100, 3); // 4000 × 120 of content in a 200 × 200 frame.
        sync_in(&f.registry, f.key);
        drain_pending(); // sizes the documentView
        let handle = make_handle(&f.node);
        assert_eq!(handle.scrollport(), Some(Scrollport::overlay(200.0, 200.0)), "overlay: the whole frame");

        let seen: Rc<RefCell<Vec<Scrollport>>> = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        let sub = handle.on_scrollport(move |p| s.borrow_mut().push(p));
        assert_eq!(*seen.borrow(), vec![Scrollport::overlay(200.0, 200.0)]);

        let MacosNode::View(scroll) = &f.node else { unreachable!() };
        let _: () = unsafe { msg_send![&**scroll, setScrollerStyle: 0isize] }; // NSScrollerStyleLegacy
        let _: () = unsafe { msg_send![&**scroll, tile] };
        runtime_shared::drain_buffered_microtasks();
        let bar: f64 = unsafe {
            msg_send![objc2::class!(NSScroller), scrollerWidthForControlSize: 0usize, scrollerStyle: 0isize]
        };
        let bar = bar as f32;
        assert!(bar > 0.0, "a legacy scroller takes room");
        let legacy = Scrollport { width: 200.0, height: 200.0 - bar, scrollbar_width: 0.0, scrollbar_height: bar };
        assert_eq!(handle.scrollport(), Some(legacy), "content fits vertically: horizontal bar only");
        assert_eq!(seen.borrow().last(), Some(&legacy), "followed the style change: {:?}", seen.borrow());

        drop(sub);
        let _: () = unsafe { msg_send![&**scroll, setScrollerStyle: 1isize] };
        let _: () = unsafe { msg_send![&**scroll, tile] };
        runtime_shared::drain_buffered_microtasks();
        assert_eq!(seen.borrow().len(), 2, "a dropped subscription delivers nothing");

        backend_apple_core::scheduler::end_mount_buffering();
    }

    /// The load-bearing omission: queueing a sync must not arm a layout
    /// pass. `sync_all` queues every grid on every pass, so a schedule
    /// here made each pass arm the next — 25,488 passes in 90 s with
    /// nothing changing.
    ///
    /// The bug has two faces and this asserts both, because which one
    /// shows depends on the test binary: with the real scheduler
    /// installed (the `newcore` suite does, first-wins) the pass is
    /// DEFERRED and the flag is armed; with none, `schedule_microtask`
    /// runs inline and the pass — which drains the queue on its way out
    /// — mounts the cells during the sync itself.
    #[test]
    fn regression_queuing_a_sync_arms_no_layout_pass() {
        drain_pending();
        let f = grid(2, 2);
        sync_in(&f.registry, f.key);
        assert!(!crate::imp::layout_pass_is_queued(), "queue_sync armed a deferred pass");
        assert_eq!(f.mounts.get(), 0, "queue_sync ran a pass inline, which drained the queue");
        drain_pending();
    }

    /// Two syncs of one grid before a drain are one job — the diff is
    /// computed at drain time, so the later subsumes the earlier — while
    /// another grid's sync keeps its own slot.
    #[test]
    fn repeated_syncs_of_one_grid_coalesce_and_other_grids_keep_theirs() {
        drain_pending();
        let a = grid(1, 1);
        let b = grid(1, 1);
        sync_in(&a.registry, a.key);
        sync_in(&a.registry, a.key);
        assert_eq!(pending_len(), 1);
        sync_in(&b.registry, b.key);
        assert_eq!(pending_len(), 2);
        drain_pending();
        assert_eq!((a.mounts.get(), b.mounts.get()), (1, 1));
    }

    /// A teardown's scope cleanups can queue another grid's work; the
    /// drain loops until the queue is empty rather than leaving it for
    /// a pass that may never come.
    #[test]
    fn drain_runs_work_queued_while_it_drains() {
        drain_pending();
        let ran = Rc::new(Cell::new(0usize));
        let inner = ran.clone();
        let second: Rc<dyn Fn(u64)> = Rc::new(move |_| inner.set(inner.get() + 1));
        let outer = ran.clone();
        let first: Rc<dyn Fn(u64)> = Rc::new(move |_| {
            outer.set(outer.get() + 1);
            PENDING.with(|p| {
                p.borrow_mut().push(PendingJob::Teardown(TeardownJob {
                    cells: vec![cell()],
                    release_cell: Some(second.clone()),
                }))
            });
        });
        PENDING.with(|p| {
            p.borrow_mut().push(PendingJob::Teardown(TeardownJob {
                cells: vec![cell()],
                release_cell: Some(first),
            }))
        });
        drain_pending();
        assert_eq!(ran.get(), 2);
        assert_eq!(pending_len(), 0);
    }

    fn cell() -> MountedCell {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        MountedCell {
            view: Retained::into_super(crate::imp::view::FlippedView::new(mtm)),
            scope_id: 0,
            key: 0,
            bands: (Band::Scrolling, Band::Scrolling),
        }
    }

    fn view_at(f: &Fixture, slot: (usize, usize)) -> Option<Retained<NSView>> {
        f.registry
            .get(&f.key)
            .and_then(|i| i.mounted.borrow().get(&slot).map(|m| m.view.clone()))
    }

    /// Regression: a data change that rewrote one cell's content left
    /// the cell's old view in place — `mounted` was keyed by position
    /// alone, so a filled slot was skipped whatever `cell_key` said.
    /// The changed cell is rebuilt (its stale scope released first, so
    /// the count stays flat); the other three are left alone.
    #[test]
    fn regression_a_cell_whose_content_changed_is_remounted_on_data_changed() {
        drain_pending();
        let mut f = grid(2, 2);
        sync_in(&f.registry, f.key);
        drain_pending();
        assert_eq!(f.mounts.get(), 4);
        let before = view_at(&f, (0, 0)).expect("mounted");
        let neighbour = view_at(&f, (1, 1)).expect("mounted");

        f.origin_version.set(1);
        data_changed_in(&mut f.registry, &f.node);
        drain_pending();

        assert_eq!(f.releases.get(), 1, "the stale cell's scope goes");
        assert_eq!(f.mounts.get(), 5, "exactly the changed cell is rebuilt");
        let after = view_at(&f, (0, 0)).expect("mounted");
        assert!(!std::ptr::eq(&*before, &*after), "the old view stayed");
        assert!(
            std::ptr::eq(&*neighbour, &*view_at(&f, (1, 1)).expect("mounted")),
            "an unchanged cell was churned"
        );
        let superview: *mut NSView = unsafe { msg_send![&*before, superview] };
        assert!(superview.is_null(), "the stale view is still in the document");
    }

    fn frame_of(f: &Fixture, slot: (usize, usize)) -> (f64, f64, f64) {
        let v = view_at(f, slot).unwrap_or_else(|| panic!("{slot:?} is mounted"));
        let r: CGRect = unsafe { msg_send![&*v, frame] };
        (r.origin.x, r.origin.y, r.size.width)
    }

    /// Where cell `slot` shows, in the SCROLL VIEW's coordinates — the
    /// visible box, whatever container the cell sits in.
    fn on_screen(f: &Fixture, slot: (usize, usize)) -> (f64, f64) {
        let v = view_at(f, slot).unwrap_or_else(|| panic!("{slot:?} is mounted"));
        let MacosNode::View(scroll) = &f.node else { unreachable!() };
        let frame: CGRect = unsafe { msg_send![&*v, frame] };
        let sup: *mut NSView = unsafe { msg_send![&*v, superview] };
        let r: CGRect = unsafe { msg_send![&**scroll, convertRect: frame, fromView: sup] };
        (r.origin.x.round(), r.origin.y.round())
    }

    /// Frozen panes on AppKit: after scrolling both axes the header row, the
    /// pinned columns and the corner show at the sticky position, and a
    /// further scroll moves none of them — AppKit holds them (floating
    /// subviews, the scroll view itself), so nothing has to re-place them
    /// from the scroll notification, which arrives after an asynchronous
    /// scroll has already moved the content. The regression: frozen cells
    /// were document subviews moved on that notification, and rode along
    /// with the rows before snapping back.
    #[test]
    fn regression_frozen_panes_ride_along_with_the_scroll() {
        drain_pending();
        let mut f = grid(10, 100); // 40x40 cells, 200x200 scroller
        set_frozen_in(
            &mut f.registry,
            &f.node,
            FrozenPanes { top: 1, left: 1, right: 1, ..FrozenPanes::NONE },
        );
        drain_pending();
        let MacosNode::View(scroll) = f.node.clone() else { unreachable!() };
        scroll_clip_to(&scroll, 120.0, 800.0);
        sync_in(&f.registry, f.key);
        drain_pending();

        assert_eq!(on_screen(&f, (0, 0)), (0.0, 0.0), "corner");
        assert_eq!(on_screen(&f, (4, 0)), (40.0, 0.0), "header: scrolled sideways, held at the top");
        assert_eq!(on_screen(&f, (0, 22)), (0.0, 80.0), "left pin: scrolled down, held at the left");
        assert_eq!(on_screen(&f, (9, 22)), (160.0, 80.0), "right pin at the right edge");
        assert_eq!(on_screen(&f, (4, 22)), (40.0, 80.0), "body cell");

        // Scroll WITHOUT a sync — what an asynchronous scroll looks like
        // before the main thread hears of it. Frozen cells stay put; the
        // body moves with the content.
        scroll_clip_to(&scroll, 121.0, 801.0);
        assert_eq!(on_screen(&f, (0, 0)), (0.0, 0.0));
        assert_eq!(on_screen(&f, (4, 0)), (39.0, 0.0), "the header still scrolls sideways");
        assert_eq!(on_screen(&f, (0, 22)), (0.0, 79.0), "the left pin still scrolls down");
        assert_eq!(on_screen(&f, (4, 22)), (39.0, 79.0));
    }

    /// Z-order comes from the containers, so it can't depend on the order
    /// cells mounted in: a corner is a scroll-view subview (above the clip
    /// view and its floating layer), an edge cell floats above the document,
    /// a body cell is in the document. The regression: cells were ordered one
    /// by one, below "a" corner, and a row's selection cell landed above the
    /// select-all corner.
    #[test]
    fn regression_a_frozen_column_cell_draws_over_the_header_corner() {
        drain_pending();
        let mut f = grid(6, 40);
        set_frozen_in(&mut f.registry, &f.node, FrozenPanes { top: 1, left: 2, ..FrozenPanes::NONE });
        drain_pending();
        let MacosNode::View(scroll) = f.node.clone() else { unreachable!() };
        let doc = f.registry.get(&f.key).unwrap().document_view.clone();
        let parent = |slot| -> *mut NSView {
            let v = view_at(&f, slot).expect("mounted");
            unsafe { msg_send![&*v, superview] }
        };
        let scroll_ptr = &*scroll as *const NSView as *mut NSView;
        let doc_ptr = &*doc as *const NSView as *mut NSView;
        for corner in [(0, 0), (1, 0)] {
            assert_eq!(parent(corner), scroll_ptr, "corner {corner:?} sits on the scroll view");
        }
        for edge in [(0, 3), (1, 3), (3, 0)] {
            let p = parent(edge);
            assert!(p != scroll_ptr && p != doc_ptr, "edge cell {edge:?} floats");
        }
        assert_eq!(parent((3, 3)), doc_ptr, "a body cell scrolls in the document");
    }

    /// The control for the pool: a data change that changes nothing
    /// mounts and releases nothing.
    #[test]
    fn a_data_change_with_unchanged_keys_leaves_every_cell_alone() {
        drain_pending();
        let mut f = grid(2, 2);
        sync_in(&f.registry, f.key);
        drain_pending();
        data_changed_in(&mut f.registry, &f.node);
        drain_pending();
        assert_eq!((f.mounts.get(), f.releases.get()), (4, 0));
    }
}

#[cfg(test)]
mod intrinsic_size_tests {
    //! The grid's intrinsic size through a REAL `MacosBackend`: the
    //! measure_fn `create_virtual_grid_impl` installs, and the
    //! re-measure `data_changed` triggers.
    use super::*;
    use std::cell::Cell;

    /// Regression (CrewForge: "a grid cannot fit its content"). A grid
    /// styled to fit (`flex_grow: 0` + `flex_basis: auto`) inside a
    /// 390×725 column must be exactly as tall as its rows — and follow
    /// them when the row count changes, which only happens if
    /// `data_changed` dirties the node (Taffy caches the old measure).
    #[test]
    fn regression_fit_content_grid_is_as_tall_as_its_rows_and_follows_them() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = crate::imp::MacosBackend::new(mtm);
        let rows = Rc::new(Cell::new(3usize));
        let r = rows.clone();
        let callbacks = GridCallbacks::<MacosNode> {
            // 30 × 120 = 3600 wide: far wider than the 390 column.
            col_count: Rc::new(|| 30),
            row_count: Rc::new(move || r.get()),
            col_width: Rc::new(|_| 120.0),
            row_height: Rc::new(|_| 40.0),
            cell_key: Rc::new(|c, r| (c * 1000 + r) as u64),
            mount_cell: Rc::new(move |_, _| {
                let view: Retained<NSView> =
                    Retained::into_super(crate::imp::view::FlippedView::new(mtm));
                (MacosNode::View(view), 0)
            }),
            release_cell: Rc::new(|_| {}),
            on_scroll: None,
        };
        let node = backend.create_virtual_grid_impl(callbacks, 1.0, &Default::default());
        let grid = backend.layout_of(node.as_view()).expect("grid has a layout node");
        let mut fit = runtime_shared::StyleRules::default();
        fit.flex_grow = Some(0.0f32.into());
        fit.flex_basis = Some(runtime_shared::Length::Auto.into());
        backend.layout.set_style(grid, &fit);
        let root = backend.layout.new_node();
        let mut rs = runtime_shared::StyleRules::default();
        rs.width = Some(runtime_shared::Length::Px(390.0).into());
        rs.height = Some(runtime_shared::Length::Px(725.0).into());
        backend.layout.set_style(root, &rs);
        backend.layout.add_child(root, grid);

        backend.layout.compute(root, 390.0, 725.0);
        assert_eq!(backend.layout.frame_of(grid).height, 120.0, "3 rows × 40, no gutter (overlay scrollers)");

        rows.set(5);
        backend.virtual_grid_data_changed_impl(&node);
        backend.layout.compute(root, 390.0, 725.0);
        assert_eq!(backend.layout.frame_of(grid).height, 200.0, "re-measured to 5 rows");

        backend.release_virtual_grid_impl(&node);
        drain_pending();
    }

    /// Lay out a fit-content grid (30 × 120 wide, `rows` × 40 tall) in a
    /// 390 × 725 column and apply the frame to the real `NSScrollView`.
    /// Returns `(backend, node, layout height, clip-view height)`.
    fn fit_grid_in_legacy_style(legacy_before_create: bool) -> (crate::imp::MacosBackend, MacosNode, f32, f32) {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = crate::imp::MacosBackend::new(mtm);
        let callbacks = GridCallbacks::<MacosNode> {
            col_count: Rc::new(|| 30),
            row_count: Rc::new(|| 3),
            col_width: Rc::new(|_| 120.0),
            row_height: Rc::new(|_| 40.0),
            cell_key: Rc::new(|c, r| (c * 1000 + r) as u64),
            mount_cell: Rc::new(move |_, _| {
                let view: Retained<NSView> =
                    Retained::into_super(crate::imp::view::FlippedView::new(mtm));
                (MacosNode::View(view), 0)
            }),
            release_cell: Rc::new(|_| {}),
            on_scroll: None,
        };
        let node = backend.create_virtual_grid_impl(callbacks, 1.0, &Default::default());
        let scroll = node.as_view();
        if legacy_before_create {
            set_style_and_remeasure(&mut backend, &node, 0);
        }
        let grid = backend.layout_of(scroll).expect("grid has a layout node");
        let mut fit = runtime_shared::StyleRules::default();
        fit.flex_grow = Some(0.0f32.into());
        fit.flex_basis = Some(runtime_shared::Length::Auto.into());
        backend.layout.set_style(grid, &fit);
        let root = backend.layout.new_node();
        let mut rs = runtime_shared::StyleRules::default();
        rs.width = Some(runtime_shared::Length::Px(390.0).into());
        rs.height = Some(runtime_shared::Length::Px(725.0).into());
        backend.layout.set_style(root, &rs);
        backend.layout.add_child(root, grid);
        if !legacy_before_create {
            set_style_and_remeasure(&mut backend, &node, 0);
        }
        backend.layout.compute(root, 390.0, 725.0);
        let f = backend.layout.frame_of(grid);
        let _: () = unsafe {
            msg_send![scroll, setFrame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(f.width as f64, f.height as f64))]
        };
        // Size the documentView (what the scroller's auto-show reads).
        let key = scroll as *const NSView as usize;
        sync_in(&backend.virtual_grid_registry, key);
        drain_pending();
        let _: () = unsafe { msg_send![scroll, tile] };
        let clip: Option<Retained<NSView>> = unsafe { msg_send_id![scroll, contentView] };
        let clip_frame: CGRect = unsafe { msg_send![&clip.unwrap(), frame] };
        (backend, node, f.height, clip_frame.size.height as f32)
    }

    /// REGRESSION: scrolling down a grid jumped back to the top once new rows
    /// mounted (around row 20), and the elastic bounce past the top made the
    /// cells jump up and down. Mounting dirties the layout, and the layout
    /// pass's `sync_scroll_document_views` resized EVERY scroll view's
    /// document view to its Taffy children's bounding box — for a grid,
    /// whose cells are not Taffy children, the viewport. AppKit then clamped
    /// the scroll offset back into the shrunk document. The grid sizes its
    /// own document; the pass must leave it alone.
    #[test]
    fn regression_a_layout_pass_scrolls_the_grid_back_to_the_top() {
        drain_pending();
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = crate::imp::MacosBackend::new(mtm);
        let callbacks = GridCallbacks::<MacosNode> {
            col_count: Rc::new(|| 5),
            row_count: Rc::new(|| 100),
            col_width: Rc::new(|_| 40.0),
            row_height: Rc::new(|_| 40.0),
            cell_key: Rc::new(|c, r| (c * 1000 + r) as u64),
            mount_cell: Rc::new(move |_, _| {
                let view: Retained<NSView> = Retained::into_super(crate::imp::view::FlippedView::new(mtm));
                (MacosNode::View(view), 0)
            }),
            release_cell: Rc::new(|_| {}),
            on_scroll: None,
        };
        let node = backend.create_virtual_grid_impl(callbacks, 1.0, &Default::default());
        let scroll = node.as_view();
        let _: () = unsafe {
            msg_send![scroll, setFrame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(200.0, 200.0))]
        };
        let key = scroll as *const NSView as usize;
        sync_in(&backend.virtual_grid_registry, key);
        drain_pending();
        let retained: Retained<NSView> = unsafe { Retained::retain(scroll as *const NSView as *mut NSView) }.unwrap();
        scroll_clip_to(&retained, 0.0, 800.0);

        backend.sync_scroll_document_views();

        let ((_, y), _) = clip_offset_and_size(scroll);
        assert_eq!(y, 800.0, "the scroll offset survives a layout pass");
        let doc = backend.virtual_grid_registry.get(&key).unwrap().document_view.clone();
        let frame: CGRect = unsafe { msg_send![&doc, frame] };
        assert_eq!(frame.size.height, 4000.0, "the grid's document keeps the content height");
    }

    /// What the "show scroll bars" preference does to a live grid: AppKit
    /// sets `scrollerStyle` on the scroll view, and the backend hears it
    /// through `scroller_style_changed`.
    fn set_style_and_remeasure(backend: &mut crate::imp::MacosBackend, node: &MacosNode, style: isize) {
        let _: () = unsafe { msg_send![node.as_view(), setScrollerStyle: style] };
        scroller_style_changed(backend, node.as_view() as *const NSView as usize);
    }

    /// Regression: a fit-content grid measured with a gutter of 0 assumed
    /// OVERLAY scrollers. Under the LEGACY style ("Always show scroll
    /// bars", or a mouse attached with "Automatic") the horizontal scroller
    /// takes its thickness from the clip view, so the 120-tall rows sat in
    /// a 105-tall clip — the last row cut, the grid scrolling vertically:
    /// the exact web bug `1f95d84c` fixed, back on macOS.
    #[test]
    fn regression_fit_content_grid_keeps_its_last_row_under_legacy_scrollers() {
        for legacy_before_layout in [true, false] {
            let (mut backend, node, laid_out, clip_h) = fit_grid_in_legacy_style(legacy_before_layout);
            let bar: f64 = unsafe {
                msg_send![objc2::class!(NSScroller), scrollerWidthForControlSize: 0usize, scrollerStyle: 0isize]
            };
            assert!(
                clip_h >= 120.0,
                "every row visible: clip {clip_h}, laid out {laid_out}, bar {bar} ({legacy_before_layout})"
            );
            assert_eq!(laid_out, 120.0 + bar as f32, "rows + the legacy bar ({legacy_before_layout})");
            backend.release_virtual_grid_impl(&node);
            drain_pending();
        }
    }

    /// The wiring: the preference notification alone (AppKit restyling the
    /// scroll view, then posting `NSPreferredScrollerStyleDidChangeNotification`)
    /// reaches the grid and re-measures it — no test-side call to
    /// `scroller_style_changed`.
    #[test]
    fn the_scroller_style_preference_notification_remeasures_a_mounted_grid() {
        backend_apple_core::scheduler::install_scheduler();
        backend_apple_core::scheduler::begin_mount_buffering();
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let backend = Rc::new(RefCell::new(crate::imp::MacosBackend::new(mtm)));
        crate::imp::install_global_self(Rc::downgrade(&backend));
        let (node, grid, root) = {
            let mut b = backend.borrow_mut();
            let callbacks = GridCallbacks::<MacosNode> {
                col_count: Rc::new(|| 30),
                row_count: Rc::new(|| 3),
                col_width: Rc::new(|_| 120.0),
                row_height: Rc::new(|_| 40.0),
                cell_key: Rc::new(|c, r| (c * 1000 + r) as u64),
                mount_cell: Rc::new(move |_, _| {
                    let view: Retained<NSView> =
                        Retained::into_super(crate::imp::view::FlippedView::new(mtm));
                    (MacosNode::View(view), 0)
                }),
                release_cell: Rc::new(|_| {}),
                on_scroll: None,
            };
            let node = b.create_virtual_grid_impl(callbacks, 1.0, &Default::default());
            let grid = b.layout_of(node.as_view()).unwrap();
            let mut fit = runtime_shared::StyleRules::default();
            fit.flex_grow = Some(0.0f32.into());
            fit.flex_basis = Some(runtime_shared::Length::Auto.into());
            b.layout.set_style(grid, &fit);
            let root = b.layout.new_node();
            let mut rs = runtime_shared::StyleRules::default();
            rs.width = Some(runtime_shared::Length::Px(390.0).into());
            rs.height = Some(runtime_shared::Length::Px(725.0).into());
            b.layout.set_style(root, &rs);
            b.layout.add_child(root, grid);
            b.layout.compute(root, 390.0, 725.0);
            assert_eq!(b.layout.frame_of(grid).height, 120.0, "overlay: rows only");
            (node, grid, root)
        };

        let _: () = unsafe { msg_send![node.as_view(), setScrollerStyle: 0isize] };
        let center: *mut objc2::runtime::AnyObject =
            unsafe { msg_send![objc2::class!(NSNotificationCenter), defaultCenter] };
        let name = objc2_foundation::NSString::from_str("NSPreferredScrollerStyleDidChangeNotification");
        let nil: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
        let _: () = unsafe { msg_send![center, postNotificationName: &*name, object: nil] };
        runtime_shared::drain_buffered_microtasks();

        let bar: f64 = unsafe {
            msg_send![objc2::class!(NSScroller), scrollerWidthForControlSize: 0usize, scrollerStyle: 0isize]
        };
        {
            let mut b = backend.borrow_mut();
            b.layout.compute(root, 390.0, 725.0);
            assert_eq!(b.layout.frame_of(grid).height, 120.0 + bar as f32, "re-measured with the legacy bar");
            b.release_virtual_grid_impl(&node);
        }
        drain_pending();
        backend_apple_core::scheduler::end_mount_buffering();
    }

    /// Back to overlay: the measurement drops the gutter again.
    #[test]
    fn fit_content_grid_drops_the_gutter_when_scrollers_overlay_again() {
        let (mut backend, node, _, _) = fit_grid_in_legacy_style(true);
        set_style_and_remeasure(&mut backend, &node, 1);
        let grid = backend.layout_of(node.as_view()).unwrap();
        let root = backend.layout.parent_of(grid).expect("in the column");
        backend.layout.compute(root, 390.0, 725.0);
        assert_eq!(backend.layout.frame_of(grid).height, 120.0);
        backend.release_virtual_grid_impl(&node);
        drain_pending();
    }
}


