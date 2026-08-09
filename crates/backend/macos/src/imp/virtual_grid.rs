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
}

/// Re-window one grid: resize the document view to the content
/// extent, then diff the visible cell rect.
pub(crate) fn sync(backend: &mut crate::imp::MacosBackend, key: usize) {
    let Some(inst) = backend.virtual_grid_registry.get(&key) else {
        return;
    };
    // Clone handles so the backend borrow ends before `mount_cell`
    // runs — it realizes a subtree, re-entering the backend.
    let scroll = inst.scroll_view.clone();
    let document = inst.document_view.clone();
    let callbacks = inst.callbacks.clone();
    let metrics = inst.metrics.clone();
    let mounted = inst.mounted.clone();
    let last_window = inst.last_window.clone();

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
    let MacosNode::View(view) = node else { return };
    let key = &**view as *const NSView as usize;
    let Some(mut inst) = backend.virtual_grid_registry.remove(&key) else {
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
    let drained: Vec<MountedCell> = inst.mounted.borrow_mut().drain().map(|(_, v)| v).collect();
    for cell in drained {
        let _: () = unsafe { msg_send![&cell.view, removeFromSuperview] };
        if let Some(release) = release_cell.as_ref() {
            release(cell.scope_id);
        }
    }
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
