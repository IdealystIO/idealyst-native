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
//! # The two lowerings
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
//! The columns are `auto`, which `runtime-layout` treats as the
//! `table-layout: auto` signal: it measures each column's content, then
//! short columns hug their content while a text-heavy column absorbs the
//! remaining width and wraps — the same layout a browser gives the web
//! `<table>`.
//!
//! Why a grid and not nested row/cell views: Taffy has no subgrid and no
//! `display: contents`, so a grid can only align the columns of its
//! *direct* children. Keeping `TableRow` as a layout box would make each
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
//! knowledge: the [`map_cell_style`] / [`cell_base_application`] /
//! [`set_cell_style`] / [`set_cell_interaction`] helpers reach through
//! the payload cell (`PrimCell::with_mut` — the payload is not yet
//! mounted, so in-place mutation is sound) for BOTH lowerings (native
//! `ViewPrim` grid item, web [`TableCellPrim`]).
//!
//! [`map_cell_style`] is the one to reach for when layering axes over
//! a cell: it COMPOSES over whatever style the cell already has, so a
//! cell styled reactively (a column width that moves under a resize
//! drag) keeps its own reactivity and takes the overlay too. Reading a
//! base with [`cell_base_application`] and writing a replacement with
//! [`set_cell_style`] only ever worked for cells styled with a static
//! sheet, and skipped the rest in silence.
#![deny(missing_docs)]

use std::rc::Rc;

use runtime_shared::{HoverHandler, TouchHandler};
use runtime_scene::{item, Element, Host, MountCx, Registry};
use runtime_vocabulary::caps::InputOps;
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
    /// Author style, attached to the `<table>` node after children
    /// mount (the standard handler ordering).
    pub style: Option<StyleProp>,
    /// Horizontal-scroll mode — flips the mount-time width strategy
    /// from `width: 100%` (fill, wrap) to `min-width: 100%; width:
    /// max-content` (natural column widths, overflow sideways). The
    /// scroller wrapper itself is added by [`table`] at the element
    /// level, outside this payload.
    pub scroll_x: bool,
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
    /// view on native (a native row previously dropped its style
    /// silently; with the proxy it lands on the backdrop, so row-level
    /// visuals behave uniformly).
    pub style: Option<StyleProp>,
    /// Filled with the row surface's handle at mount ([`bind_row`]).
    pub ref_fill: Option<Box<dyn FnOnce(runtime_shared::ViewHandle)>>,
}

/// Scene payload for a `<td>` / `<th>` (web lowering). The interaction
/// slots exist so [`set_cell_interaction`] can attach idea-ui's
/// clickable-row handlers to a BUILT cell.
pub struct TableCellPrim {
    /// `<th>` when `true`, `<td>` otherwise.
    pub header: bool,
    /// Author style, attached to the cell node.
    pub style: Option<StyleProp>,
    /// Touch handler installed on the cell node (clickable rows).
    pub on_touch: Option<TouchHandler>,
    /// Hover handler installed on the cell node (row hover highlight).
    pub on_hover: Option<HoverHandler>,
    /// Filled with the `<td>`/`<th>`'s handle at mount ([`bind_cell`]) —
    /// the anchor animated drag offsets attach to.
    pub ref_fill: Option<Box<dyn FnOnce(runtime_shared::ViewHandle)>>,
}

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
}
table_wrapper_common!(TableBound);

impl IntoElement for TableBound {
    fn into_element(self) -> Element {
        build_table(self.children, self.style, self.scroll_x)
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
    children: Vec<Element>,
    style: Option<StyleProp>,
}
table_wrapper_common!(TableCellBound);

impl IntoElement for TableCellBound {
    fn into_element(self) -> Element {
        build_cell(self.header, self.children, self.style)
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
        children: std::mem::take(&mut props.children),
        style: None,
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

    /// `<table>` item.
    pub fn table_item(children: Vec<Element>, style: Option<StyleProp>, scroll_x: bool) -> Element {
        item(PrimCell::new(TablePrim { style, scroll_x }), children)
    }

    /// `<tr>` item.
    pub fn row_item(children: Vec<Element>, style: Option<StyleProp>) -> Element {
        item(PrimCell::new(TableRowPrim { style, ref_fill: None }), children)
    }

    /// `<td>` / `<th>` item.
    pub fn cell_item(header: bool, children: Vec<Element>, style: Option<StyleProp>) -> Element {
        item(
            PrimCell::new(TableCellPrim {
                header,
                style,
                on_touch: None,
                on_hover: None,
                ref_fill: None,
            }),
            children,
        )
    }
}

#[cfg(target_arch = "wasm32")]
fn build_table(children: Vec<Element>, style: Option<StyleProp>, scroll_x: bool) -> Element {
    if scroll_x {
        // Structure: styled SURFACE > horizontal scroller > `<table>`.
        // The author style (border/radius/background — idea-ui's
        // themed surface) sits OUTSIDE the scroller so the frame stays
        // put while the columns scroll inside it; a surface inside the
        // scroller rode along with the content and clipped its own
        // border at the viewport edge. Sticky-pinned cells still pin
        // against the scroller (their NEAREST scroll ancestor — the
        // surface's own overflow clip is further out).
        let table = item_lowering::table_item(children, None, scroll_x);
        let scroller = glue::scroll_view(vec![table]).horizontal(true).into_element();
        let surface = glue::view(vec![scroller]);
        match style {
            Some(style) => surface.with_style(style).into_element(),
            None => surface.into_element(),
        }
    } else {
        item_lowering::table_item(children, style, scroll_x)
    }
}

#[cfg(target_arch = "wasm32")]
fn build_row(children: Vec<Element>, style: Option<StyleProp>) -> Element {
    item_lowering::row_item(children, style)
}

#[cfg(target_arch = "wasm32")]
fn build_cell(header: bool, children: Vec<Element>, style: Option<StyleProp>) -> Element {
    item_lowering::cell_item(header, children, style)
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
fn build_table(rows: Vec<Element>, style: Option<StyleProp>, scroll_x: bool) -> Element {
    let mut owneds: Vec<glue::Owned> = Vec::new();
    let mut extracted: Vec<NativeRow> = Vec::new();
    let mut columns = 0usize;
    for row in rows {
        let row_data = extract_row(row, &mut owneds);
        columns = columns.max(row_data.cells.len());
        extracted.push(row_data);
    }
    let any_proxy = extracted
        .iter()
        .any(|r| r.slots.as_ref().is_some_and(|s| s.style.is_some() || s.ref_fill.is_some()));

    let mut grid_children: Vec<Element> = Vec::new();
    if !any_proxy {
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
            for (c, cell) in row.cells.into_iter().enumerate() {
                place_cell(&cell, row_line, (c + 1) as i16);
                grid_children.push(cell);
            }
        }
    }

    // Inner node: the actual grid. Its sheet (display:grid + N tracks)
    // is one level below the author-style target so it survives a
    // `.with_style(...)` on the outer node.
    let inner = glue::view(grid_children)
        .with_style(native_styles::grid_sheet(columns))
        .into_element();
    let mut el;
    if scroll_x {
        // Structure: styled SURFACE > horizontal scroll_view > content
        // (width-floored) > grid — the web lowering's mirror. The
        // author-style surface stays OUTSIDE the scroller so its
        // border/radius don't ride along with the scrolled columns;
        // the content node carries the "at least the scroller's width"
        // floor so a narrow table still fills while a wide one
        // overflows and scrolls. Sticky-pinned cells register against
        // this scroll view.
        let content = glue::view(vec![inner])
            .with_style(StyleProp::Static(Rc::new(native_styles::scroll_floor_rules())))
            .into_element();
        let scroller = glue::scroll_view(vec![content])
            .horizontal(true)
            .with_style(StyleProp::Static(Rc::new(native_styles::scroll_wrapper_rules())))
            .into_element();
        let surface = glue::view(vec![scroller]);
        el = match style {
            Some(style) => surface.with_style(style).into_element(),
            None => surface.into_element(),
        };
    } else {
        // Outer node: the author-style target. The framework's default
        // cross-axis stretch makes the inner grid fill this node's width.
        let outer = glue::view(vec![inner]);
        el = match style {
            Some(style) => outer.with_style(style).into_element(),
            None => outer.into_element(),
        };
    }
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
    slots: Option<TableRowPrim>,
}

/// Build the row-proxy backdrop: an empty view spanning every column of
/// its grid row, inserted BEFORE the row's cells so it paints beneath
/// them. Carries the row's style and hands its handle to `ref_fill` at
/// mount — the geometry surface for drop targeting / row frames.
#[cfg(not(target_arch = "wasm32"))]
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
    item(PrimCell::new(TableRowPrim { style, ref_fill: None }), children)
}

/// Native `TableCell`: a plain view that becomes a grid item; the
/// column track sizes its width. Author style REPLACES the SDK's
/// default cell sheet.
#[cfg(not(target_arch = "wasm32"))]
fn build_cell(_header: bool, children: Vec<Element>, style: Option<StyleProp>) -> Element {
    let styled = match style {
        Some(style) => glue::view(children).with_style(style),
        None => glue::view(children).with_style(native_styles::cell_sheet()),
    };
    styled.into_element()
}

/// Pull a row's cells + marker slots out so the cells can be parented
/// directly under the grid. `table_row` lowers a row to a
/// [`TableRowPrim`] marker item (whose children are the cells); a
/// `#[component]` row body that created reactive state arrives
/// `Owned`-wrapped — peel it and KEEP the scope (pushed into `owneds`,
/// re-attached by the caller). A bare fragment (legacy shape) is a
/// slotless row; any other stray element is treated as a single
/// one-cell row so nothing silently vanishes.
#[cfg(not(target_arch = "wasm32"))]
fn extract_row(row: Element, owneds: &mut Vec<glue::Owned>) -> NativeRow {
    match row {
        Element::Item { data, children } => {
            if let Some(cell) = data.downcast_ref::<PrimCell<TableRowPrim>>() {
                NativeRow { cells: children, slots: Some(cell.take()) }
            } else {
                NativeRow { cells: vec![Element::Item { data, children }], slots: None }
            }
        }
        Element::Fragment(children) => NativeRow { cells: children, slots: None },
        Element::Owned { element, owned } => {
            owneds.push(owned);
            extract_row(*element, owneds)
        }
        other => NativeRow { cells: vec![other], slots: None },
    }
}

/// Layer `overlay` rules onto whatever [`StyleProp`] shape a node
/// already carries, returning the composed prop. Native-only plumbing
/// (the web lowering never rewrites styles — placement is a grid
/// concept), so premint qualification is unaffected.
#[cfg(not(target_arch = "wasm32"))]
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

/// Post-process a built native cell with its explicit grid position.
/// Reaches through the payload cell like [`set_cell_style`]; sound for
/// the same reason (pre-mount, taken exactly once by realize).
#[cfg(not(target_arch = "wasm32"))]
fn place_cell(cell: &Element, row_line: i16, col_line: i16) {
    use runtime_shared::GridPlacement;
    let placement = StyleRules {
        grid_row: Some(GridPlacement::Line(row_line)),
        grid_column: Some(GridPlacement::Line(col_line)),
        ..Default::default()
    };
    let mut placement = Some(placement);
    visit_cell(cell, &mut |view, table_cell| {
        if let Some(p) = view {
            if let Some(rules) = placement.take() {
                p.style = Some(compose_rules(p.style.take(), rules));
            }
        } else if let Some(_p) = table_cell {
            // Web cells never take native grid placement.
            placement.take();
        }
    });
}

/// Rewrite a built native VIEW element's style in place (Owned-peeled).
/// Used by the scroll-x path to give the outer table surface its
/// width floor without disturbing the author's style.
#[cfg(not(target_arch = "wasm32"))]
fn set_view_style(
    el: &Element,
    f: impl FnOnce(Option<StyleProp>) -> Option<StyleProp> + 'static,
) {
    fn walk(el: &Element, f: &mut Option<Box<dyn FnOnce(Option<StyleProp>) -> Option<StyleProp>>>) {
        match el {
            Element::Owned { element, .. } => walk(element, f),
            Element::Item { data, .. } => {
                if let Some(c) = data.downcast_ref::<PrimCell<prims::ViewPrim>>() {
                    c.with_mut(|p| {
                        if let Some(f) = f.take() {
                            p.style = f(p.style.take());
                        }
                    });
                }
            }
            _ => {}
        }
    }
    let mut boxed: Option<Box<dyn FnOnce(Option<StyleProp>) -> Option<StyleProp>>> =
        Some(Box::new(f));
    walk(el, &mut boxed);
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

    thread_local! {
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

    /// Width floor for the scroll-x outer surface: at least the
    /// scroller's width (percent resolves against the enclosing scroll
    /// view), free to grow past it — the native mirror of the web
    /// `min-width: 100%; width: max-content` strategy. `flex_shrink: 0`
    /// is load-bearing: without it the scroller's flex line squeezes
    /// the surface back to the viewport width and nothing ever
    /// overflows (the horizontal twin of
    /// [[project_min_height_pct_scroll_shrink]]). Pinned by
    /// `regression_scroll_x_table_floors_at_scroller_width_and_overflows_past_it`
    /// in `runtime-layout`.
    pub(super) fn scroll_floor_rules() -> StyleRules {
        StyleRules {
            min_width: Some(glue::Tokenized::Literal(glue::Length::Percent(100.0))),
            flex_shrink: Some(glue::Tokenized::Literal(0.0)),
            ..Default::default()
        }
    }

    /// Style for the scroll-x wrapper itself: content on the X MAIN
    /// axis, and CONTENT height. With the default Column direction the
    /// table content would be a CROSS-axis child — stretch-clamped to
    /// the scroller's width, never able to overflow it, so there would
    /// be nothing to scroll. `flex_grow: 0` + `flex_basis: auto`
    /// override the scroll primitive's fill-the-parent seed
    /// (`flex_grow: 1` / `flex_basis: 0`): inside the content-sized
    /// table surface that seed collapses the scroller to zero height —
    /// the same trap the idea-ui Modal hit
    /// (`regression_modal_scroller_content_sized_then_capped`).
    pub(super) fn scroll_wrapper_rules() -> StyleRules {
        StyleRules {
            flex_direction: Some(glue::FlexDirection::Row),
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

/// Read the STATIC sheet application off a built cell (`None` for
/// non-cells and for cells whose style is not a static sheet). The
/// clickable-row hover overlay derives its reactive style from this
/// base.
pub fn cell_base_application(cell: &Element) -> Option<StyleApplication> {
    match cell {
        Element::Owned { element, .. } => cell_base_application(element),
        Element::Item { data, .. } => {
            let mut out = None;
            if let Some(c) = data.downcast_ref::<PrimCell<prims::ViewPrim>>() {
                c.with_mut(|p| {
                    if let Some(StyleProp::Sheet(app)) = &p.style {
                        out = Some((**app).clone());
                    }
                });
            } else if let Some(c) = data.downcast_ref::<PrimCell<TableCellPrim>>() {
                c.with_mut(|p| {
                    if let Some(StyleProp::Sheet(app)) = &p.style {
                        out = Some((**app).clone());
                    }
                });
            }
            out
        }
        _ => None,
    }
}

/// Layer reactive axis selections over a built cell's EXISTING style,
/// whatever shape that style has. Returns whether a cell style was
/// actually wrapped.
///
/// This is the composing counterpart of [`cell_base_application`] +
/// [`set_cell_style`], and the one a row overlay should reach for. That
/// pair can only read a STATIC [`StyleProp::Sheet`], so a cell whose
/// style is already reactive — a column pinned to a width that moves
/// under a resize drag, say — silently kept its style and lost the
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
/// there is none to compose with. That is the same outcome those cells
/// have always had.
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
            Element::Item { data, children } => {
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
    H: StyleServices + InputOps,
{
    let data = prim.take();
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_element("table");
    {
        // Reset the browser's default table chrome — apps style via the
        // stylesheet system. `border-collapse: collapse` keeps the cell
        // borders the author draws from doubling up. Inline
        // declarations (not a class), so `attach_style` below still
        // wins where the author sets the same property.
        let b = backend.borrow();
        if data.scroll_x {
            // Scroll-x border model: `separate` (spacing 0), NOT
            // `collapse`. Collapsed borders belong to the table's own
            // paint layer — the layer that scrolls — so a sticky
            // (frozen) cell's dividers stayed behind while its opaque
            // background rode the pin over them: hairlines vanished
            // into white-on-white. `separate` makes every cell paint
            // its own borders, which then travel with the pinned cell.
            // The idea-ui cells draw only their bottom (+ pin-edge)
            // hairlines, so nothing doubles up without collapsing.
            b.attach_html_style(&node, "border-collapse", "separate");
            b.attach_html_style(&node, "border-spacing", "0");
            // Width strategy: natural column widths (a table at
            // `width: max-content` never wraps its cells), floored
            // at the scroller's width so a narrow table still fills.
            // `width: 100%` would instead squeeze and wrap the columns
            // — no overflow, nothing to scroll.
            b.attach_html_style(&node, "min-width", "100%");
            b.attach_html_style(&node, "width", "max-content");
        } else {
            b.attach_html_style(&node, "border-collapse", "collapse");
            b.attach_html_style(&node, "width", "100%");
        }
        b.attach_html_style(&node, "table-layout", "auto");
    }
    cx.realize_children_into(&mut node, children);
    if let Some(style) = data.style {
        attach_style(&backend, &node, style);
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
    H: StyleServices + InputOps,
{
    let data = prim.take();
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_element("tr");
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
    H: StyleServices + InputOps,
{
    let data = prim.take();
    let backend = cx.backend().clone();
    let tag = if data.header { "th" } else { "td" };
    let mut node = backend.borrow_mut().create_element(tag);
    cx.realize_children_into(&mut node, children);
    if let Some(style) = data.style {
        attach_style(&backend, &node, style);
    }
    if let Some(h) = data.on_touch {
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

/// Register the Table SDK's payload handlers on a scene registry — the
/// boot registration seam. Web boots pass this to
/// `backend_web::newcore::start_in`'s `register` argument; SSR renders
/// pass it to `backend_ssr::newcore::render_path_with`. Only the WEB
/// lowering needs it: native trees lower to plain grid views handled by
/// the vocabulary built-ins.
pub fn register<H>(registry: &mut Registry<H>)
where
    H: StyleServices + InputOps + 'static,
{
    registry.register::<PrimCell<TablePrim>, _>(mount_table::<H>);
    registry.register::<PrimCell<TableRowPrim>, _>(mount_row::<H>);
    registry.register::<PrimCell<TableCellPrim>, _>(mount_cell::<H>);
}

/// Declare this SDK's payload kinds **late-bound** instead of installing
/// their handlers — the boot half of lazy registration. Pair with
/// [`register_from_chunk`] called from inside a `#[component(lazy)]`
/// body; realize parks a table item behind a placeholder until that
/// chunk lands, rather than panicking on it.
///
/// This exists so an app never has to spell the registry keys: there are
/// three of them and each is wrapped in [`PrimCell`], a framework
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
    H: Host + StyleServices + InputOps + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        registry.defer::<PrimCell<TablePrim>>();
        registry.defer::<PrimCell<TableRowPrim>>();
        registry.defer::<PrimCell<TableCellPrim>>();
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
    H: Host + StyleServices + InputOps + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        runtime_scene::defer_registration::<H, _>(|registry| {
            registry.register_deferred::<PrimCell<TablePrim>, _>(mount_table::<H>);
            registry.register_deferred::<PrimCell<TableRowPrim>, _>(mount_row::<H>);
            registry.register_deferred::<PrimCell<TableCellPrim>, _>(mount_cell::<H>);
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
        table, table_cell, table_row, Table, TableBound, TableCell, TableCellBound, TableCellProps,
        TableProps, TableRow, TableRowBound, TableRowProps,
    };
}
