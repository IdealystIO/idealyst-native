//! `table` — third-party Table SDK.
//!
//! Web emits real HTML `<table>` / `<thead>` / `<tbody>` / `<tr>` /
//! `<th>` / `<td>` so the browser's native table-layout algorithm
//! handles cross-row column alignment for free. Native targets get a
//! plain passthrough container (a flex column) — the column-alignment
//! win is web-specific; native users that want pixel-perfect grid
//! layouts already need explicit per-column widths.
//!
//! # Why this is an SDK and not a core primitive
//!
//! Native platforms have no analogous structure (UITableView is a
//! vertical list; Android RecyclerView the same; macOS NSTableView is
//! row-keyed) and a core primitive that's web-only-with-real-behavior
//! would be a web capability wearing a primitive's clothes. So Table
//! plugs in via `Element::External` — registered once per backend,
//! invisible to apps that don't use it.
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

use std::any::{Any, TypeId};
use std::rc::Rc;

use runtime_core::{BuildElement, Bound, Element, ExternalHandle, IntoElement};

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
#[derive(Default)]
pub struct TableProps {
    pub children: Vec<Element>,
}

/// Props for a single row (`<tr>`).
#[derive(Default)]
pub struct TableRowProps {
    pub children: Vec<Element>,
}

/// Props for a single cell. `header = true` renders `<th>` instead of
/// `<td>` so the browser applies its default header styling and
/// assistive tech announces it as a header.
#[derive(Default)]
pub struct TableCellProps {
    pub header: bool,
    pub children: Vec<Element>,
}

// ============================================================================
// Handles
// ============================================================================

pub type TableHandle = ExternalHandle<TableProps>;
pub type TableRowHandle = ExternalHandle<TableRowProps>;
pub type TableCellHandle = ExternalHandle<TableCellProps>;

// ============================================================================
// Constructors
// ============================================================================

/// Build a `Table` container. Lowers to `Element::External` keyed by
/// `TableProps`. The backend's registered handler returns the concrete
/// node (a `<table>` on web, a flex view on native).
pub fn table(mut props: TableProps) -> Bound<TableHandle> {
    let children = std::mem::take(&mut props.children);
    external(TypeId::of::<TableProps>(),
             std::any::type_name::<TableProps>(),
             Rc::new(props) as Rc<dyn Any>,
             children)
}

/// Build a table row.
pub fn table_row(mut props: TableRowProps) -> Bound<TableRowHandle> {
    let children = std::mem::take(&mut props.children);
    external(TypeId::of::<TableRowProps>(),
             std::any::type_name::<TableRowProps>(),
             Rc::new(props) as Rc<dyn Any>,
             children)
}

/// Build a table cell. `header = true` produces a `<th>` on web.
pub fn table_cell(mut props: TableCellProps) -> Bound<TableCellHandle> {
    let children = std::mem::take(&mut props.children);
    external(TypeId::of::<TableCellProps>(),
             std::any::type_name::<TableCellProps>(),
             Rc::new(props) as Rc<dyn Any>,
             children)
}

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

pub type Table = TableProps;
pub type TableRow = TableRowProps;
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
