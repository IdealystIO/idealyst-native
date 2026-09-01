+++
title = "Migrating 1.3 → 1.4"
order = 909
tags = ["migration", "1.4.0", "breaking", "table", "sdk", "cell", "row", "hover", "style", "map_cell_style", "cell_base_application"]
+++

# Migrating 1.3 → 1.4

> Status: in development — this guide fills in as 1.4.0 breaking changes land.

One break, in the `table` SDK's built-cell post-processing. If you do not
post-process table cells — if you use `Table` / `TableRow` / `TableCell` and
nothing lower — **nothing in your code changes**, and you get a rendering fix
for free (see *What you get*, below).

## `table::cell_base_application` is removed; compose with `map_cell_style`

**What changed.** The read-then-replace pair for restyling a built cell is
gone at the reading end:

- **Removed:** `table::cell_base_application(&cell) -> Option<StyleApplication>`.
- **Added (1.3.29):** `table::map_cell_style(&cell, f) -> bool`, where
  `f: Rc<dyn Fn(StyleApplication) -> StyleApplication>`.

`set_cell_style` and `set_cell_interaction` are unchanged. `set_cell_style`
still REPLACES a cell's style outright — it is for a cell you built yourself,
not for layering onto someone else's.

**Why.** `cell_base_application` could only see a style stored as a static
`StyleProp::Sheet`. A cell styled with a CLOSURE — `StyleProp::SheetDynamic`,
which is what you must hand over when the cell's style depends on a signal —
returned `None`, and every caller was written as:

```rust
if let Some(base) = table::cell_base_application(&cell) { /* restyle */ }
```

so the restyle was skipped **in silence**: no panic, no log, no visual error
anywhere else on the row. That is not a hypothetical shape. `TableCell`
exposes no width hook, so pinning a column to a width means dropping to
`table::table_cell` and passing a style closure (the width has to move while a
resize handle is dragged) — and those cells then sat un-highlighted in a
clickable row whose plain cells lit up normally. The reported symptom was
"the hover is being covered by the cell backgrounds"; the cells had simply
never been wired.

A reader that silently returns `None` for a legitimate input cannot be used
correctly, so it is removed rather than deprecated — see [[migrations]] on
why a repair like this may land in a minor.

**Migrate.** Mechanical: fold the `if let` into the mapper, and take the
application as the closure's argument instead of cloning a captured base.

```rust
// before (1.3)
if let Some(base) = table::cell_base_application(&cell) {
    table::set_cell_style(&cell, move || {
        base.clone()
            .with("dragging", if dragging.get() { "on" } else { "off" })
            .with("drop_target", if over.get() { "on" } else { "off" })
    });
}

// after (1.4)
table::map_cell_style(
    &cell,
    Rc::new(move |app: StyleApplication| {
        app.with("dragging", if dragging.get() { "on" } else { "off" })
            .with("drop_target", if over.get() { "on" } else { "off" })
    }),
);
```

Three things to know about the new call:

- **It composes, it does not replace.** A static `Sheet` is captured and
  re-mapped on every evaluation; a `SheetDynamic` is composed with `f` wrapped
  around it. Either way the cell keeps its own reactive inputs and gains
  whatever `f` reads, so several layers can stack — a row's hover overlay and
  your drag feedback on the same cell.
- **The result is always reactive.** The cell's style comes out as
  `StyleProp::SheetDynamic`, because `f` may read signals.
- **It returns `bool`, and `false` is not an error.** A style with no
  application behind it — a preminted class, a raw-rules closure, no style at
  all — is left EXACTLY as it was and `false` comes back. There is nothing to
  compose with; that was true of `cell_base_application`'s `None` too, only
  now you can see it. Assert on it in a test if the styling is load-bearing.

**If you were reading a cell's application to ASSERT on it** (a test checking
which axes or rules a cell resolves), there is no replacement in the SDK and
none is planned — that was introspection, not styling. Reach into the payload
directly (`PrimCell<ViewPrim>` / `PrimCell<TableCellPrim>`), or use
`idea_ui::test_support::classify`, whose `TStyle::application()` evaluates a
reactive style for you. idea-ui's own table tests moved to the latter.

Status: landed

## What you get

`TableRow(on_row_click = …)` layers its `interactive` / `row_hovered` axes
through `map_cell_style` as of 1.3.29, so **a clickable row's hover now
reaches every cell**, including ones you styled reactively. If you had a
column that never took the row highlight, it does now, with no change on your
side.

This is also the shape to copy for any row-level feedback of your own
(selection, drag lift): compose onto the cell, never replace its style, or you
will drop whatever the row put there first.

## Migration checklist

- [ ] `grep -rn "cell_base_application"` — no hits left in your code.
- [ ] Every hit rewritten as `map_cell_style(&cell, Rc::new(move |app| …))`,
      with the `if let Some(base)` wrapper dropped and `base.clone()` replaced
      by the closure's `app`.
- [ ] Any test that asserted on a cell's application moved to
      `test_support::classify(...).application()` or a direct payload downcast.
- [ ] Cells whose styling is load-bearing: check `map_cell_style`'s `bool`
      rather than assuming it applied.
- [ ] `set_cell_style` call sites reviewed — it still REPLACES, so it is the
      wrong tool for layering onto a cell someone else styled.
