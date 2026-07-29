//! The new-core (idea-lite) implementation: the SAME authored surface
//! as [`oldcore`](crate) — `table()` / `table_row()` / `table_cell()`
//! constructors, the `Table` / `TableRow` / `TableCell` `ui!` tag
//! aliases, `.with_style(…)` chaining, `register` — re-expressed over
//! the idea-lite stack:
//!
//! - **Web (wasm32)**: each primitive lowers to a scene
//!   [`Element::Item`] carrying a typed payload; the scene
//!   [`Registry`] dispatches to the handlers installed by
//!   [`register_handlers`], which emit the real `<table>` / `<tr>` /
//!   `<td>` / `<th>` DOM through the vocabulary's capability traits
//!   ([`DocumentOps::create_element`](runtime_vocabulary::caps::DocumentOps::create_element))
//!   — the new core's unified primitive==external contract (the old
//!   `Element::External` + per-backend `register_external` path,
//!   generalized). Handlers are generic over the caps traits, so the
//!   SSR backend reuses them for static rendering.
//! - **Native (non-wasm)**: the same CSS-grid lowering as the old
//!   core — rows flatten to cells under one `display: grid` node with
//!   `N` `auto` column tracks — built with the vocabulary glue's
//!   `view` builder. `StyleRules` is the shared style data model
//!   between cores, so `DisplayKind::Grid` + `grid_template_columns`
//!   flow through the new-core style attach unchanged. No handler
//!   registration is needed (exactly as on the old core).
//!
//! # Owned-scope peeling (native grid flattening)
//!
//! Under the new core a `#[component]`-wrapped row (idea-ui's
//! `TableRow` with `on_row_click`) arrives as
//! `Element::Owned { element: Fragment(cells), owned }` — the row
//! body's collected scope (its shared hover signal) rides the wrapper.
//! Flattening must NOT drop that scope: the cells' reactive hover
//! style reads the signal for the cells' whole life. [`table`] peels
//! the wrapper, flattens the cells into the grid, and re-attaches
//! every peeled scope around the finished table element
//! (`runtime_scene::owned`), so the scopes live exactly as long as the
//! subtree that uses them.
//!
//! # Built-cell post-processing ([`cell_base_application`] & co.)
//!
//! On the old core, idea-ui's clickable-row feature pattern-matched
//! the public `Element` enum to re-style built cells and attach
//! touch/hover handlers. The new core's `Element::Item` payload is
//! type-erased, so this module owns that knowledge instead: the
//! [`cell_base_application`] / [`set_cell_style`] /
//! [`set_cell_interaction`] helpers reach through the payload cell
//! (`PrimCell::with_mut` — the payload is not yet mounted, so in-place
//! mutation is sound) for BOTH lowerings (native `ViewPrim` grid item,
//! web [`TableCellPrim`]).

use std::rc::Rc;

use runtime_core::{HoverHandler, TouchHandler};
use runtime_scene::{fragment, item, Element, MountCx, Registry};
use runtime_vocabulary::caps::InputOps;
use runtime_vocabulary::glue::{
    self, BuildElement, ChildList, DisplayKind, IntoElement, StyleApplication, StyleRules,
    StyleSheet, TrackSize, VariantSet,
};
use runtime_vocabulary::prims::{self, PrimCell};
use runtime_vocabulary::style_attach::{attach_style, IntoStyleProp, StyleProp, StyleServices};

// ============================================================================
// Props — same field shapes as the old core.
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
}

/// Scene payload for a `<tr>` (web lowering).
pub struct TableRowPrim {
    /// Author style for the row node (unused by idea-ui, kept because
    /// `.with_style(…)` works on every wrapper — parity with the old
    /// core, where the External's style slot accepted it too).
    pub style: Option<StyleProp>,
}

/// Scene payload for a `<td>` / `<th>` (web lowering). The interaction
/// slots exist so [`set_cell_interaction`] can attach idea-ui's
/// clickable-row handlers to a BUILT cell (the old core wrote into the
/// `Element::External` fields).
pub struct TableCellPrim {
    /// `<th>` when `true`, `<td>` otherwise.
    pub header: bool,
    /// Author style, attached to the cell node.
    pub style: Option<StyleProp>,
    /// Touch handler installed on the cell node (clickable rows).
    pub on_touch: Option<TouchHandler>,
    /// Hover handler installed on the cell node (row hover highlight).
    pub on_hover: Option<HoverHandler>,
}

// ============================================================================
// Builder wrappers — the old `Bound<H>` call shape (`.with_style(…)` →
// `IntoElement`), deferred-build so the style lands in the right slot
// on either lowering.
// ============================================================================

macro_rules! table_wrapper_common {
    ($wrapper:ident) => {
        impl $wrapper {
            /// Attach an author style — lands on the `<table>`/`<tr>`/
            /// `<td>` node on web, on the corresponding grid node on
            /// native. Replaces any previously set style (the old
            /// core's `with_style` overwrote too).
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

        /// Mirror of the old core's `From<Bound<H>> for Element`.
        impl From<$wrapper> for Element {
            fn from(w: $wrapper) -> Element {
                w.into_element()
            }
        }
    };
}

/// Deferred `Table` build — finish with `.with_style(…)` +
/// `.into_element()` (the old `Bound<TableHandle>` call shape).
pub struct TableBound {
    children: Vec<Element>,
    style: Option<StyleProp>,
}
table_wrapper_common!(TableBound);

impl IntoElement for TableBound {
    fn into_element(self) -> Element {
        build_table(self.children, self.style)
    }
}

/// Deferred `TableRow` build (old `Bound<TableRowHandle>` call shape).
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

/// Deferred `TableCell` build (old `Bound<TableCellHandle>` call shape).
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
// Constructors — same names/shapes as the old core.
// ============================================================================

/// Build a `Table` container. Web lowers to a scene item handled by
/// [`register_handlers`] (a real `<table>`); native lowers to a
/// CSS-grid whose column tracks span every row (see the module docs).
pub fn table(mut props: TableProps) -> TableBound {
    TableBound {
        children: std::mem::take(&mut props.children),
        style: None,
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
    pub fn table_item(children: Vec<Element>, style: Option<StyleProp>) -> Element {
        item(PrimCell::new(TablePrim { style }), children)
    }

    /// `<tr>` item.
    pub fn row_item(children: Vec<Element>, style: Option<StyleProp>) -> Element {
        item(PrimCell::new(TableRowPrim { style }), children)
    }

    /// `<td>` / `<th>` item.
    pub fn cell_item(header: bool, children: Vec<Element>, style: Option<StyleProp>) -> Element {
        item(
            PrimCell::new(TableCellPrim {
                header,
                style,
                on_touch: None,
                on_hover: None,
            }),
            children,
        )
    }
}

#[cfg(target_arch = "wasm32")]
fn build_table(children: Vec<Element>, style: Option<StyleProp>) -> Element {
    item_lowering::table_item(children, style)
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
// Native lowering (CSS-grid via the glue view builder) — same recipe as
// the old core's non-wasm arm.
// ============================================================================

/// Native `Table`: outer unstyled passthrough (the author-style target)
/// wrapping the inner grid whose direct children are ALL cells from ALL
/// rows. Peeled row scopes re-attach around the finished element (see
/// the module docs).
#[cfg(not(target_arch = "wasm32"))]
fn build_table(rows: Vec<Element>, style: Option<StyleProp>) -> Element {
    let mut cells: Vec<Element> = Vec::new();
    let mut owneds: Vec<glue::Owned> = Vec::new();
    let mut columns = 0usize;
    for row in rows {
        let row_cells = extract_row_cells(row, &mut owneds);
        columns = columns.max(row_cells.len());
        cells.extend(row_cells);
    }
    // Inner node: the actual grid. Its sheet (display:grid + N tracks)
    // is one level below the author-style target so it survives a
    // `.with_style(...)` on the outer node.
    let inner = glue::view(cells)
        .with_style(native_styles::grid_sheet(columns))
        .into_element();
    // Outer node: the author-style target. The framework's default
    // cross-axis stretch makes the inner grid fill this node's width.
    let outer = glue::view(vec![inner]);
    let mut el = match style {
        Some(style) => outer.with_style(style).into_element(),
        None => outer.into_element(),
    };
    // Re-attach every peeled row scope: the cells' reactive props (the
    // clickable-row hover style) read signals those scopes own, so they
    // must live exactly as long as the flattened subtree.
    for owned in owneds {
        el = runtime_scene::owned(el, owned);
    }
    el
}

/// Native `TableRow`: a fragment of cells (no layout box). The author
/// style is dropped — a fragment has no node to style, exactly as the
/// old core's native arm (row visuals live per-cell).
#[cfg(not(target_arch = "wasm32"))]
fn build_row(children: Vec<Element>, _style: Option<StyleProp>) -> Element {
    fragment(children)
}

/// Native `TableCell`: a plain view that becomes a grid item; the
/// column track sizes its width. Author style replaces the SDK's
/// default cell sheet (old-core `with_style` semantics).
#[cfg(not(target_arch = "wasm32"))]
fn build_cell(_header: bool, children: Vec<Element>, style: Option<StyleProp>) -> Element {
    let styled = match style {
        Some(style) => glue::view(children).with_style(style),
        None => glue::view(children).with_style(native_styles::cell_sheet()),
    };
    styled.into_element()
}

/// Pull a row's cells out so they can be parented directly under the
/// grid. `table_row` lowers a row to a [`Element::Fragment`] of its
/// cells (the hot path); a `#[component]` row body that created
/// reactive state arrives `Owned`-wrapped — peel it and KEEP the scope
/// (pushed into `owneds`, re-attached by the caller). Any other stray
/// element is treated as a single one-cell row so nothing silently
/// vanishes.
#[cfg(not(target_arch = "wasm32"))]
fn extract_row_cells(row: Element, owneds: &mut Vec<glue::Owned>) -> Vec<Element> {
    match row {
        Element::Fragment(children) => children,
        Element::Owned { element, owned } => {
            owneds.push(owned);
            extract_row_cells(*element, owneds)
        }
        other => vec![other],
    }
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
    /// wraps) — see the old-core module for the full rationale. The
    /// recipe is unchanged: `StyleRules` is the shared style data model
    /// between cores.
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
// Built-cell post-processing — the new-core home of the Element
// introspection idea-ui's clickable-row feature did against the old
// public enum (see the module docs). Both lowerings are handled, so the
// callers stay target-agnostic.
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

/// Replace a built cell's style in place (no-op for non-cells —
/// mirrors the old core, where `Element::with_style` on an unexpected
/// variant was harmless). Sound because the payload is not yet mounted:
/// realization takes it exactly once, after this returns.
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
/// (native grid item / web `<td>`-`<th>`), preserving the
/// buttons-in-a-clickable-row fall-through the old core had.
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

/// Mount the `<table>` container: `create_element("table")`, the same
/// browser-default reset the old-core External handler set inline
/// (`border-collapse: collapse; width: 100%; table-layout: auto`),
/// children, then the author style.
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
        // borders the author draws from doubling up. Inline declarations
        // (not a class) exactly like the old handler, and `attach_style`
        // below still wins where the author sets the same property.
        let b = backend.borrow();
        b.attach_html_style(&node, "border-collapse", "collapse");
        b.attach_html_style(&node, "width", "100%");
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
    node
}

/// Mount a `<td>` / `<th>`: children, author style, then the
/// clickable-row interaction handlers (if [`set_cell_interaction`]
/// attached any). No inline defaults on the cell — an inline style
/// would beat the author's class-based `apply_style` (old-core
/// handler's rationale, preserved).
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
    node
}

/// Register the Table SDK's payload handlers on a scene registry — the
/// new-core registration seam. Web boots pass this to
/// `backend_web::newcore::start_in`'s `register` argument; SSR renders
/// pass it to `backend_ssr::newcore::render_path_with`. Only the WEB
/// lowering needs it: native trees lower to plain grid views handled by
/// the vocabulary built-ins.
pub fn register_handlers<H>(registry: &mut Registry<H>)
where
    H: StyleServices + InputOps + 'static,
{
    registry.register::<PrimCell<TablePrim>, _>(mount_table::<H>);
    registry.register::<PrimCell<TableRowPrim>, _>(mount_row::<H>);
    registry.register::<PrimCell<TableCellPrim>, _>(mount_cell::<H>);
}

/// Old-core bootstrap parity: the old `register(&mut backend)` fed the
/// per-backend `register_external` registry, which does not exist on
/// the new core — scene handlers register through
/// [`register_handlers`] at the boot seam instead. A no-op so
/// same-source app bootstrap code compiles unchanged (the navigator
/// SDKs keep the same convention).
pub fn register<B>(_backend: &mut B) {}

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
/// for use at `ui!` call sites (the old-core `Bound`/handle aliases
/// have no new-core counterpart — the wrappers here are
/// [`TableBound`]-family builders instead).
pub mod prelude {
    pub use super::{
        table, table_cell, table_row, Table, TableBound, TableCell, TableCellBound, TableCellProps,
        TableProps, TableRow, TableRowBound, TableRowProps,
    };
}
