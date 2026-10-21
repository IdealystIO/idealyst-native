//! `VirtualizerPrim` — a `gtk::ScrolledWindow` over a `gtk::Fixed`
//! "document" tall enough for the whole data set, with a scroll handler
//! that realizes/positions only the cells in the viewport (plus an
//! overscan margin) and recycles cells that scroll out of view.
//!
//! ## Windowing model (this IS windowed, not realize-all)
//!
//! - The `Fixed` is sized to `Σ item_size + gaps` along the scroll axis,
//!   so the scrollbar reflects the full content extent even though only a
//!   handful of cells exist as widgets at any moment.
//! - On every scroll / resize we recompute the visible index range
//!   ([`visible_range`]), mount cells that entered it (via
//!   `VirtualizerCallbacks::mount_item`, which builds a fresh per-item
//!   reactive scope), and recycle cells that left it (via `release_item`,
//!   which drops that scope — freeing its signals/effects).
//! - A mounted cell subtree is a Taffy *orphan* (built outside the app
//!   root, parented into the `Fixed` on the GTK side only). The main
//!   layout pass — scoped to nodes reachable from the framework root —
//!   never frames it, so we lay each cell out against its allocated box
//!   via [`super::LinuxBackend::layout_detached_root`] (the same
//!   subtree-scoped pass the portal uses). Without this every widget
//!   inside a cell stays 0×0.
//!
//! ## State ownership
//!
//! Per-virtualizer state lives in a thread-local map keyed by the node
//! id, NOT on `LinuxBackend` — GTK is single-threaded so a thread-local
//! is safe, and it keeps virtualizer wiring self-contained in this module
//! (the backend just forwards `create`/`data_changed`/`release`). The
//! adjustment signal closures hold a clone of the `Rc<RefCell<State>>`;
//! `release` disconnects those signals and drops the map entry, breaking
//! the widget→signal→closure→state cycle.
//!
//! ## Item-size strategy (matches macOS / iOS Phase-1)
//!
//! `ItemSize::Known` is authoritative. `ItemSize::Measured` is treated as
//! `Known` using the author's estimate — the framework's measure pass
//! doesn't reach into a detached cell subtree, the same core gap the
//! Apple backends documented. `set_measured_size` is therefore not
//! called; a measured list lays out from the estimate. Refining this
//! needs a framework-core change to measure orphaned subtrees.
//!
//! ## Row stacking
//!
//! Rows are siblings in the `Fixed`, which paints its children in sibling
//! order and picks them last-sibling-first. On web a virtualized list's
//! rows are plain siblings in one container, so a `z_index` on a row's
//! root orders it against the neighbouring rows. The same holds here: the
//! `Fixed`'s sibling list is kept in `runtime_shared::style::z_paint_order`
//! of the row roots' stacking z, ties in ROW order (not mount order —
//! scrolling back up mounts earlier rows after later ones). The backend
//! reports a row root's z changes through [`cell_z_changed`]; a row root
//! has no `parent_of` entry (it is a detached Taffy root), so the
//! backend's own `reorder_siblings` never sees it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gtk4::glib;
use gtk4::prelude::*;

use runtime_shared::{VirtualLayout, VirtualizerCallbacks};

use crate::{IdealystView, LinuxBackend, LinuxNode};

// =========================================================================
// Per-cell + per-virtualizer state
// =========================================================================

struct MountedCell {
    node: LinuxNode,
    scope_id: u64,
    /// The row root's stacking z (`StyleRules::stacking_z()` / animated
    /// `ZIndex`), mirrored from the backend so a restack needs no backend
    /// borrow. Read at mount, then updated by [`cell_z_changed`].
    z: f32,
}

struct State {
    callbacks: VirtualizerCallbacks<LinuxNode>,
    layout: VirtualLayout,
    overscan: f32,
    horizontal: bool,
    fixed: gtk4::Fixed,
    scrolled: gtk4::ScrolledWindow,
    backend: Weak<RefCell<LinuxBackend>>,
    /// index → mounted cell. Only visible-range cells are present.
    mounted: HashMap<usize, MountedCell>,
    /// Adjustment signal-handler ids, disconnected on release so the
    /// closure (which holds the `Rc<RefCell<State>>`) is freed.
    handlers: Vec<(gtk4::Adjustment, glib::SignalHandlerId)>,
    /// Flipped false by `release`; every queued closure checks it and
    /// bails so a scroll/resize event landing after teardown is inert.
    alive: bool,
    /// A deferred reload (see [`data_changed`]) is queued and has not run
    /// yet; further data changes in the same turn ride on it.
    reload_pending: bool,
}

thread_local! {
    static STATES: RefCell<HashMap<u64, Rc<RefCell<State>>>> =
        RefCell::new(HashMap::new());
}

// =========================================================================
// Pure windowing math (unit-tested — no GTK context)
// =========================================================================

/// Cumulative start offsets for each item along the scroll axis, plus the
/// total extent. `offsets[i]` is the main-axis position of item `i`'s
/// leading edge; successive items are separated by `main_gap`. Total
/// includes the inter-item gaps but no trailing gap.
pub(crate) fn cumulative(sizes: &[f32], main_gap: f32) -> (Vec<f32>, f32) {
    let mut offsets = Vec::with_capacity(sizes.len());
    let mut acc = 0.0f32;
    for (i, &s) in sizes.iter().enumerate() {
        if i > 0 {
            acc += main_gap;
        }
        offsets.push(acc);
        acc += s.max(0.0);
    }
    (offsets, acc)
}

/// The half-open index range `[first, last)` whose items intersect the
/// viewport `[scroll, scroll + viewport)` expanded by `overscan` viewport
/// -extents on each side. Returns `(0, 0)` for an empty / unmeasured list.
///
/// `offsets` + `sizes` are parallel (see [`cumulative`]).
pub(crate) fn visible_range(
    offsets: &[f32],
    sizes: &[f32],
    scroll: f32,
    viewport: f32,
    overscan: f32,
) -> (usize, usize) {
    debug_assert_eq!(offsets.len(), sizes.len());
    if sizes.is_empty() || viewport <= 0.0 {
        return (0, 0);
    }
    let pad = viewport * overscan.max(0.0);
    let top = (scroll - pad).max(0.0);
    let bottom = scroll + viewport + pad;
    let mut first = sizes.len();
    let mut last = 0usize;
    for i in 0..sizes.len() {
        let start = offsets[i];
        let end = start + sizes[i].max(0.0);
        // Intersects the window if it ends after `top` and starts before
        // `bottom`. Zero-size items (end == start) still count if inside.
        if end > top && start < bottom {
            if i < first {
                first = i;
            }
            last = i + 1;
        } else if start >= bottom {
            // Items are monotonically increasing in offset — once one
            // starts past the window, all later ones do too.
            break;
        }
    }
    if first >= last {
        (0, 0)
    } else {
        (first, last)
    }
}

// =========================================================================
// Backend entry points (called from `LinuxBackend`)
// =========================================================================

/// Stand up a virtualizer: register the state and wire scroll/resize
/// handlers. `node_id` is the wrapped `ScrolledWindow` node's id.
pub(crate) fn create(
    node_id: u64,
    scrolled: gtk4::ScrolledWindow,
    fixed: gtk4::Fixed,
    callbacks: VirtualizerCallbacks<LinuxNode>,
    overscan: f32,
    layout: VirtualLayout,
    backend: Weak<RefCell<LinuxBackend>>,
) {
    let horizontal = layout.axis.is_horizontal();
    let state = Rc::new(RefCell::new(State {
        callbacks,
        layout,
        overscan,
        horizontal,
        fixed,
        scrolled: scrolled.clone(),
        backend,
        mounted: HashMap::new(),
        handlers: Vec::new(),
        alive: true,
        reload_pending: false,
    }));

    // Re-window on scroll (value-changed) and on viewport/content resize
    // (changed → page_size/upper updated). Connect the main-axis
    // adjustment for both; the cross axis only affects cell width, which
    // a `changed` on either axis picks up, so wiring the main axis covers
    // scroll + the common resize case.
    let adj = if horizontal {
        scrolled.hadjustment()
    } else {
        scrolled.vadjustment()
    };
    {
        let st = state.clone();
        let id = adj.connect_value_changed(move |_| resync(&st));
        state.borrow_mut().handlers.push((adj.clone(), id));
    }
    {
        let st = state.clone();
        let id = adj.connect_changed(move |_| resync(&st));
        state.borrow_mut().handlers.push((adj.clone(), id));
    }
    // Also cover a cross-axis-only resize (viewport width changes for a
    // vertical list): the cross adjustment's `changed` re-lays cell widths.
    let cross_adj = if horizontal {
        scrolled.vadjustment()
    } else {
        scrolled.hadjustment()
    };
    {
        let st = state.clone();
        let id = cross_adj.connect_changed(move |_| resync(&st));
        state.borrow_mut().handlers.push((cross_adj.clone(), id));
    }

    STATES.with(|m| m.borrow_mut().insert(node_id, state.clone()));

    // Kick an initial pass once GTK has allocated the scroll view (the
    // adjustments read 0 until then). `idle_add_local_once` runs on the
    // main loop after the current mount unwinds.
    let st = state.clone();
    glib::idle_add_local_once(move || resync(&st));
}

/// Re-sync after a data change: full reload (recycle every mounted cell,
/// then re-window). Keyed identity preservation is a future optimization
/// — mirrors the macOS/iOS `reloadData` shape.
///
/// **The reload is deferred, never run inside this call.** The vocabulary
/// invokes `virtualizer_data_changed` from its data effect while holding
/// `backend.borrow_mut()`, and the reload re-enters the backend twice:
/// `release_item` drops each row's scope (a styled row's teardown
/// re-borrows the backend to unstyle its node) and `mount_item` realizes
/// the new rows (realize borrows the backend). Run synchronously, either
/// one panics "already borrowed" the first time the data changes while
/// rows are mounted. Every other backend defers the same way (web and iOS
/// hop a microtask; Android's `RecyclerView` and macOS's
/// `NSCollectionView` re-bind on their next layout pass), so the
/// vocabulary's contract is that `data_changed` never realizes in-call.
///
/// The hop is a glib idle at `HIGH_IDLE` priority, not
/// `runtime_shared::scheduling::schedule_microtask` — that one runs
/// SYNCHRONOUSLY when no scheduler is installed (see `newcore.rs`), which
/// would reintroduce the panic. `HIGH_IDLE` (100) runs ahead of GTK's
/// resize/redraw sources (`GDK_PRIORITY_REDRAW` = `HIGH_IDLE + 20`), so the
/// next painted frame already shows the new rows. Data changes landing in
/// the same turn coalesce onto one pending reload (each reload re-reads
/// count/keys/sizes, so only the last state matters).
pub(crate) fn data_changed(node_id: u64) {
    let state = STATES.with(|m| m.borrow().get(&node_id).cloned());
    let Some(state) = state else { return };
    {
        let mut s = state.borrow_mut();
        if !s.alive || s.reload_pending {
            return;
        }
        s.reload_pending = true;
    }
    glib::idle_add_local_full(glib::Priority::HIGH_IDLE, move || {
        reload(&state);
        glib::ControlFlow::Break
    });
}

/// The deferred body of [`data_changed`]: recycle every mounted cell, free
/// what the backend kept for each, then re-window. Runs with no backend
/// borrow held; a released virtualizer (`alive == false`) is left alone —
/// `release` already recycled its cells.
fn reload(state: &Rc<RefCell<State>>) {
    {
        let mut s = state.borrow_mut();
        s.reload_pending = false;
        if !s.alive {
            return;
        }
    }
    for cell in recycle_all(state) {
        free_cell(state, cell);
    }
    resync(state);
}

/// Tear down: disconnect signals, recycle mounted cells (dropping their
/// scopes), drop the state entry. Returns the recycled cells' root node
/// ids for the backend to free — the caller holds `&mut LinuxBackend`
/// here, and row teardown is safe under that borrow because the
/// vocabulary drains the row scopes before calling `release_virtualizer`
/// (so `release_item` is a no-op by now).
#[must_use = "recycled cells must be freed by the backend"]
pub(crate) fn release(node_id: u64) -> Vec<u64> {
    let state = STATES.with(|m| m.borrow_mut().remove(&node_id));
    let Some(state) = state else { return Vec::new() };
    {
        let mut s = state.borrow_mut();
        s.alive = false;
        let handlers = std::mem::take(&mut s.handlers);
        for (adj, id) in handlers {
            adj.disconnect(id);
        }
    }
    recycle_all(&state)
}

/// A node's stacking z changed and it has no tracked parent: if it is a
/// mounted row root of some virtualizer, record the new z and restack that
/// virtualizer's rows. Returns whether the node was a mounted row.
///
/// Called from `apply_style` / `set_animated_f32` with the backend
/// borrowed, so this touches only GTK widgets and virtualizer state.
/// No `State` borrow is ever held across a framework call (every borrow
/// in this module is statement- or block-scoped around plain reads), so
/// the `borrow_mut` here cannot collide.
pub(crate) fn cell_z_changed(node_id: u64, z: f32) -> bool {
    let states: Vec<Rc<RefCell<State>>> =
        STATES.with(|m| m.borrow().values().cloned().collect());
    for state in states {
        let found = {
            let mut s = state.borrow_mut();
            match s.mounted.values_mut().find(|c| c.node.id == node_id) {
                Some(cell) => {
                    cell.z = z;
                    true
                }
                None => false,
            }
        };
        if found {
            restack(&state);
            return true;
        }
    }
    false
}

/// Put the `Fixed`'s row widgets in `z_paint_order` of their z, ties in
/// row-index order — paint order AND pick order, since `GtkFixed` does
/// both by sibling order.
fn restack(state: &Rc<RefCell<State>>) {
    let (fixed, order) = {
        let s = state.borrow();
        let mut rows: Vec<(usize, f32, gtk4::Widget)> = s
            .mounted
            .iter()
            .map(|(idx, c)| (*idx, c.z, c.node.widget.clone()))
            .collect();
        rows.sort_by_key(|r| r.0);
        let zs: Vec<f32> = rows.iter().map(|r| r.1).collect();
        let order: Vec<gtk4::Widget> = runtime_shared::style::z_paint_order(&zs)
            .into_iter()
            .map(|i| rows[i].2.clone())
            .collect();
        (s.fixed.clone(), order)
    };
    crate::view::restack_widgets(fixed.upcast_ref(), &order);
}

/// How many virtualizers hold live state — the per-node-state test seam
/// (`LinuxBackend::per_node_state_counts_for_test`).
pub(crate) fn live_count() -> usize {
    STATES.with(|m| m.borrow().len())
}

/// Free a cell that scrolled out of the window, from a GTK signal callback.
///
/// The backend is usually not borrowed here, but a scroll/resize signal can
/// fire inside a layout pass that holds it; then the free waits for an idle
/// rather than being dropped (a dropped free is exactly the leak this
/// exists to close). The cell is already out of the document and its scope
/// is gone, so nothing observes the delay.
fn free_cell(state: &Rc<RefCell<State>>, cell_id: u64) {
    let backend = state.borrow().backend.clone();
    free_cell_via(backend, cell_id);
}

fn free_cell_via(backend: Weak<RefCell<LinuxBackend>>, cell_id: u64) {
    let Some(rc) = backend.upgrade() else { return };
    let Ok(mut b) = rc.try_borrow_mut() else {
        glib::idle_add_local_once(move || free_cell_via(backend, cell_id));
        return;
    };
    b.free_subtree(cell_id);
}

// =========================================================================
// Internals
// =========================================================================

/// Recycle every mounted cell: detach its widget and drop its scope.
/// Returns the cells' root node ids so the caller can free their state.
fn recycle_all(state: &Rc<RefCell<State>>) -> Vec<u64> {
    let mut s = state.borrow_mut();
    let fixed = s.fixed.clone();
    let release_item = s.callbacks.release_item.clone();
    let mounted = std::mem::take(&mut s.mounted);
    drop(s); // release_item may re-enter the framework; hold no State borrow
    let mut recycled = Vec::with_capacity(mounted.len());
    for (_, cell) in mounted {
        if cell.node.widget.parent().as_ref() == Some(fixed.upcast_ref::<gtk4::Widget>()) {
            fixed.remove(&cell.node.widget);
        }
        (release_item)(cell.scope_id);
        recycled.push(cell.node.id);
    }
    recycled
}

/// Recompute the visible window and reconcile mounted cells against it.
fn resync(state: &Rc<RefCell<State>>) {
    // ---- Read geometry + item metrics under a short borrow ----
    let (horizontal, overscan, main_gap, count, fixed, scrolled) = {
        let s = state.borrow();
        if !s.alive {
            return;
        }
        (
            s.horizontal,
            s.overscan,
            s.layout.main_spacing,
            (s.callbacks.item_count)(),
            s.fixed.clone(),
            s.scrolled.clone(),
        )
    };

    let sizes: Vec<f32> = {
        let s = state.borrow();
        (0..count).map(|i| (s.callbacks.item_size)(i)).collect()
    };
    let (offsets, total) = cumulative(&sizes, main_gap);

    // Viewport + scroll from the main-axis adjustment; cross extent from
    // the allocated widget size.
    let (scroll, viewport, cross) = if horizontal {
        let adj = scrolled.hadjustment();
        (adj.value() as f32, adj.page_size() as f32, scrolled.height() as f32)
    } else {
        let adj = scrolled.vadjustment();
        (adj.value() as f32, adj.page_size() as f32, scrolled.width() as f32)
    };

    // Size the document so the scrollbar reflects the full content extent.
    if horizontal {
        fixed.set_size_request(total.max(0.0) as i32, -1);
    } else {
        fixed.set_size_request(-1, total.max(0.0) as i32);
    }

    // Nothing measurable yet (pre-allocation) — wait for the next pass.
    if viewport <= 0.0 || cross <= 0.0 {
        return;
    }

    let (first, last) = visible_range(&offsets, &sizes, scroll, viewport, overscan);

    // ---- Unmount cells that left the window ----
    let to_unmount: Vec<usize> = {
        let s = state.borrow();
        s.mounted
            .keys()
            .copied()
            .filter(|i| *i < first || *i >= last || *i >= count)
            .collect()
    };
    for idx in to_unmount {
        let cell = state.borrow_mut().mounted.remove(&idx);
        if let Some(cell) = cell {
            if cell.node.widget.parent().as_ref()
                == Some(fixed.upcast_ref::<gtk4::Widget>())
            {
                fixed.remove(&cell.node.widget);
            }
            let release = state.borrow().callbacks.release_item.clone();
            (release)(cell.scope_id);
            // The row's scope is gone; free what the backend kept for it.
            free_cell(state, cell.node.id);
        }
    }

    // ---- Mount cells that entered the window ----
    let mut mounted_any = false;
    for idx in first..last {
        let already = state.borrow().mounted.contains_key(&idx);
        if already {
            // Re-position (offsets can shift when earlier item sizes vary).
            reposition(state, idx, &offsets, &sizes, cross);
            continue;
        }
        // `mount_item` re-enters the framework (borrows the backend), so
        // hold no State/backend borrow across it.
        let mount_item = state.borrow().callbacks.mount_item.clone();
        let (node, scope_id) = (mount_item)(idx);
        // The row was styled during `mount_item`'s realize, before it was
        // a mounted cell, so `cell_z_changed` could not record its z.
        // Read it now: `mount_item` just borrowed the backend mutably and
        // returned, so nothing up the stack holds it.
        let z = state
            .borrow()
            .backend
            .upgrade()
            .map_or(0.0, |b| b.borrow().node_z(node.id));

        // Parent into the document + lay the cell subtree out.
        let (x, y, w, h) = cell_box(state, idx, &offsets, &sizes, cross);
        fixed.put(&node.widget, x as f64, y as f64);
        size_and_layout(state, &node, (x, y, w, h));

        state
            .borrow_mut()
            .mounted
            .insert(idx, MountedCell { node, scope_id, z });
        mounted_any = true;
    }
    if mounted_any {
        restack(state);
    }
}

/// Compute a cell's `(x, y, w, h)` box in the document from the axis.
fn cell_box(
    state: &Rc<RefCell<State>>,
    idx: usize,
    offsets: &[f32],
    sizes: &[f32],
    cross: f32,
) -> (f32, f32, f32, f32) {
    let horizontal = state.borrow().horizontal;
    let main = offsets[idx];
    let extent = sizes[idx].max(0.0);
    if horizontal {
        (main, 0.0, extent, cross)
    } else {
        (0.0, main, cross, extent)
    }
}

/// Move an already-mounted cell to its current box (offsets shift when a
/// preceding variable-size item changes) and re-lay it out.
fn reposition(
    state: &Rc<RefCell<State>>,
    idx: usize,
    offsets: &[f32],
    sizes: &[f32],
    cross: f32,
) {
    let (x, y, w, h) = cell_box(state, idx, offsets, sizes, cross);
    let widget = state
        .borrow()
        .mounted
        .get(&idx)
        .map(|c| c.node.widget.clone());
    let Some(widget) = widget else { return };
    state.borrow().fixed.move_(&widget, x as f64, y as f64);
    let node = state.borrow().mounted.get(&idx).map(|c| c.node.clone());
    if let Some(node) = node {
        size_and_layout(state, &node, (x, y, w, h));
    }
}

/// Pin a cell's box size, run the detached layout pass over its subtree so
/// its descendants get framed, then record the cell's slot `(x, y)` as the
/// row root's frame origin.
///
/// The slot must go through the backend, not just `Fixed::put`/`move_`:
/// the layout pass frames the row root at `(0, 0)` (it is its own Taffy
/// root) and writes that frame as the root's `GtkFixed` child transform,
/// which REPLACES the position `put`/`move_` set — every row would land
/// on the first row's slot. `place_detached_root` records the slot as the
/// root's `detached_origin`, so this pass and every later main layout pass
/// (which re-frames detached roots too) compose it with the row root's own
/// transform.
fn size_and_layout(state: &Rc<RefCell<State>>, node: &LinuxNode, (x, y, w, h): (f32, f32, f32, f32)) {
    let (wi, hi) = (w.round().max(0.0) as i32, h.round().max(0.0) as i32);
    if let Some(v) = node.widget.downcast_ref::<IdealystView>() {
        v.set_layout_size(wi, hi);
    } else {
        node.widget.set_size_request(wi, hi);
    }
    let backend = state.borrow().backend.clone();
    if let Some(b) = backend.upgrade() {
        if let Ok(mut b) = b.try_borrow_mut() {
            b.layout_detached_root(node.id, w, h, None);
            b.place_detached_root(node.id, x, y);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Pure windowing-math tests — no GTK context needed.
    use super::*;

    #[test]
    fn cumulative_offsets_include_gaps() {
        let (offs, total) = cumulative(&[10.0, 20.0, 30.0], 5.0);
        // 0 ; 10+5 ; 15+20+5 = 40. total = 40 + 30 = 70.
        assert_eq!(offs, vec![0.0, 15.0, 40.0]);
        assert_eq!(total, 70.0);
    }

    #[test]
    fn cumulative_no_gap_is_prefix_sum() {
        let (offs, total) = cumulative(&[40.0, 40.0, 40.0], 0.0);
        assert_eq!(offs, vec![0.0, 40.0, 80.0]);
        assert_eq!(total, 120.0);
    }

    #[test]
    fn visible_range_selects_only_intersecting_items() {
        // 100 items of 40px, no gap, no overscan.
        let sizes: Vec<f32> = vec![40.0; 100];
        let (offs, _) = cumulative(&sizes, 0.0);
        // Viewport [0, 200): items 0..5 (0,40,80,120,160 fully or partly in).
        let (first, last) = visible_range(&offs, &sizes, 0.0, 200.0, 0.0);
        assert_eq!((first, last), (0, 5));
        // Scrolled to 400: window [400,600) → items 10..15.
        let (first, last) = visible_range(&offs, &sizes, 400.0, 200.0, 0.0);
        assert_eq!((first, last), (10, 15));
    }

    #[test]
    fn visible_range_expands_by_overscan() {
        let sizes: Vec<f32> = vec![40.0; 100];
        let (offs, _) = cumulative(&sizes, 0.0);
        // overscan 1.0 pads one viewport (200px) each side.
        // scroll 400, viewport 200 → window [200, 800) → items 5..20.
        let (first, last) = visible_range(&offs, &sizes, 400.0, 200.0, 1.0);
        assert_eq!((first, last), (5, 20));
    }

    #[test]
    fn visible_range_clamps_top_at_zero() {
        let sizes: Vec<f32> = vec![40.0; 100];
        let (offs, _) = cumulative(&sizes, 0.0);
        // Near the top with overscan: window can't go negative → starts at 0.
        let (first, _last) = visible_range(&offs, &sizes, 40.0, 200.0, 1.0);
        assert_eq!(first, 0);
    }

    #[test]
    fn visible_range_empty_or_unmeasured_is_zero() {
        assert_eq!(visible_range(&[], &[], 0.0, 200.0, 0.0), (0, 0));
        let sizes = vec![40.0; 3];
        let (offs, _) = cumulative(&sizes, 0.0);
        // Zero viewport → nothing visible yet.
        assert_eq!(visible_range(&offs, &sizes, 0.0, 0.0, 0.0), (0, 0));
    }
}
