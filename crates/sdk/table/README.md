# `table`

Cross-platform tabular layout — three primitives (`Table`, `TableRow`,
`TableCell`) built on the framework's `Element::External` extension
mechanism. On **web** they lower to real HTML `<table>` / `<tr>` /
`<th>` / `<td>`, so the browser's native table-layout algorithm handles
cross-row column alignment for free. On **native** they compose plain
flex views.

```rust
use table::prelude::*;

// Register once at app boot (only does anything on web).
table::register(&mut backend);

ui! {
    Table {
        TableRow {
            TableCell(header = true) { text { "Prop".to_string() } }
            TableCell(header = true) { text { "Type".to_string() } }
            TableCell(header = true) { text { "Description".to_string() } }
        }
        for row in rows {
            TableRow {
                TableCell { text { row.name.to_string() } }
                TableCell { text { row.ty.to_string() } }
                TableCell { text { row.desc.to_string() } }
            }
        }
    }
}
```

## Per-platform behavior

| Target | Mechanism |
| --- | --- |
| Web (wasm32) | Real `<table>`/`<tr>`/`<th>`/`<td>` via `Element::External`. `border-collapse: collapse; width: 100%; table-layout: auto;` on the `<table>`; the browser sizes every column to fit its widest cell and applies that width to every row. |
| iOS / Android / macOS / terminal / gpu | Plain `Element::View` tree with Taffy flex: the table stacks rows in a column, each row lays its cells out in a row, and cells claim **equal** width via `flex_grow: 1` + `flex_basis: 0`. No per-backend handler registration needed. |

Native does **not** reproduce HTML's column-fits-widest behavior —
cells share width equally. Authors that need per-column widths attach an
explicit `width` / `flex_grow` style to individual cells via
`.with_style(...)`.

## Why this is an SDK and not a core primitive

Web's `<table>` is a layout primitive with no native equivalent —
`UITableView` is a vertical list, Android `RecyclerView` the same, macOS
`NSTableView` is row-keyed. Putting a web-only-with-real-behavior
primitive in the framework would be a web capability wearing a
primitive's clothes. The SDK keeps that behavior pluggable: web wires up
a real `<table>` via `Element::External`, native composes plain views.

## Structure

Three primitives, each its own `Element::External` payload type:

- [`Table`] — the outer container (`<table>` on web; an implicit
  `<tbody>` wraps all rows, since we don't yet surface a
  `TableHead`/`TableBody` distinction).
- [`TableRow`] — `<tr>` on web, a flex row of cells on native.
- [`TableCell`] — `<td>` (or `<th>` when `header = true`) on web, a flex
  item on native.

## Styling

`Bound<H>::with_style(...)` is provided by runtime-core on every `Bound`,
including these. Attach a style to a cell by calling it on the
constructor's return value (use the raw-expression child syntax inside
`ui!` because the macro doesn't auto-chain methods onto user-component
tags):

```rust
ui! {
    TableRow {
        { table_cell(TableCellProps { /* … */ ..Default::default() })
            .with_style(MyCellStyle()) }
    }
}
```

Put borders on the **cell**, not on an inner wrapper view —
`border-collapse: collapse` on the `<table>` merges adjacent cell borders
into one continuous boundary.

## Registration

`table::register(&mut backend)` is the one-line bootstrap call. On web it
installs the three external handlers; on every native target it's a no-op
(the flex fallback needs no handler).

[`Table`]: src/lib.rs
[`TableRow`]: src/lib.rs
[`TableCell`]: src/lib.rs

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet. Tick each
item as you exercise it.

**Automated**
- [ ] `cargo build -p table --target wasm32-unknown-unknown` — web target

**Rendering / behavior**

Rows and cells should align into a coherent grid; `header = true` cells read as
headers; borders on cells merge cleanly under `border-collapse`.

- [ ] **Web** — inspect the DOM for a real `<table>`/`<tr>`/`<th>`/`<td>`; the
  browser's `table-layout: auto` sizes each column to its widest cell and applies
  that width across every row; `border-collapse: collapse` merges adjacent cell
  borders.
- [ ] **iOS** — ⚠️ not yet device-confirmed. Plain `view`/Taffy-flex tree: rows stack
  in a column, cells lay out in a row with **equal** width (`flex_grow:1` +
  `flex_basis:0`). Confirm cells share width equally (native does *not* fit columns
  to content); `header` has no visual effect unless styled.
- [ ] **Android** — ⚠️ not yet device-confirmed. Same equal-width flex layout as iOS.
- [ ] **macOS / terminal / gpu** — same flex fallback (no per-backend handler);
  verify rows/cells lay out (⚠️ not yet device-confirmed where applicable).
