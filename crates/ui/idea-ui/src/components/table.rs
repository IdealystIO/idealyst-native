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
//!   table-wide settings: `variant`, `scroll_x`, `scroll_y`,
//!   `max_height`, `inset_x`, `density`, and the frame slots.
//! - [`TableRow`] is a `<tr>`: `on_row_click` for a clickable row,
//!   `tone` for a whole-row tint, `bind_to` for the row's handle.
//! - [`TableCell`] wraps `<td>` (or `<th>` when `header = true`) with
//!   the cell-level padding + row divider, and wraps cell contents in
//!   a themed `text` node so values without explicit Typography pick
//!   up the right column treatment. Column layout lives here too —
//!   `width`, `min_width`, `max_width`, `flex`, `align`, `truncate`,
//!   `span`, `pinned`.
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
//!   meaningful in a column with a `width` or a `flex` (an unsized
//!   column widens to fit the line instead).
//!
//! ## Grid columns: `flex` and `max_width`
//!
//! The settings above size columns the way a browser sizes an auto
//! `<table>`. Two more need a different model, because a table cell
//! ignores them: `flex = Some(n)` (this column takes a share of the width
//! the others leave, and may shrink to its `min_width`, default 0) and
//! `max_width = Some(px)` (no wider than this). Setting either on ANY
//! column lays the whole table out as CSS grid columns, on every backend:
//!
//! - a `width` column is exactly that wide;
//! - a `flex` column shares the leftover width (relative to the other
//!   `flex` columns), up to its `max_width` if it has one;
//! - a `max_width` column is its content's width, capped;
//! - every other column is its content's width and never shrinks.
//!
//! Spare width goes only to `flex` columns, so a grid-column table
//! without one is as wide as its columns. The everyday use is a long
//! name beside short columns that must stay readable:
//!
//! ```ignore
//! TableRow {
//!     TableCell(header = true, text = Some("Name".into()), flex = Some(1.0), truncate = true)
//!     TableCell(header = true, text = Some("Type".into()))
//!     TableCell(header = true, text = Some("Page".into()))
//! }
//! ```
//!
//! The column takes its sizing from its first cell (set it on the header
//! cell, or on every cell of the column like the other settings). On web
//! the table stays a real `<table>`, with explicit table roles since its
//! rows become `display: contents`.
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
//! # Vertical scrolling & a pinned header
//!
//! `Table(scroll_y = true)` makes the table own its vertical scroll: the
//! rows scroll inside the table's own height and the header row stays
//! pinned to the top. Bound it with `max_height = Some(px)`, or let it
//! fill a bounded parent (without `max_height` a `scroll_y` table takes
//! the height its parent column gives it, and is as tall as its rows in
//! an unbounded one). With `scroll_x` too the table owns ONE two-axis
//! scroller, so the header pins to the top while frozen columns keep
//! pinning to the sides, and a frozen header cell — the corner — pins on
//! both axes, painted above the header row, which is painted above the
//! body. That is the only way to freeze both: a sticky cell pins against
//! its nearest scroll container, so in a `scroll_x` table that scrolls
//! with the page the header can never stick to the page.
//!
//! The header rows are the LEADING rows whose cells are all head cells
//! (`TableCell(header = true)`).
//!
//! # Edge inset
//!
//! `Table(inset_x = TableInset::Xl)` sets the outer padding of every row's
//! first and last cell (and of the frame slots), leaving the cells in
//! between alone — so the row dividers run the table's full width while
//! the text lines up with the page margin. The token sizes follow the
//! theme's spacing scale; `TableInset::Px(28.0)` takes an exact margin
//! that is not on it. Typically paired with `variant = TableVariant::Plain`
//! for an edge-to-edge list.
//!
//! # Plain tables
//!
//! `Table(variant = TableVariant::Plain)` drops the card — no frame, no
//! rounded corners, no header band, no fill of its own — so it sits on
//! the surface around it with the row dividers as its only lines. In a
//! `scroll_x` table a frozen column then draws its inner-edge hairline
//! only while columns are scrolled beneath it: the left-frozen edge once
//! the scroll offset passes 0, the right-frozen edge while the columns
//! run on past the right. The table learns that from the SDK's
//! `on_scroll_edges` and re-styles only its frozen cells (the edge goes
//! transparent rather than thin, so the column never shifts); every
//! other cell stays a static selection. Frozen cells must stay opaque
//! and paint `color-surface`, so a plain table belongs on that surface.
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

/// The [`Table`]'s outer treatment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema, runtime_core::Remote)]
pub enum TableVariant {
    /// A card: hairline frame, rounded corners, the surface fill, a
    /// tinted header band. The default.
    #[default]
    Framed,
    /// No card: no frame, no rounded corners, no header band and no
    /// fill of its own — the table sits on the surface around it, and
    /// the row dividers are its only lines. In a `scroll_x` table a
    /// frozen column draws its inner edge only while columns are
    /// scrolled beneath it. Frozen cells still paint `color-surface`
    /// (content slides beneath them), so sit a plain table on that
    /// surface.
    Plain,
}

// The variant string IS the stylesheet arm the value selects.
impl VariantEnum for TableVariant {
    fn as_variant_str(self) -> &'static str {
        match self {
            TableVariant::Framed => "framed",
            TableVariant::Plain => "plain",
        }
    }

    fn all_variants() -> &'static [Self] {
        &[TableVariant::Framed, TableVariant::Plain]
    }
}

/// The outer padding of a [`Table`]'s edge cells — see the module docs'
/// "Edge inset". The token sizes are the theme's spacing scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, IdealystSchema)]
pub enum TableInset {
    /// The cells' own padding at the edges too. The default.
    #[default]
    None,
    /// `spacing.xs`.
    Xs,
    /// `spacing.sm`.
    Sm,
    /// `spacing.md`.
    Md,
    /// `spacing.lg` — the same as the cells' own horizontal padding.
    Lg,
    /// `spacing.xl`.
    Xl,
    /// `spacing.xxl`.
    Xxl,
    /// An exact length in px, for a page margin that is not on the
    /// spacing scale.
    Px(f32),
}

impl TableInset {
    /// The sheets' `inset_left` / `inset_right` arm a token inset
    /// selects; `None` for [`TableInset::None`] and [`TableInset::Px`].
    fn arm(self) -> Option<&'static str> {
        match self {
            TableInset::None | TableInset::Px(_) => None,
            TableInset::Xs => Some("xs"),
            TableInset::Sm => Some("sm"),
            TableInset::Md => Some("md"),
            TableInset::Lg => Some("lg"),
            TableInset::Xl => Some("xl"),
            TableInset::Xxl => Some("xxl"),
        }
    }

    /// Apply this inset to one side of a cell or slot application:
    /// a token selects the sheet arm (preminted), an exact length rides
    /// an override.
    fn apply(self, app: StyleApplication, side: InsetSide) -> StyleApplication {
        if let Some(arm) = self.arm() {
            return app.with(side.axis(), arm);
        }
        match self {
            TableInset::Px(v) => {
                let len = Some(Tokenized::Literal(Length::Px(v)));
                app.with_overrides(match side {
                    InsetSide::Left => StyleRules { padding_left: len, ..Default::default() },
                    InsetSide::Right => StyleRules { padding_right: len, ..Default::default() },
                })
            }
            _ => app,
        }
    }
}

/// Which edge of a row [`TableInset::apply`] pads.
#[derive(Clone, Copy)]
enum InsetSide {
    Left,
    Right,
}

impl InsetSide {
    fn axis(self) -> &'static str {
        match self {
            InsetSide::Left => "inset_left",
            InsetSide::Right => "inset_right",
        }
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
    /// Vertical-scroll mode: the rows scroll inside the table's own
    /// height and the header row stays pinned to the top. Bound it with
    /// `max_height`, or let it fill a bounded parent. With `scroll_x` the
    /// table owns one two-axis scroller, so frozen columns and the header
    /// pin together — see the module docs.
    // STRUCTURAL — selects the SDK's scroller and the header pin at build.
    #[prop(static)]
    pub scroll_y: bool,
    /// The most the table grows to, in px, before its rows scroll.
    /// Ignored without `scroll_y`.
    // STATIC — an override on the surface style at build.
    #[prop(static)]
    pub max_height: Option<f32>,
    /// The outer padding of every row's first and last cell (and of the
    /// frame slots), so the row dividers run the table's full width while
    /// the text keeps a page margin — see [`TableInset`].
    // STATIC — selected onto the edge cells when the table is built.
    #[prop(static)]
    pub inset_x: TableInset,
    /// Row padding for every cell — `Compact`, `Standard` or
    /// `Comfortable`.
    // STATIC — selected onto every built cell's sheet when the table is
    // built (the cells already exist by then; see `Table`).
    #[prop(static)]
    pub density: TableDensity,
    /// `Framed` (the default card) or `Plain` (row dividers only, frozen
    /// edges drawn while content scrolls beneath them) — see
    /// [`TableVariant`].
    // STATIC — selected onto the surface and every built cell, and it
    // decides whether the scroller is watched, all at build.
    #[prop(static)]
    pub variant: TableVariant,
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
    // The SDK lands this style on a surface VIEW around the `<table>` /
    // native grid on every table, never on the `<table>`: browsers
    // ignore `border-radius` on a collapsed-border table. The sheet's
    // clip keeps the corner cells inside the rounded frame.
    let frame = props.variant.as_variant_str();
    let style = TableStyle()
        .into_style_application()
        .with("frame", frame)
        .with("scroll", if props.scroll_y { "fill" } else { "off" });
    let style = match props.max_height.filter(|_| props.scroll_y) {
        Some(h) => style.with_overrides(StyleRules {
            max_height: Some(Tokenized::Literal(Length::Px(h))),
            ..Default::default()
        }),
        None => style,
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
    // The row rendered last loses its divider unless a footer slot sits
    // below it: otherwise the frame's bottom border is directly under that
    // divider and the two read as one 2px line. Static selection, so the
    // cells stay preminted; header cells carry the axis too, for a table
    // that is only a header row.
    if props.footer_slot.is_none() {
        table::visit_last_rendered_row(&children, |row| {
            table::visit_row_cells(row, |cell| {
                table::map_cell_application(cell, Rc::new(|app| app.with("divider", "off")));
            });
        });
    }
    // A scroll-y table pins its header rows: the leading rows whose cells
    // are all head cells. Static selection, so the cells stay preminted.
    if props.scroll_y {
        let mut head = true;
        for child in &children {
            table::visit_rows(child, |row| {
                head = head && row_is_head(row);
                if head {
                    table::visit_row_cells(row, |cell| {
                        table::map_cell_application(
                            cell,
                            Rc::new(|app| app.with("sticky_header", "on")),
                        );
                    });
                }
            });
        }
    }
    // Edge inset: the first and last cell of every row — a single-cell
    // row (a `ColSpan::Rest` blank row) takes both sides.
    let inset = props.inset_x;
    if inset != TableInset::None {
        for child in &children {
            table::visit_rows(child, |row| {
                let mut count = 0usize;
                table::visit_row_cells(row, |_| count += 1);
                let mut i = 0usize;
                table::visit_row_cells(row, |cell| {
                    let (first, last) = (i == 0, i + 1 == count);
                    i += 1;
                    if first || last {
                        table::map_cell_application(
                            cell,
                            Rc::new(move |app| {
                                let app = if first { inset.apply(app, InsetSide::Left) } else { app };
                                if last { inset.apply(app, InsetSide::Right) } else { app }
                            }),
                        );
                    }
                });
            });
        }
    }
    let plain = props.variant == TableVariant::Plain;
    if plain {
        for child in &children {
            table::visit_rows(child, |row| {
                table::visit_row_cells(row, |cell| {
                    table::map_cell_application(cell, Rc::new(|app| app.with("frame", "plain")));
                });
            });
        }
    }
    // A plain table's frozen columns draw their inner edge only while
    // columns are scrolled beneath it. Last, so it composes over every
    // selection above; and only on cells that may be pinned, so the rest
    // stay static (a static cell that is not pinned is skipped, a reactive
    // one is asked when it evaluates — see `pin_edge`).
    let edges = (plain && props.scroll_x).then(|| {
        let edges = signal(table::ScrollEdges::default());
        for child in &children {
            table::visit_rows(child, |row| {
                table::visit_row_cells(row, |cell| {
                    let unpinned = table::cell_application(cell)
                        .is_some_and(|app| !app.variants.0.contains_key("pinned"));
                    if !unpinned {
                        table::map_cell_style(cell, Rc::new(move |app| pin_edge(app, edges)));
                    }
                });
            });
        }
        edges
    });
    let slot_style = move |edge: &'static str| {
        let app = TableSlot().into_style_application().with("edge", edge);
        inset.apply(inset.apply(app, InsetSide::Left), InsetSide::Right)
    };
    let header_slot = props.header_slot.map(|slot| {
        let style = slot_style("top");
        ui! { view(style = style) { slot } }
    });
    let footer_slot = props.footer_slot.map(|slot| {
        let style = slot_style("bottom");
        ui! { view(style = style) { slot } }
    });
    // SDK's `table()` returns a `Bound<TableHandle>`; chain
    // `.with_style(...)` to land the themed style on its surface view,
    // then convert to Element.
    let table = sdk_table(SdkTableProps {
        children,
        scroll_x: props.scroll_x,
        scroll_y: props.scroll_y,
        header_slot,
        footer_slot,
    })
    .with_style(style);
    match edges {
        Some(edges) => table.on_scroll_edges(move |e| edges.set(e)).into_element(),
        None => table.into_element(),
    }
}

/// Whether every cell of `row` is a head cell — `TableCell(header = true)`
/// selects the head sheet's `sticky_header` axis, which a body cell's
/// sheet does not have. An empty row is not a head row.
fn row_is_head(row: &Element) -> bool {
    let mut cells = 0usize;
    let mut head = 0usize;
    table::visit_row_cells(row, |cell| {
        cells += 1;
        if table::cell_application(cell).is_some_and(|app| app.variants.0.contains_key("sticky_header")) {
            head += 1;
        }
    });
    cells > 0 && cells == head
}

/// Select the `pin_edge` arm for a frozen cell: its inner-edge hairline
/// shows only while columns are scrolled beneath that edge. A cell that
/// is not pinned is returned untouched WITHOUT reading `edges`, so it
/// never re-styles on a scroll.
fn pin_edge(app: StyleApplication, edges: Signal<table::ScrollEdges>) -> StyleApplication {
    let behind = match app.variants.0.get("pinned").map(String::as_str) {
        Some("left") => edges.get().left,
        Some("right") => edges.get().right,
        _ => return app,
    };
    app.with("pin_edge", if behind { "shown" } else { "hidden" })
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
        table::bind_row(&row, move |h| runtime_core::fill_ref_while_mounted(r, h));
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
    /// Never let this cell's column be wider than this (px): it sizes
    /// to its content up to the cap (or, with `flex`, grows into spare
    /// width up to it). Setting it on any column makes the table lay out
    /// as grid columns — see the module docs, "Columns".
    // STRUCTURAL — decides the table's layout mode at build time.
    #[prop(static)]
    pub max_width: Option<f32>,
    /// Give this column a share of the width the other columns leave —
    /// `Some(1.0)` for "the rest". Shares are relative to the table's
    /// other `flex` columns. The column may shrink to its `min_width`
    /// (default 0), so pair it with `truncate` for a long-name column
    /// that ends in "…" instead of pushing the others out. Setting it on
    /// any column makes the table lay out as grid columns.
    // STRUCTURAL — decides the table's layout mode at build time.
    #[prop(static)]
    pub flex: Option<f32>,
    /// How the content sits in the column — `Left` (default), `Center`
    /// or `Right`. Give every cell of the column the same value.
    // STRUCTURAL — selects the `align` arm on three nodes (the cell, its
    // text node, the rich-children wrapper) at build time.
    #[prop(static)]
    pub align: CellAlign,
    /// Keep the content to one line ending in "…". Pair with `width` or
    /// `flex`.
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
            max_width: None,
            flex: None,
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

    // The column's sizing, for the table's grid columns. Only a cell that
    // sizes its column carries one, so an unsized table keeps the
    // browser auto-table layout untouched. Live: `width` / `min_width`
    // can follow a resize drag.
    let column = {
        let (width, min_width) = (props.width.clone(), props.min_width.clone());
        let (max_width, flex) = (props.max_width, props.flex);
        let sized = !matches!(width, Reactive::Static(None))
            || !matches!(min_width, Reactive::Static(None))
            || max_width.is_some()
            || flex.is_some();
        sized.then(|| {
            table::ColumnSpec::live(move || table::ColumnTrack {
                width: width.get(),
                min_width: min_width.get(),
                max_width,
                flex,
            })
        })
    };
    let bound = sdk_cell(SdkTableCellProps {
        header,
        span: props.span,
        column,
        children: cell_children,
    });
    // A cell's style is handed over as an EXPLICIT `StyleProp::Sheet`, not
    // as a bare application. A clickable row composes the `interactive` /
    // `row_hovered` axes onto each cell's own style
    // (`table::map_cell_style`), and a `--premint` build's opaque
    // `Preminted` class carries no application to compose with — the app
    // must stay introspectable in the payload.
    //
    // `.into_style_application()` alone is not enough: `IntoStyleProp for
    // StyleApplication` has a preminted fast path, so the application would
    // premint anyway, the composition would find nothing to compose with,
    // and `make_row_cell_interactive` would silently skip the whole overlay
    // — clickable rows lose their pointer cursor and hover highlight in
    // every `--premint` build, with nothing logged. Naming the variant pins
    // the intent; `regression_premint_keeps_table_cells_on_the_live_engine`
    // holds it there.
    //
    // This costs the premint nothing: the row overlay is axis
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
        // `sticky_header: off` stated outright: the key marks a head cell
        // for `Table`, which pins the leading head rows of a scroll-y table.
        let app = TableHeadCell().into_style_application().with("sticky_header", "off");
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

    runtime_tls::thread_local! {
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

            // The hovered arm resolves to the themed row highlight.
            let hovered_app = TableBodyCell()
                .into_style_application()
                .with("interactive", "on")
                .with("row_hovered", "on");
            assert!(hovered_app.preminted_class_list().is_some());
            let hovered = runtime_core::resolve_style(&hovered_app);
            // Asserted against the palette, not a literal: a restated hex
            // fallback can drift from the palette. Deriving the expectation
            // from `light_theme()` means the two can't disagree.
            let surface_alt = crate::light_theme().colors.surface_alt.value().0.to_ascii_lowercase();
            assert_eq!(
                hovered.background.as_ref().map(|b| b.resolve().0.to_ascii_lowercase()),
                Some(surface_alt),
                "row_hovered arm resolves the themed surface-alt highlight"
            );
        });
    }

    /// The header band reads its OWN token, not `color-surface-alt`, so
    /// "retint table headers" doesn't drag every other `surface_alt`
    /// consumer (cards, field wells, row hover) along. Asserting the
    /// token NAME is the load-bearing half: a literal — or a
    /// `color-surface-alt` reference — is exactly what an app-level
    /// `color-table-header` override could never reach.
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
    /// other. The app-side shape: an author who needs a width on a cell
    /// (idea-ui's `TableCell` has none) drops to the SDK cell and hands it
    /// a style CLOSURE, because the width has to move while a resize
    /// handle is being dragged. A read-then-replace wiring that only sees a
    /// static `StyleProp::Sheet` leaves every such column un-tinted in a
    /// row whose other cells light up, with nothing logged. Composing means
    /// the cell keeps its width and takes the axes.
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

    /// Regression: a plain `Table` — no `scroll_x`, no slot — rendered
    /// square corners on web, because its sheet landed on the
    /// collapsed-border `<table>`, where browsers ignore the radius (the
    /// SDK wraps it; `table`'s
    /// `regression_styled_plain_table_rounds_on_a_surface_not_the_collapsed_table`
    /// pins that placement). The plain table's surface must carry the
    /// radius AND the clip, or the header band paints square over the
    /// corners the radius rounds.
    #[test]
    fn regression_plain_table_surface_rounds_and_clips() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![TableRow(TableRowProps {
                    children: vec![body_cell("a")],
                    ..Default::default()
                })],
                ..Default::default()
            });
            let vp = match peel_owned_keepalive(t) {
                Element::Item { data, .. } => data
                    .downcast_ref::<runtime_vocabulary::prims::PrimCell<
                        runtime_vocabulary::prims::ViewPrim,
                    >>()
                    .expect("plain table root is the styled surface view")
                    .take(),
                _ => panic!("plain table must lower to the surface view"),
            };
            let app = match vp.style.expect("surface carries the themed Table sheet") {
                runtime_vocabulary::StyleProp::Sheet(app) => *app,
                _ => panic!("Table sheet is a static application"),
            };
            let rules = runtime_core::resolve_style(&app);
            assert!(rules.border_top_left_radius.is_some(), "surface is rounded");
            assert_eq!(
                rules.overflow,
                Some(runtime_core::Overflow::Hidden),
                "plain table's surface must clip its cells to the rounded frame"
            );
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
                        .is_some_and(|c| c.take().axis == runtime_core::ScrollAxis::Horizontal),
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

    /// The cell's own node: a cell that sizes its column (`width`,
    /// `min_width`, `max_width`, `flex`) arrives in the SDK's
    /// `CellMarker`, which `table` unwraps; a test looking at one cell
    /// outside a table looks through it (scopes kept alive).
    fn cell_node(cell: Element) -> Element {
        match peel_owned_keepalive(cell) {
            Element::Item { data, mut children, .. }
                if data.downcast_ref::<runtime_vocabulary::prims::PrimCell<table::CellMarker>>().is_some() =>
            {
                children.pop().expect("a cell marker wraps the cell")
            }
            other => other,
        }
    }

    fn cell_app(cell: Element) -> runtime_core::StyleApplication {
        match classify(cell_node(cell)) {
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
        let children = match classify(cell_node(cell)) {
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
            let style = match classify(cell_node(cell)) {
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

    /// The cells of a table that may be WRAPPED (scroll-x / frame slots):
    /// the surface view holds [header slot?, content, footer slot?], and a
    /// scroll-x content is a horizontal scroller around the plain table.
    fn any_grid_cells(t: Element, wrapped: bool, header_slot: bool, scroll_x: bool) -> Vec<Element> {
        if !wrapped {
            return grid_cells(t);
        }
        let surface = match peel_owned_keepalive(t) {
            Element::Item { children, .. } => children,
            _ => panic!("a wrapped table lowers to its surface view"),
        };
        let content = surface.into_iter().nth(header_slot as usize).expect("surface holds the rows");
        if !scroll_x {
            return grid_cells(content);
        }
        match peel_owned_keepalive(content) {
            Element::Item { children, .. } => {
                grid_cells(children.into_iter().next().expect("the scroller wraps the table"))
            }
            _ => panic!("scroll-x content is the scroller"),
        }
    }

    /// Each rendered cell's bottom-divider width, in render order.
    fn dividers(cells: Vec<Element>) -> Vec<Option<f32>> {
        cells
            .into_iter()
            .map(|cell| {
                let app = cell_app(cell);
                assert!(app.preminted_class_list().is_some(), "the divider selection premints");
                runtime_core::resolve_style(&app).border_bottom_width.as_ref().map(|w| w.resolve())
            })
            .collect()
    }

    fn row_of(cells: Vec<Element>) -> Element {
        TableRow(TableRowProps { children: cells, ..Default::default() })
    }

    fn head_cell(text: &str) -> Element {
        TableCell(TableCellProps {
            header: true,
            text: Reactive::Static(Some(text.into())),
            ..Default::default()
        })
    }

    /// REGRESSION (idea-ui 4.0.0): the bottom edge of every table was a
    /// double line. Every cell drew its row divider — the last row's
    /// included — and the frame drew its own 1px border right under it.
    /// The row rendered last must drop its divider; every other row keeps
    /// one.
    #[test]
    fn regression_table_bottom_edge_is_not_a_double_line() {
        with_test_world(|| {
            for scroll_x in [false, true] {
                let t = Table(TableProps {
                    children: vec![
                        row_of(vec![head_cell("Name"), head_cell("Hours")]),
                        row_of(vec![body_cell("Ana"), body_cell("8")]),
                        row_of(vec![body_cell("Bo"), body_cell("6")]),
                    ],
                    scroll_x,
                    ..Default::default()
                });
                assert_eq!(
                    dividers(any_grid_cells(t, scroll_x, false, scroll_x)),
                    vec![Some(1.0), Some(1.0), Some(1.0), Some(1.0), Some(0.0), Some(0.0)],
                    "scroll_x = {scroll_x}: only the last row drops its divider"
                );
            }
        });
    }

    /// A footer renders after the body rows wherever it is written, so
    /// ITS last row is the one against the frame — a totals foot doubled
    /// the same way. Written first here to prove the choice follows render
    /// order, not source order.
    #[test]
    fn regression_table_footer_is_the_last_row_wherever_written() {
        with_test_world(|| {
            let foot = TableFooter(TableFooterProps {
                children: vec![row_of(vec![body_cell("Total"), body_cell("14")])],
            });
            let t = Table(TableProps {
                children: vec![
                    foot,
                    row_of(vec![head_cell("Name"), head_cell("Hours")]),
                    row_of(vec![body_cell("Ana"), body_cell("8")]),
                ],
                ..Default::default()
            });
            assert_eq!(
                dividers(grid_cells(t)),
                vec![Some(1.0), Some(1.0), Some(1.0), Some(1.0), Some(0.0), Some(0.0)],
                "the body's last row keeps its divider above the foot"
            );
        });
    }

    /// With a footer slot below the rows, the last row's divider IS the
    /// line between rows and slot — the frame's border is under the slot —
    /// so it stays.
    #[test]
    fn table_footer_slot_keeps_the_last_row_divider() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![row_of(vec![head_cell("Name")]), row_of(vec![body_cell("Ana")])],
                footer_slot: Some(runtime_core::text("Add entry".to_string()).into_element()),
                ..Default::default()
            });
            assert_eq!(dividers(any_grid_cells(t, true, false, false)), vec![Some(1.0), Some(1.0)]);
        });
    }

    /// A table that is only its header row: the header cells are the last
    /// row, so they carry the axis too.
    #[test]
    fn header_only_table_drops_the_header_divider() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![row_of(vec![head_cell("Name"), head_cell("Hours")])],
                ..Default::default()
            });
            assert_eq!(dividers(grid_cells(t)), vec![Some(0.0), Some(0.0)]);
        });
    }

    /// The cells of a `Plain` + `scroll_x` table: the surface holds the
    /// viewport holder the edge watch measures, around the scroller.
    fn plain_scroll_cells(t: Element) -> Vec<Element> {
        let surface = match peel_owned_keepalive(t) {
            Element::Item { children, .. } => children,
            _ => panic!("a scroll-x table lowers to its surface view"),
        };
        let holder = match peel_owned_keepalive(surface.into_iter().next().expect("surface holds the rows")) {
            Element::Item { children, .. } => children,
            _ => panic!("a watched scroller sits in its viewport holder"),
        };
        match peel_owned_keepalive(holder.into_iter().next().expect("the holder wraps the scroller")) {
            Element::Item { children, .. } => {
                grid_cells(children.into_iter().next().expect("the scroller wraps the table"))
            }
            _ => panic!("the holder's child is the scroller"),
        }
    }

    fn pinned_head(text: &str, pin: ColumnPin) -> Element {
        TableCell(TableCellProps {
            header: true,
            text: Reactive::Static(Some(text.into())),
            pinned: Some(pin),
            ..Default::default()
        })
    }

    fn pinned_body(text: &str, pin: ColumnPin) -> Element {
        TableCell(TableCellProps {
            text: Reactive::Static(Some(text.into())),
            pinned: Some(pin),
            ..Default::default()
        })
    }

    fn is_transparent(c: &Option<Tokenized<runtime_core::Color>>) -> bool {
        c.as_ref().is_some_and(|c| c.resolve().0 == "transparent")
    }

    /// `Plain` drops the card: no frame, no radius, no fill of its own —
    /// and `Framed` (the default) keeps every one of them.
    #[test]
    fn plain_table_surface_has_no_frame() {
        with_test_world(|| {
            let plain = runtime_core::resolve_style(
                &TableStyle().into_style_application().with("frame", "plain"),
            );
            for w in [
                &plain.border_top_width,
                &plain.border_right_width,
                &plain.border_bottom_width,
                &plain.border_left_width,
            ] {
                assert_eq!(w.as_ref().map(|w| w.resolve()), Some(0.0), "plain draws no frame");
            }
            assert!(is_transparent(&plain.background), "plain paints no fill of its own");

            let framed = runtime_core::resolve_style(&TableStyle().into_style_application());
            assert_eq!(framed.border_top_width.as_ref().map(|w| w.resolve()), Some(1.0));
            assert_eq!(bg_token(&framed).as_deref(), Some("color-surface"));
        });
    }

    /// A plain table has no header band, but a FROZEN head cell must stay
    /// opaque — the columns slide beneath it — so it paints the surface.
    /// Every cell of the table takes the selection, as a static one.
    #[test]
    fn plain_table_drops_the_header_band_but_frozen_heads_stay_opaque() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![
                    row_of(vec![pinned_head("Name", ColumnPin::Left), head_cell("Hours")]),
                    row_of(vec![pinned_body("Ana", ColumnPin::Left), body_cell("8")]),
                ],
                variant: TableVariant::Plain,
                ..Default::default()
            });
            let mut cells = grid_cells(t).into_iter();
            let frozen_head = cell_app(cells.next().unwrap());
            let head = cell_app(cells.next().unwrap());
            assert!(head.preminted_class_list().is_some(), "the frame selection premints");
            assert!(
                is_transparent(&runtime_core::resolve_style(&head).background),
                "a plain head cell has no band"
            );
            assert_eq!(
                bg_token(&runtime_core::resolve_style(&frozen_head)).as_deref(),
                Some("color-surface"),
                "a frozen plain head cell is opaque"
            );
        });
    }

    /// The whole feature at the component layer: in a plain scroll-x
    /// table, ONLY the frozen cells go reactive, and they start with their
    /// inner edge hidden (nothing is measured as scrolled beneath them
    /// yet). Every other cell stays a static, preminted selection — a
    /// scroll must not re-style the table.
    #[test]
    fn plain_scroll_table_watches_only_the_frozen_cells() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![
                    row_of(vec![
                        pinned_head("Name", ColumnPin::Left),
                        head_cell("Hours"),
                        pinned_head("", ColumnPin::Right),
                    ]),
                    row_of(vec![
                        pinned_body("Ana", ColumnPin::Left),
                        body_cell("8"),
                        pinned_body("…", ColumnPin::Right),
                    ]),
                ],
                variant: TableVariant::Plain,
                scroll_x: true,
                ..Default::default()
            });
            for (i, cell) in plain_scroll_cells(t).into_iter().enumerate() {
                let style = match classify(cell_node(cell)) {
                    P::View { style, .. } => style.expect("cell keeps a style"),
                    _ => panic!("native cell must classify as a View"),
                };
                let rules = runtime_core::resolve_style(&style.application());
                match i % 3 {
                    0 => {
                        assert!(style.is_reactive(), "cell {i}: a left-frozen cell follows the scroll");
                        assert_eq!(rules.border_right_width.as_ref().map(|w| w.resolve()), Some(1.0));
                        assert!(
                            is_transparent(&rules.border_right_color),
                            "cell {i}: the edge is hidden while nothing is behind it"
                        );
                    }
                    2 => {
                        assert!(style.is_reactive(), "cell {i}: a right-frozen cell follows the scroll");
                        assert!(is_transparent(&rules.border_left_color), "cell {i}");
                    }
                    _ => assert!(
                        !style.is_reactive() && style.application().preminted_class_list().is_some(),
                        "cell {i}: an unfrozen cell stays static"
                    ),
                }
            }
        });
    }

    /// The edge follows the scroll: hidden at rest, shown once columns
    /// are behind THAT edge, for the side the cell is frozen to only.
    #[test]
    fn pin_edge_shows_only_the_edge_with_content_behind_it() {
        with_test_world(|| {
            let edges = signal(table::ScrollEdges::default());
            let left = TableBodyCell().into_style_application().with("pinned", "left");
            let right = TableBodyCell().into_style_application().with("pinned", "right");
            let colours = || {
                let l = runtime_core::resolve_style(&pin_edge(left.clone(), edges));
                let r = runtime_core::resolve_style(&pin_edge(right.clone(), edges));
                (is_transparent(&l.border_right_color), is_transparent(&r.border_left_color))
            };
            assert_eq!(colours(), (true, true), "at rest with nothing behind: both hidden");
            edges.set(table::ScrollEdges { left: false, right: true });
            idea_theme::testing::commit();
            assert_eq!(colours(), (true, false), "columns run off the right: the right edge shows");
            edges.set(table::ScrollEdges { left: true, right: false });
            idea_theme::testing::commit();
            assert_eq!(colours(), (false, true), "scrolled to the end: the left edge shows");

            let shown = pin_edge(left.clone(), edges);
            assert!(shown.preminted_class_list().is_some(), "both arms premint");
            let unpinned = TableBodyCell().into_style_application();
            assert!(
                !pin_edge(unpinned, edges).variants.0.contains_key("pin_edge"),
                "an unfrozen cell is returned untouched"
            );
        });
    }

    /// `Framed` (the default) is unchanged: a frozen column in a scroll-x
    /// table keeps its hairline at rest and stays static — nothing
    /// watches its scroller.
    #[test]
    fn framed_scroll_table_keeps_its_frozen_edge_static() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![row_of(vec![pinned_body("Ana", ColumnPin::Left), body_cell("8")])],
                scroll_x: true,
                ..Default::default()
            });
            let frozen = any_grid_cells(t, true, false, true).into_iter().next().unwrap();
            let app = cell_app(frozen);
            assert!(app.preminted_class_list().is_some(), "a framed frozen cell stays static");
            let rules = runtime_core::resolve_style(&app);
            assert!(!is_transparent(&rules.border_right_color), "and draws its edge at rest");
        });
    }

    // =========================================================================
    // Scroll-y (pinned header) and edge inset.
    // =========================================================================

    /// The surface and scroller of a scrolling table: (surface rules,
    /// scroller axis, rendered cells).
    fn scrolling_parts(t: Element) -> (Rc<StyleRules>, runtime_core::ScrollAxis, Vec<Element>) {
        use runtime_vocabulary::prims::{PrimCell, ScrollViewPrim, ViewPrim};
        let (surface_rules, mut kids) = match peel_owned_keepalive(t) {
            Element::Item { data, children, .. } => {
                let vp = data.downcast_ref::<PrimCell<ViewPrim>>().expect("surface view").take();
                (surface_rules_of(vp.style), children)
            }
            _ => panic!("a scrolling table lowers to its surface view"),
        };
        let scroller = kids.pop().expect("surface wraps the scroller");
        let (axis, mut inner) = match peel_owned_keepalive(scroller) {
            Element::Item { data, children, .. } => (
                data.downcast_ref::<PrimCell<ScrollViewPrim>>().expect("scroll_view").take().axis,
                children,
            ),
            _ => panic!("the surface's content is the scroller"),
        };
        (surface_rules, axis, grid_cells(inner.pop().expect("the scroller wraps the table")))
    }

    fn surface_rules_of(style: Option<runtime_vocabulary::StyleProp>) -> Rc<StyleRules> {
        match style.expect("surface carries the Table sheet") {
            runtime_vocabulary::StyleProp::Sheet(app) => runtime_core::resolve_style(&app),
            _ => panic!("the Table sheet is a static application"),
        }
    }

    fn z_of(rules: &StyleRules) -> Option<i32> {
        rules.z_index
    }

    /// CrewForge: a header row could not stay put while the rows scroll.
    /// `scroll_y` pins the leading head row to the top of the table's own
    /// scroller, and with `scroll_x` the corner (a frozen head cell) pins
    /// on both axes, stacked above the header row, which is stacked above
    /// the frozen body column. Body cells take no top pin.
    #[test]
    fn regression_scroll_y_pins_the_header_row_and_stacks_the_corner_on_top() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![
                    row_of(vec![pinned_head("Name", ColumnPin::Left), head_cell("Hours")]),
                    row_of(vec![pinned_body("Ana", ColumnPin::Left), body_cell("8")]),
                ],
                scroll_x: true,
                scroll_y: true,
                ..Default::default()
            });
            let (_, axis, cells) = scrolling_parts(t);
            assert_eq!(axis, runtime_core::ScrollAxis::Both, "one two-axis scroller");
            let rules: Vec<_> = cells
                .into_iter()
                .map(|c| {
                    let app = cell_app(c);
                    assert!(app.preminted_class_list().is_some(), "the header pin premints");
                    runtime_core::resolve_style(&app)
                })
                .collect();
            let (corner, head, frozen, body) = (&rules[0], &rules[1], &rules[2], &rules[3]);

            assert_eq!(corner.position, Some(runtime_core::Position::Sticky));
            assert_eq!(px_of(&corner.top), Some(0.0), "the corner pins to the top");
            assert_eq!(px_of(&corner.left), Some(0.0), "…and to the left");
            assert_eq!(head.position, Some(runtime_core::Position::Sticky));
            assert_eq!(px_of(&head.top), Some(0.0), "the header pins to the top");
            assert!(head.left.is_none(), "an unfrozen header cell scrolls sideways");
            assert!(frozen.top.is_none(), "a body cell never pins to the top");
            assert_eq!(px_of(&frozen.left), Some(0.0), "the frozen column keeps pinning left");
            assert_eq!(body.position, None, "a plain body cell stays in flow");

            let z = |r: &StyleRules| r.stacking_z();
            assert!(z(frozen) < z(head), "rows slide under the header row");
            assert!(z(head) < z(corner), "the header row slides under the corner");
            assert!(z(body) < z(frozen), "body cells slide under the frozen column");
            assert_eq!(z_of(corner), Some(3));
        });
    }

    /// Without `scroll_y` nothing pins to the top — the header scrolls
    /// with the page, as before — and a scroll-y table without `scroll_x`
    /// owns a vertical scroller.
    #[test]
    fn header_pins_only_in_a_scroll_y_table() {
        with_test_world(|| {
            let still = Table(TableProps {
                children: vec![row_of(vec![head_cell("Name")]), row_of(vec![body_cell("Ana")])],
                ..Default::default()
            });
            let head = rules_of(grid_cells(still).into_iter().next().unwrap());
            assert!(head.top.is_none() && head.position.is_none(), "{head:?}");

            let t = Table(TableProps {
                children: vec![row_of(vec![head_cell("Name")]), row_of(vec![body_cell("Ana")])],
                scroll_y: true,
                ..Default::default()
            });
            let (_, axis, cells) = scrolling_parts(t);
            assert_eq!(axis, runtime_core::ScrollAxis::Vertical);
            let head = rules_of(cells.into_iter().next().unwrap());
            assert_eq!(px_of(&head.top), Some(0.0));
        });
    }

    /// Only the LEADING head rows pin: a row of head cells after the body
    /// (a totals row built from head cells) scrolls with the rows.
    #[test]
    fn only_the_leading_head_rows_pin() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![
                    row_of(vec![head_cell("Name")]),
                    row_of(vec![head_cell("Group")]),
                    row_of(vec![body_cell("Ana")]),
                    row_of(vec![head_cell("Total")]),
                ],
                scroll_y: true,
                ..Default::default()
            });
            let (_, _, cells) = scrolling_parts(t);
            let tops: Vec<_> = cells.into_iter().map(|c| px_of(&rules_of(c).top)).collect();
            assert_eq!(tops, vec![Some(0.0), Some(0.0), None, None]);
        });
    }

    /// `scroll_y` makes the surface take its parent's height (grow, and
    /// shrink below its rows) and `max_height` caps it; `max_height`
    /// without `scroll_y` does nothing.
    #[test]
    fn scroll_y_surface_fills_and_max_height_caps_it() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![row_of(vec![body_cell("Ana")])],
                scroll_y: true,
                max_height: Some(320.0),
                ..Default::default()
            });
            let (surface, _, _) = scrolling_parts(t);
            assert_eq!(surface.flex_grow.as_ref().map(|g| g.resolve()), Some(1.0));
            assert_eq!(px_of(&surface.min_height), Some(0.0), "may shrink below its rows");
            assert_eq!(px_of(&surface.max_height), Some(320.0));

            let unscrolled = Table(TableProps {
                children: vec![row_of(vec![body_cell("Ana")])],
                max_height: Some(320.0),
                footer_slot: Some(runtime_core::text("x".to_string()).into_element()),
                ..Default::default()
            });
            let surface = match peel_owned_keepalive(unscrolled) {
                Element::Item { data, .. } => data
                    .downcast_ref::<runtime_vocabulary::prims::PrimCell<runtime_vocabulary::prims::ViewPrim>>()
                    .unwrap()
                    .take(),
                _ => panic!("surface"),
            };
            let rules = surface_rules_of(surface.style);
            assert!(rules.max_height.is_none(), "max_height is a scroll-y setting");
            assert!(rules.flex_grow.is_none());
        });
    }

    /// CrewForge: cell padding was fixed, so the row dividers could not run
    /// the full width while the text kept a page margin. `inset_x` pads
    /// the outer side of each row's FIRST and LAST cell only — the cells
    /// between keep the base padding — and a single-cell row takes both.
    #[test]
    fn regression_inset_x_pads_only_the_edge_cells() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![
                    row_of(vec![head_cell("A"), head_cell("B"), head_cell("C")]),
                    row_of(vec![body_cell("1"), body_cell("2"), body_cell("3")]),
                    row_of(vec![TableCell(TableCellProps {
                        span: ColSpan::Rest,
                        text: Reactive::Static(Some("Nobody yet".into())),
                        ..Default::default()
                    })]),
                ],
                inset_x: TableInset::Xl,
                ..Default::default()
            });
            let lg = px_of(&rules_of(body_cell("x")).padding_left).expect("base cell padding");
            let mut pads = Vec::new();
            // (No premint assertion here: the spanning row switches the grid
            // to explicit placement, which rides an override on every cell.)
            for c in grid_cells(t) {
                let app = cell_app(c);
                let r = runtime_core::resolve_style(&app);
                pads.push((px_of(&r.padding_left).unwrap(), px_of(&r.padding_right).unwrap()));
            }
            let xl = pads[0].0;
            assert!(xl > lg, "the xl inset is wider than the base padding ({xl} vs {lg})");
            assert_eq!(
                pads,
                vec![(xl, lg), (lg, lg), (lg, xl), (xl, lg), (lg, lg), (lg, xl), (xl, xl)],
                "first cell: left inset; last cell: right inset; middle: neither"
            );
        });
    }

    /// An exact inset rides an override on the edge cells (and the frame
    /// slots take the same inset on both sides, so slot content lines up
    /// with the first column's text).
    #[test]
    fn inset_x_px_is_exact_and_reaches_the_slots() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![row_of(vec![body_cell("1"), body_cell("2")])],
                inset_x: TableInset::Px(28.0),
                header_slot: Some(runtime_core::text("note".to_string()).into_element()),
                ..Default::default()
            });
            let mut surface = match peel_owned_keepalive(t) {
                Element::Item { children, .. } => children,
                _ => panic!("surface"),
            };
            let holder = surface.pop().expect("grid holder");
            let slot = surface.pop().expect("header slot");
            let cells = grid_cells(holder);
            let r0 = rules_of(cells.into_iter().next().unwrap());
            assert_eq!(px_of(&r0.padding_left), Some(28.0));
            let slot_rules = match classify(slot) {
                P::View { style, .. } => runtime_core::resolve_style(&style.unwrap().application()),
                _ => panic!("slot is a view"),
            };
            assert_eq!(px_of(&slot_rules.padding_left), Some(28.0));
            assert_eq!(px_of(&slot_rules.padding_right), Some(28.0));
        });
    }

    /// A token inset is an axis selection, so the edge cells stay
    /// preminted (build-time CSS per arm).
    #[test]
    fn token_inset_keeps_edge_cells_preminted() {
        with_test_world(|| {
            let t = Table(TableProps {
                children: vec![row_of(vec![head_cell("A"), head_cell("B")]), row_of(vec![body_cell("1"), body_cell("2")])],
                inset_x: TableInset::Xxl,
                ..Default::default()
            });
            for c in grid_cells(t) {
                assert!(cell_app(c).preminted_class_list().is_some());
            }
        });
    }
}
