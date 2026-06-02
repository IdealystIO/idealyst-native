//! `table` — third-party Table SDK.
//!
//! Web emits real HTML `<table>` / `<thead>` / `<tbody>` / `<tr>` /
//! `<th>` / `<td>` so the browser's native table-layout algorithm
//! handles cross-row column alignment for free.
//!
//! Native (iOS / Android / macOS / terminal / gpu) builds a plain
//! `Element::View` tree with Taffy flex styling — the outer container
//! stacks rows in a column, each row lays cells out in a row, and
//! cells claim equal width via `flex_grow: 1` + `flex_basis: 0`. No
//! per-backend handler registration needed; the framework's existing
//! `view` path renders correctly on every target.
//!
//! Native does not reproduce HTML's column-fits-widest behavior — cells
//! share width equally. Authors that need per-column widths attach an
//! explicit `width`/`flex_grow` style to individual cells.
//!
//! # Why this is an SDK and not a core primitive
//!
//! Web's `<table>` is a layout primitive with no native equivalent —
//! UITableView is a vertical list, Android RecyclerView the same,
//! macOS NSTableView is row-keyed. Putting a web-only-with-real-
//! behavior primitive in the framework would be a web capability
//! wearing a primitive's clothes. The SDK keeps that behavior pluggable:
//! web wires up real `<table>` via `Element::External`, native composes
//! plain views.
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
//!   `TableHead`/`TableBody` distinction yet).
//! - [`TableRow`] — `<tr>` on web, a flex row of cells on native.
//! - [`TableCell`] — `<td>` (or `<th>` when `header = true`) on web,
//!   a flex item on native.
//!
//! The author controls per-cell column proportions through the
//! component's `style` prop (a normal stylesheet), same as any other
//! primitive. The web backend layers `<table>`'s native column-fits-
//! widest algorithm on top.
#![deny(missing_docs)]

use std::rc::Rc;

use runtime_core::{BuildElement, Bound, Element, ExternalHandle, IdealystSchema, IntoElement};

#[cfg(target_arch = "wasm32")]
use std::any::{Any, TypeId};

#[cfg(not(target_arch = "wasm32"))]
use runtime_core::{
    FlexDirection, StyleApplication, StyleSheet, Tokenized, VariantSet,
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
    /// The row's cells. Parented into the `<tr>` on web / the flex row
    /// on native. Populated by the `ui!` children block.
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
    /// On native it's a layout passthrough with no visual effect; the
    /// caller styles header cells via `.with_style(...)`.
    pub header: bool,
    /// The cell's contents (typically a `text`). Parented into the
    /// `<td>`/`<th>` on web / the flex item on native. Populated by the
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
/// `<table>`); on native lowers to a plain `view` column with the
/// SDK's default table styling.
#[cfg(target_arch = "wasm32")]
pub fn table(mut props: TableProps) -> Bound<TableHandle> {
    let children = std::mem::take(&mut props.children);
    external(TypeId::of::<TableProps>(),
             std::any::type_name::<TableProps>(),
             Rc::new(props) as Rc<dyn Any>,
             children)
}

/// Build a `Table` container. On native lowers to a plain `view` column
/// with the SDK's default table styling (web builds the `Element::External`
/// variant in the `wasm32` arm above).
#[cfg(not(target_arch = "wasm32"))]
pub fn table(mut props: TableProps) -> Bound<TableHandle> {
    let children = std::mem::take(&mut props.children);
    native_view::<TableHandle>(children, native_styles::table_sheet())
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

/// Build a table row. On native lowers to a plain `view` row laid out
/// horizontally with the SDK's default row styling.
#[cfg(not(target_arch = "wasm32"))]
pub fn table_row(mut props: TableRowProps) -> Bound<TableRowHandle> {
    let children = std::mem::take(&mut props.children);
    native_view::<TableRowHandle>(children, native_styles::row_sheet())
}

/// Build a table cell. `header = true` produces a `<th>` on web; on
/// native it's a layout passthrough — visual treatment lives on the
/// caller's `with_style(...)` (e.g. idea-ui's `TableHeadCell`).
#[cfg(target_arch = "wasm32")]
pub fn table_cell(mut props: TableCellProps) -> Bound<TableCellHandle> {
    let children = std::mem::take(&mut props.children);
    external(TypeId::of::<TableCellProps>(),
             std::any::type_name::<TableCellProps>(),
             Rc::new(props) as Rc<dyn Any>,
             children)
}

/// Build a table cell. On native lowers to a plain `view` claiming equal
/// width via the SDK's default cell styling; `header` has no visual effect
/// here (web emits a `<th>` in the `wasm32` arm above).
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

    thread_local! {
        // Cache the sheets so every `table()` call reuses the same
        // `Rc<StyleSheet>` — the framework's style resolver dedup's
        // applications by sheet pointer + variant set, so sharing a
        // single sheet across all instances keeps the class table
        // small (one class per role, not one per call site).
        static TABLE_SHEET: RefCell<Option<Rc<StyleSheet>>> = RefCell::new(None);
        static ROW_SHEET: RefCell<Option<Rc<StyleSheet>>> = RefCell::new(None);
        static CELL_SHEET: RefCell<Option<Rc<StyleSheet>>> = RefCell::new(None);
    }

    pub(super) fn table_sheet() -> Rc<StyleSheet> {
        TABLE_SHEET.with(|slot| {
            slot.borrow_mut()
                .get_or_insert_with(|| {
                    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
                        // Default View direction is already Column;
                        // stating it explicitly documents intent and
                        // protects against future default changes.
                        flex_direction: Some(FlexDirection::Column),
                        ..Default::default()
                    }))
                })
                .clone()
        })
    }

    pub(super) fn row_sheet() -> Rc<StyleSheet> {
        ROW_SHEET.with(|slot| {
            slot.borrow_mut()
                .get_or_insert_with(|| {
                    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
                        // Rows lay their cells out horizontally.
                        flex_direction: Some(FlexDirection::Row),
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
                        // Equal-width column distribution. `flex_grow: 1`
                        // alone would size cells by their content
                        // (the typical flex behavior); `flex_basis: 0`
                        // forces them to share remaining space evenly,
                        // matching the "every column the same width"
                        // expectation for a generic data table.
                        flex_grow: Some(Tokenized::Literal(1.0)),
                        flex_basis: Some(Tokenized::Literal(runtime_core::Length::Px(0.0))),
                        // Stack cell contents vertically — multi-line
                        // cell content (a label + a description) should
                        // wrap naturally inside the column.
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
// Per-target registration. Only the web target actually does anything
// meaningful — native fallthrough relies on the framework's "external
// not registered" placeholder, which renders a flex container by
// default.
// ============================================================================

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;

#[cfg(not(target_arch = "wasm32"))]
mod fallback {
    use runtime_core::Backend;

    /// No-op register for non-web targets. The framework's
    /// `External` placeholder renders an unstyled flex view —
    /// columns won't be width-fit but the row/cell tree still
    /// lays out.
    pub fn register<B: Backend>(_backend: &mut B) {}
}
#[cfg(not(target_arch = "wasm32"))]
pub use fallback::register;
