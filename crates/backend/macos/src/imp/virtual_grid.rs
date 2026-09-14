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

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_app_kit::NSView;
use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSObject};
use runtime_shared::primitives::virtual_grid::{GridCallbacks, GridMetrics, GridWindow};

use super::MacosNode;

struct MountedCell {
    view: Retained<NSView>,
    scope_id: u64,
}

pub(crate) struct VirtualGridInstance {
    scroll_view: Retained<NSView>,
    document_view: Retained<NSView>,
    callbacks: Rc<RefCell<Option<GridCallbacks<MacosNode>>>>,
    metrics: Rc<RefCell<GridMetrics>>,
    mounted: Rc<RefCell<HashMap<(usize, usize), MountedCell>>>,
    last_window: Rc<RefCell<Option<GridWindow>>>,
    /// Bounds-change observer; the notification center holds it
    /// non-owningly, so the instance must.
    observer: Option<Retained<NSObject>>,
}

pub(crate) type GridRegistry = HashMap<usize, VirtualGridInstance>;

pub(crate) fn create(
    mtm: MainThreadMarker,
    registry: &mut GridRegistry,
    callbacks: GridCallbacks<MacosNode>,
    _overscan: f32,
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

    registry.insert(
        key,
        VirtualGridInstance {
            scroll_view: scroll.clone(),
            document_view,
            callbacks,
            metrics,
            mounted: Rc::new(RefCell::new(HashMap::new())),
            last_window: Rc::new(RefCell::new(None)),
            observer,
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

pub(crate) fn data_changed(backend: &mut crate::imp::MacosBackend, node: &MacosNode) {
    let MacosNode::View(view) = node else { return };
    let key = &**view as *const NSView as usize;
    {
        let Some(inst) = backend.virtual_grid_registry.get(&key) else {
            return;
        };
        let Some(m) = inst.callbacks.borrow().as_ref().map(build_metrics) else {
            return;
        };
        *inst.metrics.borrow_mut() = m;
        *inst.last_window.borrow_mut() = None;
    }
    sync(backend, key);
    // The one queueing path with no layout pass already behind it, so
    // it arms the drain itself. Safe where `queue_sync` is not: this
    // fires on a real data change, not once per pass.
    crate::imp::schedule_layout_pass();
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
    /// `mount_cell` realizes a subtree and `release_cell` drops one, and
    /// both re-enter the backend through `create_*` / scope cleanups —
    /// `handlers::view::mount_view` opens with `backend.borrow_mut()`.
    /// Every path into a sync already holds that borrow: the layout pass
    /// runs inside `with_global_backend`, `virtual_grid_data_changed` is
    /// dispatched through `backend.borrow_mut()`, and the bounds
    /// observer goes through `with_backend`. Cloning the instance's
    /// `Rc`s — which this module used to do, with a comment claiming it
    /// ended the borrow — does not end it: the borrow is the CALLER's.
    ///
    /// So the borrow-free half is queued here and drained where the
    /// borrow is provably gone. Cells mount one pass later than the
    /// layout that discovered them and mark the tree dirty on mount, so
    /// the following pass frames them — already true of the first fill,
    /// which `create` defers for this same reason.
    ///
    /// Proven on iOS 2026-09-14, where wiring `GridOps` up for the first
    /// time aborted the app with `RefCell already borrowed` inside
    /// `virtual_grid::mount_cell` the moment a grid mounted. This
    /// backend had the identical structure and the identical latent bug.
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
/// two-axis form of the virtualizer teardown bug (FRAMEWORK-NOTES
/// Wave-19).
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
        },
    );
}

/// The body of the old `sync`, with no backend borrow held.
fn sync_now(h: &GridHandles) {
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
    let clip_bounds: CGRect = unsafe { msg_send![&clip, bounds] };
    if clip_bounds.size.width <= 0.0 || clip_bounds.size.height <= 0.0 {
        // Layout hasn't run. Bail WITHOUT caching, so the next pass
        // retries instead of remembering an empty window as current.
        return;
    }

    let overscan = 1.0;
    let window = metrics.borrow().visible_window(
        (clip_bounds.origin.x as f32, clip_bounds.origin.y as f32),
        (
            clip_bounds.size.width as f32,
            clip_bounds.size.height as f32,
        ),
        overscan,
    );

    if *last_window.borrow() == Some(window) {
        return;
    }
    *last_window.borrow_mut() = Some(window);

    let leaving: Vec<(usize, usize)> = mounted
        .borrow()
        .keys()
        .copied()
        .filter(|(c, r)| !window.contains(*c, *r))
        .collect();
    for slot in leaving {
        let cell = mounted.borrow_mut().remove(&slot);
        if let Some(cell) = cell {
            let _: () = unsafe { msg_send![&cell.view, removeFromSuperview] };
            let release = callbacks.borrow().as_ref().map(|c| c.release_cell.clone());
            if let Some(release) = release {
                release(cell.scope_id);
            }
        }
    }

    for (col, row) in window.cells() {
        if mounted.borrow().contains_key(&(col, row)) {
            continue;
        }
        let mount = callbacks.borrow().as_ref().map(|c| c.mount_cell.clone());
        let Some(mount) = mount else { break };
        let (node, scope_id) = mount(col, row);
        let MacosNode::View(view) = &node else { continue };
        let (x, y) = metrics.borrow().cell_origin(col, row);
        let (w, h) = metrics.borrow().cell_size(col, row);
        let frame = CGRect {
            origin: CGPoint { x: x as f64, y: y as f64 },
            size: CGSize::new(w as f64, h as f64),
        };
        let _: () = unsafe { msg_send![&**view, setFrame: frame] };
        let _: () = unsafe { msg_send![&document, addSubview: &**view] };
        mounted.borrow_mut().insert(
            (col, row),
            MountedCell {
                view: view.clone(),
                scope_id,
            },
        );
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
    let cbs = inst.callbacks.borrow_mut().take();
    let release_cell = cbs.as_ref().map(|c| c.release_cell.clone());
    let cells: Vec<MountedCell> = inst.mounted.borrow_mut().drain().map(|(_, v)| v).collect();
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
        let origin: std::cell::Cell<Option<(f32, f32)>> = std::cell::Cell::new(None);
        crate::imp::with_backend(|b| {
            if let Some(i) = b.virtual_grid_registry.get(&key) {
                origin.set(Some(i.metrics.borrow().cell_origin(col, row)));
            }
        });
        let Some((x, y)) = origin.get() else { return };
        scroll_clip_to(view, x, y);
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
    }

    fn grid(cols: usize, rows: usize) -> Fixture {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mounts = Rc::new(Cell::new(0usize));
        let releases = Rc::new(Cell::new(0usize));
        let m = mounts.clone();
        let r = releases.clone();
        let callbacks = GridCallbacks::<MacosNode> {
            col_count: Rc::new(move || cols),
            row_count: Rc::new(move || rows),
            col_width: Rc::new(|_| 40.0),
            row_height: Rc::new(|_| 40.0),
            cell_key: Rc::new(|c, r| (c * 1000 + r) as u64),
            mount_cell: Rc::new(move |c, r| {
                m.set(m.get() + 1);
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
        Fixture { registry, node: MacosNode::View(scroll), key, mounts, releases }
    }

    fn pending_len() -> usize {
        PENDING.with(|p| p.borrow().len())
    }

    /// Regression: `sync` used to mount cells inline, under the
    /// caller's backend borrow, and the first grid to mount on iOS
    /// aborted with `RefCell already borrowed`. A sync must mount
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

    /// Regression: `release` used to drop every cell scope inline, for
    /// the same reason and with the same abort. The scopes go when the
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
        }
    }
}
