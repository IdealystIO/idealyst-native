# `table`

Cross-platform tabular layout — three primitives (`Table`, `TableRow`,
`TableCell`). On **web** they lower to real HTML `<table>` / `<tr>` /
`<th>` / `<td>` through scene-registry handlers, so the browser's native
table-layout algorithm handles cross-row column alignment for free. On
**native** they flatten into ONE shared-track CSS grid, which gives the
same column alignment.

```rust
use table::prelude::*;

// Register the web handlers once at app boot (native needs none):
// backend_web::newcore::start_in("#app", table::register_handlers, app);

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
| Web (wasm32) | Real `<table>`/`<tr>`/`<th>`/`<td>` via scene-registry handlers. `border-collapse: collapse; width: 100%; table-layout: auto;` on the `<table>`; the browser sizes every column to fit its widest cell and applies that width to every row. |
| iOS / Android / macOS / terminal / gpu | Plain views built with the vocabulary glue's `view` builder: rows lower to an `Element::Fragment` (no box) and every cell parents directly under one `display: grid` node with `N` `auto` column tracks. Because the tracks are shared, column `i` is one width across every row. No per-backend handler registration needed. |

`runtime-layout` treats `auto` tracks as the `table-layout: auto`
signal — it measures each column's content, then short columns hug their
content while a text-heavy column absorbs the remaining width and wraps,
matching the browser. Authors that need explicit per-column widths attach
a `width` style to individual cells via `.with_style(...)`.

## Why this is an SDK and not a core primitive

Web's `<table>` is a layout primitive with no native equivalent —
`UITableView` is a vertical list, Android `RecyclerView` the same, macOS
`NSTableView` is row-keyed. Putting a web-only-with-real-behavior
primitive in the framework would be a web capability wearing a
primitive's clothes. The SDK keeps that behavior pluggable: web wires up
a real `<table>` via scene-registry handlers, native composes a grid out
of the framework's own layout primitives.

## Structure

Three primitives, each its own scene payload type on web:

- [`Table`] — the outer container (`<table>` on web; an implicit
  `<tbody>` wraps all rows, since we don't yet surface a
  `TableHead`/`TableBody` distinction).
- [`TableRow`] — `<tr>` on web, a flex row of cells on native.
- [`TableCell`] — `<td>` (or `<th>` when `header = true`) on web, a flex
  item on native.

## Styling

`.with_style(...)` is provided on each of these constructors' builder
return values. Attach a style to a cell by calling it on the
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

`table::register_handlers(&mut registry)` is the one-line bootstrap call
(`table::register(&mut backend)` remains as a no-op for older call sites).
On web it
installs the three mount handlers; on every native target it's a no-op
(the grid lowering needs no handler).

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
- [ ] **iOS** — ⚠️ not yet device-confirmed. Plain `view`/Taffy tree: cells flatten
  into one `display: grid` with `N` `auto` tracks. Confirm column `i` is the same
  width in every row and that short columns hug their content while the text column
  absorbs the slack; `header` has no visual effect unless styled.
- [ ] **Android** — ⚠️ not yet device-confirmed. Same shared-track grid as iOS.
- [ ] **macOS / terminal / gpu** — same grid lowering (no per-backend handler);
  verify rows/cells lay out (⚠️ not yet device-confirmed where applicable).
