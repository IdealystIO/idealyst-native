//! `flat_list` / `virtualizer` on the painted scene: a scroll view whose
//! children are only the rows in (or near) the viewport.
//!
//! ## Model
//!
//! The list is a painted view with [`ScrollInfo`](crate::ScrollInfo) — the
//! same node a `scroll_view` is, so painting, hit testing, wheel scrolling,
//! clipping and native-child positioning need nothing new. Its scene
//! children are the MOUNTED rows only; they are not Taffy children. Each
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
//! `item_size` is used as given. In `measure_sizes` mode it is the
//! author's estimate and is not refined: the framework's measure pass does
//! not reach a detached row subtree (the same limit the GTK and Apple
//! backends document). Lanes (grids) are laid out as one lane.

use std::collections::BTreeMap;
use std::rc::{Rc, Weak};
use std::cell::RefCell;

use runtime_shared::{VirtualLayout, VirtualizerCallbacks};

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

/// Cumulative start offsets along the main axis plus the total extent;
/// consecutive items are `gap` apart, with no trailing gap.
pub(crate) fn cumulative(sizes: &[f32], gap: f32) -> (Vec<f32>, f32) {
    let mut offsets = Vec::with_capacity(sizes.len());
    let mut acc = 0.0f32;
    for (i, &s) in sizes.iter().enumerate() {
        if i > 0 {
            acc += gap;
        }
        offsets.push(acc);
        acc += s.max(0.0);
    }
    (offsets, acc)
}

/// The half-open index range of items intersecting `[scroll, scroll +
/// viewport)` widened by `overscan` viewports on each side; `(0, 0)` for
/// an empty list or no viewport yet.
pub(crate) fn visible_range(offsets: &[f32], sizes: &[f32], scroll: f32, viewport: f32, overscan: f32) -> (usize, usize) {
    if sizes.is_empty() || viewport <= 0.0 {
        return (0, 0);
    }
    let pad = viewport * overscan.max(0.0);
    let (top, bottom) = ((scroll - pad).max(0.0), scroll + viewport + pad);
    let mut range: Option<(usize, usize)> = None;
    for i in 0..sizes.len() {
        let (start, end) = (offsets[i], offsets[i] + sizes[i].max(0.0));
        if start >= bottom {
            break;
        }
        if end > top {
            range = Some((range.map_or(i, |r| r.0), i + 1));
        }
    }
    range.unwrap_or((0, 0))
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
        b.children.entry(id).or_default().push(node.id);
        if let Some(st) = b.virtualizers.get_mut(&id) {
            st.mounted.insert(idx, Row { id: node.id, scope });
        }
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
    pub(crate) fn layout_virtual_rows(&mut self) {
        let ids: Vec<u64> = self.virtualizers.keys().copied().collect();
        for id in ids {
            let Some(list_layout) = self.layout_for_id.get(&id).copied() else { continue };
            let f = self.layout.frame_of(list_layout);
            let viewport = (f.width, f.height);
            let rows: Vec<(usize, Row, f32)> = match self.virtualizers.get(&id) {
                Some(st) => {
                    if (st.viewport.0 - viewport.0).abs() > 0.5 || (st.viewport.1 - viewport.1).abs() > 0.5 {
                        schedule(self, id);
                    }
                    let Some(st) = self.virtualizers.get(&id) else { continue };
                    st.mounted.iter().filter_map(|(&i, &r)| st.sizes.get(i).map(|&s| (i, r, s))).collect()
                }
                None => continue,
            };
            let horizontal = self.virtualizers.get(&id).is_some_and(|st| st.horizontal);
            for (_, row, size) in rows {
                if let Some(l) = self.layout_for_id.get(&row.id).copied() {
                    let (w, h) = if horizontal { (size, viewport.1) } else { (viewport.0, size) };
                    self.layout.compute(l, w, h);
                }
            }
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

    #[test]
    fn cumulative_offsets_include_gaps() {
        let (offs, total) = cumulative(&[10.0, 20.0, 30.0], 5.0);
        assert_eq!(offs, vec![0.0, 15.0, 40.0]);
        assert_eq!(total, 70.0);
    }

    #[test]
    fn visible_range_selects_intersecting_items_with_overscan() {
        let sizes = vec![40.0; 100];
        let (offs, _) = cumulative(&sizes, 0.0);
        assert_eq!(visible_range(&offs, &sizes, 0.0, 200.0, 0.0), (0, 5));
        assert_eq!(visible_range(&offs, &sizes, 400.0, 200.0, 0.0), (10, 15));
        assert_eq!(visible_range(&offs, &sizes, 400.0, 200.0, 1.0), (5, 20));
        assert_eq!(visible_range(&[], &[], 0.0, 200.0, 0.0), (0, 0));
        assert_eq!(visible_range(&offs, &sizes, 0.0, 0.0, 0.0), (0, 0), "no viewport yet");
    }
}
