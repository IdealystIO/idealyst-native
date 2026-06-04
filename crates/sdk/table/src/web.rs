//! Web implementation of the Table SDK.
//!
//! Three handlers — one per `Element::External` payload type — that
//! return real HTML table elements. The framework parents each
//! external's children INTO the returned node, so a `Table` containing
//! `TableRow`s containing `TableCell`s flows together naturally:
//!
//! ```text
//! <table>          (Table handler returns this)
//!   <tr>           (TableRow handler returns this; parented into <table>)
//!     <td>...      (TableCell handler returns this; parented into <tr>)
//!   </tr>
//! </table>
//! ```
//!
//! The browser's `table-layout: auto` algorithm sizes every column to
//! fit the widest cell in that column, then applies that width to
//! every row — exactly the behavior the docs' hand-rolled flex table
//! couldn't reproduce.

use crate::{TableCellProps, TableProps, TableRowProps};
use backend_web::WebBackend;
use std::rc::Rc;

/// Register all three Table SDK handlers against a `WebBackend`. One-
/// line call from the app's bootstrap (alongside other `*::register`
/// SDK setups).
pub fn register(backend: &mut WebBackend) {
    backend.register_external::<TableProps, _>(|_props, _backend| build_table());
    backend.register_external::<TableRowProps, _>(|_props, _backend| build_row());
    backend.register_external::<TableCellProps, _>(|props, _backend| build_cell(props));
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
inventory::submit! {
    backend_web::WebExternalRegistrar(register)
}

fn document() -> web_sys::Document {
    web_sys::window()
        .expect("no window")
        .document()
        .expect("no document")
}

fn build_table() -> web_sys::Element {
    let el = document()
        .create_element("table")
        .expect("create_element(table) failed");
    let _ = el.set_attribute("data-external-kind", "table::TableProps");
    // Reset the browser's default table chrome — apps style via the
    // stylesheet system on the wrapping View or via a Table-level
    // style prop in a future iteration. `border-collapse: collapse`
    // keeps the cell borders the author draws from doubling up.
    let _ = el.set_attribute(
        "style",
        "border-collapse: collapse; width: 100%; table-layout: auto;",
    );
    el
}

fn build_row() -> web_sys::Element {
    let el = document()
        .create_element("tr")
        .expect("create_element(tr) failed");
    let _ = el.set_attribute("data-external-kind", "table::TableRowProps");
    el
}

fn build_cell(props: &Rc<TableCellProps>) -> web_sys::Element {
    let tag = if props.header { "th" } else { "td" };
    let el = document()
        .create_element(tag)
        .expect("create_element(td/th) failed");
    let _ = el.set_attribute("data-external-kind", "table::TableCellProps");
    // Deliberately no inline `style` here. Inline styles win over the
    // framework's class-based `apply_style`, so any inline default
    // would block the author's `.with_style(…)` from taking effect.
    // Cells without an author style get the browser's UA defaults
    // (a couple of pixels of padding on `<td>`/`<th>`, `<th>`'s
    // `text-align: center` + `font-weight: bold`). Authors that want
    // a different vocabulary attach their own style; see PropsTable
    // in idea-ui-docs for the canonical pattern.
    el
}
