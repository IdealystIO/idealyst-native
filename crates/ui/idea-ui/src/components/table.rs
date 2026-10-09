//! `Table` — themed wrapper over the `table` SDK.
//!
//! ```ignore
//! ui! {
//!     Table {
//!         TableRow {
//!             TableCell(header = true, width = Some(160.0), truncate = true) { text { "Name".to_string() } }
//!             TableCell(header = true) { text { "Notes".to_string() } }
//!             TableCell(header = true, align = CellAlign::Right) { text { "Hours".to_string() } }
//!         }
//!         for row in rows {
//!             TableRow(tone = row.tone()) {
//!                 TableCell(width = Some(160.0), truncate = true, text = Some(row.name.clone()))
//!                 TableCell(text = Some(row.notes.clone()))
//!                 TableCell(align = CellAlign::Right, text = Some(row.hours.clone()))
//!             }
//!         }
//!         if rows.is_empty() {
//!             TableRow { TableCell(span = ColSpan::Rest, text = Some("Nobody yet".into())) }
//!         }
//!         TableFooter {
//!             TableRow {
//!                 TableCell(span = ColSpan::Columns(2), text = Some("Crew totals".into()))
//!                 TableCell(align = CellAlign::Right, text = Some(total.clone()))
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! Four components mirror the SDK's shape:
//! - [`Table`] wraps the SDK's `<table>` with the themed surface
//!   (rounded corners, hairline border, theme background), and owns the
//!   table-wide settings: `scroll_x`, `density`, and the frame slots.
//! - [`TableRow`] is a `<tr>`: `on_row_click` for a clickable row,
//!   `tone` for a whole-row tint, `bind_to` for the row's handle.
//! - [`TableCell`] wraps `<td>` (or `<th>` when `header = true`) with
//!   the cell-level padding + row divider, and wraps cell contents in
//!   a themed `text` node so values without explicit Typography pick
//!   up the right column treatment. Column layout lives here too —
//!   `width`, `min_width`, `align`, `truncate`, `span`, `pinned`.
//! - [`TableFooter`] holds footer rows (`<tfoot>` on web).
//!
//! See [Table] / [TableRow] / [TableCell] / [TableFooter] for the full
//! prop surface.
//!
//! # Columns: width, alignment, truncation
//!
//! A column is the cells at one position in every row, so its settings
//! go on EVERY cell of it, header included — header and body cells that
//! disagree about a width break the column on both lowerings. An app
//! with many tables usually keeps one column definition and spreads it
//! into each cell.
//!
//! - `width = Some(px)` holds the column at exactly that width. The
//!   table's spare width goes to the columns WITHOUT one, so a table
//!   whose columns are all sized but one makes that one fill the rest —
//!   which is also what a browser does, and why there is no separate
//!   "fill" setting (`width: 100%` on a cell, the CSS spelling, squeezes
//!   every other column down to its narrowest). Both props are
//!   reactive, so a column can follow a resize drag.
//! - `min_width = Some(px)` floors a column without fixing it.
//! - `align` sets the header's and body's alignment together when every
//!   cell of the column carries it.
//! - `truncate = true` keeps the content to one line ending in "…" —
//!   meaningful in a column with a `width` (an unsized column widens to
//!   fit the line instead).
//!
//! There is deliberately no `max_width`: a browser ignores `max-width`
//! on a table cell unless the cell also has a `width`, so it could not
//! mean the same thing on web as on native.
//!
//! # Spanning cells
//!
//! `TableCell(span = ColSpan::Columns(n))` covers `n` columns;
//! `ColSpan::Rest` covers every column to the end of the row — the blank
//! row, an "Add entry" row, a group title. On web it is `colspan`; on
//! native, a multi-track grid placement. A spanning cell's content does
//! not widen the columns it covers on native (it wraps in the width they
//! already have).
//!
//! # Horizontal scrolling & frozen columns
//!
//! `Table(scroll_x = true)` wraps the table in a horizontal scroller
//! (columns lay out at natural width, overflow sideways, and the table
//! still fills the scroller when narrow). `TableCell(pinned =
//! ColumnPin::Left)` / `Right` freezes that cell's column against the
//! scroller edge — a `pinned` axis on the cell stylesheets
//! (`position: Sticky` + an inset + an opaque background), which
//! the browser pins natively on web and the shared sticky registry
//! pins on native. Every backend raises the frozen cells above the
//! content sliding beneath them, including cells an app positions
//! itself (web lowers sticky with `z-index: 1`). Pin the SAME cell in every row (header included) or
//! the column freezes only partially.
//!
//! To freeze SEVERAL leading columns, give each column a `width` and set
//! `pin_offset` on the later ones to the summed widths of the pinned
//! columns before it (a select column 48 wide, then the name column at
//! `pin_offset = Some(48.0)`). Without the offset they all stick at the edge
//! and overlap.
//!
//! # Frame slots
//!
//! `header_slot` / `footer_slot` draw content inside the table's frame,
//! above and below the rows, OUTSIDE the horizontal scroller: a notice
//! strip that must not scroll away with the columns, a blank-state
//! sentence, an "Add entry" action. Slot content gets the cells'
//! padding, and the header slot a divider under it.
//!
//! # Footer rows, row tones, density
//!
//! - `TableFooter { TableRow { … } }` — a totals row: `<tfoot>` on web
//!   (announced as the table's footer), the header band's tint, and
//!   always after the body rows wherever it is written.
//! - `TableRow(tone = RowTone::Warning)` — a whole-row tint
//!   (`Highlight` / `Warning` / `Danger`) that composes with the
//!   clickable-row hover (a toned row darkens its own tint) and with a
//!   frozen column (the `color-table-row-*` tokens are opaque). Applies
//!   to body cells.
//! - `Table(density = TableDensity::Comfortable)` — row padding:
//!   `Compact`, `Standard` (default) or `Comfortable`, which puts a
//!   single-line row in the 44–52pt touch-target band.
//!
//! # Theming the header band
//!
//! Head cells (`TableCell(header = true)` — a head row, and any footer
//! row built the same way) paint `color-table-header`, a token of their
//! own. It ships with the same value as `color-surface-alt`, so the
//! default look is unchanged, but retinting table headers is now a
//! one-token override that leaves cards, field wells, and row hover
//! alone. Row tones read `color-table-row-{highlight,warning,danger}`
//! and their `-hover` variants.
//!
//! # Row drag & drop — bring your own
//!
//! This component deliberately ships NO drag-and-drop behavior. What
//! it (and the `table` SDK underneath) exposes are the HANDLES a
//! custom implementation needs, so apps own the interaction:
//!
//! - `TableRow(bind_to = Some(r))` (or `table::bind_row(&row, fill)`) —
//!   the row's proxy surface handle (the `<tr>` on web, a row-spanning
//!   backdrop view on native): row geometry for drop targeting / frame
//!   reads, or an anchor for a hover card.
//! - `table::visit_row_cells` + `table::set_cell_touch` — fan a drag
//!   recognizer across a row's cells (row-level touch must live
//!   per-cell; see the SDK docs for why).
//! - `table::bind_cell` — per-cell node handles for binding animated
//!   drag offsets (`AnimatedValue::bind`).
//! - `table::map_cell_style` — layer reactive feedback over the
//!   themed cell styles, composing over whatever style a cell already
//!   carries. The cell sheets
//!   ship inert `dragging` / `drop_target` axes (dim + highlight) so
//!   a custom implementation can select themed arms instead of
//!   inventing overrides.
//! The idea-ui docs' data/table page carries a complete userland
//! reorder implementation built from these plus the `dnd` SDK.
//!
//! # Layering
//!
//! Mirrors `Spinner` → `activity_indicator` and `Switch` → `toggle`:
//! the underlying primitive (here, the `table` SDK that emits real
//! HTML `<table>` on web) is generic and cross-platform; idea-ui
//! supplies the opinionated visual that reads the active theme.

use std::rc::Rc;

use runtime_core::{
    component, signal, text as text_node, ui, ChildList, Element, IdealystSchema,
    IntoElement, Length, Reactive, Ref, Signal, StyleApplication, StyleRules, Tokenized,
    VariantEnum, ViewHandle,
};
use runtime_vocabulary::StyleProp;
pub use table::ColSpan;
use table::{table as sdk_table, table_cell as sdk_cell, table_foot as sdk_foot, table_row as sdk_row};
use table::{
    TableCellProps as SdkTableCellProps, TableFootProps as SdkTableFootProps,
    TableProps as SdkTableProps, TableRowProps as SdkTableRowProps,
};

use crate::stylesheets::{
    Table as TableStyle, TableBodyCell, TableBodyText, TableCellInner, TableHeadCell, TableHeadText,
    TableSlot,
};

/// Which edge a [`TableCell`] freezes against in a
/// `Table(scroll_x = true)` — see the module docs. (Named `ColumnPin`
/// rather than `Pin` to stay clear of `std::pin::Pin`.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, IdealystSchema, runtime_core::Remote)]
pub enum ColumnPin {
    /// Freeze against the scroller's left edge.
    Left,
    /// Freeze against the scroller's right edge.
    Right,
}

/// How a [`TableCell`]'s content sits in its column. Give every cell of
/// a column the same value, header included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema, runtime_core::Remote)]
pub enum CellAlign {
    /// Hug the leading edge (the default — text, names).
    #[default]
    Left,
    /// Centre — constant-width content: a status chip, a count, an icon.
    Center,
    /// Hug the trailing edge — figures a reader sums by eye (hours,
    /// money), so they end on one line like the totals row under them.
    Right,
}

// The variant string IS the stylesheet arm the value selects.
impl VariantEnum for CellAlign {
    fn as_variant_str(self) -> &'static str {
        match self {
            CellAlign::Left => "left",
            CellAlign::Center => "center",
            CellAlign::Right => "right",
        }
    }

    fn all_variants() -> &'static [Self] {
        &[CellAlign::Left, CellAlign::Center, CellAlign::Right]
    }
}

/// A whole-row tint for [`TableRow`]. Composes with the clickable-row
/// hover and with frozen columns — see the module docs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema, runtime_core::Remote)]
pub enum RowTone {
    /// No tint.
    #[default]
    None,
    /// A row picked out for attention — the selected or current item.
    Highlight,
    /// A row that needs a look — a flagged timesheet line.
    Warning,
    /// A row in trouble — an alarm, a failed check.
    Danger,
}

// The variant string IS the stylesheet arm the value selects.
impl VariantEnum for RowTone {
    fn as_variant_str(self) -> &'static str {
        match self {
            RowTone::None => "none",
            RowTone::Highlight => "highlight",
            RowTone::Warning => "warning",
            RowTone::Danger => "danger",
        }
    }

    fn all_variants() -> &'static [Self] {
        &[RowTone::None, RowTone::Highlight, RowTone::Warning, RowTone::Danger]
    }
}

/// Row padding for a whole [`Table`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema, runtime_core::Remote)]
pub enum TableDensity {
    /// Tight rows for dense grids.
    Compact,
    /// The default.
    #[default]
    Standard,
    /// Roomy rows — a single line of text lands in the 44–52pt touch
    /// target band, for clickable rows and phone lists.
    Comfortable,
}

// The variant string IS the stylesheet arm the value selects.
impl VariantEnum for TableDensity {
    fn as_variant_str(self) -> &'static str {
        match self {
            TableDensity::Compact => "compact",
            TableDensity::Standard => "standard",
            TableDensity::Comfortable => "comfortable",
        }
    }

    fn all_variants() -> &'static [Self] {
        &[TableDensity::Compact, TableDensity::Standard, TableDensity::Comfortable]
    }
}

// =============================================================================
// Table
// =============================================================================

/// Themed table container. Wraps the `table` SDK's `<table>` with
/// idea-ui's surface tokens (rounded corners + hairline border + theme
/// background). Pass `TableRow`s (and optionally a `TableFooter`) as
/// children.
#[runtime_core::props]
#[derive(Default, IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TableProps {
    /// Table rows. Pass `TableRow`s (a header row plus body rows), and
    /// optionally a `TableFooter`.
    pub children: Vec<Element>,
    /// Horizontal-scroll mode: columns lay out at natural width and
    /// overflow sideways inside a horizontal scroller instead of
    /// squeezing and wrapping. Required for `TableCell(pinned = …)`
    /// frozen columns (they pin against this scroller).
    // STRUCTURAL — selects the SDK's scroller wrapper at build time.
    #[prop(static)]
    pub scroll_x: bool,
    /// Row padding for every cell — `Compact`, `Standard` or
    /// `Comfortable`.
    // STATIC — selected onto every built cell's sheet when the table is
    // built (the cells already exist by then; see `Table`).
    #[prop(static)]
    pub density: TableDensity,
    /// Content drawn inside the table's frame ABOVE the rows and
    /// outside the horizontal scroller — it keeps still while a
    /// `scroll_x` table's columns scroll, and shares the table's border.
    /// Gets the cells' padding and a divider beneath it.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub header_slot: Option<Element>,
    /// Content drawn inside the frame BELOW the rows, outside the
    /// scroller — a blank-state sentence, an "Add entry" action.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub footer_slot: Option<Element>,
}

/// A themed data table — a header row plus body rows. Wraps the
/// cross-platform `table` SDK: a real HTML `<table>` on web, a CSS-grid
/// with column tracks shared across rows on native — so columns line up
/// the same way on every platform. Pass `TableRow`s as children.
#[component(children)]
pub fn Table(props: TableProps) -> Element {
    // The style lands on a surface WRAPPER around the table whenever
    // there is one (scroll-x, or a slot to draw inside the frame), and
    // that wrapper must clip its contents to the rounded frame — the
    // `scrolling` axis (named for its first use; renaming it would churn
    // every table's preminted class). A plain table keeps the axis off —
    // its style lands on the `<table>` itself, where a clip would shave
    // the outer half of the collapsed border (see the sheet).
    let wrapped = props.scroll_x || props.header_slot.is_some() || props.footer_slot.is_some();
    let style = if wrapped {
        TableStyle().into_style_application().with("scrolling", "on")
    } else {
        TableStyle().into_style_application()
    };
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    // Density is a table-wide setting, but the cells are already built
    // when this body runs (the `ui!` children block builds them first),
    // so it is selected onto each built cell. A static selection keeps a
    // static sheet static — it still premints. `Standard` is every
    // sheet's default arm, so it needs no pass at all.
    if props.density != TableDensity::Standard {
        let arm = props.density.as_variant_str();
        for child in &children {
            table::visit_rows(child, |row| {
                table::visit_row_cells(row, |cell| {
                    table::map_cell_application(cell, Rc::new(move |app| app.with("density", arm)));
                });
            });
        }
    }
    let header_slot = props.header_slot.map(|slot| {
        let style = TableSlot().into_style_application().with("edge", "top");
        ui! { view(style = style) { slot } }
    });
    let footer_slot = props.footer_slot.map(|slot| {
        let style = TableSlot().into_style_application().with("edge", "bottom");
        ui! { view(style = style) { slot } }
    });
    // SDK's `table()` returns a `Bound<TableHandle>`; chain
    // `.with_style(...)` to land the themed style on the `<table>`
    // itself (or the surface wrapper), then convert to Element.
    sdk_table(SdkTableProps {
        children,
        scroll_x: props.scroll_x,
        header_slot,
        footer_slot,
    })
    .with_style(style)
    .into_element()
}

// =============================================================================
// TableRow
// =============================================================================

/// Themed table row. A thin passthrough by default; set `on_row_click`
/// to make the whole row interactive (pointer cursor + a themed hover
/// highlight across every cell + a tap callback), `tone` to tint it.
///
/// Note: on native the SDK lowers a row to a layout-transparent fragment
/// (its cells become direct children of the table's grid — Taffy has no
/// subgrid), so a row has no box of its own there. Row-level visuals and
/// interaction must therefore be applied per-cell rather than to a single
/// row element — which is exactly what `on_row_click` and `tone` do (see
/// [`make_row_cell_interactive`]).
#[runtime_core::props]
#[derive(Default, IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TableRowProps {
    /// Cells in this row. Pass `TableCell`s.
    pub children: Vec<Element>,
    /// Optional row-click handler. When `Some`, the whole row becomes
    /// interactive: every cell shows a pointer cursor, hovering any cell
    /// tints the entire row (`color-surface-alt`), and a tap anywhere in
    /// the row invokes this callback.
    ///
    /// On a touch backend the tint comes from the PRESS instead — a
    /// finger down on the row tints it and lifting clears it — so the
    /// row gives the same feedback on a phone that a pointer gets from
    /// hover. A touch that slides away (the start of a scroll) clears
    /// the tint without invoking the callback.
    ///
    /// The click surface spans the full row, but it sits on each cell's
    /// own node rather than a layer above the content, so an interactive
    /// child (a `Button`, `Link`) inside a cell still receives its own
    /// tap first: its recognizer consumes the event, which stops it from
    /// reaching the row handler (the standard "buttons in a clickable row"
    /// pattern). Taps on plain content or empty cell space fall through to
    /// the row callback.
    pub on_row_click: Option<Rc<dyn Fn()>>,
    /// Whole-row tint — `Highlight`, `Warning` or `Danger`. Reactive: a
    /// row can change tone in place. Composes with the clickable-row
    /// hover (a toned row hovers to a deeper shade of its own tint) and
    /// with frozen columns. Applies to body cells.
    pub tone: RowTone,
    /// When `Some`, filled with the row's handle on mount — the `<tr>` on
    /// web, a row-spanning backdrop view on native. Anchor a hover card
    /// or popover to a row with it, or read the row's frame.
    pub bind_to: Option<Ref<ViewHandle>>,
}

/// A row within a [`Table`] — holds `TableCell`s. Use the first row as
/// the header (its cells set `header = true`).
#[component(children)]
pub fn TableRow(props: TableRowProps) -> Element {
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }

    // Tone first, so a clickable row's hover axes compose OVER it. A
    // static tone is a static selection (the sheet stays preminted); a
    // live one wraps each cell's style so the row re-tints in place.
    match props.tone {
        Reactive::Static(RowTone::None) => {}
        Reactive::Static(tone) => {
            let arm = tone.as_variant_str();
            for cell in &children {
                table::map_cell_application(cell, Rc::new(move |app| app.with("tone", arm)));
            }
        }
        Reactive::Dynamic(f) => {
            for cell in &children {
                let f = f.clone();
                table::map_cell_style(cell, Rc::new(move |app| app.with("tone", f().as_variant_str())));
            }
        }
    }

    // A clickable row shares ONE hover flag across all its cells so
    // hovering any cell highlights the whole row. The cells arrive here
    // already built (the `ui!` children block builds them before the row
    // runs), so we post-process each: layer the reactive hover background
    // + pointer cursor onto its themed style and attach the tap/hover
    // handlers. The signal is created in this row's scope, so it lives as
    // long as the cells that subscribe to it.
    let children = if let Some(cb) = props.on_row_click {
        // TWO flags, not one. A pointer backend drives both — hover on
        // enter/leave, press on down/up — and folding them into a single
        // bool means whichever fires last wins: releasing a click would
        // report "not pressed" and clear a tint the pointer is still
        // entitled to, and any source that fails to report its own
        // release leaves the row stuck lit with nothing able to correct
        // it. Kept apart, each reports only about itself and the tint is
        // their OR.
        let hovered = signal(false);
        let pressed = signal(false);
        children
            .into_iter()
            .map(|cell| make_row_cell_interactive(cell, hovered, pressed, cb.clone()))
            .collect()
    } else {
        children
    };

    let row = sdk_row(SdkTableRowProps { children }).into_element();
    if let Some(r) = props.bind_to {
        table::bind_row(&row, move |h| r.fill(h));
    }
    row
}

/// Attach whole-row click + hover to a single cell.
///
/// The reactive row-hover style is layered over the cell's existing themed
/// sheet, and the tap recognizer + shared hover flag ride on the cell's OWN
/// backend node — a grid item on native, a `<td>`/`<th>` on web. The scene
/// payload is type-erased, so the cell introspection lives in the table SDK
/// (`map_cell_style` / `set_cell_interaction`, which reach both
/// shapes). Payloads are pre-mount, so in-place mutation is
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
fn make_row_cell_interactive(
    cell: Element,
    hovered: Signal<bool>,
    pressed: Signal<bool>,
    cb: Rc<dyn Fn()>,
) -> Element {
    use runtime_core::{tap_with_press, TapRecognizer};

    // Reactive whole-row hover style: select the cell sheet's
    // `interactive`/`row_hovered` AXES instead of layering runtime
    // overrides. Every arm has build-time CSS, so on a premint build the
    // flip is a class swap through the reactive diversion — no engine —
    // while native resolves the same arms through the engine as always.
    // (The former `with_overrides` spelling disqualified every clickable
    // cell from preminting.) A cell with no application to compose over
    // keeps its style untouched but stays clickable.
    //
    // `map_cell_style` COMPOSES the axes over whatever style the cell
    // already carries. Reading a base application and writing a
    // replacement — what this did — could only see a STATIC
    // `StyleProp::Sheet`, so a cell styled reactively (an author pinning
    // a column to a width that moves under a resize drag) was skipped
    // entirely: no pointer cursor, no hover tint, nothing logged. In a
    // row of otherwise-static cells that reads as one column refusing
    // the row's highlight, which is a long way from its cause.
    table::map_cell_style(
        &cell,
        Rc::new(move |app: runtime_core::StyleApplication| {
            app.with("interactive", "on")
                // The AXIS is still called `row_hovered`; what drives it
                // is "hovered OR pressed", because a touch device has no
                // hover and press is its equivalent. The arm name is part
                // of the sheet's premint identity, so renaming it would
                // churn every table's CSS class for a comment's worth of
                // accuracy.
                .with(
                    "row_hovered",
                    if hovered.get() || pressed.get() { "on" } else { "off" },
                )
        }),
    );
    // Press drives the tint as well as hover, because a touch device has
    // no hover and would otherwise get no row feedback at all: the hover
    // reporter never fires there (nothing to hover with), so a clickable
    // row on a phone looked identical to a static one right up until it
    // navigated. Press is the touch equivalent — a finger down on a row
    // means what a pointer over it does.
    //
    // Each reporter writes ONLY its own flag. A pointer backend drives
    // both at once (enter, down, up, leave), so sharing one flag would
    // make the last writer win: `mouseup` reporting "not pressed" would
    // clear a tint the still-hovering pointer is entitled to.
    let recognizer = tap_with_press(
        TapRecognizer::new(),
        move || (cb)(),
        move |down| pressed.set(down),
    );
    table::set_cell_interaction(
        &cell,
        recognizer,
        Rc::new(move |entering| hovered.set(entering)),
    );
    cell
}

// =============================================================================
// TableFooter
// =============================================================================

/// Props for [`TableFooter`].
#[runtime_core::props]
#[derive(Default, IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TableFooterProps {
    /// The footer rows — `TableRow`s, typically one totals row.
    pub children: Vec<Element>,
}

/// The table's footer — a totals row. `<tfoot>` on web, so assistive
/// tech announces it as the footer rather than as another row of data;
/// its cells take the header band's tint; and it renders after the body
/// rows wherever it is written among the table's children.
///
/// ```ignore
/// TableFooter {
///     TableRow {
///         TableCell(span = ColSpan::Columns(2), text = Some("Crew totals · 4".into()))
///         TableCell(align = CellAlign::Right, text = Some("38.5".into()))
///     }
/// }
/// ```
#[component(children)]
pub fn TableFooter(props: TableFooterProps) -> Element {
    let mut rows: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut rows);
    }
    // Footer rows are ordinary rows; what makes them a footer is the
    // section, selected onto each built cell (static — still preminted).
    for row in &rows {
        table::visit_row_cells(row, |cell| {
            table::map_cell_application(cell, Rc::new(|app| app.with("section", "foot")));
        });
    }
    sdk_foot(SdkTableFootProps { children: rows })
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
// Reactive-by-default: `text`, `width` and `min_width` are reactive and
// `children` is a LIST (auto-skipped). The `#[prop(static)]` fields are
// STRUCTURAL — each picks an SDK element, a grid placement, or a style
// arm on several nodes at build time (see each field).
#[runtime_core::props]
#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TableCellProps {
    /// When `true`, render as `<th>` (and use the `color-table-header`
    /// band + uppercase muted text style). When `false`, render as
    /// `<td>`.
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
    /// Freeze this cell's column against a scroller edge — meaningful
    /// inside a `Table(scroll_x = true)`. Pin the same cell in EVERY
    /// row (header included) or the column freezes only partially.
    // STRUCTURAL — selects the `pinned` stylesheet arm at build time.
    #[prop(static)]
    pub pinned: Option<ColumnPin>,
    /// Inset from the pinned edge, in px — how a SECOND (third, …)
    /// frozen column clears the ones before it: set it to the summed
    /// `width`s of the pinned columns between this one and the edge.
    /// `None` (the default) pins at the edge. Ignored unless `pinned` is
    /// set. (An `Option` rather than a bare `f32` so a `ui!` literal —
    /// `pin_offset = Some(48.0)` — infers `f32` instead of tripping the
    /// float-literal fallback a bare `48.0.into()` hits.)
    // STRUCTURAL — part of the pinned arm's geometry, fixed at build.
    #[prop(static)]
    pub pin_offset: Option<f32>,
    /// Hold this cell's column at exactly this width (px). Spare table
    /// width goes to the columns without one. Give every cell of the
    /// column the same value, header included.
    pub width: Option<f32>,
    /// Never let this cell's column be narrower than this (px).
    pub min_width: Option<f32>,
    /// How the content sits in the column — `Left` (default), `Center`
    /// or `Right`. Give every cell of the column the same value.
    // STRUCTURAL — selects the `align` arm on three nodes (the cell, its
    // text node, the rich-children wrapper) at build time.
    #[prop(static)]
    pub align: CellAlign,
    /// Keep the content to one line ending in "…". Pair with `width`.
    // STRUCTURAL — selects the `truncate` arm on the cell and its text.
    #[prop(static)]
    pub truncate: bool,
    /// How many columns this cell covers: `ColSpan::Columns(n)`, or
    /// `ColSpan::Rest` for every column to the end of the row.
    // STRUCTURAL — a `colspan` attribute / grid placement fixed at build.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub span: ColSpan,
}

impl Default for TableCellProps {
    fn default() -> Self {
        Self {
            header: false,
            text: Reactive::Static(None),
            children: Vec::new(),
            pinned: None,
            pin_offset: None,
            width: Reactive::Static(None),
            min_width: Reactive::Static(None),
            align: CellAlign::Left,
            truncate: false,
            span: ColSpan::default(),
        }
    }
}

/// A cell within a [`TableRow`]. Set `header = true` for a header
/// (`<th>`) cell; otherwise it renders as a data (`<td>`) cell.
#[component(children)]
pub fn TableCell(props: TableCellProps) -> Element {
    let header = props.header;
    let align = props.align.as_variant_str();
    let truncate = if props.truncate { "on" } else { "off" };

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
        let inner_style = TableCellInner().into_style_application().with("align", align);
        vec![ui! { view(style = inner_style) { inner } }]
    } else {
        cell_text_children(header, props.text, align, truncate)
    };

    let bound = sdk_cell(SdkTableCellProps { header, span: props.span, children: cell_children });
    // A cell's style is handed over as an EXPLICIT `StyleProp::Sheet`, not
    // as a bare application. A clickable row composes the `interactive` /
    // `row_hovered` axes onto each cell's own style
    // (`table::map_cell_style`), and a `--premint` build's opaque
    // `Preminted` class carries no application to compose with — the app
    // must stay introspectable in the payload.
    //
    // `.into_style_application()` alone used to say that and stopped:
    // `IntoStyleProp for StyleApplication` gained a preminted fast path, so
    // the application preminted anyway, the composition found nothing to
    // compose with, and `make_row_cell_interactive` silently skipped the
    // whole overlay — clickable rows lost their pointer cursor and their
    // hover highlight in every `--premint` build, with nothing logged.
    // Naming the variant is what actually pins the intent, and
    // `regression_premint_keeps_table_cells_on_the_live_engine` holds it
    // there.
    //
    // This no longer costs the premint anything: the row overlay is axis
    // SELECTION (build-time CSS per arm), not runtime overrides, and the
    // `--premint-only` attach premints an explicit `Sheet` whose
    // application qualifies — the spelling only pins introspectability.
    //
    // Branching here (rather than boxing) keeps each style concrete so
    // `IntoStyleSource` resolves on the call, which the trait requires.
    // A pinned cell selects the sheet's `pinned` AXIS on top of the
    // same explicit-`Sheet` hand-off (the axis arms carry build-time
    // CSS, so this premints as a class like every other axis — and the
    // application stays introspectable for the clickable-row overlay,
    // which re-derives it and re-selects axes on top).
    let pin_arm = props.pinned.map(|p| match p {
        ColumnPin::Left => "left",
        ColumnPin::Right => "right",
    });
    let app = if header {
        let app = TableHeadCell().into_style_application();
        select_cell_axes(app, pin_arm, align, truncate)
    } else {
        let app = TableBodyCell().into_style_application();
        select_cell_axes(app, pin_arm, align, truncate)
    };

    // Geometry the sheet cannot enumerate — a column width, a pin inset —
    // rides `with_overrides` on top of the selected arms. Overrides take
    // a cell off the premint path, so a cell that sets none keeps the
    // pure axis selection (and the `Sheet` hand-off above).
    let pinned = props.pinned;
    let pin_offset = props.pin_offset;
    let geometry = move |width: Option<f32>, min_width: Option<f32>| -> Option<StyleRules> {
        let px = |v: f32| Tokenized::Literal(Length::Px(v));
        let inset = pin_offset.filter(|o| pinned.is_some() && *o != 0.0).map(px);
        if width.is_none() && min_width.is_none() && inset.is_none() {
            return None;
        }
        Some(StyleRules {
            // An EXACT column: `width` alone is only a floor in a
            // browser's auto table (a long unwrapped value widens the
            // column past it); `max-width` at the same value is what holds
            // it — measured, see `runtime-layout`'s `table_column_widths`.
            width: width.map(px),
            max_width: width.map(px),
            min_width: min_width.map(px),
            left: inset.clone().filter(|_| pinned == Some(ColumnPin::Left)),
            right: inset.filter(|_| pinned == Some(ColumnPin::Right)),
            ..Default::default()
        })
    };
    match (props.width, props.min_width) {
        (Reactive::Static(width), Reactive::Static(min_width)) => {
            let app = match geometry(width, min_width) {
                Some(rules) => app.with_overrides(rules),
                None => app,
            };
            bound.with_style(StyleProp::Sheet(Box::new(app))).into_element()
        }
        (width, min_width) => {
            // A live width (a column following a resize drag): one style
            // closure reading both, composing over the same selected arms.
            let width = Rc::new(width);
            let min_width = Rc::new(min_width);
            let style = move || -> StyleApplication {
                match geometry(width.get(), min_width.get()) {
                    Some(rules) => app.clone().with_overrides(rules),
                    None => app.clone(),
                }
            };
            bound.with_style(StyleProp::SheetDynamic(Box::new(style))).into_element()
        }
    }
}

/// Select the build-time axes every cell carries, on either sheet.
fn select_cell_axes(
    app: StyleApplication,
    pin_arm: Option<&'static str>,
    align: &'static str,
    truncate: &'static str,
) -> StyleApplication {
    let app = app.with("align", align).with("truncate", truncate);
    match pin_arm {
        Some(arm) => app.with("pinned", arm),
        None => app,
    }
}

/// Render a cell's `text` prop with the role-appropriate themed
/// stylesheet. Split out so the `header` branch can pick its
/// concrete style without needing `Box<dyn IntoStyleSource>`.
fn cell_text_children(
    header: bool,
    content: Reactive<Option<String>>,
    align: &'static str,
    truncate: &'static str,
) -> Vec<Element> {
    let style = if header {
        TableHeadText().into_style_application()
    } else {
        TableBodyText().into_style_application()
    }
    .with("align", align)
    .with("truncate", truncate);
    match content {
        Reactive::Static(None) => Vec::new(),
        Reactive::Static(Some(s)) => vec![text_node(s).with_style(style).into_element()],
        Reactive::Dynamic(f) => {
            vec![text_node(move || f().unwrap_or_default()).with_style(style).into_element()]
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
    use runtime_core::Cursor;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;

    fn body_cell(text: &str) -> Element {
        TableCell(TableCellProps {
            text: Reactive::Static(Some(text.into())),
            ..Default::default()
        })
    }

    thread_local! {
        /// Peeled row scopes, retained for the test's lifetime — a
        /// clickable row's marker arrives `Owned`-wrapped around the
        /// shared hover signal, and dropping the scope here would
        /// stale-handle the signal the cells' reactive styles read
        /// (same rationale as `test_support::classify`'s keepalive).
        static ROW_SCOPES: std::cell::RefCell<Vec<runtime_core::Owned>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn row_cells(mut row: Element) -> Vec<Element> {
        // Rows lower to the SDK's `TableRowPrim` marker item (the row
        // proxy work); its children are the cells.
        loop {
            match row {
                Element::Owned { element, owned, .. } => {
                    ROW_SCOPES.with(|k| k.borrow_mut().push(owned));
                    row = *element;
                }
                Element::Item { children, .. } => return children,
                _ => panic!("TableRow must lower to the SDK row marker item"),
            }
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
    /// Regression: a `--premint` build must not premint a table cell's
    /// style.
    ///
    /// A clickable row layers the pointer cursor + hover highlight over
    /// each cell by re-deriving the cell's `StyleApplication` from the
    /// built element. A preminted cell is an opaque class string with no
    /// application behind it, so the derivation returns `None` and
    /// `make_row_cell_interactive` silently skips the overlay — the row
    /// stays clickable but loses its cursor and its highlight, with
    /// nothing logged.
    ///
    /// That shipped: `.into_style_application()` was written to keep cells
    /// off the premint path, then `IntoStyleProp for StyleApplication`
    /// gained a preminted fast path and preminted the application anyway.
    /// Caught by a computed-style A/B of the catalog against a live build
    /// (54 differing `cursor` properties across the table pages), not by a
    /// test — which is why there are now two.
    ///
    /// This half asserts the SEAM: the cell's style must be composable.
    /// Under the default (non-premint) cfg it passes either way, so
    /// `premint_must_not_reach_table_cell_styles` guards the actual
    /// spelling.
    #[test]
    fn regression_premint_keeps_table_cells_on_the_live_engine() {
        with_test_world(|| {
            let cell = body_cell("x");
            assert!(
                table::map_cell_style(&cell, Rc::new(|app| app)),
                "a clickable row composes the pointer cursor + hover \
                 highlight onto the cell's own style; a cell with no \
                 application behind it is skipped silently"
            );
        });
    }

    /// The source-level half of the guard above.
    ///
    /// Whether a style preminted is decided by a `--cfg` this test binary
    /// is not built with, so no assertion on a value can observe the
    /// regression here (same limitation `premint_only_surface.rs`
    /// documents). What IS observable is the spelling: cells must hand
    /// over an explicit `StyleProp::Sheet`, which no `IntoStyleProp` fast
    /// path can reinterpret. A bare application can, and did.
    #[test]
    fn premint_must_not_reach_table_cell_styles() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/components/table.rs"),
        )
        .expect("read table.rs");
        // The application is built explicitly per role (so the `pinned`
        // axis can select on it)…
        for sheet in ["TableHeadCell", "TableBodyCell"] {
            let needle = format!("let app = {sheet}().into_style_application();");
            assert!(
                src.contains(&needle),
                "the {sheet} cell application must be derived explicitly \
                 before hand-off"
            );
        }
        // …and handed over as an explicit `StyleProp::Sheet` — a bare
        // application premints, and the clickable-row overlay is then
        // dropped without a word.
        assert!(
            src.contains("bound.with_style(StyleProp::Sheet(Box::new(app))).into_element()"),
            "cell styles must be handed over as an explicit `StyleProp::Sheet`"
        );
    }

    /// The clickable-row overlay must PREMINT: it selects the cell
    /// sheet's `interactive`/`row_hovered` AXES, whose every arm has
    /// build-time CSS, instead of layering runtime overrides — the
    /// override spelling disqualified every clickable cell from
    /// preminting (one of the last two `--premint-only` blockers on the
    /// docs corpus). Fails against the override form: an overridden
    /// application's `preminted_class_list()` is `None` by construction.
    /// The live engine must resolve the same arms to the same rules the
    /// overrides produced (pointer cursor; themed hover background).
    #[test]
    fn regression_clickable_row_overlay_premints_via_axes() {
        with_test_world(|| {
            let row = TableRow(TableRowProps {
                children: vec![body_cell("x")],
                on_row_click: Some(Rc::new(|| {})),
                ..Default::default()
            });
            let mut cells = row_cells(row);
            let style = match classify(cells.remove(0)) {
                P::View { style, .. } => style.expect("clickable cell keeps a style"),
                _ => panic!("native cell must classify as a View"),
            };
            // Evaluate the reactive style at its resting state (not hovered).
            let app = style.application();
            assert!(
                app.preminted_class_list().is_some(),
                "the axis-selected cell application must premint (overrides would return None)"
            );
            let resting = runtime_core::resolve_style(&app);
            assert_eq!(
                resting.cursor,
                Some(Cursor::Pointer),
                "interactive arm carries the pointer cursor"
            );
            // Resting STATES a background — transparent — rather than
            // saying nothing. That inversion is the fix for a stuck
            // highlight: an arm that sets no background produces a
            // re-resolve with no background in it, and a re-resolve that
            // mentions no background cannot undo one an earlier resolve
            // already applied to the view. The tint went on at press and
            // stayed on after release, on every backend that applies a
            // style by merging rather than replacing.
            //
            // Transparent, not a surface token: a body cell has no
            // background of its own, so this is the resting appearance
            // it always had — now spelled instead of implied.
            assert_eq!(
                resting.background.as_ref().map(|c| c.resolve().0),
                Some("transparent".to_string()),
                "resting (row_hovered=off) states a transparent background, so a \
                 later resolve has something to overwrite the tint with"
            );

            // The hovered arm resolves to the themed row highlight — same
            // value the old override layer produced.
            let hovered_app = TableBodyCell()
                .into_style_application()
                .with("interactive", "on")
                .with("row_hovered", "on");
            assert!(hovered_app.preminted_class_list().is_some());
            let hovered = runtime_core::resolve_style(&hovered_app);
            // Asserted against the palette, not a literal: this used to
            // pin `#eef0f7`, the fallback the cell's own
            // `Tokenized::token("color-surface-alt", …)` restated — which
            // had drifted from the palette's `#f1f5f9`. Deriving the
            // expectation from `light_theme()` means the two can't
            // disagree again.
            let surface_alt = crate::light_theme().colors.surface_alt.value().0.to_ascii_lowercase();
            assert_eq!(
                hovered.background.as_ref().map(|b| b.resolve().0.to_ascii_lowercase()),
                Some(surface_alt),
                "row_hovered arm resolves the themed surface-alt highlight"
            );
        });
    }

    /// The header band reads its OWN token, not `color-surface-alt`.
    /// Head cells used to paint `surface_alt` directly, which made
    /// "retint table headers" impossible without dragging every other
    /// `surface_alt` consumer (cards, field wells, row hover) along.
    /// Asserting the token NAME is the load-bearing half: a literal —
    /// or the old `color-surface-alt` reference — is exactly what an
    /// app-level `color-table-header` override could never reach.
    #[test]
    fn head_cell_background_reads_the_table_header_token() {
        with_test_world(|| {
            let app = TableHeadCell().into_style_application();
            let rules = runtime_core::resolve_style(&app);
            let bg = rules.background.as_ref().expect("head cells paint a background");
            assert_eq!(
                bg.name(),
                Some("color-table-header"),
                "the head-cell band must resolve through its own token"
            );

            // …and its default value is the `surface_alt` tint the band
            // had before the token existed, so adding the token is not
            // a visual change. Derived from the palette, never restated
            // as a literal (see `regression_clickable_row_premints…`).
            let colors = &crate::light_theme().colors;
            assert_eq!(
                colors.table_header.value().0.to_ascii_lowercase(),
                colors.surface_alt.value().0.to_ascii_lowercase(),
                "color-table-header ships as the surface-alt tint"
            );
            assert_eq!(
                bg.resolve().0.to_ascii_lowercase(),
                colors.table_header.value().0.to_ascii_lowercase(),
            );
        });
    }

    /// A cell styled REACTIVELY takes the row's hover the same as any
    /// other. This is the app-side shape the overlay used to miss: an
    /// author who needs a width on a cell (idea-ui's `TableCell` has
    /// none) drops to the SDK cell and hands it a style CLOSURE, because
    /// the width has to move while a resize handle is being dragged. The
    /// old read-then-replace wiring could only see a static
    /// `StyleProp::Sheet`, so every such column sat un-tinted in a row
    /// whose other cells lit up — sloppy, and with nothing logged to say
    /// why. Composing means the cell keeps its width and takes the axes.
    #[test]
    fn regression_reactive_cell_style_still_takes_the_row_hover() {
        with_test_world(|| {
            let width_pinned = table::table_cell(table::TableCellProps {
                header: false,
                children: Vec::new(),
                ..Default::default()
            })
            .with_style(|| {
                TableBodyCell().into_style_application().with_overrides(StyleRules {
                    width: Some(runtime_core::Tokenized::Literal(
                        runtime_core::Length::Px(240.0),
                    )),
                    ..Default::default()
                })
            })
            .into_element();

            let row = TableRow(TableRowProps {
                children: vec![width_pinned],
                on_row_click: Some(Rc::new(|| {})),
                ..Default::default()
            });
            let mut cells = row_cells(row);
            let style = match classify(cells.remove(0)) {
                P::View { style, .. } => style.expect("the cell keeps a style"),
                _ => panic!("native cell must classify as a View"),
            };
            let rules = runtime_core::resolve_style(&style.application());
            assert_eq!(
                rules.cursor,
                Some(Cursor::Pointer),
                "the interactive arm reaches a reactively-styled cell"
            );
            assert_eq!(
                rules.width,
                Some(runtime_core::Tokenized::Literal(runtime_core::Length::Px(240.0))),
                "and the cell's own width override survives the composition"
            );
        });
    }

    #[test]
    fn clickable_row_makes_every_cell_interactive() {
        with_test_world(|| {
            let row = TableRow(TableRowProps {
                children: vec![body_cell("a"), body_cell("b")],
                on_row_click: Some(Rc::new(|| {})),
                ..Default::default()
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

    /// A pinned cell selects the sheet's `pinned` AXIS: `position:
    /// Sticky` plus the matching zero inset, an opaque background
    /// (content slides beneath a frozen column), and a preminting
    /// application (the axis arms carry build-time CSS — a frozen
    /// column must not knock the cell off the premint path). This is
    /// the whole frozen-column feature at the component layer; the pin
    /// itself is the sticky substrate's job.
    #[test]
    fn pinned_cell_selects_sticky_axis_and_premints() {
        with_test_world(|| {
            let left = TableCell(TableCellProps {
                text: Reactive::Static(Some("x".into())),
                pinned: Some(ColumnPin::Left),
                ..Default::default()
            });
            let app = match classify(left) {
                P::View { style, .. } => style
                    .expect("pinned cell keeps an introspectable application")
                    .application(),
                _ => panic!("native cell must classify as a View"),
            };
            assert!(
                app.preminted_class_list().is_some(),
                "the pinned axis must premint (build-time CSS arms)"
            );
            let rules = runtime_core::resolve_style(&app);
            assert_eq!(
                rules.position,
                Some(runtime_core::Position::Sticky),
                "pinned arm carries position: Sticky"
            );
            assert!(
                matches!(
                    rules.left.as_ref().map(|t| t.resolve()),
                    Some(runtime_core::Length::Px(v)) if v == 0.0
                ),
                "left-pinned arm pins at left: 0"
            );
            assert!(rules.right.is_none(), "left pin must not also set right");
            // `is_some()` is NOT enough, and that gap shipped: when
            // `row_hovered`'s resting arm began stating `transparent` it
            // won the merge (axes resolve in alphabetical axis order and
            // `"pinned" < "row_hovered"`), the resolve stayed `Some(...)`,
            // this assertion stayed green — and frozen columns went
            // see-through on a live device. Assert the VALUE.
            let bg = rules
                .background
                .as_ref()
                .map(|t| format!("{t:?}"))
                .expect("pinned body cell must state a background");
            assert!(
                !bg.contains("transparent"),
                "pinned body cell must be OPAQUE — content slides beneath a \
                 frozen column — but resolved to `{bg}`"
            );

            let right = TableCell(TableCellProps {
                text: Reactive::Static(Some("x".into())),
                pinned: Some(ColumnPin::Right),
                ..Default::default()
            });
            let right_app = match classify(right) {
                P::View { style, .. } => style.expect("application").application(),
                _ => panic!("native cell must classify as a View"),
            };
            let rules = runtime_core::resolve_style(&right_app);
            assert_eq!(rules.position, Some(runtime_core::Position::Sticky));
            assert!(
                matches!(
                    rules.right.as_ref().map(|t| t.resolve()),
                    Some(runtime_core::Length::Px(v)) if v == 0.0
                ),
                "right-pinned arm pins at right: 0"
            );
            assert!(rules.left.is_none(), "right pin must not also set left");
        });
    }

    /// `Table(scroll_x = true)` lowers to the themed SURFACE wrapping
    /// the SDK's horizontal scroller — the surface (border/radius/
    /// background + overflow clip) must sit OUTSIDE the scroller so it
    /// stays put while the columns scroll ("the card border scrolls
    /// away and clips" regression), and without the scroller there is
    /// nothing for a pinned column to pin against.
    #[test]
    fn scroll_x_table_lowers_to_surface_around_horizontal_scroller() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![TableRow(TableRowProps {
                    children: vec![body_cell("a")],
                    ..Default::default()
                })],
                scroll_x: true,
                ..Default::default()
            });
            let el = peel_owned_keepalive(t);
            // Root: the surface view carrying the themed Table sheet,
            // which must clip (rounded corners over scrolling columns).
            let mut surface_children = match el {
                Element::Item { data, children, .. } => {
                    let vp = data
                        .downcast_ref::<runtime_vocabulary::prims::PrimCell<
                            runtime_vocabulary::prims::ViewPrim,
                        >>()
                        .expect("scroll_x root is the styled surface view")
                        .take();
                    let style = vp.style.expect("surface carries the themed Table sheet");
                    let app = match style {
                        runtime_vocabulary::StyleProp::Sheet(app) => *app,
                        _ => panic!("Table sheet is a static application"),
                    };
                    let rules = runtime_core::resolve_style(&app);
                    assert_eq!(
                        rules.overflow,
                        Some(runtime_core::Overflow::Hidden),
                        "surface must clip the scrolling columns to its rounded frame"
                    );
                    assert!(rules.border_top_width.is_some(), "surface carries the border");
                    children
                }
                _ => panic!("scroll_x table must lower to the surface view"),
            };
            match surface_children.pop().expect("surface wraps the scroller") {
                Element::Item { data, .. } => {
                    assert!(
                        data.downcast_ref::<runtime_vocabulary::prims::PrimCell<
                            runtime_vocabulary::prims::ScrollViewPrim,
                        >>()
                        .is_some_and(|c| c.take().horizontal),
                        "surface must wrap a HORIZONTAL scroll_view"
                    );
                }
                _ => panic!("surface must wrap the scroll_view item"),
            }
        });
    }

    /// Peel `Owned` wrappers, retaining the scopes (reactive styles
    /// inside read signals those scopes own).
    fn peel_owned_keepalive(mut el: Element) -> Element {
        loop {
            match el {
                Element::Owned { element, owned, .. } => {
                    ROW_SCOPES.with(|k| k.borrow_mut().push(owned));
                    el = *element;
                }
                other => return other,
            }
        }
    }

    /// Regression: a PLAIN table's surface style must NOT carry the
    /// overflow clip. The style lands on the `<table>` element itself,
    /// whose `border-collapse: collapse` outer border straddles the
    /// box edge — `overflow: hidden` there clips the border's outer
    /// half and the frame renders visibly thinned. The clip belongs
    /// only to the scroll-x surface wrapper (the `scrolling` axis),
    /// which is a plain view the quirk can't touch.
    #[test]
    fn regression_plain_table_surface_does_not_clip_its_collapsed_border() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![TableRow(TableRowProps {
                    children: vec![body_cell("a")],
                    ..Default::default()
                })],
                ..Default::default()
            });
            let el = peel_owned_keepalive(t);
            let style = match el {
                Element::Item { data, .. } => data
                    .downcast_ref::<runtime_vocabulary::prims::PrimCell<
                        runtime_vocabulary::prims::ViewPrim,
                    >>()
                    .expect("plain table outer is a view")
                    .take()
                    .style
                    .expect("outer carries the themed Table sheet"),
                _ => panic!("plain table lowers to the outer view"),
            };
            let app = match style {
                runtime_vocabulary::StyleProp::Sheet(app) => *app,
                _ => panic!("Table sheet is a static application"),
            };
            let rules = runtime_core::resolve_style(&app);
            assert_eq!(
                rules.overflow, None,
                "plain table must not clip — overflow: hidden shaves the \
                 collapsed border's outer half"
            );
            assert!(rules.border_top_width.is_some(), "surface keeps its border");
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
                ..Default::default()
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

    // =========================================================================
    // CrewForge 10-09 table gaps — one regression per gap.
    // =========================================================================

    fn cell_app(cell: Element) -> runtime_core::StyleApplication {
        match classify(cell) {
            P::View { style, .. } => style.expect("cell keeps a style").application(),
            _ => panic!("native cell must classify as a View"),
        }
    }

    fn rules_of(cell: Element) -> Rc<StyleRules> {
        runtime_core::resolve_style(&cell_app(cell))
    }

    fn bg_token(rules: &StyleRules) -> Option<String> {
        rules.background.as_ref().and_then(|b| b.name().map(str::to_string))
    }

    /// The text node a `text = Some(..)` cell renders, classified.
    fn cell_text_rules(cell: Element) -> Rc<StyleRules> {
        let children = match classify(cell) {
            P::View { children, .. } => children,
            _ => panic!("native cell must classify as a View"),
        };
        match classify(children.into_iter().next().expect("cell has its text")) {
            P::Text { style, .. } => {
                runtime_core::resolve_style(&style.expect("text is styled").application())
            }
            _ => panic!("cell content must be its text node"),
        }
    }

    fn px_of(t: &Option<Tokenized<Length>>) -> Option<f32> {
        match t.as_ref().map(|t| t.resolve()) {
            Some(Length::Px(v)) => Some(v),
            _ => None,
        }
    }

    /// Gap #1: `TableCell` had no width or alignment, so CrewForge rebuilt
    /// every sized column a layer below idea-ui and copied the themed
    /// sheets by hand. A sized cell must carry the EXACT-column pair
    /// (`width` + `max-width`: `width` alone is a floor in a browser's auto
    /// table), a floor, and the alignment on the cell AND its text.
    #[test]
    fn regression_cell_width_floor_and_alignment() {
        with_test_world(|| {
            let make = || {
                TableCell(TableCellProps {
                    text: Reactive::Static(Some("12.5".into())),
                    width: Reactive::Static(Some(120.0)),
                    min_width: Reactive::Static(Some(80.0)),
                    align: CellAlign::Right,
                    ..Default::default()
                })
            };
            let rules = rules_of(make());
            assert_eq!(px_of(&rules.width), Some(120.0));
            assert_eq!(px_of(&rules.max_width), Some(120.0), "exact column needs max-width too");
            assert_eq!(px_of(&rules.min_width), Some(80.0));
            assert_eq!(rules.text_align, Some(runtime_core::TextAlign::Right));
            assert_eq!(
                cell_text_rules(make()).text_align,
                Some(runtime_core::TextAlign::Right),
                "the text node (what native reads) aligns too"
            );
            // A cell with no geometry keeps the pure axis selection — it
            // still premints.
            let plain = cell_app(body_cell("x"));
            assert!(plain.preminted_class_list().is_some());
        });
    }

    /// Gap #1, resize half: a LIVE width re-styles the cell in place (the
    /// column follows a drag) rather than being snapshotted at build.
    #[test]
    fn reactive_cell_width_follows_its_signal() {
        with_test_world(|| {
            let w = signal(Some(100.0_f32));
            let cell = TableCell(TableCellProps {
                width: Reactive::Dynamic(Rc::new(move || w.get())),
                ..Default::default()
            });
            let style = match classify(cell) {
                P::View { style, .. } => style.expect("styled"),
                _ => panic!("view"),
            };
            assert!(style.is_reactive(), "a live width is a reactive style");
            let at = |s: &crate::test_support::TStyle| {
                px_of(&runtime_core::resolve_style(&s.application()).width)
            };
            assert_eq!(at(&style), Some(100.0));
            w.set(Some(240.0));
            idea_theme::testing::commit();
            assert_eq!(at(&style), Some(240.0));
        });
    }

    /// Truncation (gap #1's "ends in …"): `max_lines: 1` on the cell —
    /// what truncates on web, where the text is an inline span — and on
    /// the text node, what truncates on native.
    #[test]
    fn truncate_limits_cell_and_text_to_one_line() {
        with_test_world(|| {
            let make = || {
                TableCell(TableCellProps {
                    text: Reactive::Static(Some("A long crew member name".into())),
                    width: Reactive::Static(Some(120.0)),
                    truncate: true,
                    ..Default::default()
                })
            };
            assert_eq!(rules_of(make()).max_lines, Some(1));
            assert_eq!(cell_text_rules(make()).max_lines, Some(1));
            assert_eq!(rules_of(body_cell("x")).max_lines, None, "off by default");
        });
    }

    /// The native grid a `Table` lowers to, and its cells.
    fn grid_cells(t: Element) -> Vec<Element> {
        let outer = match peel_owned_keepalive(t) {
            Element::Item { children, .. } => children,
            _ => panic!("table lowers to its outer view"),
        };
        match peel_owned_keepalive(outer.into_iter().next().expect("outer wraps the grid")) {
            Element::Item { children, .. } => children,
            _ => panic!("grid view"),
        }
    }

    /// Gap #2: no colspan, so a "Nobody yet" message got column 1's width
    /// and wrapped inside it. `ColSpan::Rest` must cover every column.
    #[test]
    fn regression_rest_span_covers_the_row() {
        with_test_world(|| {
            let head = TableRow(TableRowProps {
                children: (0..4).map(|_| body_cell("h")).collect(),
                ..Default::default()
            });
            let blank = TableRow(TableRowProps {
                children: vec![TableCell(TableCellProps {
                    text: Reactive::Static(Some("Nobody yet".into())),
                    span: ColSpan::Rest,
                    ..Default::default()
                })],
                ..Default::default()
            });
            let cells = grid_cells(Table(TableProps {
                children: vec![head, blank],
                ..Default::default()
            }));
            assert_eq!(cells.len(), 5);
            let rules = rules_of(cells.into_iter().nth(4).unwrap());
            assert_eq!(
                rules.grid_column,
                Some(runtime_core::GridPlacement::Lines(1, 5)),
                "the blank message spans all four columns"
            );
        });
    }

    fn tone_row(tone: RowTone, clickable: bool, pinned: Option<ColumnPin>) -> Vec<Element> {
        row_cells(TableRow(TableRowProps {
            children: vec![TableCell(TableCellProps {
                text: Reactive::Static(Some("x".into())),
                pinned,
                ..Default::default()
            })],
            tone: Reactive::Static(tone),
            on_row_click: clickable.then(|| Rc::new(|| {}) as Rc<dyn Fn()>),
            ..Default::default()
        }))
    }

    /// Gap #3: tinting a row by overriding each cell's background beat the
    /// hover axis, so a warning row stopped responding to the pointer, and
    /// it was not preminted. A tone must be an axis that premints, keeps
    /// the hover (as a deeper shade of its own tint), and stays opaque on
    /// a frozen column.
    #[test]
    fn regression_row_tone_composes_with_hover_and_pin() {
        with_test_world(|| {
            // Static tone on a plain row: a static, preminted selection.
            let app = cell_app(tone_row(RowTone::Warning, false, None).remove(0));
            assert!(app.preminted_class_list().is_some(), "a row tone premints");
            assert_eq!(
                bg_token(&runtime_core::resolve_style(&app)).as_deref(),
                Some("color-table-row-warning")
            );

            // Clickable + toned: resting shows the tone, hovered the tone's
            // hover shade — the hover is not lost.
            let resting = cell_app(tone_row(RowTone::Danger, true, None).remove(0));
            assert_eq!(
                bg_token(&runtime_core::resolve_style(&resting)).as_deref(),
                Some("color-table-row-danger")
            );
            let hovered = resting.clone().with("row_hovered", "on");
            assert_eq!(
                bg_token(&runtime_core::resolve_style(&hovered)).as_deref(),
                Some("color-table-row-danger-hover"),
                "a toned row still answers the pointer"
            );

            // Frozen + toned: the tone, not the pinned surface — and the
            // tone tokens are opaque in both themes.
            let pinned = cell_app(tone_row(RowTone::Highlight, true, Some(ColumnPin::Left)).remove(0));
            assert_eq!(
                bg_token(&runtime_core::resolve_style(&pinned)).as_deref(),
                Some("color-table-row-highlight")
            );
            for theme in [crate::light_theme(), crate::dark_theme()] {
                let c = &theme.colors;
                for tok in [
                    &c.table_row_highlight, &c.table_row_highlight_hover,
                    &c.table_row_warning, &c.table_row_warning_hover,
                    &c.table_row_danger, &c.table_row_danger_hover,
                ] {
                    let v = tok.value().0.to_ascii_lowercase();
                    assert!(
                        v.starts_with('#') && v.len() == 7,
                        "row tone tokens must be opaque hex (a frozen column covers \
                         content with them), got {v}"
                    );
                }
            }
        });
    }

    /// Gap #3, live half: a reactive tone re-tints the row in place.
    #[test]
    fn reactive_row_tone_retints_in_place() {
        with_test_world(|| {
            let tone = signal(RowTone::None);
            let cells = row_cells(TableRow(TableRowProps {
                children: vec![body_cell("x")],
                tone: Reactive::Dynamic(Rc::new(move || tone.get())),
                ..Default::default()
            }));
            let style = match classify(cells.into_iter().next().unwrap()) {
                P::View { style, .. } => style.expect("styled"),
                _ => panic!("view"),
            };
            let bg = |s: &crate::test_support::TStyle| {
                bg_token(&runtime_core::resolve_style(&s.application()))
            };
            assert_ne!(bg(&style).as_deref(), Some("color-table-row-warning"));
            tone.set(RowTone::Warning);
            idea_theme::testing::commit();
            assert_eq!(bg(&style).as_deref(), Some("color-table-row-warning"));
        });
    }

    /// Gap #4: no footer row — a totals row read as data. `TableFooter`
    /// lowers to the SDK's footer section and tints its cells with the
    /// header band, frozen ones included.
    #[test]
    fn regression_footer_row_is_a_section_with_the_band_tint() {
        with_test_world(|| {
            let footer = TableFooter(TableFooterProps {
                children: vec![TableRow(TableRowProps {
                    children: vec![
                        TableCell(TableCellProps {
                            text: Reactive::Static(Some("Totals".into())),
                            pinned: Some(ColumnPin::Left),
                            ..Default::default()
                        }),
                        body_cell("38.5"),
                    ],
                    ..Default::default()
                })],
            });
            let rows = match peel_owned_keepalive(footer) {
                Element::Item { data, children, .. } => {
                    assert!(
                        data.downcast_ref::<runtime_vocabulary::prims::PrimCell<table::TableFootPrim>>()
                            .is_some(),
                        "TableFooter lowers to the SDK footer section"
                    );
                    children
                }
                _ => panic!("footer section item"),
            };
            for cell in row_cells(rows.into_iter().next().unwrap()) {
                assert_eq!(bg_token(&rules_of(cell)).as_deref(), Some("color-table-header"));
            }
        });
    }

    /// Gap #5: content that must not scroll sideways with the columns had
    /// to sit outside the table's frame. The slots land inside the clipped
    /// surface, either side of the scroller, with the cells' padding.
    #[test]
    fn regression_frame_slots_sit_inside_the_surface() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![TableRow(TableRowProps {
                    children: vec![body_cell("a")],
                    ..Default::default()
                })],
                scroll_x: true,
                header_slot: Some(runtime_core::text("Holiday").into_element()),
                footer_slot: Some(runtime_core::text("Add entry").into_element()),
                ..Default::default()
            });
            let (style, kids) = match peel_owned_keepalive(t) {
                Element::Item { data, children, .. } => (
                    data.downcast_ref::<runtime_vocabulary::prims::PrimCell<
                        runtime_vocabulary::prims::ViewPrim,
                    >>()
                    .expect("surface view")
                    .take()
                    .style,
                    children,
                ),
                _ => panic!("surface"),
            };
            let surface = match style {
                Some(StyleProp::Sheet(app)) => runtime_core::resolve_style(&app),
                _ => panic!("themed surface sheet"),
            };
            assert_eq!(surface.overflow, Some(runtime_core::Overflow::Hidden), "surface clips");
            assert_eq!(kids.len(), 3, "header slot, scroller, footer slot");
            let mut kids = kids.into_iter();
            let header = rules_of(kids.next().unwrap());
            assert!(header.border_bottom_width.is_some(), "header slot draws its divider");
            assert!(header.padding_left.is_some(), "slot content gets the cells' padding");
        });
    }

    /// Gap #6: two pinned columns both stuck at `left: 0` and overlapped.
    /// `pin_offset` insets the second; the first keeps the preminted zero.
    #[test]
    fn regression_second_pinned_column_insets_by_its_offset() {
        with_test_world(|| {
            let second = rules_of(TableCell(TableCellProps {
                pinned: Some(ColumnPin::Left),
                pin_offset: Some(48.0),
                ..Default::default()
            }));
            assert_eq!(second.position, Some(runtime_core::Position::Sticky));
            assert_eq!(px_of(&second.left), Some(48.0));
            let right = rules_of(TableCell(TableCellProps {
                pinned: Some(ColumnPin::Right),
                pin_offset: Some(30.0),
                ..Default::default()
            }));
            assert_eq!(px_of(&right.right), Some(30.0));
            assert!(right.left.is_none());
            let first = cell_app(TableCell(TableCellProps {
                pinned: Some(ColumnPin::Left),
                ..Default::default()
            }));
            assert!(first.preminted_class_list().is_some(), "offset 0 stays preminted");
        });
    }

    /// Gap #7: no ref on `TableRow`. `bind_to` routes to the SDK's row
    /// proxy slot — the `<tr>` on web, the row backdrop on native.
    #[test]
    fn regression_row_bind_to_fills_the_row_proxy() {
        with_test_world(|| {
            let r: Ref<ViewHandle> = Ref::new();
            let mut row = TableRow(TableRowProps {
                children: vec![body_cell("x")],
                bind_to: Some(r),
                ..Default::default()
            });
            loop {
                match row {
                    Element::Owned { element, owned, .. } => {
                        ROW_SCOPES.with(|k| k.borrow_mut().push(owned));
                        row = *element;
                    }
                    Element::Item { data, .. } => {
                        let prim = data
                            .downcast_ref::<runtime_vocabulary::prims::PrimCell<table::TableRowPrim>>()
                            .expect("row marker")
                            .take();
                        assert!(prim.ref_fill.is_some(), "bind_to sets the row proxy slot");
                        break;
                    }
                    _ => panic!("row marker item"),
                }
            }
        });
    }

    /// Gap #8: row padding was fixed at `spacing.md`. `density` reaches
    /// every cell — head and body — as a static (preminted) selection.
    #[test]
    fn regression_table_density_reaches_every_cell() {
        with_test_world(|| {
            let spacing = &crate::light_theme().spacing;
            for (density, expect) in [
                (TableDensity::Compact, spacing.sm),
                (TableDensity::Standard, spacing.md),
                (TableDensity::Comfortable, spacing.lg),
            ] {
                let head = TableRow(TableRowProps {
                    children: vec![TableCell(TableCellProps {
                        header: true,
                        text: Reactive::Static(Some("h".into())),
                        ..Default::default()
                    })],
                    ..Default::default()
                });
                let body = TableRow(TableRowProps {
                    children: vec![body_cell("b")],
                    ..Default::default()
                });
                let cells = grid_cells(Table(TableProps {
                    children: vec![head, body],
                    density,
                    ..Default::default()
                }));
                for cell in cells {
                    let app = cell_app(cell);
                    assert!(app.preminted_class_list().is_some(), "density premints");
                    let rules = runtime_core::resolve_style(&app);
                    let pad = px_of(&rules.padding_top);
                    assert_eq!(pad, Some(expect), "{density:?}");
                }
            }
        });
    }
}
