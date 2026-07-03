//! `virtualized` — opinionated virtualized-collection constructors.
//!
//! The framework ships one low-level windowing + recycling *engine* (the
//! `flat_list` / `virtualizer` primitive). It has to be a primitive:
//! native cell recycling (UICollectionView, RecyclerView,
//! NSCollectionView) can't be composed from `view`/`scroll_view`. This
//! crate is the layer above it — the *components* for different
//! virtualization use cases:
//!
//! - [`list`] — one item per cross-axis line (a plain list).
//! - [`grid`] — a uniform grid of a fixed number of columns/lanes.
//! - [`responsive_grid`] — a grid whose lane count is derived from the
//!   container width, like CSS `repeat(auto-fill, minmax(min, 1fr))`.
//!
//! Every constructor returns the same [`Bound<VirtualizerHandle>`] the
//! primitive does, so the builder knobs still chain:
//!
//! ```ignore
//! use virtualized::{grid, fixed_size};
//! use runtime_core::Axis;
//!
//! grid(items, |_, it| it.id, fixed_size(120.0), render_cell, 3)
//!     .gap(8.0)            // 8px between rows and lanes
//!     .overscan(2.0)       // buffer two viewports
//!     .bind(grid_ref)      // grab a handle for scroll_to_index
//! ```
//!
//! # Lanes, not list-vs-grid
//!
//! A list is a grid with one lane. These constructors only differ in the
//! lane count they preset; everything downstream (range math, recycling,
//! measurement) is the one shared engine. That's also why masonry /
//! shortest-lane packing is a natural future addition here — it's another
//! lane mode, not a separate engine — without changing the list/grid
//! constructors below.

use runtime_core::primitives::flat_list::{flat_list, FlatListItemSize};
use runtime_core::primitives::virtualizer::{ItemKey, VirtualizerHandle};
use runtime_core::{Bound, Element, Lanes, Signal};

pub use runtime_core::primitives::flat_list::{fixed_size, FlatListItemSize as ItemSize};
pub use runtime_core::{Axis, Lanes as LaneCount, VirtualLayout, VirtualizerHandle as Handle};

/// A plain virtualized list: one item per cross-axis line, vertical
/// scroll by default. Identical to the framework's `flat_list` — exposed
/// here so the whole virtual-collection family lives behind one import.
///
/// Chain `.axis(Axis::Horizontal)` for a horizontal list, `.gap(n)` for
/// inter-row spacing, `.overscan(f)` to widen the buffer.
pub fn list<T, K, R>(
    data: Signal<Vec<T>>,
    key: K,
    item_size: FlatListItemSize<T>,
    render: R,
) -> Bound<VirtualizerHandle>
where
    T: Clone + 'static,
    K: Fn(usize, &T) -> ItemKey + 'static,
    R: Fn(usize, &T) -> Element + 'static,
{
    flat_list::<T, K, (), R>(data, key, item_size, render)
}

/// A uniform grid of `columns` lanes. Item `i` lands in lane `i %
/// columns` of grid-row `i / columns`; each lane takes an equal share of
/// the cross axis. `item_size` is the *main-axis* extent (row height for
/// a vertical grid). `columns` is clamped to at least 1, so `grid(.., 1)`
/// degrades to a list.
///
/// Chain `.gap(n)` (or `.spacing(main, cross)`) for gutters, and
/// `.axis(Axis::Horizontal)` to scroll sideways with `columns` rows.
pub fn grid<T, K, R>(
    data: Signal<Vec<T>>,
    key: K,
    item_size: FlatListItemSize<T>,
    render: R,
    columns: usize,
) -> Bound<VirtualizerHandle>
where
    T: Clone + 'static,
    K: Fn(usize, &T) -> ItemKey + 'static,
    R: Fn(usize, &T) -> Element + 'static,
{
    list(data, key, item_size, render).lanes(Lanes::Fixed(columns.max(1)))
}

/// A responsive grid: as many lanes as fit, each at least
/// `min_item_cross` points along the cross axis (mirrors CSS
/// `repeat(auto-fill, minmax(min_item_cross, 1fr))`). The lane count is
/// resolved against the container's measured cross extent at layout time,
/// so a resize / rotation re-lanes the grid. Falls back to a single lane
/// on a container narrower than `min_item_cross`.
pub fn responsive_grid<T, K, R>(
    data: Signal<Vec<T>>,
    key: K,
    item_size: FlatListItemSize<T>,
    render: R,
    min_item_cross: f32,
) -> Bound<VirtualizerHandle>
where
    T: Clone + 'static,
    K: Fn(usize, &T) -> ItemKey + 'static,
    R: Fn(usize, &T) -> Element + 'static,
{
    list(data, key, item_size, render).lanes(Lanes::AutoFit {
        min_cross: min_item_cross,
    })
}

#[cfg(test)]
mod tests {
    //! Construction smoke tests. The lane-layout *behavior*
    //! (`grid` ⇒ `Lanes::Fixed(N)`, `responsive_grid` ⇒ `AutoFit`, the
    //! clamp, and the visible-range math) is asserted in `runtime-core`,
    //! where the `Bound`'s primitive is reachable — see
    //! `primitives::virtualizer` and the walker grid tests. Here we only
    //! prove each generic constructor type-checks and builds against a
    //! real `Signal<Vec<T>>` with non-`Copy` data.
    use super::*;
    use runtime_core::signal;

    fn render(_i: usize, _v: &String) -> Element {
        runtime_core::view(Vec::new()).into()
    }

    #[test]
    fn constructors_build_for_non_copy_data() {
        let data: Signal<Vec<String>> = signal!(vec!["a".to_string(), "b".to_string()]);
        let _l = list(data, |i, _| i as u64, fixed_size(40.0), render);

        let data: Signal<Vec<String>> = signal!(vec!["a".to_string(), "b".to_string()]);
        let _g = grid(data, |i, _| i as u64, fixed_size(40.0), render, 3).gap(8.0);

        let data: Signal<Vec<String>> = signal!(vec!["a".to_string(), "b".to_string()]);
        let _rg = responsive_grid(data, |i, _| i as u64, fixed_size(40.0), render, 160.0)
            .axis(Axis::Horizontal);
    }
}
