//! A virtualized list's geometry — item offsets, the visible window, and
//! laying out its mounted rows over a [`LayoutTree`]. Pure, so
//! `tests/virtual_rows.rs` runs it on every host (the crate body is
//! `cfg(target_os = "windows")`).
//!
//! Each row is a detached Taffy root. In an `ItemSize::Measured` list
//! (`VirtualizerCallbacks::measure_sizes`) the size a row is laid out at
//! is the author's estimate until the row has been measured, so before
//! sizing it the row is measured at the list's cross size
//! ([`LayoutTree::measure_main`]: main axis follows the content). A row
//! whose measurement differs from the size in use is reported through
//! `set_measured_size` — the vocabulary stores it by item key, and
//! `item_size(idx)` returns it from then on — and the size in use is
//! updated in place so this pass already lays the row out (and the
//! caller already offsets the following rows) at its rendered size.
//!
//! Converges: the next pass measures the row at the same cross size, gets
//! the same value, finds it within [`MEASURE_EPSILON`] of the size now in
//! use, and reports nothing.

use runtime_layout::{LayoutNode, LayoutTree};

/// A measurement within this many points of the size in use is not a
/// change. Absorbs float noise between passes so a measured row never
/// re-reports (and re-triggers a resync) on every layout.
pub(crate) const MEASURE_EPSILON: f32 = 0.5;

/// One mounted row to lay out: its index in the list, its detached root
/// and the scope id `set_measured_size` is keyed by.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowLayout {
    pub idx: usize,
    pub root: LayoutNode,
    pub scope: u64,
}

/// Lay out `rows` at `cross` across the list and `sizes[idx]` along it.
///
/// With `measure_sizes`, each row is measured first; a measurement that
/// differs from `sizes[idx]` by more than [`MEASURE_EPSILON`] is passed to
/// `report(scope, measured)` and written into `sizes[idx]`. Returns
/// whether any size changed (the caller then recomputes offsets and the
/// scroll extent, and re-windows). Without `measure_sizes` the sizes are
/// authoritative: nothing is measured, nothing is reported.
pub(crate) fn lay_out_rows(
    tree: &mut LayoutTree,
    rows: &[RowLayout],
    sizes: &mut [f32],
    horizontal: bool,
    cross: f32,
    measure_sizes: bool,
    report: &dyn Fn(u64, f32),
) -> bool {
    let mut changed = false;
    for row in rows {
        let Some(size) = sizes.get_mut(row.idx) else { continue };
        if measure_sizes && cross > 0.0 {
            let measured = tree.measure_main(row.root, horizontal, cross);
            if measured.is_finite() && (measured - *size).abs() > MEASURE_EPSILON {
                report(row.scope, measured);
                *size = measured;
                changed = true;
            }
        }
        let (w, h) = if horizontal { (*size, cross) } else { (cross, *size) };
        tree.compute(row.root, w, h);
    }
    changed
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
