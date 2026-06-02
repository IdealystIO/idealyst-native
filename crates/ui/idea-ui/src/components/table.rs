//! `Table` — themed wrapper over the `table` SDK.
//!
//! ```ignore
//! ui! {
//!     Table {
//!         TableRow {
//!             TableCell(header = true) { text { "Prop".to_string() } }
//!             TableCell(header = true) { text { "Type".to_string() } }
//!             TableCell(header = true) { text { "Description".to_string() } }
//!         }
//!         for row in rows {
//!             TableRow {
//!                 TableCell { text { row.name.clone() } }
//!                 TableCell { text { row.ty.clone() } }
//!                 TableCell { text { row.desc.clone() } }
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! Three components mirror the SDK's shape:
//! - [`Table`] wraps the SDK's `<table>` with the themed surface
//!   (rounded corners, hairline border, theme background).
//! - [`TableRow`] is a thin passthrough over the SDK's `<tr>` — present
//!   for symmetry and future row-level affordances (hover, zebra).
//! - [`TableCell`] wraps `<td>` (or `<th>` when `header = true`) with
//!   the cell-level padding + row divider, and wraps cell contents in
//!   a themed `text` node so values without explicit Typography pick
//!   up the right column treatment.
//!
//! See [Table] / [TableRow] / [TableCell] for the full prop surface.
//!
//! # Layering
//!
//! Mirrors `Spinner` → `activity_indicator` and `Switch` → `toggle`:
//! the underlying primitive (here, the `table` SDK that emits real
//! HTML `<table>` on web) is generic and cross-platform; idea-ui
//! supplies the opinionated visual that reads the active theme.

use runtime_core::{
    component, text as text_node, ui, ChildList, Element, IdealystSchema, IntoElement, Reactive,
};
use table::{table as sdk_table, table_cell as sdk_cell, table_row as sdk_row};
use table::{TableCellProps as SdkTableCellProps, TableProps as SdkTableProps, TableRowProps as SdkTableRowProps};

use crate::stylesheets::{
    Table as TableStyle, TableBodyCell, TableBodyText, TableCellInner, TableHeadCell, TableHeadText,
};

// =============================================================================
// Table
// =============================================================================

/// Themed table container. Wraps the `table` SDK's `<table>` with
/// idea-ui's surface tokens (rounded corners + hairline border + theme
/// background). Pass `TableRow`s as children.
#[derive(Default, IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TableProps {
    /// Table rows. Pass `TableRow`s (a header row plus body rows).
    pub children: Vec<Element>,
}

/// A themed data table — a header row plus body rows. Wraps the
/// cross-platform `table` SDK: a real HTML `<table>` on web, styled flex
/// columns on native. Pass `TableRow`s as children.
#[component(children)]
pub fn Table(props: TableProps) -> Element {
    let style = TableStyle();
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    // SDK's `table()` returns a `Bound<TableHandle>`; chain
    // `.with_style(...)` to land the themed style on the `<table>`
    // itself, then convert to Element.
    sdk_table(SdkTableProps { children })
        .with_style(style)
        .into_element()
}

// =============================================================================
// TableRow
// =============================================================================

/// Themed table row. Currently a thin passthrough — kept as its own
/// component so future row-level affordances (hover highlight, zebra
/// striping, density variants) have a place to land without changing
/// call sites.
#[derive(Default, IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TableRowProps {
    /// Cells in this row. Pass `TableCell`s.
    pub children: Vec<Element>,
}

/// A row within a [`Table`] — holds `TableCell`s. Use the first row as
/// the header (its cells set `header = true`).
#[component(children)]
pub fn TableRow(props: TableRowProps) -> Element {
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    sdk_row(SdkTableRowProps { children }).into_element()
}

// =============================================================================
// TableCell
// =============================================================================

/// Themed table cell. Renders as `<th>` when `header = true`, `<td>`
/// otherwise. Padding + row divider live on the cell itself so
/// `border-collapse: collapse` on the parent table merges adjacent
/// cell borders into one continuous row boundary regardless of how
/// many lines a cell's content wraps to.
///
/// If `text` is `Some`, the cell wraps it in a themed `text` node
/// using the header/body typography token. To compose richer content
/// (links, badges, multiple inline pieces) pass `text = None` and
/// use the `children` block instead.
#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TableCellProps {
    /// When `true`, render as `<th>` (and use the head-cell surface +
    /// uppercase muted text style). When `false`, render as `<td>`.
    pub header: bool,
    /// Convenience text content. The themed `TableHeadText` /
    /// `TableBodyText` styling lands on the inner text node so the
    /// caller doesn't need to wire Typography for the common case.
    /// `Reactive<String>` — static literal, `Signal<String>`, or
    /// `rx!(...)` all work.
    ///
    /// Pass `children` instead when the cell needs richer content
    /// (multiple inline pieces, links, badges, …).
    pub text: Reactive<Option<String>>,
    /// Fully custom cell contents. When set, the `text` prop is
    /// ignored and these children render inside the `<td>` / `<th>`
    /// directly — cell-level padding still applies.
    pub children: Vec<Element>,
}

impl Default for TableCellProps {
    fn default() -> Self {
        Self {
            header: false,
            text: Reactive::Static(None),
            children: Vec::new(),
        }
    }
}

/// A cell within a [`TableRow`]. Set `header = true` for a header
/// (`<th>`) cell; otherwise it renders as a data (`<td>`) cell.
#[component(children)]
pub fn TableCell(props: TableCellProps) -> Element {
    let header = props.header;

    // Resolve the cell contents. When the author supplied `children`,
    // wrap them in a row-flex inner container so flex-grow items
    // (Tag/Button) sit at natural width inside the cell instead of
    // stretching. Otherwise wrap the `text` prop in the role-
    // appropriate themed text node.
    let cell_children: Vec<Element> = if !props.children.is_empty() {
        let mut inner: Vec<Element> = Vec::with_capacity(props.children.len());
        for c in props.children {
            ChildList::append_to(c, &mut inner);
        }
        let inner_style = TableCellInner();
        vec![ui! { view(style = inner_style) { inner } }]
    } else {
        cell_text_children(header, props.text)
    };

    let bound = sdk_cell(SdkTableCellProps { header, children: cell_children });
    // Cell-level styling (padding + border-bottom) on the `<td>` /
    // `<th>` itself. Branching here keeps each style concrete so
    // `IntoStyleSource` resolves on the call (not on a `Box<dyn>`,
    // which the trait doesn't support).
    if header {
        bound.with_style(TableHeadCell()).into_element()
    } else {
        bound.with_style(TableBodyCell()).into_element()
    }
}

/// Render a cell's `text` prop with the role-appropriate themed
/// stylesheet. Split out so the `header` branch can pick its
/// concrete style without needing `Box<dyn IntoStyleSource>`.
fn cell_text_children(header: bool, content: Reactive<Option<String>>) -> Vec<Element> {
    if header {
        match content {
            Reactive::Static(None) => Vec::new(),
            Reactive::Static(Some(s)) => vec![text_node(s).with_style(TableHeadText()).into_element()],
            Reactive::Dynamic(f) => vec![text_node(move || f().unwrap_or_default())
                .with_style(TableHeadText())
                .into_element()],
        }
    } else {
        match content {
            Reactive::Static(None) => Vec::new(),
            Reactive::Static(Some(s)) => vec![text_node(s).with_style(TableBodyText()).into_element()],
            Reactive::Dynamic(f) => vec![text_node(move || f().unwrap_or_default())
                .with_style(TableBodyText())
                .into_element()],
        }
    }
}
