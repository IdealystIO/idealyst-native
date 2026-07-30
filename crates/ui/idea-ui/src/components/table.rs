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
    IntoElement, Reactive, Signal, StyleRules, Tokenized,
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
    /// The click surface spans the full row, but it sits on each cell's
    /// own node rather than a layer above the content, so an interactive
    /// child (a `Button`, `Link`) inside a cell still receives its own
    /// tap first: its recognizer consumes the event, which stops it from
    /// reaching the row handler (the standard "buttons in a clickable row"
    /// pattern). Taps on plain content or empty cell space fall through to
    /// the row callback.
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
        let hovered = signal(false);
        let cells: Vec<Element> = children
            .into_iter()
            .map(|cell| make_row_cell_interactive(cell, hovered, cb.clone()))
            .collect();
        return sdk_row(SdkTableRowProps { children: cells }).into_element();
    }

    sdk_row(SdkTableRowProps { children }).into_element()
}

/// The whole-row hover overlay: pointer cursor always, the themed
/// `color-surface-alt` background while `hovered` is on. The override
/// layer resolves last, so the `Some` background wins only while hovered
/// and otherwise leaves the cell's base sheet (padding, border, head/body
/// surface) untouched.
fn row_hover_overlay(on: bool) -> StyleRules {
    let mut overlay = StyleRules {
        cursor: Some(Cursor::Pointer),
        ..Default::default()
    };
    if on {
        overlay.background = Some(Tokenized::token("color-surface-alt", Color("#eef0f7".into())));
    }
    overlay
}

/// Attach whole-row click + hover to a single cell.
///
/// The reactive row-hover style is layered over the cell's existing themed
/// sheet, and the tap recognizer + shared hover flag ride on the cell's OWN
/// backend node — a grid item on native, a `<td>`/`<th>` on web. The scene
/// payload is type-erased, so the cell introspection lives in the table SDK
/// (`cell_base_application` / `set_cell_style` / `set_cell_interaction`,
/// which reach both shapes). Payloads are pre-mount, so in-place mutation is
/// the sanctioned path (the navigator handlers' style-override fold uses the
/// same `PrimCell::with_mut` mechanism).
///
/// Putting the handler on the cell itself — an *ancestor* of whatever the
/// cell contains — is what makes buttons-in-a-clickable-row work: an
/// interactive child (a `Button`, `Link`) recognizes its own tap first and
/// returns `consumed`, which stops the event before it reaches this row
/// handler (bubbling + `stop_propagation` on web, the responder chain on
/// native). Taps on plain content or empty space fall through to the row.
/// This replaces the earlier web-only full-bleed overlay, which physically
/// covered the cell's content and so swallowed a button's click.
fn make_row_cell_interactive(cell: Element, hovered: Signal<bool>, cb: Rc<dyn Fn()>) -> Element {
    use runtime_core::{tap, TapRecognizer};

    // Reactive whole-row hover style, layered over the cell's existing
    // themed sheet. A cell without a static sheet application keeps its
    // style untouched but stays clickable.
    if let Some(base) = table::cell_base_application(&cell) {
        table::set_cell_style(&cell, move || {
            base.clone().with_overrides(row_hover_overlay(hovered.get()))
        });
    }
    // `tap(..)` yields the `TouchHandler` Rc directly; the hover reporter
    // feeds the row's shared flag.
    let recognizer = tap(TapRecognizer::new(), move || (cb)());
    table::set_cell_interaction(&cell, recognizer, Rc::new(move |entering| hovered.set(entering)));
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
        // `.into_style_application()` (→ `StyleSource::Static`), NOT the
        // bare builder: a clickable row's hover highlight re-derives the
        // cell's `StyleApplication` from the built element
        // (`apply_row_hover_style`), which a `--premint` build's opaque
        // `Preminted` class can't provide. Cells are compose-happy by
        // design, so they deliberately stay on the live engine.
        bound.with_style(TableHeadCell().into_style_application()).into_element()
    } else {
        bound.with_style(TableBodyCell().into_style_application()).into_element()
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
// Tests — native (non-web) lowering. On native a `TableRow` lowers to a
// fragment of its cells (see the `table` SDK), so we can read each cell's
// wiring straight off the built tree without a backend. Introspection goes
// through `test_support::classify`.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;

    fn body_cell(text: &str) -> Element {
        TableCell(TableCellProps {
            text: Reactive::Static(Some(text.into())),
            ..Default::default()
        })
    }

    fn row_cells(row: Element) -> Vec<Element> {
        match classify(row) {
            P::Fragment { children } => children,
            _ => panic!("TableRow must lower to a fragment of cells"),
        }
    }

    /// With `on_row_click` set, every cell in the row becomes interactive:
    /// it carries the tap handler (`on_touch`) and the shared hover handler
    /// (`on_hover`), and its style is upgraded to a reactive source so the
    /// whole-row hover highlight can re-apply. This is the whole feature —
    /// if a refactor drops any of the three, the row stops being clickable
    /// or stops highlighting. (The wiring goes through the table SDK's
    /// `set_cell_style`/`set_cell_interaction` helpers — this test is what
    /// fails if that seam regresses.)
    #[test]
    fn clickable_row_makes_every_cell_interactive() {
        with_test_world(|| {
            let row = TableRow(TableRowProps {
                children: vec![body_cell("a"), body_cell("b")],
                on_row_click: Some(Rc::new(|| {})),
            });
            let cells = row_cells(row);
            assert_eq!(cells.len(), 2, "both cells survive post-processing");
            for cell in cells {
                match classify(cell) {
                    P::View {
                        on_hover,
                        on_touch,
                        style,
                        ..
                    } => {
                        assert!(on_touch, "clickable cell carries a tap handler");
                        assert!(
                            on_hover,
                            "clickable cell reports hover into the shared row flag"
                        );
                        assert!(
                            style.expect("clickable cell keeps a style").is_reactive(),
                            "cell style is reactive so the row-hover highlight re-applies"
                        );
                    }
                    _ => panic!("native cell must classify as a View"),
                }
            }
        });
    }

    /// A plain row (no `on_row_click`) leaves its cells untouched: no
    /// handlers, and the static themed style is preserved. Guards against
    /// accidentally making every table row interactive / reactive.
    #[test]
    fn static_row_leaves_cells_passive() {
        with_test_world(|| {
            let row = TableRow(TableRowProps {
                children: vec![body_cell("a")],
                on_row_click: None,
            });
            let mut cells = row_cells(row);
            match classify(cells.remove(0)) {
                P::View {
                    on_hover,
                    on_touch,
                    style,
                    ..
                } => {
                    assert!(!on_touch, "passive cell has no tap handler");
                    assert!(!on_hover, "passive cell has no hover handler");
                    assert!(
                        !style.expect("passive cell keeps its themed style").is_reactive(),
                        "passive cell keeps its static themed style (no per-node Effect)"
                    );
                }
                _ => panic!("native cell must classify as a View"),
            }
        });
    }
}
