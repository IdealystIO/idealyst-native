# `virtualized`

Opinionated constructors for virtualized collections — lists, grids, and
responsive grids — built on the framework's low-level `flat_list` /
`virtualizer` windowing engine. The engine itself must be a primitive (native
cell recycling via UICollectionView / RecyclerView / NSCollectionView can't be
composed from `view`/`scroll_view`); this crate is the thin, generic layer above
it that picks a lane layout for each use case.

Every constructor takes the author's `Signal<Vec<T>>` plus key, size, and render
closures — exactly like `flat_list` — and returns the same
`Bound<VirtualizerHandle>` the primitive does, so all the builder knobs
(`.axis`, `.gap`, `.overscan`, `.lanes`, `.bind`) still chain. There is no
styling, header, or selection model here; those are app- or higher-SDK policy.

## What you get

- `list(data, key, item_size, render)` — a plain virtualized list, one item per
  cross-axis line, vertical scroll by default.
- `grid(data, key, item_size, render, columns)` — a uniform grid of `columns`
  lanes (clamped to at least 1). `item_size` is the main-axis extent (row height
  for a vertical grid).
- `responsive_grid(data, key, item_size, render, min_item_cross)` — a grid whose
  lane count is derived from the container's measured cross extent, mirroring CSS
  `repeat(auto-fill, minmax(min_item_cross, 1fr))`. A resize or rotation re-lanes
  it.
- Re-exports: `fixed_size` / `ItemSize` (the `item_size` builders), and `Axis`,
  `LaneCount`, `VirtualLayout`, `Handle` for the chained builder + `.bind`.

## Usage

```rust
use virtualized::{responsive_grid, fixed_size};
use runtime_core::{signal, Axis, Bound, Element, VirtualizerHandle};

struct Cell { key: u64 /* ... */ }

fn icon_grid(cells: runtime_core::Signal<Vec<Cell>>) -> Bound<VirtualizerHandle> {
    responsive_grid(
        cells,
        |_idx, c: &Cell| c.key,            // stable item key
        fixed_size(120.0),                 // main-axis extent (cell height)
        |_idx, c: &Cell| render_cell(c),   // -> Element per item
        96.0,                              // min cell width; lanes fill the rest
    )
    .gap(8.0)        // 8px between rows and lanes
    .overscan(2.0)   // buffer two viewports
}

// A fixed 3-column grid, or a plain list:
fn examples(items: runtime_core::Signal<Vec<Cell>>) {
    let _g = grid_three(items.clone());
    let _l = virtualized::list(items, |i, _| i as u64, fixed_size(44.0), render_row);
}

fn grid_three(items: runtime_core::Signal<Vec<Cell>>) -> Bound<VirtualizerHandle> {
    virtualized::grid(items, |_, c: &Cell| c.key, fixed_size(120.0), |_, c| render_cell(c), 3)
        .gap(8.0)
        .axis(Axis::Vertical)
}
```

Grab a handle by chaining `.bind(grid_ref)` to call `scroll_to_index` later, or
`.into_element().with_style(...)` to style the scroll container directly.

## Lanes, not list-vs-grid

A list is just a grid with one lane. These constructors differ only in the lane
count they preset (`grid` ⇒ `Lanes::Fixed(N)`, `responsive_grid` ⇒
`Lanes::AutoFit`); everything downstream — range math, recycling, measurement —
is the one shared engine. That's why masonry / shortest-lane packing is a natural
future addition here: another lane mode, not a separate engine.

Pure Rust on top of `runtime-core`; nothing platform-specific to gate. No
permissions required.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet. Tick each
item as you exercise it. This crate is a thin generic layer; the windowing /
recycling / lane-math *behavior* is owned by the `flat_list` / `virtualizer`
primitive and tested in `runtime-core`, so the boxes below exercise the
underlying engine through these constructors.

**Automated**
- [ ] `cargo test -p virtualized` — `list` / `grid` / `responsive_grid`
  construction smoke tests against a real `Signal<Vec<T>>` with non-`Copy` data
- [ ] `cargo build -p virtualized --target wasm32-unknown-unknown` — web target

**Behavior**
- [ ] **Web** — a long `list` only renders visible items; `grid(N)` lays out N
  lanes; `responsive_grid` reflows its lane count on window resize; fast scroll
  keeps up with no blank gaps.
- [ ] **iOS** — virtualized cell recycling (UICollectionView) renders only
  visible items; grid lanes + responsive re-laning on rotation correct. ⚠️ not
  yet device-confirmed.
- [ ] **Android** — recycling (RecyclerView) renders only visible items; grid +
  responsive lanes correct. ⚠️ not yet device-confirmed.
- [ ] **macOS** — recycling (NSCollectionView) renders only visible items; grid
  lanes + responsive reflow on resize correct. ⚠️ not yet device-confirmed.
