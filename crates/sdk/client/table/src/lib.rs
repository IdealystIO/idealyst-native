//! `table` — third-party Table SDK.
//!
//! Web emits real HTML `<table>` / `<thead>` / `<tbody>` / `<tr>` /
//! `<th>` / `<td>` so the browser's native table-layout algorithm
//! handles cross-row column alignment for free. Native (iOS / Android /
//! macOS / terminal / gpu / SSR) builds a single **CSS-grid**: every
//! row is flattened to its cells and the cells are parented directly
//! under one grid node whose `N` column tracks span all rows, giving
//! the same cross-row alignment the browser gives a real `<table>`.
//!
//! # Why this is an SDK and not a core primitive
//!
//! Web's `<table>` is a layout primitive with no native equivalent —
//! UITableView is a vertical list, Android RecyclerView the same,
//! macOS NSTableView is row-keyed. Putting a web-only-with-real-
//! behavior primitive in the framework would be a web capability
//! wearing a primitive's clothes. The SDK keeps that behavior
//! pluggable: web wires up a real `<table>` through the scene registry,
//! native composes a grid out of the framework's own layout primitives.
//!
//! # Scroll-x mode & pinned columns
//!
//! `TableProps::scroll_x` restructures the table as
//! styled surface > horizontal scroller > columns: the author-style
//! surface (border/radius/background) stays put while the columns
//! scroll inside it, and the width strategy flips to "natural column
//! widths, floored at the scroller's width" (web: `min-width: 100%;
//! width: max-content` plus `border-collapse: separate` so cell
//! borders travel with sticky cells; native: a `min_width: 100%`
//! floor on the scroll content). Frozen columns are pure STYLING on
//! top:
//! a cell with `position: Sticky` + `left: 0` / `right: 0` pins inside
//! that scroller on every backend — the browser natively, native
//! through `runtime_shared::sticky` (which also raises pinned cells
//! above the content sliding beneath them). The SDK has no pin API of
//! its own; idea-ui's `TableCell(pinned = …)` axis is the author
//! surface.
//!
//! [`TableBound::on_scroll_edges`] reports which sides of the scroller
//! have columns scrolled out of view ([`ScrollEdges`]) — what a frozen
//! column needs to draw its edge only while content slides beneath it.
//!
//! # Scroll-y mode & a pinned header
//!
//! `TableProps::scroll_y` makes the table own its VERTICAL scrolling too:
//! the rows scroll inside the table's own height, which the author bounds
//! on the table's style (`max_height`, `height`, or a flex fill — the
//! scroller shrinks to whatever the styled surface leaves it). With
//! `scroll_x` as well, ONE two-axis scroller
//! ([`ScrollAxis::Both`](runtime_shared::ScrollAxis)) owns both
//! directions, so a header row styled `position: Sticky` + `top: 0` pins
//! to the top while frozen columns keep pinning left/right, and a corner
//! cell styled with both insets pins on both axes. (A horizontal
//! scroller nested in the page's vertical one cannot do this: a sticky
//! cell pins against its NEAREST scroll container, which is the
//! horizontal one, so the header could never stick to the page.) As with
//! frozen columns, the SDK has no pin API of its own — idea-ui's
//! `Table(scroll_y = true)` styles its header cells sticky.
//!
//! # Grid columns
//!
//! A cell can size its column ([`TableCellProps::column`], a
//! [`ColumnTrack`]). `width` and `min_width` keep the auto-table sizing
//! described below. `flex` or `max_width` on any column switches the
//! table to explicit CSS grid columns on both lowerings — see
//! [`ColumnTrack`] for the column-to-track mapping. Native sets the
//! tracks on its grid; web keeps the `<table>` but makes it
//! `display: grid`, with its sections and rows `display: contents`, its
//! cells column flex boxes, explicit table roles, an explicit `<tbody>`
//! ([`TableBodyPrim`]) and, for a bound or styled row, the native row
//! backdrop.
//!
//! # Spans, footer sections, frame slots
//!
//! - [`TableCellProps::span`] ([`ColSpan`]) covers several columns:
//!   `colspan` on web (a `Rest` span resolved to a number before mount),
//!   a multi-track grid placement on native. A spanning native cell is
//!   carried in a [`CellMarker`] that [`table`] unwraps, and its
//!   presence switches the grid to explicit placement — the same switch a
//!   row proxy makes, for the same reason.
//! - [`table_foot`] is a footer section: `<tfoot>` on web; on native its
//!   rows flatten after every body row, wherever the section was written.
//! - [`TableProps::header_slot`] / [`TableProps::footer_slot`] put
//!   content inside the styled surface but outside the horizontal
//!   scroller.
//!
//! # The author style never lands on the `<table>`
//!
//! On web, `.with_style(...)` styles a SURFACE view wrapped around the
//! `<table>` (the scroller sits between them in scroll-x mode), never the
//! `<table>` node: its `border-collapse: collapse` reset makes the browser
//! ignore `border-radius`, so a rounded frame drew square. Native already
//! styles an outer view around the grid, so both lowerings carry the
//! author style on the same node. An unstyled plain table stays a bare
//! `<table>`.
//!
//! # Row proxies (drag & drop geometry)
//!
//! A dissolved row has no node of its own, which made row-level
//! geometry (drop targeting, row frames) inexpressible. [`bind_row`]
//! fixes that: on web it hands out the real `<tr>`'s handle; on native
//! the table emits a row-spanning BACKDROP view (explicit grid
//! placement, painted beneath the row's cells) and hands out that.
//! When any row is bound (or styled), every cell is explicitly placed
//! — see [`build_table`]'s doc for why auto-flow and spanning items
//! cannot mix. Row TOUCH stays on the per-cell fan-out
//! ([`set_cell_interaction`]): a sibling backdrop can never receive
//! touches landing on cell content on native.
//!
//! # Two platform lowerings
//!
//! - **Web (wasm32)**: each primitive lowers to a scene
//!   [`Element::Item`] carrying a typed payload; the scene
//!   [`Registry`] dispatches to the handlers installed by
//!   [`register`], which emit the real `<table>` / `<tr>` /
//!   `<td>` / `<th>` DOM through the vocabulary's capability traits
//!   ([`create_element`](runtime_vocabulary::caps::DocumentOps::create_element))
//!   — the runtime's unified primitive==external contract. The
//!   handlers are generic over the caps traits, so the SSR backend
//!   reuses them for static rendering.
//! - **Native (non-wasm)**: each row lowers to a [`TableRowPrim`]
//!   MARKER item that [`table`] consumes at build time, flattening the
//!   row's cells so they are parented directly under one
//!   `display: grid` node with `N` `auto` column tracks, built with
//!   the vocabulary glue's `view` builder. Because the column tracks
//!   are shared, column `i` is one width across every row — the same
//!   cross-row alignment the browser gives a real `<table>`. No
//!   handler registration is needed; the built grid is plain views,
//!   handled by the vocabulary built-ins. (A `table_row` used OUTSIDE
//!   a `table` panics at realize as an unregistered payload — the
//!   loud failure the registry promises.)
//!
//! Unless the table uses grid columns (above), the columns are `auto`,
//! which `runtime-layout` treats as the `table-layout: auto` signal: it measures each column's content, then
//! short columns hug their content while a text-heavy column absorbs the
//! remaining width and wraps — the same layout a browser gives the web
//! `<table>`.
//!
//! Why a grid and not nested row/cell views: Taffy has no subgrid, and
//! `runtime_layout`'s `display: contents` emulation
//! (`LayoutTree::new_contents_node`) is a backend-internal tool for
//! reactive anchors, not something an author style can ask for — so a
//! grid can only align the columns of its *direct* children. Keeping
//! `TableRow` as a layout box would make each
//! row a single grid item and break alignment. On native a row therefore
//! lowers to an [`Element::Fragment`] (no box); the row look is carried
//! by per-cell styling (head/body surface + per-cell `border-bottom`
//! separators), exactly as on web.
//!
//! # Owned-scope peeling (native grid flattening)
//!
//! A `#[component]`-wrapped row (idea-ui's `TableRow` with
//! `on_row_click`) arrives as
//! `Element::Owned { element: Fragment(cells), owned }` — the row
//! body's collected scope (its shared hover signal) rides the wrapper.
//! Flattening must NOT drop that scope: the cells' reactive hover
//! style reads the signal for the cells' whole life. [`table`] peels
//! the wrapper, flattens the cells into the grid, and re-attaches
//! every peeled scope around the finished table element
//! (`runtime_scene::owned`), so the scopes live exactly as long as the
//! subtree that uses them.
//!
//! # Built-cell post-processing ([`map_cell_style`] & co.)
//!
//! idea-ui's clickable-row feature re-styles built cells and attaches
//! touch/hover handlers AFTER the cell element exists. An
//! `Element::Item` payload is type-erased, so this crate owns that
//! knowledge: the [`map_cell_style`] / [`set_cell_style`] /
//! [`set_cell_interaction`] helpers reach through the payload cell
//! (`PrimCell::with_mut` — the payload is not yet mounted, so in-place
//! mutation is sound) for BOTH lowerings (native `ViewPrim` grid item,
//! web [`TableCellPrim`]).
//!
//! [`map_cell_style`] is the one to reach for when layering axes over
//! a cell: it COMPOSES over whatever style the cell already has, so a
//! cell styled reactively (a column width that moves under a resize
//! drag) keeps its own reactivity and takes the overlay too. Reading a
//! cell's style and writing back a replacement ([`set_cell_style`]) only
//! works for cells styled with a STATIC sheet and silently skips every
//! other cell.
#![deny(missing_docs)]

use std::rc::Rc;

use runtime_shared::{HoverHandler, TouchHandler};
use runtime_scene::{item, Element, Host, MountCx, Registry};
use runtime_vocabulary::caps::{InputOps, IntrospectionOps};
use runtime_vocabulary::robot_ext::register_extension_node;
use runtime_vocabulary::glue::{
    self, BuildElement, ChildList, DisplayKind, IntoElement, StyleApplication, StyleRules,
    StyleSheet, TrackSize, VariantSet,
};
use runtime_vocabulary::prims::{self, PrimCell};
use runtime_vocabulary::style_attach::{attach_style, IntoStyleProp, StyleProp, StyleServices};

// ============================================================================
// Props
// ============================================================================

/// Props for the outer `<table>` container.
///
/// `children` carries the rows. On web they become real DOM children
/// of the `<table>` element so the browser's table-layout algorithm
/// sees the full row set; on native the rows are flattened into one
/// grid (see the module docs).
#[derive(Default)]
pub struct TableProps {
    /// The table's rows. Populated by the `ui!` children block.
    pub children: Vec<Element>,
    /// Horizontal-scroll mode. When `true` the table is wrapped in a
    /// horizontal scroller (a DOM `overflow-x` container on web, the
    /// `scroll_view` primitive on native) and its width strategy flips
    /// from "fill and wrap" to "natural column widths, at least the
    /// scroller's width": columns lay out at their content width, the
    /// table overflows sideways when they don't fit, and still fills
    /// the scroller when they do. Pinned (frozen) columns are styled by
    /// the CALLER, not the SDK — give a cell `position: Sticky` with
    /// `left: 0` / `right: 0` (idea-ui's `TableCell(pinned = …)` axis
    /// does exactly this) and both lowerings pin it: the browser
    /// natively, the native backends through the shared sticky
    /// registry, which also raises pinned cells above the content
    /// sliding beneath them.
    pub scroll_x: bool,
    /// Vertical-scroll mode. When `true` the rows scroll inside the
    /// table's own height instead of growing it: bound the table with
    /// `max_height` / `height` (or a flex fill) on its style, and the
    /// scroller takes whatever the styled surface leaves after the
    /// slots. Combined with [`scroll_x`](Self::scroll_x) the table owns
    /// ONE two-axis scroller, so a header row styled `position: Sticky` +
    /// `top: 0` pins while frozen columns pin sideways (see the module
    /// docs). Uses the separate border model on web, as `scroll_x` does,
    /// so a pinned cell's hairlines travel with it.
    pub scroll_y: bool,
    /// Content drawn INSIDE the table's frame, above the rows and
    /// OUTSIDE the horizontal scroller — a caption, a strip of notices,
    /// a toolbar. It keeps still while a `scroll_x` table's columns
    /// scroll sideways beneath it, and it shares the table's border.
    /// Slots sit inside the styled surface wrapper (styled surface >
    /// content), which carries the author style on every table.
    pub header_slot: Option<Element>,
    /// Content drawn inside the frame BELOW the rows, outside the
    /// horizontal scroller — a blank-state sentence, an "Add entry"
    /// action, a pager. See [`header_slot`](Self::header_slot).
    pub footer_slot: Option<Element>,
}

/// Props for a footer section (`<tfoot>` on web). Holds one or more
/// rows — a totals row — that assistive tech announces as the table's
/// footer rather than as another row of data. A footer section always
/// renders after the body rows, wherever it sits among the table's
/// children (the browser does this for `<tfoot>`; native moves the rows
/// to the end of the grid to match).
#[derive(Default)]
pub struct TableFootProps {
    /// The footer rows. Populated by the `ui!` children block.
    pub children: Vec<Element>,
}

/// How many columns a cell covers — `colspan` on web, a multi-track
/// grid placement on native.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColSpan {
    /// Cover this many columns (`0` is treated as `1`).
    Columns(u16),
    /// Cover every column from this cell to the end of the row — the
    /// "blank row" / "add row" / group-title shape. Resolved against the
    /// table's column count (its widest row) when the table is built.
    Rest,
}

impl Default for ColSpan {
    fn default() -> Self {
        ColSpan::Columns(1)
    }
}

impl From<u16> for ColSpan {
    fn from(n: u16) -> Self {
        ColSpan::Columns(n)
    }
}

impl ColSpan {
    /// The column count this span covers given `remaining` columns to the
    /// end of the row. Never zero.
    pub fn resolve(self, remaining: usize) -> usize {
        match self {
            ColSpan::Columns(n) => (n as usize).max(1),
            ColSpan::Rest => remaining.max(1),
        }
    }

    /// The span's contribution to a row's column count before the
    /// table's width is known: `Rest` counts as one column (it stretches
    /// to fit whatever the other rows establish).
    fn min_columns(self) -> usize {
        self.resolve(1)
    }

    fn is_single(self) -> bool {
        self == ColSpan::Columns(1) || self == ColSpan::Columns(0)
    }
}

/// How one column is sized, read from its cells ([`TableCellProps::column`]).
///
/// A table whose columns set none of `max_width` / `flex` sizes them the
/// way a browser sizes an auto `<table>` (see the module docs). Setting
/// either on any column switches the WHOLE table to grid columns
/// ([`ColumnTrack::needs_grid`]), where each column is one CSS grid track:
///
/// | set | track | meaning |
/// | --- | --- | --- |
/// | `width` | `Npx` | exactly that wide (`flex` / `min` / `max` ignored) |
/// | `flex` | `minmax(min, fr)` | a share of the width the other columns leave, down to `min_width` (default 0, so its content truncates or wraps inside) |
/// | `flex` + `max_width` | `minmax(min, max)` | grows into the leftover width up to `max_width` |
/// | `max_width` | `fit-content(max)` | its content's width, at most `max_width` |
/// | nothing / `min_width` | `max-content` | its content's width, never squeezed (`min_width` floors it through the cell's own style) |
///
/// In this mode spare width goes only to `flex` columns: a grid-column
/// table without one is as wide as its columns, not its container.
///
/// Content-sized columns are `max-content`, not `auto`: the native
/// layout gives grid items `min-width: 0` (table semantics), and with it
/// Taffy widens an `auto` track beside an `fr` one and stretches a
/// `fit-content` track past its cap. `max-content` sizes the same on
/// every backend, browser included.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColumnTrack {
    /// Fixed width, px.
    pub width: Option<f32>,
    /// Floor, px.
    pub min_width: Option<f32>,
    /// Ceiling, px.
    pub max_width: Option<f32>,
    /// Share of the leftover width (CSS `fr`), relative to the table's
    /// other `flex` columns.
    pub flex: Option<f32>,
}

impl ColumnTrack {
    /// Whether this column needs the grid sizing mode: `flex` and
    /// `max_width` have no meaning in a browser's auto table layout
    /// (`max-width` on a table cell is ignored), so a table with either
    /// on any column lays out as a grid on every backend.
    pub fn needs_grid(&self) -> bool {
        self.flex.is_some() || self.max_width.is_some()
    }

    /// This column's grid track (see the table on [`ColumnTrack`]).
    pub fn track(&self) -> TrackSize {
        let px = TrackSize::Px;
        if let Some(w) = self.width {
            return px(w);
        }
        match (self.flex, self.max_width) {
            (Some(f), max) => TrackSize::Minmax(
                Box::new(px(self.min_width.unwrap_or(0.0))),
                Box::new(match max {
                    Some(m) => px(m),
                    None => TrackSize::Fr(f),
                }),
            ),
            (None, Some(m)) => TrackSize::FitContent(m),
            (None, None) => TrackSize::MaxContent,
        }
    }
}

/// A column's sizing, possibly live: a [`ColumnTrack`] read each time the
/// table's columns are laid out, so a width that follows a resize drag
/// moves the grid track with it.
#[derive(Clone)]
pub struct ColumnSpec(Rc<dyn Fn() -> ColumnTrack>);

impl ColumnSpec {
    /// A fixed column.
    pub fn new(track: ColumnTrack) -> Self {
        ColumnSpec(Rc::new(move || track))
    }

    /// A column whose sizing is read live (signal reads inside `f`
    /// subscribe the table's column style).
    pub fn live(f: impl Fn() -> ColumnTrack + 'static) -> Self {
        ColumnSpec(Rc::new(f))
    }

    /// The column's current sizing.
    pub fn get(&self) -> ColumnTrack {
        (self.0)()
    }
}

/// Props for a single row (`<tr>`).
#[derive(Default)]
pub struct TableRowProps {
    /// The row's cells. Parented into the `<tr>` on web; on native the
    /// row lowers to a fragment and these cells become direct children
    /// of the table's grid. Populated by the `ui!` children block.
    pub children: Vec<Element>,
}

/// Props for a single cell. `header = true` renders `<th>` instead of
/// `<td>` so the browser applies its default header styling and
/// assistive tech announces it as a header.
#[derive(Default)]
pub struct TableCellProps {
    /// When `true`, render a `<th>` (header cell) instead of a `<td>`
    /// on web. On native `header` has no built-in visual effect (the
    /// cell is a grid item); the caller styles header cells via
    /// `.with_style(...)`.
    pub header: bool,
    /// How many columns this cell covers. See [`ColSpan`].
    pub span: ColSpan,
    /// This cell's column sizing (see [`ColumnTrack`]). A column takes
    /// its sizing from the first single-column cell that sets one, so
    /// set it on the header cell — or on every cell of the column, which
    /// is what keeps a column consistent whichever row renders first.
    pub column: Option<ColumnSpec>,
    /// The cell's contents (typically a `text`). Populated by the
    /// `ui!` children block.
    pub children: Vec<Element>,
}

// ============================================================================
// Web item payloads. Always COMPILED (the handlers are generic over the
// caps traits, so host-side tests drive them through the SSR backend);
// only the wasm32 CONSTRUCTOR arm emits them in an app tree.
// ============================================================================

/// Scene payload for the `<table>` container (web lowering). Wrapped in
/// [`PrimCell`] at the item boundary — the registry key is
/// `PrimCell<TablePrim>`.
pub struct TablePrim {
    /// Style attached to the `<table>` node after children mount (the
    /// standard handler ordering). [`table`] never sets it — the author
    /// style goes on the surface wrapper (see the module docs); it is
    /// here for direct item construction.
    pub style: Option<StyleProp>,
    /// Horizontal-scroll mode — flips the mount-time width strategy
    /// from `width: 100%` (fill, wrap) to `min-width: 100%; width:
    /// max-content` (natural column widths, overflow sideways). The
    /// scroller wrapper itself is added by [`table`] at the element
    /// level, outside this payload.
    pub scroll_x: bool,
    /// Vertical-scroll mode — the rows scroll inside the table's own
    /// height, so a sticky header pins. Switches the border model to
    /// `separate` (as `scroll_x` does) and leaves the width strategy to
    /// `scroll_x`.
    pub scroll_y: bool,
    /// Grid-column mode (a column sets `flex` / `max_width` — see
    /// [`ColumnTrack`]): the `<table>` is a CSS grid whose tracks ride in
    /// `style`, with an explicit `role="table"` because its rows and
    /// sections are `display: contents`. Set by [`table`].
    pub grid: bool,
    /// Filled with the `<table>`'s handle at mount — how
    /// [`TableBound::on_scroll_edges`] measures the scrolled content's
    /// width. Set by [`table`].
    pub ref_fill: Option<Box<dyn FnOnce(runtime_shared::ViewHandle)>>,
}

/// Scene payload for a table row — the `<tr>` on web, and (since the
/// row-proxy work) the pre-flatten row MARKER on native: native
/// `table_row` lowers to this item so row-level slots survive until
/// [`table`] dissolves the row into the grid. [`table`] consumes the
/// marker at build time; it never reaches realize on native.
///
/// The `ref_fill` slot is what gives a dissolved row a real, bindable
/// surface (the "row proxy"): on web it hands out the `<tr>`'s own
/// handle; on native, [`table`] emits a row-spanning BACKDROP view
/// (explicit grid placement `grid_row: r`, `grid_column: 1 / -1`,
/// inserted before the row's cells so it paints beneath them) and the
/// handle is the backdrop's. Row geometry (drag-and-drop drop
/// targeting, row frames) reads through that handle. Row-level TOUCH
/// does NOT live here — a sibling backdrop can never receive touches
/// that land on cell content on native, so interaction stays on the
/// per-cell fan-out ([`set_cell_interaction`]), which is also how the
/// clickable-row feature already works.
pub struct TableRowPrim {
    /// Author style for the row node — the `<tr>` on web, the backdrop
    /// view on native (via the proxy, so row-level visuals behave
    /// uniformly).
    pub style: Option<StyleProp>,
    /// Filled with the row surface's handle at mount ([`bind_row`]).
    pub ref_fill: Option<Box<dyn FnOnce(runtime_shared::ViewHandle)>>,
    /// Web, grid-column tables only: the `<tr>` is `display: contents`
    /// so its cells are items of the table's grid. Set by [`table`].
    pub grid: bool,
}

/// Scene payload for a `<td>` / `<th>` (web lowering). The interaction
/// slots exist so [`set_cell_interaction`] can attach idea-ui's
/// clickable-row handlers to a BUILT cell.
pub struct TableCellPrim {
    /// `<th>` when `true`, `<td>` otherwise.
    pub header: bool,
    /// Columns covered. [`table`] resolves a [`ColSpan::Rest`] to a
    /// count against the table's column count before mount; the mount
    /// writes `colspan` when it is more than one.
    pub span: ColSpan,
    /// Author style, attached to the cell node.
    pub style: Option<StyleProp>,
    /// Touch handler installed on the cell node (clickable rows).
    pub on_touch: Option<TouchHandler>,
    /// Hover handler installed on the cell node (row hover highlight).
    pub on_hover: Option<HoverHandler>,
    /// Filled with the `<td>`/`<th>`'s handle at mount ([`bind_cell`]) —
    /// the anchor animated drag offsets attach to.
    pub ref_fill: Option<Box<dyn FnOnce(runtime_shared::ViewHandle)>>,
    /// The cell's column sizing ([`TableCellProps::column`]), read by
    /// [`table`] before mount.
    pub column: Option<ColumnSpec>,
    /// Web, grid-column tables only: the cell is a grid item (a span is
    /// `grid-column: span n`, and it carries an explicit cell role, since
    /// its `display: contents` row no longer gives it one). Set by
    /// [`table`].
    pub grid: bool,
}

/// Scene payload for a footer section — `<tfoot>` on web, and on native
/// a pre-flatten MARKER that [`table`] consumes (its rows are flattened
/// into the grid after every body row).
#[derive(Default)]
pub struct TableFootPrim {
    /// Web, grid-column tables only: the `<tfoot>` is `display: contents`.
    /// Set by [`table`].
    pub grid: bool,
}

/// Scene payload for a grid-column web table's body section — an explicit
/// `<tbody>`, `display: contents`. [`table`] wraps the body rows in one
/// in grid mode only, and only on the web lowering.
///
/// Why it exists: prerendered (SSR/SSG) markup `<table><tr>…` is
/// re-parsed by the browser, whose HTML parser inserts a `<tbody>` the
/// live DOM never had. That implicit `<tbody>` is an ordinary row-group
/// box, so it became the grid's ONE item and every cell landed in the
/// first column. Emitting the section ourselves gives parsed and live
/// DOM the same, grid-transparent structure.
#[derive(Default)]
pub struct TableBodyPrim;

/// Native-only marker carried by a cell that covers more than one
/// column or declares a [`ColumnSpec`]: the cell's view is its single
/// child. [`table`] unwraps it while flattening (the span becomes grid
/// placement, the spec a column track), so it never reaches realize (such
/// a `table_cell` used outside a `table` panics there as an unregistered
/// payload — the loud failure the registry promises). The cell helpers
/// ([`map_cell_style`] & co.) look through it.
pub struct CellMarker {
    /// The cell's span, resolved by [`table`].
    pub span: ColSpan,
    /// The cell's column sizing.
    pub column: Option<ColumnSpec>,
}

/// The marker's name before it also carried a cell's [`ColumnSpec`].
#[deprecated(note = "renamed to `CellMarker`")]
pub type CellSpanMarker = CellMarker;

// ============================================================================
// Builder wrappers — `.with_style(…)` → `IntoElement`, deferred-build
// so the style lands in the right slot on either lowering.
// ============================================================================

macro_rules! table_wrapper_common {
    ($wrapper:ident) => {
        impl $wrapper {
            /// Attach an author style — lands on the `<table>`/`<tr>`/
            /// `<td>` node on web, on the corresponding grid node on
            /// native. Replaces any previously set style.
            pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
                self.style = Some(style.into_style_prop());
                self
            }
        }

        impl ChildList for $wrapper {
            fn append_to(self, out: &mut Vec<Element>) {
                out.push(self.into_element());
            }
        }

        /// Element coercion for bare `{ … }` interpolation sites.
        impl From<$wrapper> for Element {
            fn from(w: $wrapper) -> Element {
                w.into_element()
            }
        }
    };
}

/// Deferred `Table` build — finish with `.with_style(…)` +
/// `.into_element()`.
pub struct TableBound {
    children: Vec<Element>,
    style: Option<StyleProp>,
    scroll_x: bool,
    scroll_y: bool,
    header_slot: Option<Element>,
    footer_slot: Option<Element>,
    on_edges: Option<Rc<dyn Fn(ScrollEdges)>>,
}
table_wrapper_common!(TableBound);

impl TableBound {
    /// Report which sides of a `scroll_x` table have columns scrolled out
    /// of view — see [`ScrollEdges`]. Called once the scroller and its
    /// content are measured, then again whenever the answer changes (a
    /// scroll, a resize, rows that widen a column); never twice with the
    /// same value. Ignored without `scroll_x`, where nothing scrolls.
    ///
    /// The callback stops when the scope that built the table drops.
    pub fn on_scroll_edges(mut self, f: impl Fn(ScrollEdges) + 'static) -> Self {
        self.on_edges = Some(Rc::new(f));
        self
    }
}

impl IntoElement for TableBound {
    fn into_element(self) -> Element {
        build_table(TableParts {
            rows: self.children,
            style: self.style,
            scroll_x: self.scroll_x,
            scroll_y: self.scroll_y,
            header_slot: self.header_slot,
            footer_slot: self.footer_slot,
            on_edges: self.on_edges,
        })
    }
}

/// Deferred `TableRow` build.
pub struct TableRowBound {
    children: Vec<Element>,
    style: Option<StyleProp>,
}
table_wrapper_common!(TableRowBound);

impl IntoElement for TableRowBound {
    fn into_element(self) -> Element {
        build_row(self.children, self.style)
    }
}

/// Deferred `TableCell` build.
pub struct TableCellBound {
    header: bool,
    span: ColSpan,
    column: Option<ColumnSpec>,
    children: Vec<Element>,
    style: Option<StyleProp>,
}
table_wrapper_common!(TableCellBound);

impl IntoElement for TableCellBound {
    fn into_element(self) -> Element {
        build_cell(self.header, self.span, self.column, self.children, self.style)
    }
}

// ============================================================================
// Constructors
// ============================================================================

/// Build a `Table` container. Web lowers to a scene item handled by
/// [`register`] (a real `<table>`); native lowers to a
/// CSS-grid whose column tracks span every row (see the module docs).
pub fn table(mut props: TableProps) -> TableBound {
    TableBound {
        children: std::mem::take(&mut props.children),
        style: None,
        scroll_x: props.scroll_x,
        scroll_y: props.scroll_y,
        header_slot: props.header_slot.take(),
        footer_slot: props.footer_slot.take(),
        on_edges: None,
    }
}

/// Build a table row. Web lowers to a `<tr>` item; native lowers to an
/// [`Element::Fragment`] of the row's cells — it produces no layout box
/// of its own (Taffy has no subgrid, so cells must be direct grid
/// children for cross-row column alignment).
pub fn table_row(mut props: TableRowProps) -> TableRowBound {
    TableRowBound {
        children: std::mem::take(&mut props.children),
        style: None,
    }
}

/// Build a table cell. `header = true` produces a `<th>` on web; on
/// native the cell is a grid item — visual treatment lives on the
/// caller's `.with_style(...)` (e.g. idea-ui's `TableHeadCell`).
pub fn table_cell(mut props: TableCellProps) -> TableCellBound {
    TableCellBound {
        header: props.header,
        span: props.span,
        column: props.column.take(),
        children: std::mem::take(&mut props.children),
        style: None,
    }
}

/// Build a footer section — `<tfoot>` on web; on native its rows are
/// flattened into the grid after every body row. Pass it as a child of
/// [`table`], holding [`table_row`]s.
pub fn table_foot(mut props: TableFootProps) -> Element {
    item(PrimCell::new(TableFootPrim::default()), std::mem::take(&mut props.children))
}

/// The author-facing pieces of a table, gathered by [`TableBound`].
struct TableParts {
    rows: Vec<Element>,
    style: Option<StyleProp>,
    scroll_x: bool,
    scroll_y: bool,
    header_slot: Option<Element>,
    footer_slot: Option<Element>,
    on_edges: Option<Rc<dyn Fn(ScrollEdges)>>,
}

impl TableParts {
    /// Whether the author style lands on a surface WRAPPER around the
    /// table (a scroller, or a slot to draw inside the frame) rather than
    /// on the table node itself.
    fn wrapped(&self) -> bool {
        self.scroll_x || self.scroll_y || self.header_slot.is_some() || self.footer_slot.is_some()
    }
}

/// The axes a table's scroller scrolls: both when the table owns both
/// directions (so a sticky header and frozen columns pin against the
/// same scroller), else the one it owns.
fn scroller_axis(scroll_x: bool, scroll_y: bool) -> runtime_shared::ScrollAxis {
    use runtime_shared::ScrollAxis;
    match (scroll_x, scroll_y) {
        (true, true) => ScrollAxis::Both,
        (true, false) => ScrollAxis::Horizontal,
        _ => ScrollAxis::Vertical,
    }
}

/// Wrap `content` in the styled surface with the slots either side —
/// shared by both lowerings. The slots sit OUTSIDE `content` (which,
/// in scroll-x mode, is the horizontal scroller), so they keep still
/// while columns scroll and share the surface's border.
fn surface_with_slots(
    content: Element,
    style: Option<StyleProp>,
    header_slot: Option<Element>,
    footer_slot: Option<Element>,
) -> Element {
    let mut kids = Vec::with_capacity(3);
    kids.extend(header_slot);
    kids.push(content);
    kids.extend(footer_slot);
    let surface = glue::view(kids);
    match style {
        Some(style) => surface.with_style(style).into_element(),
        None => surface.into_element(),
    }
}

/// Which sides of a `scroll_x` table's scroller have columns out of
/// view — reported by [`TableBound::on_scroll_edges`]. A frozen column
/// on that side has content beneath it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollEdges {
    /// Columns are scrolled off the LEFT edge (the offset is past 0).
    pub left: bool,
    /// Columns run past the RIGHT edge (the scroll has further to go).
    pub right: bool,
}

impl ScrollEdges {
    /// The edges for a horizontal scroll `offset` into `content` seen
    /// through a `viewport`, all in logical px. Nothing is hidden until
    /// both widths are known, so a scroller that has not laid out yet
    /// does not flash an edge.
    pub fn measure(offset: f32, viewport: f32, content: f32) -> Self {
        // Half a pixel of slack either way: a `max-content` table lays
        // out at a fractional width the scroll offset never quite
        // reaches, so a table scrolled fully right would keep reporting
        // a sliver hidden.
        const SLACK: f32 = 0.5;
        if viewport <= 0.0 || content <= 0.0 {
            return Self::default();
        }
        Self {
            left: offset > SLACK,
            right: offset + viewport < content - SLACK,
        }
    }
}

/// The measurements behind [`TableBound::on_scroll_edges`]: the scroll
/// offset (from the scroller's `on_scroll`), the viewport width (a
/// holder view around the scroller) and the content width (the
/// `<table>` on web, the scroll content view on native). Each feeds in
/// as it changes, and the callback hears only a CHANGED answer.
///
/// Ownership: the scroller's scroll handler holds the strong `Rc`; the
/// layout callbacks hold `Weak`s, so the subscriptions they live in do
/// not keep their own owner alive. `disarm` (on the building scope's
/// drop) drops the subscriptions and the callback — a late
/// `ResizeObserver` delivery must not write the signals of a scope that
/// is gone.
struct EdgeWatch {
    offset: std::cell::Cell<f32>,
    viewport: std::cell::Cell<f32>,
    content: std::cell::Cell<f32>,
    last: std::cell::Cell<Option<ScrollEdges>>,
    report: std::cell::RefCell<Option<Rc<dyn Fn(ScrollEdges)>>>,
    subs: std::cell::RefCell<Vec<runtime_shared::LayoutSubscription>>,
}

impl EdgeWatch {
    fn new(report: Rc<dyn Fn(ScrollEdges)>) -> Rc<Self> {
        let watch = Rc::new(Self {
            offset: Default::default(),
            viewport: Default::default(),
            content: Default::default(),
            last: Default::default(),
            report: std::cell::RefCell::new(Some(report)),
            subs: Default::default(),
        });
        let on_drop = watch.clone();
        glue::on_scope_drop(move || on_drop.disarm());
        watch
    }

    fn set_offset(&self, x: f32) {
        self.offset.set(x);
        self.update();
    }

    fn update(&self) {
        let edges = ScrollEdges::measure(self.offset.get(), self.viewport.get(), self.content.get());
        if self.last.get() == Some(edges) {
            return;
        }
        self.last.set(Some(edges));
        // Cloned out so the callback runs with no borrow held.
        let report = self.report.borrow().clone();
        if let Some(report) = report {
            report(edges);
        }
    }

    /// Track `handle`'s laid-out width into the slot `pick` names.
    fn measure(
        self: &Rc<Self>,
        handle: runtime_shared::ViewHandle,
        pick: fn(&EdgeWatch) -> &std::cell::Cell<f32>,
    ) {
        if self.report.borrow().is_none() {
            return;
        }
        // Seeded from the current frame where the backend has one, so a
        // backend whose layout callback only fires on a CHANGE still
        // starts from a real width.
        if let Some(frame) = handle.frame() {
            pick(self).set(frame.width);
        }
        let weak = Rc::downgrade(self);
        let sub = handle.on_layout(move |w, _h| {
            if let Some(watch) = weak.upgrade() {
                pick(&watch).set(w);
                watch.update();
            }
        });
        self.subs.borrow_mut().push(sub);
        self.update();
    }

    fn disarm(&self) {
        self.report.borrow_mut().take();
        self.subs.borrow_mut().clear();
    }
}

/// The scroller around a scrolling table's `content` (horizontal,
/// vertical, or both — [`scroller_axis`]), plus — when `watch` is set —
/// the scroll and viewport measurements for
/// [`TableBound::on_scroll_edges`]. The viewport is measured on a
/// holder view rather than the scroller itself (a scroll view hands out
/// no layout handle). The holder sits between the bounded surface and
/// the scroller, so it carries [`edge_watch_holder_rules`]:
/// sized like the scroller (content-sized, shrinkable to 0) so a bounded
/// surface still caps the scroller. A table that does not watch keeps
/// the bare scroller.
fn edge_watched_scroller(
    scroller: glue::primitives::scroll_view::GlueScrollView,
    axis: runtime_shared::ScrollAxis,
    watch: Option<Rc<EdgeWatch>>,
) -> Element {
    // `bounces(false)`: a table's scroller is a bounded pane INSIDE a
    // page, and an overscroll spring there has no end to signal — the
    // thing that springs is not the thing the gesture appears to grab,
    // so it reads as a glitch rather than as a boundary. On web this
    // also stops the swipe chaining into the page behind the table once
    // the columns (or a scroll-y table's rows) run out.
    let scroller = scroller.axis(axis).bounces(false);
    let Some(watch) = watch else {
        return scroller.into_element();
    };
    let on_scroll = watch.clone();
    let scroller = scroller.on_scroll(move |x, _y| on_scroll.set_offset(x)).into_element();
    runtime_vocabulary::builders::view()
        .children(vec![scroller])
        .style(StyleProp::Static(Rc::new(edge_watch_holder_rules())))
        .on_handle(move |h| watch.measure(h, |w| &w.viewport))
        .build()
}

/// The viewport-measuring holder `edge_watched_scroller` puts around a
/// watched (plain `scroll_x`) table's scroller. It must pass the
/// surface's bound through to the scroller: an unstyled node's
/// automatic minimum size is its content (a flex item's `min-height:
/// auto` on web, Taffy's content-based minimum natively), so it grew to
/// every row and the scroller inside it had nothing to scroll — a
/// `scroll_y` plain table showed its first rows and cut the rest off.
/// Content-sized like the scroller (`flex_grow: 0`, `flex_basis:
/// auto`), shrinkable to 0 on both axes, and a column so the scroller
/// fills its width and takes its height.
fn edge_watch_holder_rules() -> StyleRules {
    StyleRules {
        flex_direction: Some(glue::FlexDirection::Column),
        flex_grow: Some(glue::Tokenized::Literal(0.0)),
        flex_shrink: Some(glue::Tokenized::Literal(1.0)),
        flex_basis: Some(glue::Tokenized::Literal(glue::Length::Auto)),
        min_height: Some(glue::Tokenized::Literal(glue::Length::Px(0.0))),
        min_width: Some(glue::Tokenized::Literal(glue::Length::Px(0.0))),
        ..Default::default()
    }
}

/// Every row a table child holds: the row itself, or a footer
/// section's rows (Owned-peeled). Rows come back in order.
fn for_each_row<'a>(el: &'a Element, f: &mut dyn FnMut(&'a Element)) {
    match el {
        Element::Owned { element, .. } => for_each_row(element, f),
        Element::Item { data, children, .. }
            if data.downcast_ref::<PrimCell<TableFootPrim>>().is_some() =>
        {
            for r in children {
                for_each_row(r, f);
            }
        }
        other => f(other),
    }
}

/// The span a built cell declares (Owned-peeled; either lowering).
fn cell_span(cell: &Element) -> ColSpan {
    match cell {
        Element::Owned { element, .. } => cell_span(element),
        Element::Item { data, .. } => {
            if let Some(m) = data.downcast_ref::<PrimCell<CellMarker>>() {
                let mut span = ColSpan::default();
                m.with_mut(|m| span = m.span);
                span
            } else if let Some(c) = data.downcast_ref::<PrimCell<TableCellPrim>>() {
                let mut span = ColSpan::default();
                c.with_mut(|c| span = c.span);
                span
            } else {
                ColSpan::default()
            }
        }
        _ => ColSpan::default(),
    }
}

/// Overwrite a built cell's span (either lowering) — how [`table`]
/// resolves [`ColSpan::Rest`] to a count before mount.
fn set_cell_span(cell: &Element, span: ColSpan) {
    match cell {
        Element::Owned { element, .. } => set_cell_span(element, span),
        Element::Item { data, .. } => {
            if let Some(m) = data.downcast_ref::<PrimCell<CellMarker>>() {
                m.with_mut(|m| m.span = span);
            } else if let Some(c) = data.downcast_ref::<PrimCell<TableCellPrim>>() {
                c.with_mut(|c| c.span = span);
            }
        }
        _ => {}
    }
}

/// The table's column count — its widest row, counting each cell's span
/// (a [`ColSpan::Rest`] counts one) — after which every `Rest` span is
/// rewritten to the count it covers in its row. Both lowerings run
/// this before mount: the browser needs a number in `colspan`, the grid
/// needs a line.
///
/// Rows the table cannot see at build time (a reactive row region) are
/// not counted, the same limit the native grid flattening already has.
fn resolve_spans(rows: &[Element]) -> usize {
    let mut columns = 0usize;
    for r in rows {
        for_each_row(r, &mut |row| {
            let mut n = 0usize;
            visit_row_cells(row, |c| n += cell_span(c).min_columns());
            columns = columns.max(n);
        });
    }
    for r in rows {
        for_each_row(r, &mut |row| {
            let mut at = 0usize;
            visit_row_cells(row, |c| {
                let span = cell_span(c);
                let n = span.resolve(columns.saturating_sub(at));
                if span == ColSpan::Rest {
                    set_cell_span(c, ColSpan::Columns(n as u16));
                }
                at += n;
            });
        });
    }
    columns
}

/// The [`ColumnSpec`] a built cell declares (Owned-peeled; either
/// lowering).
fn cell_column(cell: &Element) -> Option<ColumnSpec> {
    match cell {
        Element::Owned { element, .. } => cell_column(element),
        Element::Item { data, .. } => {
            let mut column = None;
            if let Some(m) = data.downcast_ref::<PrimCell<CellMarker>>() {
                m.with_mut(|m| column = m.column.clone());
            } else if let Some(c) = data.downcast_ref::<PrimCell<TableCellPrim>>() {
                c.with_mut(|c| column = c.column.clone());
            }
            column
        }
        _ => None,
    }
}

/// Each column's sizing: the first single-column cell, in row order, that
/// declares a [`ColumnSpec`]. Run after [`resolve_spans`].
fn column_specs(rows: &[Element], columns: usize) -> Vec<Option<ColumnSpec>> {
    let mut specs: Vec<Option<ColumnSpec>> = vec![None; columns];
    for r in rows {
        for_each_row(r, &mut |row| {
            let mut at = 0usize;
            visit_row_cells(row, |c| {
                let span = cell_span(c).resolve(columns.saturating_sub(at));
                if span == 1 && at < columns && specs[at].is_none() {
                    specs[at] = cell_column(c);
                }
                at += span;
            });
        });
    }
    specs
}

/// Whether the table lays out as grid columns (see [`ColumnTrack`]). Read
/// untracked: the mode is decided once, when the table is built.
fn needs_grid(specs: &[Option<ColumnSpec>]) -> bool {
    glue::untrack(|| specs.iter().flatten().any(|s| s.get().needs_grid()))
}

/// The table node's style in grid-column mode: `display: grid` and one
/// track per column, re-read live so a column width bound to a signal
/// moves its track. A column no cell sized is content-sized, like an
/// unsized [`ColumnTrack`].
fn grid_columns_style(specs: Vec<Option<ColumnSpec>>) -> StyleProp {
    StyleProp::Dynamic(Box::new(move || {
        Rc::new(StyleRules {
            display: Some(DisplayKind::Grid),
            grid_template_columns: Some(
                specs
                    .iter()
                    .map(|s| s.as_ref().map_or(TrackSize::MaxContent, |s| s.get().track()))
                    .collect(),
            ),
            ..Default::default()
        })
    }))
}

/// Prepare a web table's rows for grid-column mode.
///
/// - Footer sections move after every body row. A `<tfoot>` is
///   `display: contents` here, so the grid would place its rows wherever
///   they sit in the DOM; the browser's own `<tfoot>`-last rule does not
///   apply to grid items. Native flattens the same way.
/// - Every row, footer section and cell is flagged `grid` for its mount
///   handler (`display: contents` on rows and sections, a span as
///   `grid-column: span n`, explicit table roles).
/// - A row with a style or a bound handle gets the native lowering's
///   row-spanning BACKDROP as its first child — its `<tr>` has no box
///   to paint or measure — and, as on native, every cell is then placed
///   explicitly at its `(row, column)`: a spanning backdrop and auto-flow
///   cells cannot share a grid.
fn into_grid_rows(rows: Vec<Element>) -> Vec<Element> {
    fn is_foot(el: &Element) -> bool {
        match el {
            Element::Owned { element, .. } => is_foot(element),
            Element::Item { data, .. } => data.downcast_ref::<PrimCell<TableFootPrim>>().is_some(),
            _ => false,
        }
    }
    let (mut ordered, foot): (Vec<Element>, Vec<Element>) =
        rows.into_iter().partition(|r| !is_foot(r));
    ordered.extend(foot);

    let any_proxy = {
        let mut any = false;
        for r in &ordered {
            for_each_row(r, &mut |row| {
                if let Element::Item { data, .. } = row {
                    if let Some(c) = data.downcast_ref::<PrimCell<TableRowPrim>>() {
                        c.with_mut(|p| any |= p.style.is_some() || p.ref_fill.is_some());
                    }
                }
            });
        }
        any
    };

    let mut row_line = 0i16;
    let foot_from = ordered.iter().position(is_foot).unwrap_or(ordered.len());
    for r in ordered.iter_mut() {
        for_each_row_mut(r, &mut |row| {
            row_line += 1;
            // The row's slots (taken only when rows are placed explicitly).
            let mut slots: Option<TableRowPrim> = None;
            match row {
                Element::Item { data, .. } => {
                    if let Some(c) = data.downcast_ref::<PrimCell<TableRowPrim>>() {
                        c.with_mut(|p| {
                            p.grid = true;
                            if any_proxy && (p.style.is_some() || p.ref_fill.is_some()) {
                                slots = Some(TableRowPrim {
                                    style: p.style.take(),
                                    ref_fill: p.ref_fill.take(),
                                    grid: false,
                                });
                            }
                        });
                    } else if let Some(c) = data.downcast_ref::<PrimCell<TableFootPrim>>() {
                        c.with_mut(|p| p.grid = true);
                    }
                }
                _ => {}
            }
            let mut col_line = 1i16;
            visit_row_cells(row, |cell| {
                let span = cell_span(cell).resolve(1) as i16;
                visit_cell(cell, &mut |_, table_cell| {
                    if let Some(p) = table_cell {
                        p.grid = true;
                    }
                });
                if any_proxy {
                    place_cell(cell, row_line, col_line, span);
                    // The placement carries the span; no inline
                    // `grid-column: span n` to fight it.
                    set_cell_span(cell, ColSpan::Columns(1));
                }
                col_line += span;
            });
            if let Some(slots) = slots {
                if let Element::Item { children, .. } = row {
                    children.insert(0, build_row_backdrop(row_line, slots));
                }
            }
        });
    }
    // The body rows go in an explicit `<tbody>` (see [`TableBodyPrim`]);
    // footer sections follow it.
    let foot = ordered.split_off(foot_from);
    let mut sections = vec![item(PrimCell::new(TableBodyPrim), ordered)];
    sections.extend(foot);
    sections
}

/// [`for_each_row`], mutably — and footer sections are flagged on the
/// way through.
fn for_each_row_mut(el: &mut Element, f: &mut dyn FnMut(&mut Element)) {
    match el {
        Element::Owned { element, .. } => for_each_row_mut(element, f),
        Element::Item { data, children, .. }
            if data.downcast_ref::<PrimCell<TableFootPrim>>().is_some() =>
        {
            if let Some(c) = data.downcast_ref::<PrimCell<TableFootPrim>>() {
                c.with_mut(|p| p.grid = true);
            }
            for r in children.iter_mut() {
                for_each_row_mut(r, f);
            }
        }
        other => f(other),
    }
}

// ============================================================================
// Web lowering (scene items). The item constructors are always
// compiled so host tests can drive the registry handlers through the
// SSR backend; the wasm32 build arm below uses them for the app tree.
// ============================================================================

/// Item-payload constructors for the web lowering. `#[doc(hidden)]`:
/// test/handler plumbing, not author surface — apps go through
/// [`table`] / [`table_row`] / [`table_cell`], which pick the right
/// lowering per target.
#[doc(hidden)]
pub mod item_lowering {
    use super::*;

    /// `<table>` item. The caller resolves cell spans first
    /// ([`table`] does, through `resolve_spans`).
    pub fn table_item(children: Vec<Element>, style: Option<StyleProp>, scroll_x: bool) -> Element {
        scrolling_table_item(children, style, scroll_x, false)
    }

    /// `<table>` item for a table that may also own vertical scrolling
    /// ([`TableProps::scroll_y`]).
    pub fn scrolling_table_item(
        children: Vec<Element>,
        style: Option<StyleProp>,
        scroll_x: bool,
        scroll_y: bool,
    ) -> Element {
        item(
            PrimCell::new(TablePrim { style, scroll_x, scroll_y, grid: false, ref_fill: None }),
            children,
        )
    }

    /// The whole web table element — span resolution, the `<table>`
    /// item, and (scroll-x, a slot, or an author style) the surface
    /// wrapper. What
    /// [`table`] builds on wasm32; always compiled so host tests can
    /// render the real web shape through the SSR backend.
    pub fn web_table(
        rows: Vec<Element>,
        style: Option<StyleProp>,
        scroll_x: bool,
        header_slot: Option<Element>,
        footer_slot: Option<Element>,
    ) -> Element {
        web_table_from(TableParts {
            rows,
            style,
            scroll_x,
            scroll_y: false,
            header_slot,
            footer_slot,
            on_edges: None,
        })
    }

    /// [`web_table`] for a table that may also own vertical scrolling
    /// ([`TableProps::scroll_y`]).
    pub fn web_scrolling_table(
        rows: Vec<Element>,
        style: Option<StyleProp>,
        scroll_x: bool,
        scroll_y: bool,
    ) -> Element {
        web_table_from(TableParts {
            rows,
            style,
            scroll_x,
            scroll_y,
            header_slot: None,
            footer_slot: None,
            on_edges: None,
        })
    }

    /// [`web_table`] from gathered parts — what [`table`] builds on
    /// wasm32, edge watching included.
    pub(crate) fn web_table_from(parts: TableParts) -> Element {
        let columns = resolve_spans(&parts.rows);
        let specs = column_specs(&parts.rows, columns);
        let grid = needs_grid(&specs);
        let wrapped = parts.wrapped();
        let TableParts { rows, style, scroll_x, scroll_y, header_slot, footer_slot, on_edges } = parts;
        let watch = on_edges.filter(|_| scroll_x).map(EdgeWatch::new);
        // The `<table>` is the scrolled content, so its width is the
        // content width the edges are measured against.
        let ref_fill = watch.clone().map(|watch| {
            Box::new(move |h| watch.measure(h, |w| &w.content))
                as Box<dyn FnOnce(runtime_shared::ViewHandle)>
        });
        // A STYLED plain table is wrapped too, never styled on the
        // `<table>` itself: the reset gives the `<table>` `border-collapse:
        // collapse` (so the cells' dividers merge), and CSS ignores
        // `border-radius` on a table whose borders collapse — the author's
        // rounded frame rendered square. On a surface view the radius
        // applies, and the structure matches the native lowering, whose
        // author style always lands on an outer view around the grid.
        let table = if grid {
            item(
                PrimCell::new(TablePrim {
                    style: Some(grid_columns_style(specs)),
                    scroll_x,
                    scroll_y,
                    grid: true,
                    ref_fill,
                }),
                into_grid_rows(rows),
            )
        } else {
            item(
                PrimCell::new(TablePrim { style: None, scroll_x, scroll_y, grid: false, ref_fill }),
                rows,
            )
        };
        if !wrapped && style.is_none() {
            return table;
        }
        // Structure: styled SURFACE > [header slot, content, footer slot],
        // where content is the scroller > `<table>` in scroll-x / scroll-y
        // mode and the bare `<table>` otherwise (slots, or just a style). The author style
        // (border/radius/background — idea-ui's themed surface) sits
        // OUTSIDE the scroller so the frame stays put while the columns
        // scroll inside it; a surface inside the scroller rode along with
        // the content and clipped its own border at the viewport edge.
        // Sticky-pinned cells still pin against the scroller (their NEAREST
        // scroll ancestor — the surface's own overflow clip is further out).
        // The scroller is a flex item of the surface with the browser's
        // default `flex-shrink: 1`, and a scroll container's automatic
        // minimum height is 0, so a `max_height` on the surface bounds it.
        let content = if scroll_x || scroll_y {
            edge_watched_scroller(
                glue::scroll_view(vec![table]),
                scroller_axis(scroll_x, scroll_y),
                watch,
            )
        } else {
            table
        };
        surface_with_slots(content, style, header_slot, footer_slot)
    }

    /// `<tr>` item.
    pub fn row_item(children: Vec<Element>, style: Option<StyleProp>) -> Element {
        item(PrimCell::new(TableRowPrim { style, ref_fill: None, grid: false }), children)
    }

    /// `<td>` / `<th>` item.
    pub fn cell_item(
        header: bool,
        span: ColSpan,
        children: Vec<Element>,
        style: Option<StyleProp>,
    ) -> Element {
        item(
            PrimCell::new(TableCellPrim {
                header,
                span,
                style,
                on_touch: None,
                on_hover: None,
                ref_fill: None,
                column: None,
                grid: false,
            }),
            children,
        )
    }

    /// Set a built `<td>`/`<th>` item's column sizing — what
    /// [`table_cell`]'s `column` lands as on web.
    pub fn set_cell_column(cell: &Element, column: Option<ColumnSpec>) {
        let mut column = Some(column);
        visit_cell(cell, &mut |_, table_cell| {
            if let (Some(p), Some(c)) = (table_cell, column.take()) {
                p.column = c;
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
fn build_table(parts: TableParts) -> Element {
    item_lowering::web_table_from(parts)
}

#[cfg(target_arch = "wasm32")]
fn build_row(children: Vec<Element>, style: Option<StyleProp>) -> Element {
    item_lowering::row_item(children, style)
}

#[cfg(target_arch = "wasm32")]
fn build_cell(
    header: bool,
    span: ColSpan,
    column: Option<ColumnSpec>,
    children: Vec<Element>,
    style: Option<StyleProp>,
) -> Element {
    let cell = item_lowering::cell_item(header, span, children, style);
    item_lowering::set_cell_column(&cell, column);
    cell
}

// ============================================================================
// Native lowering (CSS-grid via the glue view builder).
// ============================================================================

/// Native `Table`: outer unstyled passthrough (the author-style target)
/// wrapping the inner grid whose direct children are ALL cells from ALL
/// rows. Peeled row scopes re-attach around the finished element (see
/// the module docs).
///
/// Rows with proxy slots (a bound handle or a row style — see
/// [`TableRowPrim`]) additionally get a row-spanning BACKDROP view.
/// Because a spanning grid item and auto-flow cells cannot mix (auto
/// placement skips occupied cells, which would push every cell out of
/// its row — see [`runtime_shared::GridPlacement`]), the moment ANY row
/// carries a proxy, EVERY cell is placed explicitly at its
/// `(grid_row, grid_column)`. The `table-layout: auto` water-fill in
/// `runtime-layout` attributes explicitly-placed cells to their named
/// column and skips spanning backdrops, so column sizing is identical
/// either way (`regression_table_grid_with_row_backdrops_keeps_column_sizing`
/// pins this).
#[cfg(not(target_arch = "wasm32"))]
fn build_table(parts: TableParts) -> Element {
    let wrapped = parts.wrapped();
    let TableParts { rows, style, scroll_x, scroll_y, header_slot, footer_slot, on_edges } = parts;
    let watch = on_edges.filter(|_| scroll_x).map(EdgeWatch::new);
    let columns_from_spans = resolve_spans(&rows);
    // Read before the rows are flattened: the specs ride the cells.
    let specs = column_specs(&rows, columns_from_spans);
    let mut owneds: Vec<glue::Owned> = Vec::new();
    let mut body: Vec<NativeRow> = Vec::new();
    let mut foot: Vec<NativeRow> = Vec::new();
    for row in rows {
        extract_rows(row, &mut owneds, &mut body, &mut foot, false);
    }
    // A footer section renders after every body row wherever the author
    // put it — what the browser does with `<tfoot>`.
    let mut extracted = body;
    extracted.extend(foot);
    let columns = extracted
        .iter()
        .map(|r| r.spans.iter().sum::<usize>())
        .max()
        .unwrap_or(0)
        .max(columns_from_spans);
    let any_proxy = extracted
        .iter()
        .any(|r| r.slots.as_ref().is_some_and(|s| s.style.is_some() || s.ref_fill.is_some()));
    // A spanning cell is a multi-track grid item, and auto-flow cannot
    // carry one safely: the layout's column attribution counts auto-flow
    // children in document order, and a span occupies several tracks
    // while counting as one child. Explicit placement throughout, as
    // for row proxies.
    let any_span = extracted.iter().any(|r| r.spans.iter().any(|s| *s != 1));

    let mut grid_children: Vec<Element> = Vec::new();
    if !any_proxy && !any_span {
        // Fast path — auto-flow, exactly the pre-proxy lowering.
        for row in extracted {
            grid_children.extend(row.cells);
        }
    } else {
        for (r, row) in extracted.into_iter().enumerate() {
            let row_line = (r + 1) as i16;
            if let Some(slots) = row.slots {
                if slots.style.is_some() || slots.ref_fill.is_some() {
                    grid_children.push(build_row_backdrop(row_line, slots));
                }
            }
            let mut col_line = 1i16;
            for (cell, span) in row.cells.into_iter().zip(row.spans) {
                place_cell(&cell, row_line, col_line, span as i16);
                col_line += span as i16;
                grid_children.push(cell);
            }
        }
    }

    // Inner node: the actual grid. Its sheet (display:grid + N tracks)
    // is one level below the author-style target so it survives a
    // `.with_style(...)` on the outer node. All-`auto` tracks get the
    // browser auto-table sizing in `runtime-layout`; a column with
    // `flex` / `max_width` makes them explicit grid tracks instead.
    let grid = glue::view(grid_children);
    let inner = if needs_grid(&specs) {
        grid.with_style(grid_columns_style(specs)).into_element()
    } else {
        grid.with_style(native_styles::grid_sheet(columns)).into_element()
    };
    let mut el = if scroll_x || scroll_y {
        // Structure: styled SURFACE > [header slot, scroll_view >
        // content > grid, footer slot] — the web lowering's mirror. The
        // author-style surface stays OUTSIDE the scroller so its
        // border/radius don't ride along with the scrolled columns or
        // rows. With `scroll_x` the content node carries the "at least
        // the scroller's width" floor so a narrow table still fills while
        // a wide one overflows and scrolls. Sticky cells (frozen columns,
        // a pinned header) register against this scroll view. The
        // scroller is content-sized but shrinkable (a scroll node's
        // automatic minimum size is 0), so a `max_height` on the surface
        // bounds it and the rows scroll — see `scroll_wrapper_rules`.
        // The content node is what scrolls, so its width is the content
        // width the edges are measured against.
        let mut content = runtime_vocabulary::builders::view()
            .children(vec![inner])
            .style(StyleProp::Static(Rc::new(native_styles::scroll_content_rules(scroll_x))));
        if let Some(watch) = watch.clone() {
            content = content.on_handle(move |h| watch.measure(h, |w| &w.content));
        }
        let scroller = edge_watched_scroller(
            glue::scroll_view(vec![content.build()]).with_style(StyleProp::Static(Rc::new(
                native_styles::scroll_wrapper_rules(scroll_x),
            ))),
            scroller_axis(scroll_x, scroll_y),
            watch,
        );
        surface_with_slots(scroller, style, header_slot, footer_slot)
    } else if wrapped {
        // Slots without scroll-x: surface > [header, grid holder,
        // footer]. The holder mirrors the plain table's outer node so
        // the grid still fills the surface's width.
        let holder = glue::view(vec![inner]).into_element();
        surface_with_slots(holder, style, header_slot, footer_slot)
    } else {
        // Outer node: the author-style target. The framework's default
        // cross-axis stretch makes the inner grid fill this node's width.
        let outer = glue::view(vec![inner]);
        match style {
            Some(style) => outer.with_style(style).into_element(),
            None => outer.into_element(),
        }
    };
    // Re-attach every peeled row scope: the cells' reactive props (the
    // clickable-row hover style) read signals those scopes own, so they
    // must live exactly as long as the flattened subtree.
    for owned in owneds {
        el = runtime_scene::owned(el, owned);
    }
    el
}

/// One native row, mid-flatten: its cells plus the row marker's slots
/// (if the row lowered to a [`TableRowPrim`] marker).
#[cfg(not(target_arch = "wasm32"))]
struct NativeRow {
    cells: Vec<Element>,
    /// Columns each cell covers (parallel to `cells`, spans resolved).
    spans: Vec<usize>,
    slots: Option<TableRowPrim>,
}

/// Build the row-proxy backdrop: an empty view spanning every column of
/// its grid row, inserted BEFORE the row's cells so it paints beneath
/// them. Carries the row's style and hands its handle to `ref_fill` at
/// mount — the geometry surface for drop targeting / row frames. Native,
/// and a grid-column web table (whose `<tr>` is `display: contents`, so
/// it has no box of its own to hand out).
fn build_row_backdrop(row_line: i16, slots: TableRowPrim) -> Element {
    use runtime_shared::GridPlacement;
    let placement = StyleRules {
        grid_row: Some(GridPlacement::Line(row_line)),
        grid_column: Some(GridPlacement::SPAN_ALL),
        ..Default::default()
    };
    let style = compose_rules(slots.style, placement);
    let mut b = runtime_vocabulary::builders::view().style(style);
    if let Some(fill) = slots.ref_fill {
        b = b.on_handle(move |h| fill(h));
    }
    b.build()
}

/// Native `TableRow`: lowers to a [`TableRowPrim`] marker item wrapping
/// the cells. [`table`] consumes the marker at build time (flattening
/// the cells into the grid and turning the slots into the row
/// backdrop); it must not escape into realize — a `table_row` used
/// outside a `table` panics there as an unregistered payload, which is
/// the loud failure the scene registry promises.
#[cfg(not(target_arch = "wasm32"))]
fn build_row(children: Vec<Element>, style: Option<StyleProp>) -> Element {
    item(PrimCell::new(TableRowPrim { style, ref_fill: None, grid: false }), children)
}

/// Native `TableCell`: a plain view that becomes a grid item; the
/// column track sizes its width. Author style REPLACES the SDK's
/// default cell sheet. A cell covering more than one column, or
/// declaring a [`ColumnSpec`], is wrapped in a [`CellMarker`] that
/// [`table`] unwraps.
#[cfg(not(target_arch = "wasm32"))]
fn build_cell(
    _header: bool,
    span: ColSpan,
    column: Option<ColumnSpec>,
    children: Vec<Element>,
    style: Option<StyleProp>,
) -> Element {
    let styled = match style {
        Some(style) => glue::view(children).with_style(style),
        None => glue::view(children).with_style(native_styles::cell_sheet()),
    };
    let cell = styled.into_element();
    if span.is_single() && column.is_none() {
        cell
    } else {
        item(PrimCell::new(CellMarker { span, column }), vec![cell])
    }
}

/// Unwrap a native cell's [`CellMarker`] (Owned-peeled, scope
/// re-attached), returning the bare cell view and its column count.
#[cfg(not(target_arch = "wasm32"))]
fn unwrap_cell(cell: Element) -> (Element, usize) {
    match cell {
        Element::Owned { element, owned } => {
            let (inner, span) = unwrap_cell(*element);
            (runtime_scene::owned(inner, owned), span)
        }
        Element::Item { data, mut children, .. }
            if data.downcast_ref::<PrimCell<CellMarker>>().is_some() =>
        {
            let span = data
                .downcast_ref::<PrimCell<CellMarker>>()
                .expect("guard just checked this downcast")
                .take()
                .span
                .resolve(1);
            let inner = children.pop().expect("a span marker wraps exactly one cell");
            (inner, span)
        }
        other => (other, 1),
    }
}

/// Pull a table child's rows out so their cells can be parented
/// directly under the grid, sorting them into body and footer. A row
/// lowers to a [`TableRowPrim`] marker item (whose children are the
/// cells); a [`TableFootPrim`] section holds rows; a `#[component]` body
/// that created reactive state arrives `Owned`-wrapped — peel it and
/// KEEP the scope (pushed into `owneds`, re-attached by the caller). A
/// bare fragment (legacy shape) is a slotless row; any other stray
/// element is treated as a single one-cell row so nothing silently
/// vanishes.
#[cfg(not(target_arch = "wasm32"))]
fn extract_rows(
    el: Element,
    owneds: &mut Vec<glue::Owned>,
    body: &mut Vec<NativeRow>,
    foot: &mut Vec<NativeRow>,
    in_foot: bool,
) {
    let push = |row: NativeRow, body: &mut Vec<NativeRow>, foot: &mut Vec<NativeRow>| {
        if in_foot {
            foot.push(row)
        } else {
            body.push(row)
        }
    };
    let row_of = |cells: Vec<Element>, slots: Option<TableRowPrim>| {
        let (cells, spans): (Vec<Element>, Vec<usize>) = cells.into_iter().map(unwrap_cell).unzip();
        NativeRow { cells, spans, slots }
    };
    match el {
        // Guarded rather than destructure-and-rebuild: rebuilding an
        // `Element::Item` from its parts DROPS anything else the variant
        // carries (under `ui-overlay`, the node's origin tag), so a
        // non-row item is passed through whole by the catch-all arm
        // below instead.
        Element::Item { data, children, .. }
            if data.downcast_ref::<PrimCell<TableRowPrim>>().is_some() =>
        {
            let cell = data
                .downcast_ref::<PrimCell<TableRowPrim>>()
                .expect("guard just checked this downcast");
            push(row_of(children, Some(cell.take())), body, foot);
        }
        Element::Item { data, children, .. }
            if data.downcast_ref::<PrimCell<TableFootPrim>>().is_some() =>
        {
            for r in children {
                extract_rows(r, owneds, body, foot, true);
            }
        }
        Element::Fragment(children) => push(row_of(children, None), body, foot),
        Element::Owned { element, owned } => {
            owneds.push(owned);
            extract_rows(*element, owneds, body, foot, in_foot);
        }
        other => push(row_of(vec![other], None), body, foot),
    }
}

/// Layer `overlay` rules onto whatever [`StyleProp`] shape a node
/// already carries, returning the composed prop. Grid plumbing: native
/// always, web only for a grid-column table (an auto `<table>` takes no
/// placement).
fn compose_rules(prop: Option<StyleProp>, overlay: StyleRules) -> StyleProp {
    match prop {
        None => StyleProp::Static(Rc::new(overlay)),
        Some(StyleProp::Static(rules)) => {
            StyleProp::Static(Rc::new((*rules).clone().merge(&overlay)))
        }
        Some(StyleProp::Dynamic(f)) => {
            let overlay = Rc::new(overlay);
            StyleProp::Dynamic(Box::new(move || {
                Rc::new((*f()).clone().merge(&overlay))
            }))
        }
        Some(StyleProp::Sheet(app)) => {
            StyleProp::Sheet(Box::new(app.with_overrides(overlay)))
        }
        Some(StyleProp::SheetDynamic(f)) => {
            let overlay_cell = std::cell::RefCell::new(overlay);
            StyleProp::SheetDynamic(Box::new(move || {
                f().with_overrides(overlay_cell.borrow().clone())
            }))
        }
        Some(other) => {
            // Remaining shapes (signal-class selection) can't carry an
            // overlay; keep them and say so once rather than silently
            // dropping the placement.
            runtime_shared::unsupported::warn_once(
                "table.style_compose",
                "table SDK: a cell/backdrop style shape that cannot carry \
                 grid placement or width-floor overlays (signal-class \
                 selection) — the overlay was skipped; the table's layout \
                 may be wrong",
            );
            other
        }
    }
}

/// Post-process a built cell with its explicit grid position — the native
/// grid item's style, or a grid-column web table's `<td>`/`<th>`.
/// Reaches through the payload cell like [`set_cell_style`]; sound for
/// the same reason (pre-mount, taken exactly once by realize).
fn place_cell(cell: &Element, row_line: i16, col_line: i16, span: i16) {
    use runtime_shared::GridPlacement;
    let placement = StyleRules {
        grid_row: Some(GridPlacement::Line(row_line)),
        grid_column: Some(if span > 1 {
            GridPlacement::Lines(col_line, col_line + span)
        } else {
            GridPlacement::Line(col_line)
        }),
        ..Default::default()
    };
    let mut placement = Some(placement);
    visit_cell(cell, &mut |view, table_cell| {
        let slot = match (view, table_cell) {
            (Some(p), _) => &mut p.style,
            (_, Some(p)) => &mut p.style,
            _ => return,
        };
        if let Some(rules) = placement.take() {
            *slot = Some(compose_rules(slot.take(), rules));
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
mod native_styles {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// The sizing function applied to every table column: `auto`.
    ///
    /// An all-`Auto` column grid is the signal `runtime-layout` uses to
    /// run its `table-layout: auto` column sizing (short columns hug
    /// content, a text-heavy column absorbs the remaining width and
    /// wraps). `min-content` is unusable as a track floor here —
    /// glyphon reports ~0 for a long unbroken token — so the water-fill
    /// column sizing in `runtime-layout` measures per-cell max-content
    /// instead.
    fn column_track() -> TrackSize {
        TrackSize::Auto
    }

    runtime_tls::thread_local! {
        // Cache one grid sheet per column count — the style resolver
        // dedups applications by sheet pointer, so reusing a sheet
        // across same-width tables keeps the class table small.
        static GRID_SHEETS: RefCell<HashMap<usize, Rc<StyleSheet>>> = RefCell::new(HashMap::new());
        static CELL_SHEET: RefCell<Option<Rc<StyleSheet>>> = RefCell::new(None);
    }

    /// Grid sheet for an `n`-column table: `display: grid` plus `n`
    /// identical column tracks (row-major auto-flow re-groups the
    /// flattened cells into rows).
    pub(super) fn grid_sheet(n: usize) -> Rc<StyleSheet> {
        // An empty table has no columns; a 1-track grid lays out a
        // single column harmlessly.
        let n = n.max(1);
        GRID_SHEETS.with(|slot| {
            slot.borrow_mut()
                .entry(n)
                .or_insert_with(|| {
                    let tracks: Vec<TrackSize> = (0..n).map(|_| column_track()).collect();
                    Rc::new(StyleSheet::new(move |_vs: &VariantSet| StyleRules {
                        display: Some(DisplayKind::Grid),
                        grid_template_columns: Some(tracks.clone()),
                        ..Default::default()
                    }))
                })
                .clone()
        })
    }

    /// The scroll content node's rules. With `scroll_x`, a width floor:
    /// at least the scroller's width (percent resolves against the enclosing scroll
    /// view), free to grow past it — the native mirror of the web
    /// `min-width: 100%; width: max-content` strategy. `flex_shrink: 0`
    /// is load-bearing: without it the scroller's flex line squeezes
    /// the surface back to the viewport width and nothing ever
    /// overflows (the horizontal twin of the vertical min-height-percent
    /// shrink trap). Pinned by
    /// `regression_scroll_x_table_floors_at_scroller_width_and_overflows_past_it`
    /// in `runtime-layout`.
    ///
    /// A scroll-y-only table (`scroll_x == false`) takes no floor: its
    /// content is a cross-axis child of a column scroller and stretches
    /// to the scroller's width, so the columns fill and wrap as in a
    /// plain table. `flex_shrink: 0` still holds the rows at their full
    /// height on the scroll axis — without it they would squeeze into
    /// the bounded scroller and nothing would scroll.
    pub(super) fn scroll_content_rules(scroll_x: bool) -> StyleRules {
        StyleRules {
            min_width: scroll_x
                .then(|| glue::Tokenized::Literal(glue::Length::Percent(100.0))),
            flex_shrink: Some(glue::Tokenized::Literal(0.0)),
            ..Default::default()
        }
    }

    /// Style for the scroller itself. With `scroll_x`: content on the X
    /// MAIN axis, and CONTENT height. With the default Column direction the
    /// table content would be a CROSS-axis child — stretch-clamped to
    /// the scroller's width, never able to overflow it, so there would
    /// be nothing to scroll. `flex_grow: 0` + `flex_basis: auto`
    /// override the scroll primitive's fill-the-parent seed
    /// (`flex_grow: 1` / `flex_basis: 0`): inside the content-sized
    /// table surface that seed collapses the scroller to zero height —
    /// the same trap the idea-ui Modal hit
    /// (`regression_modal_scroller_content_sized_then_capped`).
    ///
    /// A `scroll_x` scroller also aligns its content to the START of the
    /// cross (Y) axis: a two-axis (`scroll_y`) scroller bounded by the
    /// surface is shorter than its content, and a stretched content node
    /// would be clamped to that height with the rows spilling out of it.
    /// A scroll-y-only scroller keeps the default Column direction, where
    /// Y is the main axis and the content's `flex_shrink: 0` holds it.
    /// Either way the scroller stays shrinkable (Taffy's default
    /// `flex_shrink: 1`, and a scroll node's automatic minimum is 0), so a
    /// bounded surface caps it. Pinned by
    /// `two_axis_table_scroller_hugs_short_content_and_caps_tall_content`
    /// in `runtime-layout`.
    pub(super) fn scroll_wrapper_rules(scroll_x: bool) -> StyleRules {
        StyleRules {
            flex_direction: scroll_x.then_some(glue::FlexDirection::Row),
            align_items: scroll_x.then_some(glue::AlignItems::FlexStart),
            flex_grow: Some(glue::Tokenized::Literal(0.0)),
            flex_basis: Some(glue::Tokenized::Literal(glue::Length::Auto)),
            ..Default::default()
        }
    }

    pub(super) fn cell_sheet() -> Rc<StyleSheet> {
        CELL_SHEET.with(|slot| {
            slot.borrow_mut()
                .get_or_insert_with(|| {
                    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
                        // A cell is a grid item; the column track sizes
                        // its width. Stack the cell's own content
                        // vertically so multi-line content wraps
                        // naturally inside the column.
                        flex_direction: Some(glue::FlexDirection::Column),
                        ..Default::default()
                    }))
                })
                .clone()
        })
    }
}

// ============================================================================
// Built-cell post-processing — the payload-side home of the cell
// introspection idea-ui's clickable-row feature needs (see the crate
// docs). Both lowerings are handled, so the callers stay
// target-agnostic.
// ============================================================================

/// Layer reactive axis selections over a built cell's EXISTING style,
/// whatever shape that style has. Returns whether a cell style was
/// actually wrapped.
///
/// This is what a row overlay reaches for. A read-then-replace pair
/// (read the style, write it back with [`set_cell_style`]) can only see
/// a STATIC [`StyleProp::Sheet`], so a cell whose
/// style is already reactive — a column pinned to a width that moves
/// under a resize drag, say — would silently keep its style and lose the
/// overlay entirely, with nothing logged. The failure is invisible in
/// the common case (most cells in the row are static, so the row looks
/// hovered) and reads as one column refusing to take the row's
/// highlight.
///
/// Both reactive shapes end up as [`StyleProp::SheetDynamic`]: a static
/// application is captured and re-mapped per evaluation, a dynamic one
/// is composed with `f` around it, so the cell's own reactive inputs
/// keep their subscriptions and `f`'s are added to them.
///
/// A style the sheet engine cannot see through — a preminted class, a
/// resolved-rules closure, or no style at all — is left exactly as it
/// was and `false` comes back: composition needs an application, and
/// there is none to compose with.
///
/// Sound for the same reason [`set_cell_style`] is: the payload is not
/// yet mounted, and realization takes it exactly once after this
/// returns.
pub fn map_cell_style(
    cell: &Element,
    f: Rc<dyn Fn(StyleApplication) -> StyleApplication>,
) -> bool {
    let mut f = Some(f);
    let mut wrapped = false;
    visit_cell(cell, &mut |view, table_cell| {
        let slot = if let Some(p) = view {
            &mut p.style
        } else if let Some(p) = table_cell {
            &mut p.style
        } else {
            return;
        };
        let Some(f) = f.take() else { return };
        match slot.take() {
            Some(StyleProp::Sheet(app)) => {
                let app = *app;
                *slot = Some(StyleProp::SheetDynamic(Box::new(move || f(app.clone()))));
                wrapped = true;
            }
            Some(StyleProp::SheetDynamic(g)) => {
                *slot = Some(StyleProp::SheetDynamic(Box::new(move || f(g()))));
                wrapped = true;
            }
            // Not an application — put it back untouched.
            other => *slot = other,
        }
    });
    wrapped
}

/// Re-select a built cell's sheet application WITHOUT making it
/// reactive: a static [`StyleProp::Sheet`] stays a static sheet (so it
/// can still premint), a reactive [`StyleProp::SheetDynamic`] has `f`
/// composed around it. Returns whether a cell style was mapped.
///
/// The static counterpart of [`map_cell_style`], for row- and
/// table-level settings that are fixed when the table is built — a
/// row's tone, a footer section, a table's density. Reaching for
/// [`map_cell_style`] there would turn every cell's style into a
/// closure for a selection that never changes.
pub fn map_cell_application(
    cell: &Element,
    f: Rc<dyn Fn(StyleApplication) -> StyleApplication>,
) -> bool {
    let mut f = Some(f);
    let mut mapped = false;
    visit_cell(cell, &mut |view, table_cell| {
        let slot = if let Some(p) = view {
            &mut p.style
        } else if let Some(p) = table_cell {
            &mut p.style
        } else {
            return;
        };
        let Some(f) = f.take() else { return };
        match slot.take() {
            Some(StyleProp::Sheet(app)) => {
                *slot = Some(StyleProp::Sheet(Box::new(f(*app))));
                mapped = true;
            }
            Some(StyleProp::SheetDynamic(g)) => {
                *slot = Some(StyleProp::SheetDynamic(Box::new(move || f(g()))));
                mapped = true;
            }
            other => *slot = other,
        }
    });
    mapped
}

/// A built cell's sheet application, when its style is a STATIC
/// [`StyleProp::Sheet`] — `None` for a reactive style (whose application
/// exists only when it is evaluated) or a non-cell. Lets a table-level
/// pass read what a cell selected (idea-ui reads its `pinned` arm) and
/// leave the cells it has no business with static.
pub fn cell_application(cell: &Element) -> Option<StyleApplication> {
    let mut app = None;
    visit_cell(cell, &mut |view, table_cell| {
        let slot = match (view, table_cell) {
            (Some(p), _) => &p.style,
            (_, Some(p)) => &p.style,
            _ => return,
        };
        if let Some(StyleProp::Sheet(a)) = slot {
            app = Some((**a).clone());
        }
    });
    app
}

/// Visit every row a table child holds — the row itself, or each row
/// of a footer section ([`table_foot`]) — in order, Owned-peeled.
/// Combine with [`visit_row_cells`] to reach every cell of a table
/// before it is built (idea-ui's table density does).
pub fn visit_rows(el: &Element, mut f: impl FnMut(&Element)) {
    for_each_row(el, &mut |r| f(r));
}

/// Visit the row a table's children will RENDER last: the last row of the
/// last footer section when there is one (a footer renders after every
/// body row wherever it was written — `<tfoot>` on web, [`build_table`]'s
/// body-then-foot order on native), otherwise the last body row. No call
/// for a table with no rows. Owned-peeled, like [`visit_rows`].
///
/// The theme layer uses it to drop the last row's divider, which would
/// otherwise stack on the table frame's own bottom border.
pub fn visit_last_rendered_row(children: &[Element], f: impl FnOnce(&Element)) {
    fn is_foot(el: &Element) -> bool {
        match el {
            Element::Owned { element, .. } => is_foot(element),
            Element::Item { data, .. } => data.downcast_ref::<PrimCell<TableFootPrim>>().is_some(),
            _ => false,
        }
    }
    let any_foot = children.iter().any(is_foot);
    let mut last: Option<&Element> = None;
    for child in children.iter().filter(|c| is_foot(c) == any_foot) {
        for_each_row(child, &mut |r| last = Some(r));
    }
    if let Some(row) = last {
        f(row);
    }
}

/// Replace a built cell's style in place (no-op for non-cells, so a
/// caller that hands over an unexpected element shape is harmless).
/// Sound because the payload is not yet mounted: realization takes it
/// exactly once, after this returns.
pub fn set_cell_style(cell: &Element, style: impl IntoStyleProp) {
    let prop = style.into_style_prop();
    let mut prop = Some(prop);
    visit_cell(cell, &mut |view, table_cell| {
        if let Some(p) = view {
            p.style = prop.take();
        } else if let Some(p) = table_cell {
            p.style = prop.take();
        }
    });
}

/// Attach clickable-row interaction to a built cell: the tap handler
/// and the shared row-hover reporter land on the cell's own node
/// (native grid item / web `<td>`-`<th>`), so a button inside a
/// clickable row still eats its own click.
pub fn set_cell_interaction(cell: &Element, on_touch: TouchHandler, on_hover: HoverHandler) {
    let mut handlers = Some((on_touch, on_hover));
    visit_cell(cell, &mut |view, table_cell| {
        if let Some(p) = view {
            if let Some((t, h)) = handlers.take() {
                p.on_touch = Some(t);
                p.on_hover = Some(h);
            }
        } else if let Some(p) = table_cell {
            if let Some((t, h)) = handlers.take() {
                p.on_touch = Some(t);
                p.on_hover = Some(h);
            }
        }
    });
}

/// Bind a built row's proxy surface: `fill` receives the row's
/// [`ViewHandle`](runtime_shared::ViewHandle) at mount — the `<tr>`'s
/// on web, the row-spanning backdrop view's on native (emitting the
/// backdrop is what setting this slot opts the table into; see
/// [`TableRowPrim`]). Row GEOMETRY flows through this handle — drag &
/// drop's `Droppable::bind`, row frame reads. Row TOUCH does not:
/// install touch/hover per cell via [`set_cell_interaction`] (a
/// native backdrop is a sibling of the cells and never receives
/// touches landing on cell content).
///
/// No-op for non-row elements. Sound for the same reason the cell
/// helpers are: the payload is pre-mount, taken exactly once by the
/// consumer ([`table`] natively, realize on web).
pub fn bind_row(row: &Element, fill: impl FnOnce(runtime_shared::ViewHandle) + 'static) {
    fn walk(el: &Element, fill: &mut Option<Box<dyn FnOnce(runtime_shared::ViewHandle)>>) {
        match el {
            Element::Owned { element, .. } => walk(element, fill),
            Element::Item { data, .. } => {
                if let Some(c) = data.downcast_ref::<PrimCell<TableRowPrim>>() {
                    c.with_mut(|p| {
                        if let Some(f) = fill.take() {
                            p.ref_fill = Some(f);
                        }
                    });
                }
            }
            _ => {}
        }
    }
    let mut slot: Option<Box<dyn FnOnce(runtime_shared::ViewHandle)>> = Some(Box::new(fill));
    walk(row, &mut slot);
}

/// Visit a built row's cells in order (Owned-peeled). The visitor gets
/// each cell ELEMENT — combine with [`set_cell_touch`] /
/// [`set_cell_style`] / [`bind_cell`] for per-cell fan-out of row-level
/// behavior (the pattern row interaction uses on every lowering).
pub fn visit_row_cells(row: &Element, mut f: impl FnMut(&Element)) {
    fn walk(el: &Element, f: &mut dyn FnMut(&Element)) {
        match el {
            Element::Owned { element, .. } => walk(element, f),
            Element::Item { data, children, .. } => {
                if data.downcast_ref::<PrimCell<TableRowPrim>>().is_some() {
                    for c in children {
                        f(c);
                    }
                }
            }
            // Legacy fragment shape (a stray non-marker row).
            Element::Fragment(children) => {
                for c in children {
                    f(c);
                }
            }
            _ => {}
        }
    }
    walk(row, &mut f);
}

/// Install ONLY a touch handler on a built cell — [`set_cell_interaction`]
/// without the hover half, for behaviors that have no hover component
/// (a drag recognizer). Installing a no-op hover handler instead would
/// be the silent-no-op shape idea-ui's rule 9.6 forbids.
pub fn set_cell_touch(cell: &Element, on_touch: TouchHandler) {
    let mut handler = Some(on_touch);
    visit_cell(cell, &mut |view, table_cell| {
        if let Some(p) = view {
            if let Some(t) = handler.take() {
                p.on_touch = Some(t);
            }
        } else if let Some(p) = table_cell {
            if let Some(t) = handler.take() {
                p.on_touch = Some(t);
            }
        }
    });
}

/// Bind a built cell's node handle: `fill` receives the cell's
/// [`ViewHandle`](runtime_shared::ViewHandle) at mount — the native
/// grid item's or the web `<td>`/`<th>`'s. Animated drag offsets
/// anchor here (`AnimatedValue::bind` on a `Ref` filled by this).
pub fn bind_cell(cell: &Element, fill: impl FnOnce(runtime_shared::ViewHandle) + 'static) {
    let mut slot: Option<Box<dyn FnOnce(runtime_shared::ViewHandle)>> = Some(Box::new(fill));
    visit_cell(cell, &mut |view, table_cell| {
        if let Some(p) = view {
            if let Some(f) = slot.take() {
                p.ref_fill = Some(f);
            }
        } else if let Some(p) = table_cell {
            if let Some(f) = slot.take() {
                p.ref_fill = Some(f);
            }
        }
    });
}

/// Shared Owned-peeling walk for the post-processing helpers: calls `f`
/// with whichever cell payload shape the element carries (exactly one
/// of the two arguments is `Some`).
fn visit_cell(
    cell: &Element,
    f: &mut dyn FnMut(Option<&mut prims::ViewPrim>, Option<&mut TableCellPrim>),
) {
    match cell {
        Element::Owned { element, .. } => visit_cell(element, f),
        Element::Item { data, children, .. }
            if data.downcast_ref::<PrimCell<CellMarker>>().is_some() =>
        {
            // A spanning native cell: the view is the marker's child.
            if let Some(inner) = children.first() {
                visit_cell(inner, f);
            }
        }
        Element::Item { data, .. } => {
            if let Some(c) = data.downcast_ref::<PrimCell<prims::ViewPrim>>() {
                c.with_mut(|p| f(Some(p), None));
            } else if let Some(c) = data.downcast_ref::<PrimCell<TableCellPrim>>() {
                c.with_mut(|p| f(None, Some(p)));
            }
        }
        _ => {}
    }
}

// ============================================================================
// Registry handlers (web lowering). Generic over the caps traits so the
// SSR backend reuses them — the same contract as the vocabulary's own
// handlers and the website's codeblock precedent.
// ============================================================================

/// Mount the `<table>` container: `create_element("table")`, the
/// browser-default reset set inline (`border-collapse: collapse;
/// width: 100%; table-layout: auto`), children, then the author style.
fn mount_table<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<PrimCell<TablePrim>>,
    children: Vec<Element>,
) -> H::Node
where
    H: StyleServices + InputOps + IntrospectionOps,
{
    let data = prim.take();
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_element("table");
    // Robot identity, so the cells below parent into a real tree.
    let _robot = register_extension_node(&backend, &node, None);
    if data.grid {
        // Its rows are `display: contents`, which drops the implicit
        // table semantics in some engines; say them explicitly.
        backend.borrow().attach_html_attribute(&node, "role", "table");
    }
    {
        // Reset the browser's default table chrome — apps style via the
        // stylesheet system. `border-collapse: collapse` keeps the cell
        // borders the author draws from doubling up. Inline
        // declarations (not a class), so `attach_style` below still
        // wins where the author sets the same property.
        let b = backend.borrow();
        if data.scroll_x || data.scroll_y {
            // Scrolling border model: `separate` (spacing 0), NOT
            // `collapse`. Collapsed borders belong to the table's own
            // paint layer — the layer that scrolls — so a sticky cell
            // (a frozen column, a pinned header) kept its dividers
            // behind while its opaque background rode the pin over them:
            // hairlines vanished into white-on-white. `separate` makes
            // every cell paint its own borders, which then travel with
            // the pinned cell. The idea-ui cells draw only their bottom
            // (+ pin-edge) hairlines, so nothing doubles up without
            // collapsing.
            b.attach_html_style(&node, "border-collapse", "separate");
            b.attach_html_style(&node, "border-spacing", "0");
        } else {
            b.attach_html_style(&node, "border-collapse", "collapse");
        }
        if data.scroll_x {
            // Width strategy: natural column widths (a table at
            // `width: max-content` never wraps its cells), floored
            // at the scroller's width so a narrow table still fills.
            // `width: 100%` would instead squeeze and wrap the columns
            // — no overflow, nothing to scroll.
            b.attach_html_style(&node, "min-width", "100%");
            b.attach_html_style(&node, "width", "max-content");
        } else {
            b.attach_html_style(&node, "width", "100%");
        }
        b.attach_html_style(&node, "table-layout", "auto");
    }
    cx.realize_children_into(&mut node, children);
    if let Some(style) = data.style {
        attach_style(&backend, &node, style);
    }
    if let Some(fill) = data.ref_fill {
        let handle = backend.borrow().make_view_handle(&node);
        fill(handle);
    }
    node
}

/// Mount a `<tr>`: children, then the (rarely used) author style.
fn mount_row<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<PrimCell<TableRowPrim>>,
    children: Vec<Element>,
) -> H::Node
where
    H: StyleServices + InputOps + IntrospectionOps,
{
    let data = prim.take();
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_element("tr");
    let _robot = register_extension_node(&backend, &node, None);
    if data.grid {
        // Grid-column table: the cells are the grid's items. A row style
        // or handle was moved to the row's backdrop by `table`.
        let b = backend.borrow();
        b.attach_html_style(&node, "display", "contents");
        b.attach_html_attribute(&node, "role", "row");
    }
    cx.realize_children_into(&mut node, children);
    if let Some(style) = data.style {
        attach_style(&backend, &node, style);
    }
    // Row proxy: hand the `<tr>`'s own handle to the binder — row
    // geometry (drop targeting, row frames) reads through it. The
    // native lowering's equivalent surface is the row backdrop.
    if let Some(fill) = data.ref_fill {
        let handle = backend.borrow().make_view_handle(&node);
        fill(handle);
    }
    node
}

/// Mount a `<td>` / `<th>`: children, author style, then the
/// clickable-row interaction handlers (if [`set_cell_interaction`]
/// attached any). No inline defaults on the cell — an inline style
/// would beat the author's class-based `apply_style`.
fn mount_cell<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<PrimCell<TableCellPrim>>,
    children: Vec<Element>,
) -> H::Node
where
    H: StyleServices + InputOps + IntrospectionOps,
{
    let data = prim.take();
    let backend = cx.backend().clone();
    let tag = if data.header { "th" } else { "td" };
    let mut node = backend.borrow_mut().create_element(tag);
    let robot = register_extension_node(&backend, &node, None);
    // `table` resolved any `Rest` span to a count before mount.
    let span = data.span.resolve(1);
    if span > 1 {
        backend.borrow().attach_html_attribute(&node, "colspan", &span.to_string());
    }
    if data.grid {
        let b = backend.borrow();
        b.attach_html_attribute(&node, "role", if data.header { "columnheader" } else { "cell" });
        // Not a table cell any more. A `display: table-cell` box whose row
        // is `display: contents` is wrapped, with its siblings, in an
        // anonymous table before grid layout runs — the whole row set
        // becomes ONE grid item in the first column. A column flex box is
        // what a native cell is (a view), and `justify-content: center`
        // keeps the vertical centring a `<td>` has.
        b.attach_html_style(&node, "display", "flex");
        b.attach_html_style(&node, "flex-direction", "column");
        b.attach_html_style(&node, "justify-content", "center");
        if span > 1 {
            // `colspan` means nothing to a grid item.
            b.attach_html_style(&node, "grid-column", &format!("span {span}"));
        }
    }
    cx.realize_children_into(&mut node, children);
    if let Some(style) = data.style {
        attach_style(&backend, &node, style);
    }
    if let Some(h) = data.on_touch {
        // The clickable-row tap recognizer: robot `click` on anything in
        // the cell bubbles to it, as a real tap does.
        robot.attach_touch(&h);
        backend.borrow_mut().install_touch_handler(&node, h);
    }
    if let Some(h) = data.on_hover {
        backend.borrow_mut().install_hover_handler(&node, h);
    }
    if let Some(fill) = data.ref_fill {
        let handle = backend.borrow().make_view_handle(&node);
        fill(handle);
    }
    node
}

/// Mount a `<tfoot>`: the footer section's rows. The browser lays a
/// footer group out after every body row, wherever it sits among the
/// table's children.
fn mount_foot<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<PrimCell<TableFootPrim>>,
    children: Vec<Element>,
) -> H::Node
where
    H: StyleServices + InputOps + IntrospectionOps,
{
    let data = prim.take();
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_element("tfoot");
    let _robot = register_extension_node(&backend, &node, None);
    if data.grid {
        let b = backend.borrow();
        b.attach_html_style(&node, "display", "contents");
        b.attach_html_attribute(&node, "role", "rowgroup");
    }
    cx.realize_children_into(&mut node, children);
    node
}

/// Mount a grid-column table's `<tbody>` ([`TableBodyPrim`]):
/// `display: contents`, so its rows' cells stay the grid's items.
fn mount_body<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<PrimCell<TableBodyPrim>>,
    children: Vec<Element>,
) -> H::Node
where
    H: StyleServices + InputOps + IntrospectionOps,
{
    let _ = prim.take();
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_element("tbody");
    let _robot = register_extension_node(&backend, &node, None);
    {
        let b = backend.borrow();
        b.attach_html_style(&node, "display", "contents");
        b.attach_html_attribute(&node, "role", "rowgroup");
    }
    cx.realize_children_into(&mut node, children);
    node
}

/// Register the Table SDK's payload handlers on a scene registry — the
/// boot registration seam. Web boots pass this to
/// `backend_web::newcore::start_in`'s `register` argument; SSR renders
/// pass it to `backend_ssr::newcore::render_path_with`. Only the WEB
/// lowering needs it: native trees lower to plain grid views handled by
/// the vocabulary built-ins.
pub fn register<H>(registry: &mut Registry<H>)
where
    H: StyleServices + InputOps + IntrospectionOps + 'static,
{
    registry.register::<PrimCell<TablePrim>, _>(mount_table::<H>);
    registry.register::<PrimCell<TableRowPrim>, _>(mount_row::<H>);
    registry.register::<PrimCell<TableCellPrim>, _>(mount_cell::<H>);
    registry.register::<PrimCell<TableFootPrim>, _>(mount_foot::<H>);
    registry.register::<PrimCell<TableBodyPrim>, _>(mount_body::<H>);
}

/// Declare this SDK's payload kinds **late-bound** instead of installing
/// their handlers — the boot half of lazy registration. Pair with
/// [`register_from_chunk`] called from inside a `#[component(lazy)]`
/// body; realize parks a table item behind a placeholder until that
/// chunk lands, rather than panicking on it.
///
/// This exists so an app never has to spell the registry keys: there are
/// four of them and each is wrapped in [`PrimCell`], a framework
/// internal an app would otherwise have to import to write
/// `registry.defer::<PrimCell<TablePrim>>()`.
///
/// Only web code-splits, so on every other target this installs the
/// handlers eagerly exactly as [`register`] does. That is deliberate:
/// deferring a kind nothing later registers leaves the payload parked
/// behind a placeholder forever — no panic, no log — and native has no
/// chunk to arrive. Calling `defer` is therefore always safe: it splits
/// where splitting exists and is a plain `register` elsewhere.
pub fn defer<H>(registry: &mut Registry<H>)
where
    H: Host + StyleServices + InputOps + IntrospectionOps + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        registry.defer::<PrimCell<TablePrim>>();
        registry.defer::<PrimCell<TableRowPrim>>();
        registry.defer::<PrimCell<TableCellPrim>>();
        registry.defer::<PrimCell<TableFootPrim>>();
        registry.defer::<PrimCell<TableBodyPrim>>();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        register(registry);
    }
}

/// Install this SDK's payload handlers from inside a lazy chunk — the
/// chunk half of lazy registration. Requires [`defer`] at boot.
///
/// Generic over the host because this crate takes no backend dependency;
/// the caller pins `H` to its concrete backend, e.g.
/// `register_from_chunk::<backend_web::WebBackend>()`. Only the WEB
/// lowering has handlers to split — native tables lower to plain grid
/// views handled by the vocabulary built-ins — and web is the only
/// target that code-splits at all.
///
/// Inert off-web, where [`defer`] already registered eagerly: queueing a
/// late registration for a kind that was never declared deferred panics
/// in `Registry::register_deferred`. The stub keeps a
/// `#[component(lazy)]` body that calls this compiling on every target.
pub fn register_from_chunk<H>()
where
    H: Host + StyleServices + InputOps + IntrospectionOps + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        runtime_scene::defer_registration::<H, _>(|registry| {
            registry.register_deferred::<PrimCell<TablePrim>, _>(mount_table::<H>);
            registry.register_deferred::<PrimCell<TableRowPrim>, _>(mount_row::<H>);
            registry.register_deferred::<PrimCell<TableCellPrim>, _>(mount_cell::<H>);
            registry.register_deferred::<PrimCell<TableFootPrim>, _>(mount_foot::<H>);
            registry.register_deferred::<PrimCell<TableBodyPrim>, _>(mount_body::<H>);
        });
    }
}

// ============================================================================
// `ui!` dispatch — type aliases + BuildElement impls (glue dispatch).
// ============================================================================

/// `ui!` tag alias for the table container — `ui! { Table { … } }`
/// resolves to this type and dispatches through [`BuildElement`].
pub type Table = TableProps;
/// `ui!` tag alias for a table row.
pub type TableRow = TableRowProps;
/// `ui!` tag alias for a table cell.
pub type TableCell = TableCellProps;
/// `ui!` tag alias for a footer section.
pub type TableFoot = TableFootProps;

impl BuildElement for TableFootProps {
    fn build(self) -> Element {
        table_foot(self)
    }
}

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

/// Glob-importable bundle of the table tags, props, and constructors
/// for use at `ui!` call sites.
pub mod prelude {
    pub use super::{
        table, table_cell, table_foot, table_row, ColSpan, Table, TableBound, TableCell,
        TableCellBound, TableCellProps, TableFoot, TableFootProps, TableProps, TableRow,
        TableRowBound, TableRowProps,
    };
}

#[cfg(test)]
mod scroll_edges_tests {
    use super::*;

    /// At rest with columns running off the right, only the RIGHT edge
    /// is hidden; scrolled to the middle, both; scrolled to the end, only
    /// the left. A table that fits hides neither.
    #[test]
    fn edges_follow_the_offset_through_the_scroll() {
        assert_eq!(ScrollEdges::measure(0.0, 400.0, 900.0), ScrollEdges { left: false, right: true });
        assert_eq!(ScrollEdges::measure(200.0, 400.0, 900.0), ScrollEdges { left: true, right: true });
        assert_eq!(ScrollEdges::measure(500.0, 400.0, 900.0), ScrollEdges { left: true, right: false });
        assert_eq!(ScrollEdges::measure(0.0, 400.0, 400.0), ScrollEdges::default());
    }

    /// A `max-content` table lays out at a fractional width the offset
    /// never quite reaches; fully scrolled must still read as the end,
    /// or the right edge never goes away.
    #[test]
    fn subpixel_remainders_are_not_hidden_columns() {
        assert_eq!(ScrollEdges::measure(499.7, 400.0, 900.1), ScrollEdges { left: true, right: false });
        assert_eq!(ScrollEdges::measure(0.3, 400.0, 900.0).left, false);
    }

    /// Before both widths are measured nothing is hidden — a scroller
    /// that has not laid out yet must not flash an edge.
    #[test]
    fn unmeasured_scroller_hides_nothing() {
        assert_eq!(ScrollEdges::measure(0.0, 0.0, 900.0), ScrollEdges::default());
        assert_eq!(ScrollEdges::measure(0.0, 400.0, 0.0), ScrollEdges::default());
    }

    /// The callback hears each CHANGE once — a scroll delivers an event
    /// per pixel, and every one re-reporting the same edges would
    /// re-style every frozen cell per pixel — and nothing after disarm.
    #[test]
    fn watch_reports_changes_only_and_stops_when_disarmed() {
        let heard = Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = heard.clone();
        let watch = EdgeWatch::new(Rc::new(move |e| sink.borrow_mut().push(e)));
        watch.viewport.set(400.0);
        watch.content.set(900.0);
        watch.update();
        watch.set_offset(10.0);
        watch.set_offset(20.0);
        watch.set_offset(30.0);
        watch.set_offset(500.0);
        watch.disarm();
        watch.set_offset(0.0);
        assert_eq!(
            *heard.borrow(),
            vec![
                ScrollEdges { left: false, right: true },
                ScrollEdges { left: true, right: true },
                ScrollEdges { left: true, right: false },
            ]
        );
    }
}
