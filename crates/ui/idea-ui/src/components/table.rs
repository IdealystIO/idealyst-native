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

use std::rc::Rc;

use runtime_core::{
    component, signal, text as text_node, ui, ChildList, Color, Cursor, Element, IdealystSchema,
    IntoElement, Position, Reactive, Signal, StyleApplication, StyleRules, StyleSource, Tokenized,
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
// Reactive-by-default: only field is `children` (a LIST, auto-skipped);
// `#[props]` is a no-op here but kept for uniformity with the family.
#[runtime_core::props]
#[derive(Default, IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TableProps {
    /// Table rows. Pass `TableRow`s (a header row plus body rows).
    pub children: Vec<Element>,
}

/// A themed data table — a header row plus body rows. Wraps the
/// cross-platform `table` SDK: a real HTML `<table>` on web, a CSS-grid
/// with column tracks shared across rows on native — so columns line up
/// the same way on every platform. Pass `TableRow`s as children.
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

/// Themed table row. A thin passthrough by default; set `on_row_click`
/// to make the whole row interactive (pointer cursor + a themed hover
/// highlight across every cell + a tap callback).
///
/// Note: on native the SDK lowers a row to a layout-transparent fragment
/// (its cells become direct children of the table's grid — Taffy has no
/// subgrid), so a row has no box of its own there. Row-level visuals and
/// interaction must therefore be applied per-cell rather than to a single
/// row element — which is exactly what `on_row_click` does (see
/// [`make_row_cell_interactive`]).
// Reactive-by-default: `children` is a LIST (auto-skipped) and
// `on_row_click` is a callback (auto-skipped); `#[props]` is a no-op here
// but kept for uniformity with the family.
#[runtime_core::props]
#[derive(Default, IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TableRowProps {
    /// Cells in this row. Pass `TableCell`s.
    pub children: Vec<Element>,
    /// Optional row-click handler. When `Some`, the whole row becomes
    /// interactive: every cell shows a pointer cursor, hovering any cell
    /// tints the entire row (`color-surface-alt`, web/macOS — a no-op on
    /// touch backends where there is no hover), and a tap anywhere in the
    /// row invokes this callback.
    ///
    /// The click surface spans the full row; a cell that itself contains
    /// an interactive control (a `Button`, `Link`) will have that control
    /// shadowed by the row's click target — reserve row-click for rows
    /// whose cells are plain content.
    pub on_row_click: Option<Rc<dyn Fn()>>,
}

/// A row within a [`Table`] — holds `TableCell`s. Use the first row as
/// the header (its cells set `header = true`).
#[component(children)]
pub fn TableRow(props: TableRowProps) -> Element {
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }

    // A clickable row shares ONE hover flag across all its cells so
    // hovering any cell highlights the whole row. The cells arrive here
    // already built (the `ui!` children block builds them before the row
    // runs), so we post-process each: layer the reactive hover background
    // + pointer cursor onto its themed style and attach the tap/hover
    // handlers. The signal is created in this row's scope, so it lives as
    // long as the cells that subscribe to it.
    if let Some(cb) = props.on_row_click {
        let hovered = signal!(false);
        let cells: Vec<Element> = children
            .into_iter()
            .map(|cell| make_row_cell_interactive(cell, hovered, cb.clone()))
            .collect();
        return sdk_row(SdkTableRowProps { children: cells }).into_element();
    }

    sdk_row(SdkTableRowProps { children }).into_element()
}

/// Layer a reactive whole-row hover background + pointer cursor onto a
/// cell's existing themed style, driven by the row's shared `hovered`
/// flag. Preserves the cell's base sheet (padding, border, head/body
/// surface) by cloning its `StyleApplication` and merging an override on
/// top — the override layer resolves last, so a `Some` background wins
/// only while hovered and otherwise leaves the base untouched.
///
/// `with_position` seeds `position: relative` so a web cell can anchor
/// its absolutely-positioned hit overlay; native cells don't need it.
/// If the cell carries no static style (not the idea-ui path), the cell
/// is returned unchanged — the caller still attaches the tap handler, so
/// the row stays clickable, just without the highlight.
fn apply_row_hover_style(cell: Element, hovered: Signal<bool>, with_position: bool) -> Element {
    let base: Option<StyleApplication> = match cell_style(&cell) {
        Some(StyleSource::Static(app)) => Some(app.clone()),
        _ => None,
    };
    let Some(base) = base else { return cell };

    cell.with_style(move || {
        let on = hovered.get();
        let mut overlay = StyleRules {
            cursor: Some(Cursor::Pointer),
            ..Default::default()
        };
        if with_position {
            overlay.position = Some(Position::Relative);
        }
        if on {
            overlay.background =
                Some(Tokenized::token("color-surface-alt", Color("#eef0f7".into())));
        }
        base.clone().with_overrides(overlay)
    })
}

/// Read a built cell's current style slot. Cells lower to an
/// `Element::View` on native and an `Element::External` (`<td>`/`<th>`)
/// on web; both carry a `style` field.
fn cell_style(cell: &Element) -> Option<&StyleSource> {
    match cell {
        Element::View { style, .. } | Element::External { style, .. } => style.as_ref(),
        _ => None,
    }
}

/// Native (non-web): the cell is a real `Element::View` grid item, so the
/// tap + hover handlers ride directly on it, and the reactive row-hover
/// background lands on the cell itself.
#[cfg(not(target_arch = "wasm32"))]
fn make_row_cell_interactive(cell: Element, hovered: Signal<bool>, cb: Rc<dyn Fn()>) -> Element {
    use runtime_core::{tap, Bound, TapRecognizer, ViewHandle};

    let styled = apply_row_hover_style(cell, hovered, false);
    // `tap(..)` yields an `Rc<dyn Fn(&TouchEvent) -> TouchResponse>`; wrap
    // it so it satisfies `on_touch`'s `Fn` bound (an `Rc` isn't itself
    // `Fn`).
    let recognizer = tap(TapRecognizer::new(), move || (cb)());
    Bound::<ViewHandle>::new(styled)
        .on_hover(move |entering| hovered.set(entering))
        .on_touch(move |ev| recognizer(ev))
        .into_element()
}

/// Web: a cell lowers to an `Element::External` (`<td>`/`<th>`), which
/// can't carry `on_touch`/`on_hover`. The reactive row-hover background +
/// `position: relative` land on the `<td>` (externals DO carry a style
/// slot), and a full-bleed transparent overlay view — a real `View` that
/// anchors to the cell via `inset: 0` — captures the whole-cell tap +
/// hover that drive the shared highlight and the row callback.
#[cfg(target_arch = "wasm32")]
fn make_row_cell_interactive(cell: Element, hovered: Signal<bool>, cb: Rc<dyn Fn()>) -> Element {
    use runtime_core::{tap, view, TapRecognizer};

    let mut cell = apply_row_hover_style(cell, hovered, true);
    // `tap(..)` yields an `Rc<dyn Fn(&TouchEvent) -> TouchResponse>`; wrap
    // it so it satisfies `on_touch`'s `Fn` bound (an `Rc` isn't itself
    // `Fn`).
    let recognizer = tap(TapRecognizer::new(), move || (cb)());
    let overlay = view(vec![])
        .on_hover(move |entering| hovered.set(entering))
        .on_touch(move |ev| recognizer(ev))
        .with_style(crate::stylesheets::TableRowClickOverlay::sheet())
        .into_element();
    if let Element::External { children, .. } = &mut cell {
        children.push(overlay);
    }
    cell
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
// Reactive-by-default: `text` is already reactive and `children` is a LIST
// (auto-skipped). `header` is STRUCTURAL — it selects the `<th>`/`<td>` SDK
// element AND the head/body style + text branch; it can't be a single style
// sink, so it stays bare via `#[prop(static)]` (TODO below) rather than a
// guessed reactive route.
#[runtime_core::props]
#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TableCellProps {
    /// When `true`, render as `<th>` (and use the head-cell surface +
    /// uppercase muted text style). When `false`, render as `<td>`.
    // TODO(reactive-sweep): route `header` to the `<th>`/`<td>` element +
    // head/body style branch (structural: changes the SDK element tag, needs a
    // `when`/rebuild, not a style closure). Kept bare for now.
    #[prop(static)]
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

// =============================================================================
// Tests — native (non-web) lowering. On native a `TableRow` lowers to an
// `Element::Fragment` of its cells (see the `table` SDK), so we can read
// each cell's wiring straight off the built tree without a backend.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn body_cell(text: &str) -> Element {
        TableCell(TableCellProps {
            text: Reactive::Static(Some(text.into())),
            ..Default::default()
        })
    }

    fn row_cells(row: Element) -> Vec<Element> {
        match row {
            Element::Fragment { children } => children,
            other => panic!("TableRow must lower to a Fragment of cells"),
        }
    }

    /// With `on_row_click` set, every cell in the row becomes interactive:
    /// it carries the tap handler (`on_touch`) and the shared hover handler
    /// (`on_hover`), and its style is upgraded to a reactive source so the
    /// whole-row hover highlight can re-apply. This is the whole feature —
    /// if a refactor drops any of the three, the row stops being clickable
    /// or stops highlighting.
    #[test]
    fn clickable_row_makes_every_cell_interactive() {
        let row = TableRow(TableRowProps {
            children: vec![body_cell("a"), body_cell("b")],
            on_row_click: Some(Rc::new(|| {})),
        });
        let cells = row_cells(row);
        assert_eq!(cells.len(), 2, "both cells survive post-processing");
        for cell in &cells {
            match cell {
                Element::View {
                    on_hover,
                    on_touch,
                    style,
                    ..
                } => {
                    assert!(on_touch.is_some(), "clickable cell carries a tap handler");
                    assert!(
                        on_hover.is_some(),
                        "clickable cell reports hover into the shared row flag"
                    );
                    assert!(
                        matches!(style, Some(StyleSource::Reactive(_))),
                        "cell style is reactive so the row-hover highlight re-applies"
                    );
                }
                _ => panic!("native cell must be an Element::View"),
            }
        }
    }

    /// A plain row (no `on_row_click`) leaves its cells untouched: no
    /// handlers, and the static themed style is preserved. Guards against
    /// accidentally making every table row interactive / reactive.
    #[test]
    fn static_row_leaves_cells_passive() {
        let row = TableRow(TableRowProps {
            children: vec![body_cell("a")],
            on_row_click: None,
        });
        let cells = row_cells(row);
        match &cells[0] {
            Element::View {
                on_hover,
                on_touch,
                style,
                ..
            } => {
                assert!(on_touch.is_none(), "passive cell has no tap handler");
                assert!(on_hover.is_none(), "passive cell has no hover handler");
                assert!(
                    matches!(style, Some(StyleSource::Static(_))),
                    "passive cell keeps its static themed style (no per-node Effect)"
                );
            }
            _ => panic!("native cell must be an Element::View"),
        }
    }
}
