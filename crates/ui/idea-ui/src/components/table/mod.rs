//! `Table` — themed wrapper over the `table` SDK, and the suite an app
//! builds every data table from: a column model, resizable columns, a
//! selection column, drag-to-reorder, sortable headers, an empty state,
//! a pager footer, and a card form for phones.
//!
//! ```ignore
//! let widths = signal(saved_widths);           // in the SCREEN, not the table's rebuild
//! let selected = signal(BTreeSet::<RowId>::new());
//! ui! {
//!     Table(
//!         scroll_x = true,
//!         scroll_y = true,
//!         variant = TableVariant::Plain,
//!         inset_x = TableInset::by_breakpoint(TableInset::Px(16.0), TableInset::Px(24.0), TableInset::Px(28.0)),
//!         columns = vec![
//!             TableColumn::new("name").label("Name").width(220.0).resizable().pinned(ColumnPin::Left),
//!             TableColumn::new("notes").label("Notes").fill().truncate(),
//!             TableColumn::new("hours").label("Hours").width(80.0).align(CellAlign::Right),
//!             TableColumn::actions("menu"),
//!         ],
//!         column_widths = Some(widths),
//!         on_column_resize = Some(save_width),
//!         selection = Some(selected),
//!         on_reorder = Some(reorder),
//!         empty = Some("Nobody yet".into()),
//!         footer_slot = Some(ui! { TablePager(offset = offset, page_size = size, total = total, noun = "people".to_string(), on_offset = go) }),
//!     ) {
//!         TableRow {
//!             TableCell(header = true, text = Some("Name".to_string()), sort = rx!(sort_of("name")), on_sort = Some(sort_by("name")))
//!             TableCell(header = true, text = Some("Notes".to_string()))
//!             TableCell(header = true, text = Some("Hours".to_string()))
//!             TableCell(header = true)
//!         }
//!         for p in people {
//!             TableRow(id = Some(p.id.clone().into()), tone = p.tone(), on_row_click = Some(open(p.id.clone()))) {
//!                 TableCell(text = Some(p.name.clone()))
//!                 TableCell(text = Some(p.notes.clone()))
//!                 TableCell(text = Some(p.hours.clone()), tone = p.hours_tone())
//!                 TableCell { RowMenu(id = p.id.clone()) }
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
//!   `max_height`, `inset_x` / `inset_left` / `inset_right`, `density`,
//!   the frame slots — and the column model and the columns it adds
//!   (`columns`, `column_widths`, `on_column_resize`, `selection`,
//!   `on_reorder`, `on_group_reorder`, `grip_on_hover`, `empty`).
//! - [`TableRow`] is a `<tr>`: `on_row_click` for a clickable row,
//!   `tone` for a whole-row tint, `density` for its own padding, `bind_to`
//!   for the row's handle, and the row's part in the table's columns —
//!   `id`, `selectable`, `draggable`, `reorder_group`.
//! - [`TableCell`] wraps `<td>` (or `<th>` when `header = true`) with
//!   the cell-level padding + row divider, and wraps cell contents in
//!   a themed `text` node so values without explicit Typography pick
//!   up the right column treatment. Per-cell column settings live here
//!   too — `width`, `min_width`, `max_width`, `flex`, `align`, `truncate`,
//!   `span`, `pinned`, `padding` — along with a value's `tone` and a
//!   header's `sort`.
//! - [`TableFooter`] holds footer rows (`<tfoot>` on web).
//! - [`TableGroup`] holds a group of rows under a band row — see "Row
//!   groups" below.
//!
//! Beside them: [`TablePager`] (the paged-table footer) and
//! [`TableCards`] (the narrow form, from the same column specs).
//!
//! See [Table] / [TableRow] / [TableCell] / [TableFooter] for the full
//! prop surface.
//!
//! # The column model
//!
//! `Table(columns = vec![TableColumn::new("name")…, …])` puts each
//! column's settings on the TABLE: column `i` of the list is the `i`-th
//! column the author wrote, and the table applies its spec to every cell
//! of the column, header and body alike — exact `width`, `min_width`,
//! `max_width`, `fill` (take the spare width), `resizable`, `pinned`,
//! `align`, `truncate`, `padding`, and for the card form a `label` and a
//! `card` slot. The cells carry none of it, so a header and its body
//! cells can never disagree about a column. The columns the table adds
//! (selection, grip) come before the author's and are never counted.
//!
//! - **Resizing.** A `resizable` column's header carries a drag handle on
//!   its trailing edge, revealed while the header is hovered. Dragging
//!   resizes the whole column live — header and every body cell, nothing
//!   else re-styles — within the column's `min_width` (default
//!   [`COLUMN_RESIZE_MIN_PX`]) and `max_width`. Release reports
//!   `on_column_resize(id, Some(width))`; a double-click resets the
//!   column to its spec width and reports `None`. The live widths live in
//!   `column_widths` — a `Signal<ColumnWidths>` the screen owns and seeds
//!   from its saved layout, so a resized column keeps its width when the
//!   table is rebuilt from a new data snapshot. An entry overrides the
//!   spec's `width`; a column with neither keeps its natural width.
//! - **Frozen columns** are inset automatically: a pinned column clears
//!   the widths of the pinned columns between it and its edge (give each
//!   a `width`), and only the innermost draws the frozen block's edge.
//! - **The actions column.** [`TableColumn::actions`] is the trailing row
//!   menu column: [`ACTIONS_COLUMN_WIDTH_PX`] wide, frozen right, compact
//!   icon padding (`CellPadding::Icon`), centred. Each row's last cell is
//!   just `TableCell { IconButton(…) }`. Without a column model the same
//!   cell is `TableCell(pinned = Some(ColumnPin::Right), width = …,
//!   padding = CellPadding::Icon)`.
//!
//! # Selection, reorder, empty state
//!
//! - `Table(selection = Some(set))` adds a leading checkbox column. The
//!   header box is tri-state over the VISIBLE rows — those the table was
//!   built with whose `TableRow` has an `id` and is `selectable` — and
//!   selects or clears exactly those, keeping ids from other pages. The
//!   set is a `Signal<BTreeSet<RowId>>` the screen owns.
//! - `Table(on_reorder = Some(cb))` adds a leading grip column and makes
//!   draggable body rows reorderable (`dnd` SDK, scroll-aware pickup). The
//!   lifted row rides above the rows it crosses while they slide aside to
//!   open the gap; `cb(from, to)` gets ordinals over the draggable rows
//!   ([`move_in_place`] is the matching list move). A row lands only
//!   among rows with its `reorder_group`. The lift is the cell sheets'
//!   `drag` axis, which sorts before `pinned` — a frozen column stays
//!   frozen through the drag.
//! - The drag starts on the grip only: the row's other cells keep their
//!   own presses, so a clickable first cell (a name that opens the
//!   record, a link) works on a reorderable row.
//! - `Table(grip_on_hover = true)` shows each grip only while its row is
//!   hovered (transparent, still grabbable, otherwise); where the primary
//!   pointer cannot hover, the grips stay shown.
//! - Both added columns freeze with the first author column when it is
//!   pinned left, at their fixed widths (the leading one grown by the
//!   edge inset — see "Edge inset"), and take each row's tone, hover and
//!   click like the row's own cells.
//! - `Table(empty = Some("Nobody yet".into()))` — with no body rows, the
//!   table keeps its header and adds one row: empty cells under the added
//!   columns, then the message (muted) spanning every author column.
//!
//! # Row groups
//!
//! `TableGroup(title = …) { TableRow … }` groups rows under a band row —
//! a settings screen by category, a crew by heading:
//!
//! - **The band** spans the table in `RowTone::Group` (the header band's
//!   neutral tint), with the title (or any `header` content), a collapse
//!   toggle when the group is collapsible, and trailing `actions`. In a
//!   `scroll_x` table the title stays pinned to the scroller's left edge
//!   (past the frozen columns the table added) and the actions to its
//!   right edge while the columns scroll beneath them. `density` sets the
//!   band's own padding.
//! - **Collapsing**: `collapsed = Some(signal)` adds the toggle (announced
//!   as expanded / collapsed). A collapsed group's rows keep their place
//!   but take no height — nothing is rebuilt — and a column model keeps
//!   the columns still while rows hide.
//! - **Accessibility**: each group is a row group (`<tbody
//!   role="rowgroup">` on web) named by its title.
//! - **Reorder**: a group's rows reorder among themselves (the group's
//!   `id` is their `reorder_group`). `Table(on_group_reorder = Some(cb))`
//!   gives each draggable group's band a grip; dragging it moves the band
//!   with its rows while the other groups slide aside, and `cb(from, to)`
//!   gets ordinals over the draggable groups.
//! - A row's own `TableRow(density = Some(…))` beats the table's.
//!
//! # Sort headers and cell tones
//!
//! `TableCell(header = true, sort = Some(SortState::Asc), on_sort =
//! Some(cb))` draws the direction beside the label (a faint up-down pair
//! for a sortable column that is not the sort), makes the header a press
//! target, and calls `cb(SortState::next(current))`. The sorting is the
//! app's. `TableCell(tone = CellTone::Danger)` colours one value (an
//! overdue date) on the cell and its text; a whole row takes
//! `TableRow(tone = …)`.
//!
//! # Paged tables and the narrow form
//!
//! [`TablePager`] is the footer of a paged table — "26–50 of 312
//! projects", a "Rows" picker and the page numbers. Put it in
//! `footer_slot`: inside the frame, outside the scroller (pinned under a
//! `scroll_y` table's rows), at the table's edge inset. `inset_x` is
//! reactive — [`TableInset::by_breakpoint`] re-pads the edge cells in
//! place as the page margin steps, so the table never rebuilds on a
//! resize. A `scroll_y` table's scroller shows a vertical scrollbar only
//! while its rows overflow (it scrolls on demand); reserving a gutter is
//! a page-level choice, not the table's.
//!
//! Below `Sm` a table becomes [`TableCards`]: a virtualized list of
//! cards laid out from the same column specs (title, opposite, labelled
//! cells up to a cap, the rest behind "+N more"), with measured heights
//! and load-on-scroll. See its module docs for the switch.
//!
//! # Per-cell columns: width, alignment, truncation
//!
//! Without a column model, a column's settings go on EVERY cell of it,
//! header included — header and body cells that disagree about a width
//! break the column on both lowerings. (The column model above does this
//! for you.)
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
//! scroller edge — a `pinned` axis on the cell slots
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
//! rows scroll inside the table's own height and the header stays put
//! above them. Bound it with `max_height = Some(px)`, or let it fill a
//! bounded parent (without `max_height` a `scroll_y` table takes the
//! height its parent column gives it, and is as tall as its rows in an
//! unbounded one).
//!
//! The header rows lay out OUTSIDE the rows' scroller, above it, so the
//! vertical scrollbar runs beside the rows only — under the header, not
//! beside it. The header keeps the rows' column tracks (it measures and
//! follows them, including a resize drag and a fill column) and, with
//! `scroll_x` too, their horizontal scroll: scrolling the rows sideways
//! moves the header with them, and scrolling over the header moves the
//! rows. Frozen columns stay frozen in both, and a frozen header cell —
//! the corner — is painted above the header cells sliding beneath it.
//! The header's widths as first laid out are its columns' floors, so a
//! header label is never cut by a narrower column.
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
//! `inset_left` / `inset_right` pad one edge only, overriding `inset_x` on
//! that side (a side left at `TableInset::None` follows `inset_x`):
//! `Table(inset_left = TableInset::Px(16.0))` lines the text up with the
//! page margin and leaves the trailing edge alone.
//!
//! An author's icon column at an edge — [`TableColumn::actions`], any
//! `CellPadding::Icon` cell — keeps its compact padding: its width is
//! sized for its icon, so the inset is not added to it. The column the
//! table adds at the leading edge (selection or grip) does take the
//! inset, so its box starts at the page margin with the text below it:
//! its track grows by what the inset adds over its compact padding, so
//! the box keeps its room, and the frozen columns and band titles past
//! it move by the same amount. A live inset re-sizes it in place.
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
//! # Building your own row interaction
//!
//! For an interaction the table does not ship, the `table` SDK exposes
//! the HANDLES it is built from: `TableRow(bind_to = Some(r))` (or
//! `table::bind_row`) for a row's proxy surface — the `<tr>` on web, a
//! row-spanning backdrop on native — for geometry and anchoring;
//! `table::visit_row_cells` + `table::set_cell_touch` to fan a recognizer
//! across a row's cells (row-level touch must live per cell);
//! `table::bind_cell` for per-cell animated offsets; and
//! `table::map_cell_style` to select themed axes over whatever a cell
//! already carries (the cell sheets ship `dragging` / `drop_target`
//! arms for a custom drag).
//!
//! # Layering
//!
//! Mirrors `Spinner` → `activity_indicator` and `Switch` → `toggle`:
//! the underlying primitive (here, the `table` SDK that emits real
//! HTML `<table>` on web) is generic and cross-platform; idea-ui
//! supplies the opinionated visual that reads the active theme.

use std::collections::BTreeSet;
use std::rc::Rc;

use runtime_core::{
    component, signal, tap, text as text_node, ui, ChildList, Element, IdealystSchema,
    IntoElement, Length, Reactive, Ref, Signal, StyleApplication, StyleRules, TapRecognizer,
    Tokenized, VariantEnum, ViewHandle,
};
use runtime_vocabulary::StyleProp;


mod cards;
mod columns;
mod group;
mod pager;
mod reorder;
mod rows;
mod select;


pub use cards::{TableCards, TableCardsProps, CARD_CELL_CAP, CARD_ESTIMATE_PX, CARD_GAP_PX, CARD_LOAD_AHEAD_PX};
pub use columns::{
    dragged_width, CardSlot, CellPadding, ColumnId, ColumnResizeCallback, ColumnWidths,
    TableColumn, ACTIONS_COLUMN_WIDTH_PX, COLUMN_RESET_DOUBLE_CLICK_NS, COLUMN_RESIZE_MIN_PX,
};
pub use group::{TableGroup, TableGroupProps};
pub use pager::{page_window_text, TablePager, TablePagerProps};
pub use reorder::{move_in_place, slide_offset, GRIP_COLUMN_WIDTH_PX};
pub use rows::RowId;
pub use select::{select_all_state, toggle_select_all, SelectAllState, SELECT_COLUMN_WIDTH_PX};

use columns::ColumnModel;
use reorder::{GroupReorder, Reorder, GRIP_ICON_PX, GRIP_MARK};
mod styles;
pub use styles::*;

/// A whole-row tint for [`TableRow`] — the body cells' `tone` axis. Composes
/// with the clickable-row hover and with frozen columns (see the module
/// docs). An app adds its own tint with `TableTone::define("…")` and a
/// restyle arm on `body_cell`.
pub type RowTone = TableTone;
use rows::{decorate_added_cell, row_meta, RowInteraction, RowMeta};
use select::{select_all_box, select_row_box};
use crate::components::icon::Icon;
pub use table::ColSpan;
use table::{table as sdk_table, table_cell as sdk_cell, table_foot as sdk_foot, table_row as sdk_row};
use table::{
    TableCellProps as SdkTableCellProps, TableFootProps as SdkTableFootProps,
    TableProps as SdkTableProps, TableRowProps as SdkTableRowProps,
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

/// One value's text colour in a [`TableCell`] — an overdue date, a
/// failed check — without tinting the row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema, runtime_core::Remote)]
pub enum CellTone {
    /// The table's body text colour.
    #[default]
    None,
    /// Secondary — a placeholder dash, an empty value.
    Muted,
    /// Good — passed, paid, on time.
    Success,
    /// Needs a look — due soon.
    Warning,
    /// In trouble — overdue, failed.
    Danger,
}

// The variant string IS the stylesheet arm the value selects.
impl VariantEnum for CellTone {
    fn as_variant_str(self) -> &'static str {
        match self {
            CellTone::None => "none",
            CellTone::Muted => "muted",
            CellTone::Success => "success",
            CellTone::Warning => "warning",
            CellTone::Danger => "danger",
        }
    }

    fn all_variants() -> &'static [Self] {
        &[CellTone::None, CellTone::Muted, CellTone::Success, CellTone::Warning, CellTone::Danger]
    }
}

/// A sortable header's state — see [`TableCellProps::sort`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema, runtime_core::Remote)]
pub enum SortState {
    /// Sortable, not the sort column.
    #[default]
    None,
    /// Sorted ascending.
    Asc,
    /// Sorted descending.
    Desc,
}

impl SortState {
    /// What a press on the header asks for: ascending first, then it
    /// flips between ascending and descending.
    pub fn next(self) -> SortState {
        match self {
            SortState::Asc => SortState::Desc,
            SortState::None | SortState::Desc => SortState::Asc,
        }
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
    /// A page margin that steps with the breakpoint: `base` below `Md`,
    /// `md` from `Md`, `lg` from `Lg` up. Pass it as `Table(inset_x = …)`:
    /// crossing a step re-pads the table's edge cells in place, so the
    /// table (and anything it holds — a selection, a scroll position)
    /// never rebuilds for a window resize.
    ///
    /// ```ignore
    /// Table(inset_x = TableInset::by_breakpoint(TableInset::Px(16.0), TableInset::Px(24.0), TableInset::Px(28.0)))
    /// ```
    pub fn by_breakpoint(base: TableInset, md: TableInset, lg: TableInset) -> Reactive<TableInset> {
        let bp = runtime_core::current_breakpoint();
        Reactive::Dynamic(Rc::new(move || {
            let bp = bp.get();
            if bp.is_at_least(runtime_core::Breakpoint::Lg) {
                lg
            } else if bp.is_at_least(runtime_core::Breakpoint::Md) {
                md
            } else {
                base
            }
        }))
    }

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
    /// the inline layer (still preminted: the classes ship, the length is
    /// an inline style).
    fn apply(self, app: StyleApplication, side: InsetSide) -> StyleApplication {
        if let Some(arm) = self.arm() {
            return app.with(side.axis(), arm);
        }
        match self {
            TableInset::Px(v) => {
                let len = Some(Tokenized::Literal(Length::Px(v)));
                app.with_inline_merged(match side {
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

/// What a [`Table`] with no body rows says, in one full-width row under
/// its header — see [`TableProps::empty`]. Text takes the muted body
/// style; an element is laid out like a cell's rich children.
pub enum TableEmpty {
    /// A sentence, in the muted body text style.
    Text(String),
    /// Any content — a sentence with a link, an action.
    Content(Element),
}

impl From<&str> for TableEmpty {
    fn from(s: &str) -> Self {
        TableEmpty::Text(s.to_string())
    }
}

impl From<String> for TableEmpty {
    fn from(s: String) -> Self {
        TableEmpty::Text(s)
    }
}

impl From<Element> for TableEmpty {
    fn from(e: Element) -> Self {
        TableEmpty::Content(e)
    }
}

/// Themed table container. Wraps the `table` SDK's `<table>` with
/// idea-ui's surface tokens (rounded corners + hairline border + theme
/// background). Pass `TableRow`s (and optionally a `TableFooter`) as
/// children.
#[runtime_core::props(styles = TableStyles)]
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
    /// height under a header that stays put above them, with the
    /// scrollbar beside the rows only. Bound it with `max_height`, or let
    /// it fill a bounded parent. With `scroll_x` the header follows the
    /// rows' horizontal scroll and frozen columns stay frozen in both —
    /// see the module docs.
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
    /// the text keeps a page margin — see [`TableInset`]. Reactive: a
    /// page margin that steps with the breakpoint re-pads the edge cells
    /// in place ([`TableInset::by_breakpoint`]) — the table never
    /// rebuilds for it.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub inset_x: TableInset,
    /// The outer padding of every row's FIRST cell only (and the frame
    /// slots' leading edge), overriding `inset_x` on that side —
    /// `inset_left = TableInset::Px(16.0)` alone lines the text up with
    /// the page margin and leaves the trailing edge (an actions column)
    /// at its own padding. `TableInset::None` (the default) follows
    /// `inset_x`. Reactive, like `inset_x`.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub inset_left: TableInset,
    /// The outer padding of every row's LAST cell only (and the frame
    /// slots' trailing edge), overriding `inset_x` on that side. See
    /// `inset_left`.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub inset_right: TableInset,
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
    /// scroller — a pager ([`TablePager`]), an "Add entry" action. Pinned
    /// under the rows of a `scroll_y` table.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub footer_slot: Option<Element>,
    /// The column model: column `i` describes the `i`-th column the
    /// author wrote (the selection and grip columns the table adds are
    /// not counted). The table applies each spec to every cell of its
    /// column — width, floor, cap, fill, resize handle, frozen edge,
    /// alignment, truncation, padding — so the cells carry none of it.
    /// Empty (the default): each cell's own props size its column.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub columns: Vec<TableColumn>,
    /// The table's live column widths, by column id. A resize drag writes
    /// the dragged column's entry on every pointer move; an entry
    /// overrides the column spec's `width`. Create it in the SCREEN (not
    /// inside whatever rebuilds the table) and seed it with a saved
    /// layout, so a resized column keeps its width across rebuilds. When
    /// `None` the table keeps its own (lost on a rebuild).
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub column_widths: Option<Signal<ColumnWidths>>,
    /// Fired when a resize drag is released, with the column and its new
    /// width (whole px) — save it — and with `None` when a double-click
    /// resets the column to its spec width.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub on_column_resize: Option<Rc<dyn Fn(ColumnId, Option<f32>)>>,
    /// Adds a leading selection column: a tri-state header box that
    /// selects every visible row, and a box per row with an `id` that is
    /// `selectable`. The set is the screen's; the table only reads and
    /// writes it. Frozen with the first column when that one is pinned.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub selection: Option<Signal<BTreeSet<RowId>>>,
    /// Makes body rows draggable: adds a leading grip column (frozen with
    /// the first column when that one is pinned), lifts the dragged row
    /// above its neighbours while the others slide to open the gap, and
    /// calls this with `(from, to)` — ordinals over the draggable body
    /// rows — on drop. See [`move_in_place`].
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub on_reorder: Option<Rc<dyn Fn(usize, usize)>>,
    /// Makes [`TableGroup`]s draggable as a whole: each draggable group's
    /// band gets a grip (the grip column is added if `on_reorder` has not
    /// already), dragging it moves the band with its rows while the other
    /// groups slide aside, and this is called with `(from, to)` — ordinals
    /// over the draggable groups — on drop. Rows outside a group stay put.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub on_group_reorder: Option<Rc<dyn Fn(usize, usize)>>,
    /// Show each row's drag grip only while the row is hovered (and while
    /// it is dragged). Where the primary pointer cannot hover (a touch
    /// screen) the grips stay shown. The hidden grip is still grabbable.
    #[prop(static)]
    pub grip_on_hover: bool,
    /// What a table with no body rows says: one row under the header
    /// spanning every column (past the selection and grip columns).
    /// `Some("No crew yet".into())`, or any element. Decided from the
    /// rows the table is built with.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub empty: Option<TableEmpty>,
}

/// What a row is to its table, by position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    /// A leading row of head cells — pinned by `scroll_y`.
    Head,
    /// A data row.
    Body,
    /// A row of a `TableFooter`.
    Foot,
    /// The empty-state row the table added.
    Empty,
}

/// A column the table adds before the author's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Builtin {
    Grip,
    Select,
}

impl Builtin {
    fn width(self) -> f32 {
        match self {
            Builtin::Grip => GRIP_COLUMN_WIDTH_PX,
            Builtin::Select => SELECT_COLUMN_WIDTH_PX,
        }
    }
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
    let style = TableStyles::frame()
        .overridden(props.styles.frame.as_ref())
        .into_style_application()
        .with("variant", frame)
        .with("scroll", if props.scroll_y { "fill" } else { "off" });
    let style = match props.max_height.filter(|_| props.scroll_y) {
        Some(h) => style.with_inline_merged(StyleRules {
            max_height: Some(Tokenized::Literal(Length::Px(h))),
            ..Default::default()
        }),
        None => style,
    };
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }

    // What each row is, in visit order (`table::visit_rows` over the
    // children): the LEADING rows of head cells are the header; a
    // footer section's rows are footer rows whatever they hold.
    let mut kinds: Vec<RowKind> = Vec::new();
    let mut metas: Vec<Option<Rc<RowMeta>>> = Vec::new();
    // Per row: its `TableGroup` (by order among the groups) and its place
    // in it (the band is 0).
    let mut groups: Vec<Option<(usize, usize)>> = Vec::new();
    let mut group_count = 0usize;
    {
        let mut head_open = true;
        for child in &children {
            let foot = table::is_footer(child);
            let group = table::is_group(child).then(|| {
                group_count += 1;
                group_count - 1
            });
            let mut in_group = 0usize;
            table::visit_rows(child, |row| {
                let kind = if foot {
                    RowKind::Foot
                } else if head_open && group.is_none() && row_is_head(row) {
                    RowKind::Head
                } else {
                    head_open = false;
                    RowKind::Body
                };
                kinds.push(kind);
                metas.push(row_meta(row));
                groups.push(group.map(|g| (g, in_group)));
                in_group += 1;
            });
        }
    }

    // Group drag: the groups whose band is draggable, in order, each with
    // its row count (band included).
    let group_reorder = props.on_group_reorder.map(|cb| {
        let mut sizes = vec![0usize; group_count];
        let mut draggable = vec![false; group_count];
        for ((g, meta), _) in groups.iter().zip(&metas).zip(&kinds) {
            if let Some((g, i)) = g {
                sizes[*g] += 1;
                if *i == 0 {
                    draggable[*g] = meta.as_ref().and_then(|m| m.band.as_ref()).is_some_and(|b| b.draggable);
                }
            }
        }
        let mut ordinal = vec![None; group_count];
        let mut kept = Vec::new();
        for g in 0..group_count {
            if draggable[g] {
                ordinal[g] = Some(kept.len());
                kept.push(sizes[g]);
            }
        }
        (GroupReorder::new(&kept, cb), ordinal)
    });
    let group_drag = |row_i: usize| -> Option<(usize, usize)> {
        let (reorder, ordinal) = group_reorder.as_ref()?;
        let _ = reorder;
        let (g, i) = groups.get(row_i).copied().flatten()?;
        ordinal[g].map(|o| (o, i))
    };

    // The columns the table adds before the author's, in order.
    let lead: Vec<Builtin> = [
        (props.on_reorder.is_some() || group_reorder.is_some()).then_some(Builtin::Grip),
        props.selection.is_some().then_some(Builtin::Select),
    ]
    .into_iter()
    .flatten()
    .collect();
    // The added columns freeze with the first author column.
    let lead_pinned = match props.columns.first() {
        Some(col) => col.pinned == Some(ColumnPin::Left),
        None => first_cell_pinned_left(&children),
    };
    // The first added column is the row's first cell, so it takes the
    // leading edge inset like any first cell; its track grows by what the
    // inset adds over its compact padding, so its icon keeps its room.
    // Every width and pin measured past it grows with it.
    let lead_inset = side_inset(&props.inset_left, &props.inset_x);
    let lead_growth: Reactive<f32> = match &lead_inset {
        _ if lead.is_empty() => Reactive::Static(0.0),
        Reactive::Static(TableInset::None) => Reactive::Static(0.0),
        Reactive::Static(i @ TableInset::Px(_)) => Reactive::Static(added_column_growth(*i)),
        // A token inset resolves through the theme, so its growth
        // follows the theme's spacing.
        Reactive::Static(i) => {
            let i = *i;
            Reactive::Dynamic(Rc::new(move || added_column_growth(i)))
        }
        Reactive::Dynamic(f) => {
            let f = f.clone();
            Reactive::Dynamic(Rc::new(move || added_column_growth(f())))
        }
    };
    // Added column `k`'s width, and its distance from the leading edge.
    let lead_width = |k: usize| grown(lead[k].width(), (k == 0).then_some(&lead_growth));
    let lead_offset = |k: usize| {
        let before: f32 = lead[..k].iter().map(|b| b.width()).sum();
        grown(before, (k > 0).then_some(&lead_growth))
    };
    let lead_pin_width: Reactive<f32> =
        if lead_pinned && !lead.is_empty() { lead_offset(lead.len()) } else { Reactive::Static(0.0) };

    // The column model on the author's cells (before the added columns
    // shift their positions).
    let widths = props.column_widths.unwrap_or_else(|| signal(ColumnWidths::new()));
    let model = ColumnModel::new(props.columns, widths, lead_pin_width.clone());
    if !model.is_empty() {
        let last_head = kinds.iter().rposition(|k| *k == RowKind::Head);
        let mut row_i = 0usize;
        for child in children.iter_mut() {
            table::visit_rows_mut(child, |row| {
                let resize_here = Some(row_i) == last_head;
                model.apply_to_row(row, resize_here.then(|| props.on_column_resize.clone()));
                row_i += 1;
            });
        }
    } else if !matches!(lead_pin_width, Reactive::Static(w) if w == 0.0) {
        // No column model: the author pinned cells with their own
        // `pin_offset`s, measured from the first author column. Push the
        // left-frozen ones past the added columns.
        for child in &children {
            table::visit_rows(child, |row| {
                table::visit_row_cells(row, |cell| match &lead_pin_width {
                    Reactive::Static(w) => {
                        let w = *w;
                        table::map_cell_application(cell, Rc::new(move |app| shift_left_pin(app, w)));
                    }
                    Reactive::Dynamic(f) => {
                        let f = f.clone();
                        table::map_cell_style(cell, Rc::new(move |app| shift_left_pin(app, f())));
                    }
                });
            });
        }
    }

    // The added columns: a grip and a selection box per row, as the row
    // calls for.
    let body: Vec<(bool, Option<RowId>)> = kinds
        .iter()
        .zip(&metas)
        .filter(|(k, _)| **k == RowKind::Body)
        .map(|(_, m)| match m {
            Some(m) => (m.draggable, m.group.clone()),
            None => (true, None),
        })
        .collect();
    let reorder = props.on_reorder.map(|cb| Reorder::new(&body, cb));
    let selection = props.selection;
    // A band's content pins past the frozen columns the table adds, and
    // takes the leading edge inset when its cell is the row's first.
    for meta in metas.iter().flatten() {
        if let Some(band) = &meta.band {
            let mut lead_of = band.lead.borrow_mut();
            lead_of.pin = lead_pin_width.clone();
            if lead.is_empty() {
                lead_of.inset = side_inset(&props.inset_left, &props.inset_x);
            }
        }
    }
    let grip_on_hover = props.grip_on_hover;
    if !lead.is_empty() {
        let visible: Rc<Vec<RowId>> = Rc::new(
            kinds
                .iter()
                .zip(&metas)
                .filter(|(k, _)| **k == RowKind::Body)
                .filter_map(|(_, m)| m.as_ref().filter(|m| m.selectable).and_then(|m| m.id.clone()))
                .collect(),
        );
        let first_head = kinds.iter().position(|k| *k == RowKind::Head);
        let (mut row_i, mut body_i) = (0usize, 0usize);
        for child in children.iter_mut() {
            table::visit_rows_mut(child, |row| {
                let kind = kinds[row_i];
                let meta = metas[row_i].clone();
                // The row's drag recognizer: a band's group drag, or the
                // row's own reorder.
                let grip_touch = match (kind, group_drag(row_i)) {
                    (RowKind::Body, Some((g, 0))) => {
                        group_reorder.as_ref().map(|(r, _)| r.grip_handler(g))
                    }
                    (RowKind::Body, _) => reorder.as_ref().and_then(|r| r.grip_handler(body_i)),
                    _ => None,
                };
                // A hover-revealed grip follows its row's pointer.
                let row_hover = (grip_on_hover && grip_touch.is_some()).then(|| signal(false));
                let mut added = Vec::with_capacity(lead.len());
                for (k, b) in lead.iter().enumerate() {
                    let content: Option<Element> = match (b, kind) {
                        (Builtin::Grip, RowKind::Body) if grip_touch.is_some() => Some(grip_mark(row_hover)),
                        (Builtin::Select, RowKind::Head) if Some(row_i) == first_head => {
                            selection.map(|s| select_all_box(s, visible.clone()))
                        }
                        (Builtin::Select, RowKind::Body) => meta
                            .as_ref()
                            .filter(|m| m.selectable)
                            .and_then(|m| m.id.clone())
                            .zip(selection)
                            .map(|(id, s)| select_row_box(s, id)),
                        _ => None,
                    };
                    let cell = builtin_cell(kind == RowKind::Head, lead_width(k), content);
                    let cell = if lead_pinned { pin_builtin(cell, lead_offset(k)) } else { cell };
                    let cell = if kind == RowKind::Foot {
                        table::map_cell_application(&cell, Rc::new(|app| app.with("section", "foot")));
                        cell
                    } else {
                        cell
                    };
                    let touch = match b {
                        Builtin::Grip => grip_touch.clone(),
                        _ => None,
                    };
                    let cell = match (&meta, kind) {
                        (Some(m), RowKind::Body | RowKind::Foot) => decorate_added_cell(cell, m, touch),
                        _ => match touch {
                            Some(t) => {
                                table::set_cell_touch(&cell, t);
                                cell
                            }
                            None => cell,
                        },
                    };
                    added.push(cell);
                }
                if let Some(cells) = table::row_cells_mut(row) {
                    for (i, cell) in added.into_iter().enumerate() {
                        cells.insert(i, cell);
                    }
                }
                if let Some(hovered) = row_hover {
                    table::visit_row_cells(row, |cell| {
                        table::set_cell_hover(cell, Rc::new(move |entering| hovered.set(entering)));
                    });
                }
                if kind == RowKind::Body {
                    body_i += 1;
                }
                row_i += 1;
            });
        }
    }

    // The empty state: one row past the added columns, spanning the rest.
    if let Some(empty) = props.empty.filter(|_| !kinds.contains(&RowKind::Body)) {
        let mut cells: Vec<Element> = Vec::with_capacity(lead.len() + 1);
        for k in 0..lead.len() {
            let cell = builtin_cell(false, lead_width(k), None);
            cells.push(if lead_pinned { pin_builtin(cell, lead_offset(k)) } else { cell });
        }
        cells.push(empty_cell(empty));
        children.push(sdk_row(SdkTableRowProps { children: cells }).into_element());
        kinds.push(RowKind::Empty);
        metas.push(None);
    }

    // Reorder: every body row, once its cells are all in place.
    if let Some(reorder) = &reorder {
        let (mut row_i, mut body_i) = (0usize, 0usize);
        for child in &children {
            table::visit_rows(child, |row| {
                if kinds[row_i] == RowKind::Body {
                    reorder.wire_row(body_i, row);
                    body_i += 1;
                }
                row_i += 1;
            });
        }
    }
    // Group drag: every row of a draggable group moves with it; every
    // other body row goes opaque behind it.
    if let Some((greorder, _)) = &group_reorder {
        let mut row_i = 0usize;
        for child in &children {
            table::visit_rows(child, |row| {
                match (kinds[row_i], group_drag(row_i)) {
                    (_, Some((g, i))) => greorder.wire_row(g, i, row),
                    (RowKind::Body, None) => greorder.wire_other_row(row),
                    _ => {}
                }
                row_i += 1;
            });
        }
    }

    // Density is a table-wide setting, but the cells are already built
    // when this body runs (the `ui!` children block builds them first),
    // so it is selected onto each built cell — the row's own density
    // (`TableRow(density = …)`) over the table's. A static selection keeps
    // a static sheet static — it still premints. `Standard` is every
    // sheet's default arm, so it needs no pass.
    {
        let mut row_i = 0usize;
        for child in &children {
            table::visit_rows(child, |row| {
                let density =
                    metas.get(row_i).cloned().flatten().and_then(|m| m.density).unwrap_or(props.density);
                if density != TableDensity::Standard {
                    let arm = density.as_variant_str();
                    table::visit_row_cells(row, |cell| {
                        table::map_cell_application(cell, Rc::new(move |app| app.with("density", arm)));
                    });
                }
                row_i += 1;
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
    // A scroll-y table pins its header rows. Static selection, so the
    // cells stay preminted.
    if props.scroll_y {
        let mut row_i = 0usize;
        for child in &children {
            table::visit_rows(child, |row| {
                if kinds[row_i] == RowKind::Head {
                    table::visit_row_cells(row, |cell| {
                        table::map_cell_application(
                            cell,
                            Rc::new(|app| app.with("sticky_header", "on")),
                        );
                    });
                }
                row_i += 1;
            });
        }
    }
    // Edge inset: the first and last cell of every row — a single-cell
    // row takes both sides. A static inset is a static selection (the
    // cells stay preminted); a live one re-pads the same cells in place.
    // An author's icon column at the edge (`CellPadding::Icon`, e.g. the
    // actions column) keeps its compact padding: its width is sized for
    // its icon, and the inset would squeeze the icon out of it. The added
    // column at the leading edge takes the inset: its track grew by the
    // difference (`lead_growth`), so its icon keeps its room.
    let inset_left = side_inset(&props.inset_left, &props.inset_x);
    let inset_right = side_inset(&props.inset_right, &props.inset_x);
    let pads = |i: &Reactive<TableInset>| !matches!(i, Reactive::Static(TableInset::None));
    if pads(&inset_left) || pads(&inset_right) {
        for child in &children {
            table::visit_rows(child, |row| {
                let mut count = 0usize;
                table::visit_row_cells(row, |_| count += 1);
                let mut i = 0usize;
                table::visit_row_cells(row, |cell| {
                    let (first, last) = (i == 0, i + 1 == count);
                    i += 1;
                    let added = first && !lead.is_empty();
                    if (!first && !last) || (is_icon_cell(cell) && !added) {
                        return;
                    }
                    if first && pads(&inset_left) {
                        apply_edge_inset(cell, &inset_left, InsetSide::Left);
                    }
                    if last && pads(&inset_right) {
                        apply_edge_inset(cell, &inset_right, InsetSide::Right);
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
                    table::map_cell_application(cell, Rc::new(|app| app.with("variant", "plain")));
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
                    let watch = table::cell_application(cell).is_some_and(|app| {
                        app.variants.0.contains_key("pinned")
                            && app.variants.0.get("pin_edge").map(String::as_str) != Some("interior")
                    });
                    if watch {
                        table::map_cell_style(cell, Rc::new(move |app| pin_edge(app, edges)));
                    }
                });
            });
        }
        edges
    });
    let slot_style = {
        let (left, right) = (inset_left.clone(), inset_right.clone());
        let slot_override = props.styles.frame_slot.clone();
        move |edge: &'static str| {
            let (left, right) = (left.clone(), right.clone());
            let slot_override = slot_override.clone();
            move || {
                let app = TableStyles::frame_slot()
                    .overridden(slot_override.as_ref())
                    .into_style_application()
                    .with("edge", edge);
                right.get().apply(left.get().apply(app, InsetSide::Left), InsetSide::Right)
            }
        }
    };
    let live_slots = matches!(inset_left, Reactive::Dynamic(_)) || matches!(inset_right, Reactive::Dynamic(_));
    let header_slot = props.header_slot.map(|slot| {
        let style = slot_style("top");
        slot_view(slot, live_slots, style)
    });
    let footer_slot = props.footer_slot.map(|slot| {
        let style = slot_style("bottom");
        slot_view(slot, live_slots, style)
    });
    // A scroll-y table's leading header rows form its head section: the
    // SDK lays them out above the rows' scroller, so the vertical
    // scrollbar runs beside the body only, under the header.
    if props.scroll_y {
        let mut row_i = 0usize;
        let mut lead = 0usize;
        for child in &children {
            let mut rows = 0usize;
            table::visit_rows(child, |_| rows += 1);
            let head = rows > 0 && (row_i..row_i + rows).all(|i| kinds.get(i) == Some(&RowKind::Head));
            if !head || table::is_footer(child) {
                break;
            }
            row_i += rows;
            lead += 1;
        }
        if lead > 0 {
            let rest = children.split_off(lead);
            let head = std::mem::replace(&mut children, rest);
            children.insert(0, table::table_head(table::TableHeadProps { children: head }));
        }
    }
    // In grid-column mode every column is one track: the added columns
    // at their widths (`lead_width`), the author's from the column model.
    let mut sdk_columns: Vec<Option<table::ColumnSpec>> = (0..lead.len())
        .map(|k| match lead_width(k) {
            Reactive::Static(w) => {
                Some(table::ColumnSpec::new(table::ColumnTrack { width: Some(w), ..Default::default() }))
            }
            Reactive::Dynamic(f) => Some(table::ColumnSpec::live(move || table::ColumnTrack {
                width: Some(f()),
                ..Default::default()
            })),
        })
        .collect();
    sdk_columns.extend(model.specs());
    // SDK's `table()` returns a `Bound<TableHandle>`; chain
    // `.with_style(...)` to land the themed style on its surface view,
    // then convert to Element.
    // The corner above a classic vertical scrollbar, beside the split
    // head's strip: the head cells' band and divider (a plain table's
    // opaque surface), no padding, growing into whatever the strip leaves
    // — so the header runs across the scrollbar instead of stopping short.
    let head_corner = props.scroll_y.then(|| {
        let app = TableStyles::head_cell()
            .overridden(props.styles.head_cell.as_ref())
            .into_style_application()
            .with("sticky_header", "on");
        let app = if plain { app.with("variant", "plain") } else { app };
        let zero = || Some(Tokenized::Literal(Length::Px(0.0)));
        let app = app.with_inline_merged(StyleRules {
            flex_grow: Some(Tokenized::Literal(1.0)),
            flex_basis: zero(),
            min_width: zero(),
            padding_left: zero(),
            padding_right: zero(),
            ..Default::default()
        });
        StyleProp::Sheet(Box::new(app))
    });
    let table = sdk_table(SdkTableProps {
        children,
        scroll_x: props.scroll_x,
        scroll_y: props.scroll_y,
        header_slot,
        footer_slot,
        columns: sdk_columns,
        head_corner,
    })
    .with_style(style);
    match edges {
        Some(edges) => table.on_scroll_edges(move |e| edges.set(e)).into_element(),
        None => table.into_element(),
    }
}

/// A frame slot's view: its sheet resolved once for a static inset (the
/// slot premints), followed live for a reactive one.
fn slot_view(slot: Element, live: bool, style: impl Fn() -> StyleApplication + 'static) -> Element {
    if live {
        ui! { view(style = style) { slot } }
    } else {
        let app = style();
        ui! { view(style = app) { slot } }
    }
}

/// One edge's inset: the side's own when it sets one, else `inset_x`.
fn side_inset(side: &Reactive<TableInset>, both: &Reactive<TableInset>) -> Reactive<TableInset> {
    match side {
        Reactive::Static(TableInset::None) => both.clone(),
        Reactive::Static(_) => side.clone(),
        Reactive::Dynamic(f) => {
            let (f, both) = (f.clone(), both.clone());
            Reactive::Dynamic(Rc::new(move || match f() {
                TableInset::None => both.get(),
                own => own,
            }))
        }
    }
}

/// How much an added column's track grows when it is the row's first
/// cell and takes `inset` as its leading padding in place of its compact
/// icon padding — so its content keeps the width it has without an inset.
/// Negative for an inset narrower than the icon padding.
fn added_column_growth(inset: TableInset) -> f32 {
    if inset == TableInset::None {
        return 0.0;
    }
    let pad = |app: StyleApplication| {
        match runtime_core::resolve_style(&app).padding_left.as_ref().map(|l| l.resolve()) {
            Some(Length::Px(v)) => v,
            _ => 0.0,
        }
    };
    let icon = TableStyles::body_cell().into_style_application().with("cell_padding", "icon");
    pad(inset.apply(icon.clone(), InsetSide::Left)) - pad(icon)
}

/// `base`, plus `growth` when given.
fn grown(base: f32, growth: Option<&Reactive<f32>>) -> Reactive<f32> {
    match growth {
        None => Reactive::Static(base),
        Some(Reactive::Static(g)) => Reactive::Static(base + g),
        Some(Reactive::Dynamic(f)) => {
            let f = f.clone();
            Reactive::Dynamic(Rc::new(move || base + f()))
        }
    }
}

/// Whether a built cell has the compact icon padding (`CellPadding::Icon`).
fn is_icon_cell(cell: &Element) -> bool {
    table::cell_application(cell)
        .is_some_and(|app| app.variants.0.get("cell_padding").map(String::as_str) == Some("icon"))
}

/// Pad one side of an edge cell with the table's inset — see
/// [`TableInset::apply`].
fn apply_edge_inset(cell: &Element, inset: &Reactive<TableInset>, side: InsetSide) {
    let pad = move |app: StyleApplication, i: TableInset| i.apply(app, side);
    match inset {
        Reactive::Static(i) => {
            let i = *i;
            table::map_cell_application(cell, Rc::new(move |app| pad(app, i)));
        }
        Reactive::Dynamic(f) => {
            let f = f.clone();
            table::map_cell_style(cell, Rc::new(move |app| pad(app, f())));
        }
    }
}

/// Whether the first row's first cell is frozen to the left — how a table
/// without a column model learns its added columns freeze with it.
fn first_cell_pinned_left(children: &[Element]) -> bool {
    let mut first_row = true;
    let mut pinned = false;
    for child in children.iter().filter(|c| !table::is_footer(c)) {
        table::visit_rows(child, |row| {
            if !first_row {
                return;
            }
            first_row = false;
            let mut first_cell = true;
            table::visit_row_cells(row, |cell| {
                if first_cell {
                    first_cell = false;
                    pinned = table::cell_application(cell)
                        .is_some_and(|app| app.variants.0.get("pinned").map(String::as_str) == Some("left"));
                }
            });
        });
    }
    pinned
}

/// Move a left-frozen cell's pin past `by` px of added columns, keeping
/// its own `pin_offset`. Any other cell is returned untouched.
fn shift_left_pin(app: StyleApplication, by: f32) -> StyleApplication {
    if app.variants.0.get("pinned").map(String::as_str) != Some("left") {
        return app;
    }
    let own = match app.inline().and_then(|r| r.left.as_ref()).map(|l| l.resolve()) {
        Some(Length::Px(v)) => v,
        _ => 0.0,
    };
    app.with_inline_merged(StyleRules {
        left: Some(Tokenized::Literal(Length::Px(own + by))),
        ..Default::default()
    })
}

/// A cell of a column the table adds: its width, centred, compact
/// padding.
fn builtin_cell(header: bool, width: Reactive<f32>, content: Option<Element>) -> Element {
    let width = match width {
        Reactive::Static(w) => Reactive::Static(Some(w)),
        Reactive::Dynamic(f) => Reactive::Dynamic(Rc::new(move || Some(f()))),
    };
    TableCell(TableCellProps {
        header,
        width,
        align: CellAlign::Center,
        padding: CellPadding::Icon,
        children: content.into_iter().collect(),
        ..Default::default()
    })
}

/// Freeze an added column's cell `offset` px from the left edge. Its inner
/// edge never draws: the author's first column, frozen beside it, draws
/// the frozen block's edge.
fn pin_builtin(cell: Element, offset: Reactive<f32>) -> Element {
    let left = |app: StyleApplication, offset: f32| {
        if offset == 0.0 {
            app
        } else {
            app.with_inline_merged(StyleRules {
                left: Some(Tokenized::Literal(Length::Px(offset))),
                ..Default::default()
            })
        }
    };
    let pinned = |app: StyleApplication| app.with("pinned", "left").with("pin_edge", "interior");
    match offset {
        Reactive::Static(offset) => {
            table::map_cell_application(&cell, Rc::new(move |app| left(pinned(app), offset)));
        }
        Reactive::Dynamic(f) => {
            table::map_cell_application(&cell, Rc::new(pinned));
            table::map_cell_style(&cell, Rc::new(move |app| left(app, f())));
        }
    }
    cell
}

/// The grip glyph a draggable row's grip cell shows — revealed only while
/// `hovered` holds when the table reveals grips on hover (always, where
/// the primary pointer cannot hover).
fn grip_mark(hovered: Option<Signal<bool>>) -> Element {
    let revealed: Option<Rc<dyn Fn() -> bool>> = hovered.map(|h| {
        let pointer = runtime_core::primary_pointer();
        Rc::new(move || h.get() || pointer.get() == runtime_core::PointerKind::Coarse) as Rc<dyn Fn() -> bool>
    });
    let style = group::grip_style(revealed);
    ui! {
        view(style = style) {
            Icon(data = GRIP_MARK, size = GRIP_ICON_PX, tone = Some(crate::tone::Neutral.into()))
        }
    }
}

/// The empty-state cell: everything past the added columns.
fn empty_cell(empty: TableEmpty) -> Element {
    match empty {
        TableEmpty::Text(text) => TableCell(TableCellProps {
            span: ColSpan::Rest,
            text: Reactive::Static(Some(text)),
            tone: Reactive::Static(CellTone::Muted),
            ..Default::default()
        }),
        TableEmpty::Content(content) => TableCell(TableCellProps {
            span: ColSpan::Rest,
            children: vec![content],
            ..Default::default()
        }),
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
#[derive(IdealystSchema)]
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
    /// The row's identity: what `Table(selection = …)` holds for it. A row
    /// without one has no selection box.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub id: Option<RowId>,
    /// Whether the row takes part in `Table(selection = …)` — `false`
    /// leaves its box out and select-all passes it by. Default `true`.
    #[prop(static)]
    pub selectable: bool,
    /// Whether the row can be dragged in a `Table(on_reorder = …)` —
    /// `false` (a group title, a locked row) gives it no grip and leaves
    /// it out of the reorder ordinals. Default `true`.
    #[prop(static)]
    pub draggable: bool,
    /// The group a draggable row reorders within: a row lands only among
    /// rows with the same group. `None` (the default) is one group.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub reorder_group: Option<RowId>,
    /// This row's padding, over the table's `density` — a denser band, a
    /// roomier summary row. `None` (the default) follows the table.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[prop(static)]
    pub density: Option<TableDensity>,
}

impl Default for TableRowProps {
    fn default() -> Self {
        Self {
            children: Vec::new(),
            on_row_click: None,
            tone: Reactive::Static(RowTone::None),
            bind_to: None,
            id: None,
            selectable: true,
            draggable: true,
            reorder_group: None,
            density: None,
        }
    }
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
    for cell in &children {
        rows::apply_row_tone(cell, &props.tone);
    }

    // A clickable row shares ONE hover flag across all its cells so
    // hovering any cell highlights the whole row. The cells arrive here
    // already built (the `ui!` children block builds them before the row
    // runs), so we post-process each: layer the reactive hover background
    // + pointer cursor onto its themed style and attach the tap/hover
    // handlers. The signal is created in this row's scope, so it lives as
    // long as the cells that subscribe to it.
    let interaction = props.on_row_click.map(|on_click| RowInteraction {
        // TWO flags, not one. A pointer backend drives both — hover on
        // enter/leave, press on down/up — and folding them into a single
        // bool means whichever fires last wins: releasing a click would
        // report "not pressed" and clear a tint the pointer is still
        // entitled to, and any source that fails to report its own
        // release leaves the row stuck lit with nothing able to correct
        // it. Kept apart, each reports only about itself and the tint is
        // their OR.
        hovered: signal(false),
        pressed: signal(false),
        on_click,
    });
    let children = match &interaction {
        Some(i) => children
            .into_iter()
            .map(|cell| make_row_cell_interactive(cell, i.hovered, i.pressed, i.on_click.clone()))
            .collect(),
        None => children,
    };

    let row = sdk_row(SdkTableRowProps { children }).into_element();
    if let Some(r) = props.bind_to {
        table::bind_row(&row, move |h| runtime_core::fill_ref_while_mounted(r, h));
    }
    // What the table needs to add its own cells to this row (selection
    // box, drag grip) and treat them like the row's — see `rows`.
    table::set_row_data(
        &row,
        Rc::new(RowMeta {
            id: props.id,
            selectable: props.selectable,
            draggable: props.draggable,
            group: props.reorder_group,
            tone: props.tone,
            interaction,
            density: props.density,
            band: None,
        }),
    );
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
///
/// The geometry the table computes for a cell (its column width and
/// floor, a frozen column's offset, a pixel edge inset, its grid
/// placement) is applied after a `styles` slot override and wins over it
/// for those fields; size a column with `width` / `Table(columns = …)`.
// Reactive-by-default: `text`, `width` and `min_width` are reactive and
// `children` is a LIST (auto-skipped). The `#[prop(static)]` fields are
// STRUCTURAL — each picks an SDK element, a grid placement, or a style
// arm on several nodes at build time (see each field).
#[runtime_core::props(styles = TableStyles)]
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
    /// Convenience text content. The themed `head_text` /
    /// `body_text` styling lands on the inner text node so the
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
    /// Horizontal padding — `Icon` is the compact padding of a column of
    /// icons or icon buttons (a row menu). Give every cell of the column
    /// the same value; `TableColumn::padding` does that for you.
    // STRUCTURAL — selects the `cell_padding` arm at build time.
    #[prop(static)]
    pub padding: CellPadding,
    /// The value's text colour — `Danger` for an overdue date, `Muted`
    /// for a placeholder. Colours this cell only; a whole row takes
    /// `TableRow(tone = …)`. Reactive.
    pub tone: CellTone,
    /// Makes a header cell a sort control: `Some(SortState::None)` for a
    /// sortable column that is not the sort, `Some(Asc)` / `Some(Desc)`
    /// for the one that is. The cell draws the direction beside its label
    /// and a press calls `on_sort`. Reactive, so the arrow follows the
    /// app's sort without a rebuild. Header cells only.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub sort: Option<SortState>,
    /// Called with the sort a press asks for — [`SortState::next`] of the
    /// current one. The sorting itself is the app's.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub on_sort: Option<Rc<dyn Fn(SortState)>>,
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
            padding: CellPadding::Standard,
            tone: Reactive::Static(CellTone::None),
            sort: Reactive::Static(None),
            on_sort: None,
            styles: Default::default(),
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
    let tone = if header { Reactive::Static(CellTone::None) } else { props.tone.clone() };
    let sort = props.sort.clone();
    let sortable = header && !matches!(sort, Reactive::Static(None));
    let cell_children: Vec<Element> = if sortable {
        // A sort header: the label and its direction side by side in the
        // row-flex wrapper, which places them by the column's alignment.
        let mut inner: Vec<Element> = Vec::with_capacity(props.children.len() + 1);
        if props.children.is_empty() {
            inner.extend(cell_text_children(header, props.text, align, truncate, tone.clone(), &props.styles));
        } else {
            for c in props.children {
                ChildList::append_to(c, &mut inner);
            }
        }
        inner.push(sort_indicator(sort.clone()));
        let inner_style = TableStyles::cell_inner()
            .overridden(props.styles.cell_inner.as_ref())
            .into_style_application()
            .with("align", align);
        vec![ui! { view(style = inner_style) { inner } }]
    } else if !props.children.is_empty() {
        let mut inner: Vec<Element> = Vec::with_capacity(props.children.len());
        for c in props.children {
            ChildList::append_to(c, &mut inner);
        }
        let inner_style = TableStyles::cell_inner()
            .overridden(props.styles.cell_inner.as_ref())
            .into_style_application()
            .with("align", align);
        vec![ui! { view(style = inner_style) { inner } }]
    } else {
        cell_text_children(header, props.text, align, truncate, tone.clone(), &props.styles)
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
    let padding = props.padding.as_variant_str();
    let app = if header {
        // `sticky_header: off` stated outright: the key marks a head cell
        // for `Table`, which pins the leading head rows of a scroll-y table.
        let app = TableStyles::head_cell()
            .overridden(props.styles.head_cell.as_ref())
            .into_style_application()
            .with("sticky_header", "off");
        let app = select_cell_axes(app, pin_arm, align, truncate, padding);
        if sortable { app.with("interactive", "on") } else { app }
    } else {
        let app = TableStyles::body_cell().overridden(props.styles.body_cell.as_ref()).into_style_application();
        let app = select_cell_axes(app, pin_arm, align, truncate, padding);
        match &tone {
            Reactive::Static(CellTone::None) | Reactive::Dynamic(_) => app,
            Reactive::Static(t) => app.with("text_tone", t.as_variant_str()),
        }
    };

    // Geometry the sheet cannot enumerate — a column width, a pin inset —
    // rides the inline layer on top of the selected arms: the cell keeps
    // its preminted classes, so a `--premint-only` bundle (no runtime
    // style engine) still renders it. Not `with_overrides`, which takes an
    // application off the premint path. Table geometry is merged last, so
    // it wins over a cell slot override for the fields it sets.
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
    let cell = match (props.width, props.min_width) {
        (Reactive::Static(width), Reactive::Static(min_width)) => {
            let app = match geometry(width, min_width) {
                Some(rules) => app.with_inline_merged(rules),
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
                    Some(rules) => app.clone().with_inline_merged(rules),
                    None => app.clone(),
                }
            };
            bound.with_style(StyleProp::SheetDynamic(Box::new(style))).into_element()
        }
    };
    // A live text tone follows its signal on the cell (what rich children
    // inherit on web); the text node follows it on its own.
    if let Reactive::Dynamic(f) = tone {
        table::map_cell_style(&cell, Rc::new(move |app| app.with("text_tone", f().as_variant_str())));
    }
    // A sort header's press asks for the next sort. On the cell's own
    // node, so the whole header is the target and a resize handle inside
    // it still claims its own drag first.
    if let (true, Some(on_sort)) = (sortable, props.on_sort) {
        let press = tap(TapRecognizer::new(), move || on_sort(sort.get().unwrap_or_default().next()));
        table::set_cell_touch(&cell, Rc::new(move |ev| press(ev)));
    }
    cell
}

/// The direction glyph beside a sort header's label: an up or down arrow
/// for the sort column, a faint up-down pair for a sortable one that is
/// not.
fn sort_indicator(sort: Reactive<Option<SortState>>) -> Element {
    let state = move || sort.get().unwrap_or_default();
    ui! {
        match state() {
            SortState::Asc => {
                Icon(data = SORT_ASC_MARK, size = SORT_ICON_PX, tone = Some(crate::tone::Neutral.into()))
            }
            SortState::Desc => {
                Icon(data = SORT_DESC_MARK, size = SORT_ICON_PX, tone = Some(crate::tone::Neutral.into()))
            }
            SortState::None => {
                view(style = TableStyles::sort_idle()) {
                    Icon(data = SORT_IDLE_MARK, size = SORT_ICON_PX, tone = Some(crate::tone::Neutral.into()))
                }
            }
        }
    }
}

/// The sort direction glyph's square size, px.
const SORT_ICON_PX: f32 = 14.0;

/// An arrow up (ascending).
const SORT_ASC_MARK: runtime_core::IconData = runtime_core::IconData {
    view_box: (24, 24),
    paths: &["M12 19V5", "M5 12l7-7 7 7"],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};

/// An arrow down (descending).
const SORT_DESC_MARK: runtime_core::IconData = runtime_core::IconData {
    view_box: (24, 24),
    paths: &["M12 5v14", "M19 12l-7 7-7-7"],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};

/// Up and down chevrons (sortable, not sorted).
const SORT_IDLE_MARK: runtime_core::IconData = runtime_core::IconData {
    view_box: (24, 24),
    paths: &["M7 9l5-5 5 5", "M7 15l5 5 5-5"],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};

/// Re-select the `align` / `truncate` arms on what a built cell holds — its
/// text node or its rich-children wrapper — so a column spec applied by the
/// table reaches the node a native text reads its alignment from. Only an
/// axis the child's sheet already selects is touched, and a static sheet
/// stays static.
pub(super) fn reselect_content(cell: &Element, align: &'static str, truncate: &'static str) {
    use runtime_vocabulary::prims::{PrimCell, TextPrim, ViewPrim};
    let remap = move |slot: &mut Option<StyleProp>| {
        let pick = move |app: StyleApplication| {
            let app = if app.variants.0.contains_key("align") { app.with("align", align) } else { app };
            if app.variants.0.contains_key("truncate") {
                app.with("truncate", truncate)
            } else {
                app
            }
        };
        *slot = match slot.take() {
            Some(StyleProp::Sheet(app)) => Some(StyleProp::Sheet(Box::new(pick(*app)))),
            Some(StyleProp::SheetDynamic(f)) => Some(StyleProp::SheetDynamic(Box::new(move || pick(f())))),
            other => other,
        };
    };
    table::visit_cell_content(cell, |child| {
        let mut el = child;
        while let Element::Owned { element, .. } = el {
            el = element;
        }
        if let Element::Item { data, .. } = el {
            if let Some(t) = data.downcast_ref::<PrimCell<TextPrim>>() {
                t.with_mut(|p| remap(&mut p.style));
            } else if let Some(v) = data.downcast_ref::<PrimCell<ViewPrim>>() {
                v.with_mut(|p| remap(&mut p.style));
            }
        }
    });
}

/// Select the build-time axes every cell carries, on either sheet.
fn select_cell_axes(
    app: StyleApplication,
    pin_arm: Option<&'static str>,
    align: &'static str,
    truncate: &'static str,
    padding: &'static str,
) -> StyleApplication {
    let app = app.with("align", align).with("truncate", truncate).with("cell_padding", padding);
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
    tone: Reactive<CellTone>,
    styles: &TableSlotStyles,
) -> Vec<Element> {
    let style = if header {
        TableStyles::head_text().overridden(styles.head_text.as_ref()).into_style_application()
    } else {
        TableStyles::body_text().overridden(styles.body_text.as_ref()).into_style_application()
    }
    .with("align", align)
    .with("truncate", truncate);
    let text = match content {
        Reactive::Static(None) => return Vec::new(),
        Reactive::Static(Some(s)) => text_node(s),
        Reactive::Dynamic(f) => text_node(move || f().unwrap_or_default()),
    };
    let text = match tone {
        Reactive::Static(CellTone::None) => text.with_style(style),
        Reactive::Static(t) => text.with_style(style.with("text_tone", t.as_variant_str())),
        Reactive::Dynamic(f) => text.with_style(move || style.clone().with("text_tone", f().as_variant_str())),
    };
    vec![text.into_element()]
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
