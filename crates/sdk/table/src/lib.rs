//! `table` — third-party Table SDK.
//!
//! Web emits real HTML `<table>` / `<thead>` / `<tbody>` / `<tr>` /
//! `<th>` / `<td>` so the browser's native table-layout algorithm
//! handles cross-row column alignment for free.
//!
//! Native (iOS / Android / macOS / terminal / gpu) builds a single
//! Taffy **CSS-grid**: every row is flattened to its cells and the cells
//! are parented directly under one grid node whose `N` column tracks
//! span all rows. Because the column tracks are shared, column `i` is
//! one width across every row — the same cross-row alignment the browser
//! gives a real `<table>`. No per-backend handler registration is
//! needed; the framework's existing `view` + grid layout path renders
//! correctly on every target.
//!
//! The columns are `auto`, which `runtime-layout` treats as the
//! `table-layout: auto` signal: it measures each column's content, then
//! short columns hug their content while a text-heavy column absorbs the
//! remaining width and wraps — the same layout a browser gives the web
//! `<table>`. This replaces the old equal-width flex fallback, which
//! sized each row's columns independently and so let columns drift out of
//! alignment between rows.
//!
//! Why a grid and not nested row/cell views: Taffy has no subgrid and no
//! `display: contents`, so a grid can only align the columns of its
//! *direct* children. Keeping `TableRow` as a layout box would make each
//! row a single grid item and break alignment. On native a row therefore
//! lowers to an [`Element::Fragment`] (no box); the row look is carried
//! by per-cell styling (head/body surface + per-cell `border-bottom`
//! separators), exactly as on web.
//!
//! # Why this is an SDK and not a core primitive
//!
//! Web's `<table>` is a layout primitive with no native equivalent —
//! UITableView is a vertical list, Android RecyclerView the same,
//! macOS NSTableView is row-keyed. Putting a web-only-with-real-
//! behavior primitive in the framework would be a web capability
//! wearing a primitive's clothes. The SDK keeps that behavior pluggable:
//! web wires up real `<table>` via `Element::External`, native composes
//! a grid out of the framework's own layout primitives.
//!
//! # Usage
//!
//! ```ignore
//! use table::prelude::*;
//!
//! // Register once at app boot (only does anything on web).
//! table::register(&mut backend);
//!
//! ui! {
//!     Table {
//!         TableRow {
//!             TableCell(header = true) { text { "Prop".to_string() } }
//!             TableCell(header = true) { text { "Type".to_string() } }
//!             TableCell(header = true) { text { "Description".to_string() } }
//!         }
//!         for row in rows {
//!             TableRow {
//!                 TableCell { text { row.name.to_string() } }
//!                 TableCell { text { row.ty.to_string() } }
//!                 TableCell { text { row.desc.to_string() } }
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! # Structure
//!
//! Three primitives, each its own `Element::External` payload type:
//!
//! - [`Table`] — the outer container. Renders as `<table>` on web (an
//!   implicit `<tbody>` wraps all rows because we don't surface a
//!   `TableHead`/`TableBody` distinction yet); a CSS-grid node on native.
//! - [`TableRow`] — `<tr>` on web; an [`Element::Fragment`] of cells on
//!   native (no box — its cells become direct grid children).
//! - [`TableCell`] — `<td>` (or `<th>` when `header = true`) on web,
//!   a grid item on native.
//!
//! Authors style cells through the `style` prop (a normal stylesheet),
//! same as any other primitive. Column widths come from the column
//! tracks: the browser's column-fits-widest algorithm on web, the
//! matching grid track sizing on native. To pin a column to a fixed or
//! proportional width, set that cell's `width` (or a grid track via a
//! future template prop) — but by default both backends size a column to
//! its widest cell across all rows.
#![deny(missing_docs)]

use std::rc::Rc;

use runtime_core::{BuildElement, Bound, Element, ExternalHandle, IdealystSchema, IntoElement};

#[cfg(target_arch = "wasm32")]
use std::any::{Any, TypeId};

#[cfg(not(target_arch = "wasm32"))]
use runtime_core::{
    DisplayKind, FlexDirection, StyleApplication, StyleSheet, TrackSize, VariantSet,
};

// ============================================================================
// Props
// ============================================================================

/// Props for the outer `<table>` container.
///
/// `children` carries the rows (or a wrapping `TableHead`/`TableBody`
/// later if we surface them). The framework parents them into the
/// returned backend node — on web they become real DOM children of
/// the `<table>` element so the browser's table-layout algorithm sees
/// the full row set.
#[derive(Default, IdealystSchema)]
pub struct TableProps {
    /// The table's rows (and, later, any `TableHead`/`TableBody`
    /// wrappers if we surface them). The framework parents these into
    /// the returned backend node — on web they become real DOM
    /// children of the `<table>` so the browser's table-layout
    /// algorithm sees the full row set. Populated by the `ui!`
    /// children block.
    pub children: Vec<Element>,
}

/// Props for a single row (`<tr>`).
#[derive(Default, IdealystSchema)]
pub struct TableRowProps {
    /// The row's cells. Parented into the `<tr>` on web; on native the
    /// row lowers to a fragment and these cells become direct children
    /// of the table's grid. Populated by the `ui!` children block.
    pub children: Vec<Element>,
}

/// Props for a single cell. `header = true` renders `<th>` instead of
/// `<td>` so the browser applies its default header styling and
/// assistive tech announces it as a header.
#[derive(Default, IdealystSchema)]
pub struct TableCellProps {
    /// When `true`, render a `<th>` (header cell) instead of a `<td>`
    /// on web — the browser applies its default header styling
    /// (centered, bold) and assistive tech announces it as a header.
    /// On native `header` has no built-in visual effect (the cell is a
    /// grid item); the caller styles header cells via `.with_style(...)`.
    pub header: bool,
    /// The cell's contents (typically a `text`). Parented into the
    /// `<td>`/`<th>` on web / the grid item on native. Populated by the
    /// `ui!` children block.
    pub children: Vec<Element>,
}

// ============================================================================
// Handles
// ============================================================================

/// Typed handle for a `Table` external element; lets callers attach
/// styles/refs to the table container via the `Bound<TableHandle>` builder.
pub type TableHandle = ExternalHandle<TableProps>;
/// Typed handle for a `TableRow` external element.
pub type TableRowHandle = ExternalHandle<TableRowProps>;
/// Typed handle for a `TableCell` external element.
pub type TableCellHandle = ExternalHandle<TableCellProps>;

// ============================================================================
// Constructors
// ============================================================================

/// Build a `Table` container. On web lowers to `Element::External`
/// keyed by `TableProps` (the registered handler emits a real
/// `<table>`); on native lowers to a CSS-grid node (see the non-wasm
/// `table` below).
#[cfg(target_arch = "wasm32")]
pub fn table(mut props: TableProps) -> Bound<TableHandle> {
    let children = std::mem::take(&mut props.children);
    external(TypeId::of::<TableProps>(),
             std::any::type_name::<TableProps>(),
             Rc::new(props) as Rc<dyn Any>,
             children)
}

/// Build a `Table` container. On native lowers to a CSS-grid whose
/// column tracks span every row, so a column is one width across all
/// rows — the same cross-row alignment the browser's `<table>` gives on
/// web (web builds the `Element::External` variant in the `wasm32` arm
/// above).
///
/// Taffy has no subgrid and no `display: contents`, so a grid only
/// aligns columns of its *direct* children. We therefore flatten every
/// row into its cells and parent the cells directly under one grid node
/// (`grid-auto-flow: row` re-groups them into rows, `len` cells per
/// row). The grid sheet lives on an INNER node so a later author-side
/// `.with_style(...)` on the returned (outer) node — e.g. idea-ui's
/// themed surface — styles the table frame without clobbering the grid.
#[cfg(not(target_arch = "wasm32"))]
pub fn table(mut props: TableProps) -> Bound<TableHandle> {
    let rows = std::mem::take(&mut props.children);
    let mut cells: Vec<Element> = Vec::new();
    let mut columns = 0usize;
    for row in rows {
        let row_cells = extract_row_cells(row);
        columns = columns.max(row_cells.len());
        cells.extend(row_cells);
    }
    // Inner node: the actual grid. Its sheet (display:grid + N tracks)
    // is one level below the `.with_style(...)` target so it survives.
    let inner = runtime_core::view(cells)
        .with_style(StyleApplication::new(native_styles::grid_sheet(columns)))
        .into_element();
    // Outer node: an unstyled passthrough — the `.with_style(...)`
    // target. The framework's default cross-axis stretch makes the inner
    // grid fill this node's width.
    Bound::new(runtime_core::view(vec![inner]).into_element())
}

/// Pull a row's cells out so they can be parented directly under the
/// grid. `table_row` lowers a row to an [`Element::Fragment`] of its
/// cells (no box of its own), which is the hot path. A row that lowered
/// to a plain `view` (defensive) yields its children; any other stray
/// element is treated as a single one-cell row so nothing silently
/// vanishes.
///
/// Note: idea-ui's `TableRow` is a plain `#[component]` (no `methods!`),
/// so it is never wrapped in `Element::Component` — the Fragment arrives
/// here intact even under the `robot` feature.
#[cfg(not(target_arch = "wasm32"))]
fn extract_row_cells(row: Element) -> Vec<Element> {
    match row {
        Element::Fragment { children } => children,
        Element::View { children, .. } => children,
        other => vec![other],
    }
}

/// Build a table row.
#[cfg(target_arch = "wasm32")]
pub fn table_row(mut props: TableRowProps) -> Bound<TableRowHandle> {
    let children = std::mem::take(&mut props.children);
    external(TypeId::of::<TableRowProps>(),
             std::any::type_name::<TableRowProps>(),
             Rc::new(props) as Rc<dyn Any>,
             children)
}

/// Build a table row. On native lowers to an [`Element::Fragment`] of
/// the row's cells — it produces no layout box of its own. The parent
/// `table` flattens these fragments so every cell is a direct child of
/// one grid node, which is the only way a subgrid-less, contents-less
/// Taffy can align columns across rows. The row's identity therefore
/// lives only on web (`<tr>`); on native, cell styling carries the row
/// look (head/body surface, per-cell `border-bottom` row separators).
#[cfg(not(target_arch = "wasm32"))]
pub fn table_row(mut props: TableRowProps) -> Bound<TableRowHandle> {
    let children = std::mem::take(&mut props.children);
    Bound::new(Element::Fragment { children })
}

/// Build a table cell. `header = true` produces a `<th>` on web; on
/// native the cell is a grid item — visual treatment lives on the
/// caller's `with_style(...)` (e.g. idea-ui's `TableHeadCell`).
#[cfg(target_arch = "wasm32")]
pub fn table_cell(mut props: TableCellProps) -> Bound<TableCellHandle> {
    let children = std::mem::take(&mut props.children);
    external(TypeId::of::<TableCellProps>(),
             std::any::type_name::<TableCellProps>(),
             Rc::new(props) as Rc<dyn Any>,
             children)
}

/// Build a table cell. On native lowers to a plain `view` that becomes a
/// grid item of the table; its width is set by the column track, so the
/// default cell sheet only stacks the cell's own content. `header` has
/// no visual effect here (web emits a `<th>` in the `wasm32` arm above).
#[cfg(not(target_arch = "wasm32"))]
pub fn table_cell(mut props: TableCellProps) -> Bound<TableCellHandle> {
    let children = std::mem::take(&mut props.children);
    native_view::<TableCellHandle>(children, native_styles::cell_sheet())
}

#[cfg(target_arch = "wasm32")]
fn external<H>(
    type_id: TypeId,
    type_name: &'static str,
    payload: Rc<dyn Any>,
    children: Vec<Element>,
) -> Bound<H> {
    Bound::new(Element::External {
        type_id,
        type_name,
        payload,
        children,
        style: None,
        ref_fill: None,
        accessibility: runtime_core::accessibility::AccessibilityProps::default(),
    })
}

// =============================================================================
// Native (non-web) view-tree fallback.
//
// Each constructor returns a `Bound<H>` wrapping an `Element::View` with
// a pre-attached `StyleSource::Static` that supplies the SDK's default
// flex layout for that role. The framework's normal `view` path applies
// the style via Taffy on every native backend — no per-backend handler
// registration required.
//
// Author-side `.with_style(...)` chained on the returned `Bound` lands
// on the same `style` slot, replacing the SDK's default with the
// caller's stylesheet (the framework's `with_style` overwrites, not
// merges). The themed `Table` / `TableRow` / `TableCell` in idea-ui
// supply their own visual stylesheets that already include the right
// flex axis, so layout stays correct end-to-end.
// =============================================================================

#[cfg(not(target_arch = "wasm32"))]
fn native_view<H>(children: Vec<Element>, sheet: Rc<StyleSheet>) -> Bound<H> {
    let style = StyleApplication::new(sheet);
    // Go through `runtime_core::view(...).with_style(...)` so the
    // construction path stays insulated from `Element::View`'s field
    // shape (which includes a feature-gated `test_id` field under
    // `runtime-core/robot`). The view builder fills sensible defaults
    // for every field; `with_style` writes the SDK's role-default
    // sheet into the same `style` slot a later author-side
    // `.with_style(...)` would overwrite.
    //
    // The handle-type marker `H` differs from the `view()` return
    // (`Bound<ViewHandle>`), so we re-wrap via `Bound::new` after
    // extracting the underlying `Element`. The marker is type-check
    // only — see `Bound`'s rustdoc.
    Bound::new(runtime_core::view(children).with_style(style).into_element())
}

#[cfg(not(target_arch = "wasm32"))]
mod native_styles {
    use super::*;
    use runtime_core::StyleRules;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// The sizing function applied to every table column: `auto`.
    ///
    /// An all-`Auto` column grid is the signal `runtime-layout` uses to
    /// run its `table-layout: auto` column sizing: it measures each
    /// column's content and sizes columns so short columns hug their
    /// content while a text-heavy column (a description) absorbs the
    /// remaining width and wraps — the same layout a browser gives the web
    /// `<table>`. Using `Auto` here (not `fr`/`px`) is what opts a table
    /// into that behavior — see `LayoutTree::compute`'s table-grid pass.
    /// One function so the recipe stays in a single place.
    fn column_track() -> TrackSize {
        TrackSize::Auto
    }

    thread_local! {
        // Cache one grid sheet per column count: the framework's style
        // resolver dedup's applications by sheet pointer + variant set,
        // so reusing a sheet across same-width tables keeps the class
        // table small. Keyed by N because the column-track list (and
        // thus the resolved rules) differs per width.
        static GRID_SHEETS: RefCell<HashMap<usize, Rc<StyleSheet>>> = RefCell::new(HashMap::new());
        static CELL_SHEET: RefCell<Option<Rc<StyleSheet>>> = RefCell::new(None);
    }

    /// Grid sheet for an `n`-column table: `display: grid` plus `n`
    /// identical column tracks. Cells (the grid's direct children) are
    /// placed row-major by auto-flow, so every `n`th cell starts a new
    /// row and column `i` is one width across all of them.
    pub(super) fn grid_sheet(n: usize) -> Rc<StyleSheet> {
        // An empty table (no rows / no cells) has no columns; a 1-track
        // grid lays out a single column harmlessly.
        let n = n.max(1);
        GRID_SHEETS.with(|slot| {
            slot.borrow_mut()
                .entry(n)
                .or_insert_with(|| {
                    let tracks: Vec<TrackSize> = (0..n).map(|_| column_track()).collect();
                    Rc::new(StyleSheet::new(move |_vs: &VariantSet| StyleRules {
                        display: Some(DisplayKind::Grid),
                        grid_template_columns: Some(tracks.clone()),
                        ..Default::default()
                    }))
                })
                .clone()
        })
    }

    pub(super) fn cell_sheet() -> Rc<StyleSheet> {
        CELL_SHEET.with(|slot| {
            slot.borrow_mut()
                .get_or_insert_with(|| {
                    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
                        // A cell is a grid item; the column track sizes
                        // its width, so no flex sizing is needed here.
                        // Stack the cell's own content vertically so
                        // multi-line content (a label + a description)
                        // wraps naturally inside the column.
                        flex_direction: Some(FlexDirection::Column),
                        ..Default::default()
                    }))
                })
                .clone()
        })
    }
}

// Styling note: `Bound<H>::with_style(…)` is already provided as an
// inherent method by runtime-core on every `Bound`, including ours.
// Authors attach a style to a `<td>` / `<tr>` / `<table>` by calling
// it on the constructor's return value. Use the raw-expression child
// syntax inside `ui!` because the macro doesn't auto-chain methods
// onto user-component tags:
//
// ```ignore
// ui! {
//     TableRow {
//         { table_cell(TableCellProps { … }).with_style(MyCellStyle()) }
//     }
// }
// ```
//
// The framework's `apply_style` lands a resolved CSS class on the
// `<td>`, and `border-collapse: collapse` on the parent `<table>`
// merges adjacent cell borders into one continuous row boundary —
// which is the whole reason borders should live on the cell, not on
// an inner view wrapper that would dangle with the cell's wrapped
// content.

// ============================================================================
// `ui!` dispatch — type aliases + BuildElement impls
//
// The `ui!` macro lowers a user-tag `Table { … }` to a struct literal
// `BuildElement::build(Table { … })`, so the tag name must resolve as a
// *type* with a `BuildElement` impl whose `build` returns an
// `Element`. Each Props struct gets a matching alias + impl below.
// ============================================================================

/// `ui!` tag alias for the table container — `ui! { Table { … } }`
/// resolves to this type and dispatches through `BuildElement`.
pub type Table = TableProps;
/// `ui!` tag alias for a table row.
pub type TableRow = TableRowProps;
/// `ui!` tag alias for a table cell.
pub type TableCell = TableCellProps;

impl BuildElement for TableProps {
    fn build(self) -> Element {
        table(self).into_element()
    }
}

impl BuildElement for TableRowProps {
    fn build(self) -> Element {
        table_row(self).into_element()
    }
}

impl BuildElement for TableCellProps {
    fn build(self) -> Element {
        table_cell(self).into_element()
    }
}

// ============================================================================
// Prelude
// ============================================================================

/// Glob-importable bundle of the table tags, props, handles, and
/// constructors for use at `ui!` call sites.
pub mod prelude {
    pub use super::{
        table, table_cell, table_row, Table, TableCell, TableCellHandle, TableCellProps,
        TableHandle, TableProps, TableRow, TableRowHandle, TableRowProps,
    };
}

// ============================================================================
// Per-target registration. Only the web target registers anything — it
// emits real `<table>`/`<tr>`/`<td>` via `Element::External`. Native
// builds its grid directly in the constructors above (the `view` + grid
// layout path needs no handler), so `register` is a no-op there.
// ============================================================================

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;

#[cfg(not(target_arch = "wasm32"))]
mod fallback {
    use runtime_core::Backend;

    /// No-op register for non-web targets — native tables are built as
    /// a grid directly in `table()`, with no backend handler to install.
    pub fn register<B: Backend>(_backend: &mut B) {}
}
#[cfg(not(target_arch = "wasm32"))]
pub use fallback::register;

// ============================================================================
// Native lowering tests
// ============================================================================

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use runtime_core::{resolve_style, DisplayKind, StyleSource};

    fn cell() -> Element {
        table_cell(TableCellProps::default()).into_element()
    }

    fn row(n: usize) -> Element {
        let cells: Vec<Element> = (0..n).map(|_| cell()).collect();
        table_row(TableRowProps { children: cells }).into_element()
    }

    /// A row produces no layout box of its own — it lowers to a
    /// `Fragment` whose children are the cells, so the parent table can
    /// flatten them into a single grid.
    #[test]
    fn table_row_lowers_to_fragment_of_cells() {
        match row(3) {
            Element::Fragment { children } => assert_eq!(children.len(), 3),
            _ => panic!("table_row must lower to an Element::Fragment of its cells"),
        }
    }

    /// The table lowers to an outer passthrough wrapping an inner grid:
    /// every cell from every row is a direct child of the grid, and the
    /// grid's style is `display: grid` with one column track per column.
    #[test]
    fn table_lowers_to_grid_with_flattened_cells() {
        // header row of 3 + two body rows of 3 → 9 cells, 3 columns.
        let t = table(TableProps {
            children: vec![row(3), row(3), row(3)],
        })
        .into_element();

        // Outer node is a passthrough View wrapping exactly the grid.
        let inner = match t {
            Element::View { children, .. } => {
                assert_eq!(children.len(), 1, "outer wraps exactly the inner grid");
                children.into_iter().next().unwrap()
            }
            _ => panic!("table must lower to an outer View"),
        };

        match inner {
            Element::View { children, style, .. } => {
                assert_eq!(
                    children.len(),
                    9,
                    "all 9 cells become direct children of the grid"
                );
                let app = match style.expect("grid view carries a style") {
                    StyleSource::Static(app) => app,
                    _ => panic!("the grid sheet is constant → StyleSource::Static"),
                };
                let rules = resolve_style(&app);
                assert_eq!(
                    rules.display,
                    Some(DisplayKind::Grid),
                    "the inner node lays its children out as a grid"
                );
                assert_eq!(
                    rules.grid_template_columns.as_ref().map(|c| c.len()),
                    Some(3),
                    "one column track per table column"
                );
            }
            _ => panic!("inner node must be the grid View"),
        }
    }

    /// A ragged table (rows of differing length) sizes its column track
    /// list to the widest row, so short rows simply leave trailing
    /// columns empty rather than the grid losing a column.
    #[test]
    fn table_column_count_is_widest_row() {
        let t = table(TableProps {
            children: vec![row(2), row(4), row(3)],
        })
        .into_element();
        let inner = match t {
            Element::View { children, .. } => children.into_iter().next().unwrap(),
            _ => panic!("outer View"),
        };
        if let Element::View { style, .. } = inner {
            let app = match style.unwrap() {
                StyleSource::Static(app) => app,
                _ => panic!("Static"),
            };
            let rules = resolve_style(&app);
            assert_eq!(
                rules.grid_template_columns.as_ref().map(|c| c.len()),
                Some(4),
                "column count tracks the widest row (4)"
            );
        } else {
            panic!("inner grid View");
        }
    }
}
