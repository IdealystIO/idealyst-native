//! `flat_list` / `virtualizer` on the painted scene: a scroll view whose
//! children are only the rows in (or near) the viewport.
//!
//! ## Model
//!
//! The list is a painted view with [`ScrollInfo`](crate::ScrollInfo) — the
//! same node a `scroll_view` is, so painting, hit testing, wheel scrolling,
//! clipping and native-child positioning need nothing new. Its scene
//! children are the MOUNTED rows only, kept in row order, so a row stacks
//! against its neighbours by its item root's z (`z_index`, animated
//! `ZIndex`) with ties in row order, like any siblings
//! (`scene::child_paint_order`); they are not Taffy children. Each
//! row (`VirtualizerCallbacks::mount_item` builds it in a fresh per-row
//! scope) is a detached Taffy root: the layout pass lays it out at the
//! list's cross size by its item size ([`WindowsBackend::layout_virtual_rows`]),
//! then puts it at its slot along the main axis. The scroll range comes
//! from the summed item sizes ([`ScrollInfo::extent`](crate::ScrollInfo)),
//! not from the mounted children, which cover only the window.
//!
//! ## When rows change
//!
//! [`resync`] recomputes the visible range and mounts / releases rows. It
//! runs author code — `mount_item` realizes a row, `release_item` drops a
//! row's scope (whose teardown re-borrows the backend to unstyle) — so it
//! never runs inside a backend borrow: every trigger schedules it on a
//! fresh scheduler turn ([`schedule`]), and triggers in the same turn
//! coalesce. Triggers: the first layout that gives the list a viewport, a
//! viewport size change, a scroll-offset change, and a data change (which
//! also recycles every mounted row, since keys and sizes may all differ).
//!
//! ## Item sizes
//!
//! A `Known` list's `item_size` is authoritative. In a `Measured` list
//! (`measure_sizes`) it is the author's estimate until a row is mounted:
//! the layout pass then measures the row's natural main-axis size at the
//! list's cross size and reports it (`set_measured_size`), so the row's
//! slot, every following offset, the scroll extent and the window all
//! follow the rendered size ([`crate::virtual_rows`]). The vocabulary keeps
//! measured sizes by item key, so a row scrolled out and back keeps its
//! measured size. Lanes (grids) are laid out as one lane.

use std::collections::BTreeMap;
use std::rc::{Rc, Weak};
use std::cell::RefCell;

use runtime_shared::{VirtualLayout, VirtualizerCallbacks};

use crate::virtual_rows::{cumulative, lay_out_rows, visible_range, RowLayout};
use crate::{NodeKind, WindowsBackend, WindowsNode};

/// One mounted row: its root node and the scope `release_item` drops.
#[derive(Clone, Copy)]
pub(crate) struct Row {
    pub id: u64,
    pub scope: u64,
}

/// Per-list state, owned by the backend (`WindowsBackend::virtualizers`).
pub(crate) struct VirtState {
    pub callbacks: VirtualizerCallbacks<WindowsNode>,
    pub layout: VirtualLayout,
    pub overscan: f32,
    pub horizontal: bool,
    /// Index → mounted row; only the window's rows.
    pub mounted: BTreeMap<usize, Row>,
    /// Main-axis start and size of every item, from the last resync.
    pub offsets: Vec<f32>,
    pub sizes: Vec<f32>,
    /// Viewport `(w, h)` the last resync windowed against.
    pub viewport: (f32, f32),
    /// A resync is scheduled and has not run. Shared with the scheduled
    /// task so a resync that finds the backend busy can clear it without
    /// borrowing (see [`resync`]).
    pub pending: Rc<std::cell::Cell<bool>>,
    /// The next resync recycles every row first (data changed).
    pub reload: bool,
}

impl VirtState {
    pub(crate) fn new(callbacks: VirtualizerCallbacks<WindowsNode>, overscan: f32, layout: VirtualLayout) -> Self {
        VirtState {
            callbacks,
            horizontal: layout.axis.is_horizontal(),
            layout,
            overscan,
            mounted: BTreeMap::new(),
            offsets: Vec::new(),
            sizes: Vec::new(),
            viewport: (0.0, 0.0),
            pending: Rc::new(std::cell::Cell::new(false)),
            reload: false,
        }
    }
}

/// The callbacks a resync calls, cloned out so no borrow is held across
/// them.
struct Cbs {
    item_count: Rc<dyn Fn() -> usize>,
    item_size: Rc<dyn Fn(usize) -> f32>,
    mount_item: Rc<dyn Fn(usize) -> (WindowsNode, u64)>,
    release_item: Rc<dyn Fn(u64)>,
}

/// Schedule a resync of list `id` on a fresh turn (coalesced).
pub(crate) fn schedule(b: &mut WindowsBackend, id: u64) {
    let Some(st) = b.virtualizers.get_mut(&id) else { return };
    if st.pending.replace(true) {
        return;
    }
    let pending = st.pending.clone();
    let weak = b.self_ref.clone();
    runtime_shared::scheduling::after_ms_detached(0, move || {
        pending.set(false);
        resync(&weak, id);
    });
}

/// Window list `id` against its current viewport and scroll offset:
/// release the rows that left, mount the ones that entered, then lay out.
/// Runs with no backend borrow held across author code.
pub(crate) fn resync(weak: &Weak<RefCell<WindowsBackend>>, id: u64) {
    let Some(rc) = weak.upgrade() else { return };
    // Phase 1 (short borrow): what to read and what is mounted.
    let (callbacks, overscan, gap, horizontal, viewport, scroll, recycled) = {
        // Busy: a re-entrant message, or a scheduler-less host where the
        // "next turn" ran inline inside the borrow that scheduled it.
        // Rescheduling here could recurse forever in the latter; the
        // next trigger (layout pass, scroll, data change) re-windows.
        let Ok(mut b) = rc.try_borrow_mut() else { return };
        let (viewport, scroll) = match b.nodes.get(&id).map(|m| (&m.kind, m.frame)) {
            Some((NodeKind::View(v), frame)) => {
                let s = v.scroll.as_ref();
                ((frame.2, frame.3), s.map_or((0.0, 0.0), |s| (s.offset_x, s.offset_y)))
            }
            _ => return,
        };
        let Some(st) = b.virtualizers.get_mut(&id) else { return };
        st.viewport = viewport;
        let recycled = if std::mem::take(&mut st.reload) { std::mem::take(&mut st.mounted) } else { BTreeMap::new() };
        let c = &st.callbacks;
        let callbacks = Cbs {
            item_count: c.item_count.clone(),
            item_size: c.item_size.clone(),
            mount_item: c.mount_item.clone(),
            release_item: c.release_item.clone(),
        };
        (callbacks, st.overscan, st.layout.main_spacing, st.horizontal, viewport, scroll, recycled)
    };

    let count = (callbacks.item_count)();
    let sizes: Vec<f32> = (0..count).map(|i| (callbacks.item_size)(i)).collect();
    let (offsets, total) = cumulative(&sizes, gap);
    let (main_scroll, main_view) = if horizontal { (scroll.0, viewport.0) } else { (scroll.1, viewport.1) };
    let (first, last) = visible_range(&offsets, &sizes, main_scroll, main_view, overscan);

    // Rows to release: everything recycled, plus mounted rows outside the
    // window (or past a shrunk count).
    let mut leaving: Vec<Row> = recycled.into_values().collect();
    if let Ok(mut b) = rc.try_borrow_mut() {
        if let Some(st) = b.virtualizers.get_mut(&id) {
            let gone: Vec<usize> = st.mounted.keys().copied().filter(|&i| i < first || i >= last || i >= count).collect();
            for i in gone {
                if let Some(row) = st.mounted.remove(&i) {
                    leaving.push(row);
                }
            }
        }
    }
    for row in &leaving {
        (callbacks.release_item)(row.scope);
    }
    if let Ok(mut b) = rc.try_borrow_mut() {
        for row in &leaving {
            if let Some(kids) = b.children.get_mut(&id) {
                kids.retain(|k| *k != row.id);
            }
            b.remove_subtree(row.id);
        }
    }

    // Rows entering the window.
    for idx in first..last {
        let mounted = rc.try_borrow().map(|b| b.virtualizers.get(&id).is_some_and(|st| st.mounted.contains_key(&idx)));
        if mounted.unwrap_or(true) {
            continue;
        }
        let (node, scope) = (callbacks.mount_item)(idx);
        let Ok(mut b) = rc.try_borrow_mut() else { continue };
        insert_row(&mut b, id, idx, Row { id: node.id, scope });
    }

    // Record the geometry, then lay out and repaint.
    let Ok(mut b) = rc.try_borrow_mut() else { return };
    if let Some(st) = b.virtualizers.get_mut(&id) {
        st.offsets = offsets;
        st.sizes = sizes;
    }
    if let Some(meta) = b.nodes.get_mut(&id) {
        if let NodeKind::View(v) = &mut meta.kind {
            if let Some(s) = &mut v.scroll {
                s.extent = Some(if horizontal { (total, 0.0) } else { (0.0, total) });
            }
        }
    }
    b.layout_pass();
    b.invalidate();
}

/// Record row `idx` of list `id` and add its root to the list's scene
/// children in ROW order: before the next mounted row, else last. Rows
/// are plain siblings, so `scene::child_paint_order` stacks them by the
/// item root's z (style `z_index`, animated `ZIndex`) with ties in this
/// child order — appending would tie-break by mount order instead, and a
/// row that scrolls in above the window would stack over (and take the
/// pointer from) the row below it wherever the two overlap.
fn insert_row(b: &mut WindowsBackend, id: u64, idx: usize, row: Row) {
    let Some(st) = b.virtualizers.get_mut(&id) else { return };
    let next = st.mounted.range(idx + 1..).next().map(|(_, r)| r.id);
    st.mounted.insert(idx, row);
    let kids = b.children.entry(id).or_default();
    let at = next.and_then(|n| kids.iter().position(|k| *k == n)).unwrap_or(kids.len());
    kids.insert(at, row.id);
}

/// Free a list's rows when it is released. The vocabulary drains the row
/// scopes before `release_virtualizer`, so only the backend's own state is
/// left to drop.
pub(crate) fn release(b: &mut WindowsBackend, id: u64) {
    if let Some(st) = b.virtualizers.remove(&id) {
        for row in st.mounted.into_values() {
            if let Some(kids) = b.children.get_mut(&id) {
                kids.retain(|k| *k != row.id);
            }
            b.remove_subtree(row.id);
        }
    }
}

impl WindowsBackend {
    /// Lay out every mounted row of every list (detached roots, so the
    /// main pass never reaches them) at the list's cross size by its item
    /// size. Runs inside `layout_pass` after the main roots, so the list's
    /// own frame is current. A list whose viewport changed since its last
    /// resync gets one scheduled.
    ///
    /// In a `measure_sizes` list each row is measured first
    /// ([`lay_out_rows`]); when a row's rendered size
    /// differs from the size in use, the new size is reported, the list's
    /// offsets and scroll extent are recomputed here (so
    /// `place_virtual_rows` already slots every following row by it), and
    /// a resync is scheduled to re-window against the new geometry — it
    /// mounts rows, which re-borrows the backend, so it cannot run inside
    /// this pass.
    pub(crate) fn layout_virtual_rows(&mut self) {
        let ids: Vec<u64> = self.virtualizers.keys().copied().collect();
        for id in ids {
            let Some(list_layout) = self.layout_for_id.get(&id).copied() else { continue };
            let f = self.layout.frame_of(list_layout);
            let viewport = (f.width, f.height);
            let Some(st) = self.virtualizers.get(&id) else { continue };
            if (st.viewport.0 - viewport.0).abs() > 0.5 || (st.viewport.1 - viewport.1).abs() > 0.5 {
                schedule(self, id);
            }
            let Some(st) = self.virtualizers.get_mut(&id) else { continue };
            let rows: Vec<RowLayout> = st
                .mounted
                .iter()
                .filter_map(|(&idx, r)| self.layout_for_id.get(&r.id).map(|&root| RowLayout { idx, root, scope: r.scope }))
                .collect();
            let horizontal = st.horizontal;
            // Lanes are laid out as one lane, so a row spans the list.
            let cross = if horizontal { viewport.1 } else { viewport.0 };
            let measure = st.callbacks.measure_sizes;
            let report = st.callbacks.set_measured_size.clone();
            let changed = lay_out_rows(&mut self.layout, &rows, &mut st.sizes, horizontal, cross, measure, &*report);
            if !changed {
                continue;
            }
            let (offsets, total) = cumulative(&st.sizes, st.layout.main_spacing);
            st.offsets = offsets;
            if let Some(NodeKind::View(v)) = self.nodes.get_mut(&id).map(|m| &mut m.kind) {
                if let Some(s) = &mut v.scroll {
                    s.extent = Some(if horizontal { (total, 0.0) } else { (0.0, total) });
                }
            }
            schedule(self, id);
        }
    }

    /// Put each mounted row at its slot along the main axis. Runs after
    /// the frame pass, which gave every row root a `(0, 0)` origin.
    pub(crate) fn place_virtual_rows(&mut self) {
        let placements: Vec<(u64, f32, bool)> = self
            .virtualizers
            .values()
            .flat_map(|st| {
                st.mounted
                    .iter()
                    .filter_map(|(&i, r)| st.offsets.get(i).map(|&o| (r.id, o, st.horizontal)))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (row, offset, horizontal) in placements {
            if let Some(meta) = self.nodes.get_mut(&row) {
                if horizontal {
                    meta.frame.0 = offset;
                    meta.frame.1 = 0.0;
                } else {
                    meta.frame.0 = 0.0;
                    meta.frame.1 = offset;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_vocabulary::caps::VirtualizerOps;

    /// A live list whose rows render 200 / 200 / 100 / 200 tall against a
    /// 132 estimate (the vocabulary's override store is modelled: a
    /// reported size wins over the estimate in `item_size`). Returns the
    /// backend, the list and the reports `set_measured_size` received.
    fn measured_list(measure_sizes: bool) -> (Rc<RefCell<WindowsBackend>>, WindowsNode, Rc<RefCell<Vec<(u64, f32)>>>) {
        use runtime_shared::{Length, StyleRules};
        use runtime_vocabulary::caps::StyleOps;
        use runtime_shared::accessibility::AccessibilityProps;
        use windows::Win32::Foundation::HWND;

        const ESTIMATE: f32 = 132.0;
        let rendered = |i: usize| [200.0, 200.0, 100.0, 200.0][i % 4];
        let cell = Rc::new(RefCell::new(WindowsBackend::new(HWND(std::ptr::null_mut()))));
        cell.borrow_mut().set_self_ref(Rc::downgrade(&cell));
        let overrides: Rc<RefCell<std::collections::HashMap<usize, f32>>> = Rc::default();
        let reports: Rc<RefCell<Vec<(u64, f32)>>> = Rc::default();
        let weak = Rc::downgrade(&cell);
        let (o, r) = (overrides.clone(), reports.clone());
        let o2 = overrides.clone();
        let callbacks = VirtualizerCallbacks {
            item_count: Rc::new(|| 20),
            item_key: Rc::new(|i| i as u64),
            item_size: Rc::new(move |i| o2.borrow().get(&i).copied().unwrap_or(ESTIMATE)),
            measure_sizes,
            // A card: padding 10 around a block 20 shorter than the row.
            mount_item: Rc::new(move |i| {
                let rc = weak.upgrade().unwrap();
                let mut b = rc.borrow_mut();
                let a11y = AccessibilityProps::default();
                let mut root = b.create_view(&a11y);
                let pad = Some(Length::Px(10.0).into());
                StyleOps::apply_style(&mut *b, &root, &Rc::new(StyleRules { padding_top: pad, padding_bottom: pad, ..StyleRules::default() }));
                let body = b.create_view(&a11y);
                let height = Some(Length::Px(rendered(i) - 20.0).into());
                StyleOps::apply_style(&mut *b, &body, &Rc::new(StyleRules { height, ..StyleRules::default() }));
                b.insert(&mut root, body);
                (root, 1000 + i as u64)
            }),
            release_item: Rc::new(|_| {}),
            set_measured_size: Rc::new(move |scope, size| {
                o.borrow_mut().insert((scope - 1000) as usize, size);
                r.borrow_mut().push((scope, size));
            }),
            on_scroll: None,
            item_changed: None,
        };
        let layout = VirtualLayout {
            axis: runtime_shared::primitives::virtualizer::Axis::Vertical,
            lanes: runtime_shared::primitives::virtualizer::Lanes::Fixed(1),
            main_spacing: 0.0,
            cross_spacing: 0.0,
        };
        let list = VirtualizerOps::create_virtualizer(&mut *cell.borrow_mut(), callbacks, 0.0, layout, &AccessibilityProps::default());
        {
            // A 300×450 list. `layout_pass` needs a real client rect, so
            // the list's own Taffy frame is computed directly.
            let mut b = cell.borrow_mut();
            b.root_id = Some(list.id);
            b.nodes.get_mut(&list.id).unwrap().frame = (0.0, 0.0, 300.0, 450.0);
            let l = b.layout_for_id[&list.id];
            b.layout.compute(l, 300.0, 450.0);
        }
        (cell, list, reports)
    }

    /// Rendered row `i`: its slot along the list (after placement) and its
    /// laid-out height.
    fn row_geometry(b: &WindowsBackend, list: u64, i: usize) -> (f32, f32) {
        let id = b.virtualizers[&list].mounted[&i].id;
        (b.nodes[&id].frame.1, b.layout.frame_of(b.layout_for_id[&id]).height)
    }

    /// REGRESSION (idea-ui `TableCards`): a `Measured` list spaced its rows
    /// by the estimate — 132pt cards that render 200pt overlapped the next
    /// by 58pt. Rows are measured on layout, reported, laid out at their
    /// rendered size, and the following rows, the scroll range and the
    /// window move with them.
    #[test]
    fn regression_measured_rows_are_spaced_by_their_rendered_size() {
        use crate::scene::max_scroll_offsets;
        let (cell, list, reports) = measured_list(true);
        let weak = Rc::downgrade(&cell);
        // By the estimate, a 450 viewport holds rows 0..4 (0, 132, 264, 396).
        resync(&weak, list.id);
        assert_eq!(cell.borrow().virtualizers[&list.id].mounted.len(), 4);

        cell.borrow_mut().layout_virtual_rows();
        assert_eq!(*reports.borrow(), vec![(1000, 200.0), (1001, 200.0), (1002, 100.0), (1003, 200.0)]);
        cell.borrow_mut().place_virtual_rows();
        {
            let b = cell.borrow();
            assert_eq!(row_geometry(&b, list.id, 0), (0.0, 200.0));
            assert_eq!(row_geometry(&b, list.id, 1), (200.0, 200.0), "not 132: no overlap with row 0");
            assert_eq!(row_geometry(&b, list.id, 2), (400.0, 100.0), "a shorter row shrinks");
            assert_eq!(row_geometry(&b, list.id, 3), (500.0, 200.0));
            // 700 measured + 16 unmeasured rows at the estimate.
            assert_eq!(max_scroll_offsets(&b, list.id), (0.0, 700.0 + 16.0 * 132.0 - 450.0));
        }

        // The re-window reads the measured sizes back through `item_size`:
        // row 3 now starts at 500, past the 450 viewport, and leaves.
        resync(&weak, list.id);
        assert_eq!(cell.borrow().virtualizers[&list.id].mounted.keys().copied().collect::<Vec<_>>(), vec![0, 1, 2]);

        // Converges: measuring again at the same width reports nothing.
        reports.borrow_mut().clear();
        cell.borrow_mut().layout_virtual_rows();
        cell.borrow_mut().place_virtual_rows();
        assert!(reports.borrow().is_empty(), "a measured row is not re-reported");
        assert_eq!(row_geometry(&cell.borrow(), list.id, 2), (400.0, 100.0));
        VirtualizerOps::release_virtualizer(&mut *cell.borrow_mut(), &list);
    }

    /// A `Known` list's sizes are authoritative: rows that render taller
    /// are still laid out at the given size and nothing is reported.
    #[test]
    fn known_lists_never_call_set_measured_size() {
        let (cell, list, reports) = measured_list(false);
        resync(&Rc::downgrade(&cell), list.id);
        cell.borrow_mut().layout_virtual_rows();
        cell.borrow_mut().place_virtual_rows();
        assert!(reports.borrow().is_empty());
        assert_eq!(row_geometry(&cell.borrow(), list.id, 1), (132.0, 132.0));
        VirtualizerOps::release_virtualizer(&mut *cell.borrow_mut(), &list);
    }

    /// Rows are plain siblings of the list node, so a `z_index` on an item
    /// root orders that row against its neighbours for paint and hit
    /// testing (as on web), and equal-z rows stack in ROW order. Fails if
    /// rows are appended in mount order: row 9, scrolled in above the
    /// window, then stacks over row 10 and takes the pointer where the
    /// two overlap.
    #[test]
    fn regression_virtual_rows_stack_by_item_z_then_row_order() {
        use crate::scene::{child_paint_order, hit_chain};
        use runtime_shared::StyleRules;
        use runtime_vocabulary::caps::{StyleOps, VirtualizerOps};
        use runtime_shared::accessibility::AccessibilityProps;
        use windows::Win32::Foundation::HWND;

        let cell = Rc::new(RefCell::new(WindowsBackend::new(HWND(std::ptr::null_mut()))));
        cell.borrow_mut().set_self_ref(Rc::downgrade(&cell));
        let count = Rc::new(std::cell::Cell::new(100usize));
        let weak = Rc::downgrade(&cell);
        let c = count.clone();
        let callbacks = VirtualizerCallbacks {
            item_count: Rc::new(move || c.get()),
            item_key: Rc::new(|i| i as u64),
            item_size: Rc::new(|_| 40.0),
            measure_sizes: false,
            mount_item: Rc::new(move |i| {
                let node = weak.upgrade().unwrap().borrow_mut().create_view(&AccessibilityProps::default());
                (node, 1000 + i as u64)
            }),
            release_item: Rc::new(|_| {}),
            set_measured_size: Rc::new(|_, _| {}),
            on_scroll: None,
            item_changed: None,
        };
        let layout = VirtualLayout {
            axis: runtime_shared::primitives::virtualizer::Axis::Vertical,
            lanes: runtime_shared::primitives::virtualizer::Lanes::Fixed(1),
            main_spacing: 0.0,
            cross_spacing: 0.0,
        };
        let list = VirtualizerOps::create_virtualizer(&mut *cell.borrow_mut(), callbacks, 0.0, layout, &AccessibilityProps::default());
        let weak = Rc::downgrade(&cell);
        {
            let mut b = cell.borrow_mut();
            b.root_id = Some(list.id);
            b.nodes.get_mut(&list.id).unwrap().frame = (0.0, 0.0, 300.0, 200.0);
        }
        // The first resync gives the list its scroll extent (a scroll
        // offset clamps to it). Then window rows 10..15 and scroll up one
        // row: 9 enters, 14 leaves.
        resync(&weak, list.id);
        cell.borrow_mut().set_node_scroll(&list, 0.0, 400.0);
        resync(&weak, list.id);
        cell.borrow_mut().set_node_scroll(&list, 0.0, 360.0);
        resync(&weak, list.id);
        let row = |i: usize| cell.borrow().virtualizers[&list.id].mounted[&i].id;
        let rows: Vec<u64> = (9..14).map(row).collect();
        // Rows overlap their successor by 20px (60px rows on a 40px
        // pitch), so stacking decides who is on top in the overlap. The
        // sizes are set by hand: this test runs no real Taffy pass over
        // the detached rows.
        let overlap_rows = |b: &mut WindowsBackend| {
            b.place_virtual_rows();
            for &id in &rows {
                let frame = &mut b.nodes.get_mut(&id).unwrap().frame;
                (frame.2, frame.3) = (300.0, 60.0);
            }
        };
        overlap_rows(&mut cell.borrow_mut());
        // Content y 410 is in row 9 [360, 420) and row 10 [400, 460);
        // window y = 410 - scroll 360.
        let top_at_overlap = |b: &WindowsBackend| hit_chain(b, 10.0, 50.0)[0].id;

        assert_eq!(child_paint_order(&cell.borrow(), list.id), rows, "equal z: row order, not mount order");
        assert_eq!(top_at_overlap(&cell.borrow()), row(10), "the later row is on top");

        // A static z_index on row 9's item root raises it over row 10.
        let raise = |z: Option<i32>| {
            let node = crate::WindowsNode { id: row(9), hwnd: HWND(std::ptr::null_mut()) };
            StyleOps::apply_style(&mut *cell.borrow_mut(), &node, &Rc::new(StyleRules { z_index: z, ..StyleRules::default() }));
        };
        raise(Some(1));
        let mut raised: Vec<u64> = rows[1..].to_vec();
        raised.push(row(9));
        assert_eq!(child_paint_order(&cell.borrow(), list.id), raised);
        assert_eq!(top_at_overlap(&cell.borrow()), row(9), "the raised row takes the pointer");

        // A restyle without z_index restores row order.
        raise(None);
        assert_eq!(child_paint_order(&cell.borrow(), list.id), rows);
        assert_eq!(top_at_overlap(&cell.borrow()), row(10));

        // Animated ZIndex writes the same slot.
        let node9 = crate::WindowsNode { id: row(9), hwnd: HWND(std::ptr::null_mut()) };
        cell.borrow_mut().set_animated_f32(&node9, crate::AnimProp::ZIndex, 3.0);
        assert_eq!(top_at_overlap(&cell.borrow()), row(9));

        // Recycled rows (data change) mount fresh at z 0, in row order.
        VirtualizerOps::virtualizer_data_changed(&mut *cell.borrow_mut(), &list);
        resync(&weak, list.id);
        let rows: Vec<u64> = (9..14).map(row).collect();
        assert!(rows.iter().all(|id| cell.borrow().nodes[id].z == 0.0), "a recycled row does not keep the old z");
        assert_eq!(child_paint_order(&cell.borrow(), list.id), rows);
        assert_eq!(cell.borrow().children[&list.id], rows);
        VirtualizerOps::release_virtualizer(&mut *cell.borrow_mut(), &list);
    }
}
