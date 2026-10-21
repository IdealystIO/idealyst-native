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
    // selection above; and only on pinned cells, so the rest stay static.
    // A reactive cell is judged by the `pinned` arm it selects now — a
    // column's pin is fixed for the cell's lifetime.
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
        ..Default::default()
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
