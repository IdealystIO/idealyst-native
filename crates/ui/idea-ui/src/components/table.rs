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
//! # Horizontal scrolling & frozen columns
//!
//! `Table(scroll_x = true)` wraps the table in a horizontal scroller
//! (columns lay out at natural width, overflow sideways, and the table
//! still fills the scroller when narrow). `TableCell(pinned =
//! ColumnPin::Left)` / `Right` freezes that cell's column against the
//! scroller edge — a `pinned` axis on the cell stylesheets
//! (`position: Sticky` + a zero inset + an opaque background), which
//! the browser pins natively on web and the shared sticky registry
//! pins on native, raising the frozen cells above the content sliding
//! beneath them. Pin the SAME cell in every row (header included) or
//! the column freezes only partially.
//!
//! # Row drag & drop (`on_reorder`)
//!
//! `Table(on_reorder = …)` plus `TableRow(draggable = true)` makes
//! rows reorderable: long-press (or press-and-drag on pointer
//! devices) picks a row up, the row follows the finger vertically and
//! dims, the row currently under it highlights as the drop slot, and
//! dropping invokes `on_reorder(from, to)` with the draggable rows'
//! ordinals (header rows and other non-draggable rows don't count).
//! The callback OWNS the data mutation — reorder your source of truth
//! and let the table rebuild. Wiring: each draggable row gets a
//! `dnd::Draggable` whose recognizer fans out to the row's cells
//! (same seam as `on_row_click`) and a `dnd::Droppable` bound to the
//! row's proxy surface (the `table` SDK's row backdrop / `<tr>`).
//! A row with BOTH `draggable` and `on_row_click` keeps only the drag
//! recognizer — the touch slot is single-occupancy; give the row a
//! dedicated interactive cell if it needs both.
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
use runtime_vocabulary::StyleProp;
use table::{table as sdk_table, table_cell as sdk_cell, table_row as sdk_row};
use table::{TableCellProps as SdkTableCellProps, TableProps as SdkTableProps, TableRowProps as SdkTableRowProps};

use crate::stylesheets::{
    Table as TableStyle, TableBodyCell, TableBodyText, TableCellInner, TableHeadCell, TableHeadText,
};

/// Which edge a [`TableCell`] freezes against in a
/// `Table(scroll_x = true)` — see the module docs. (Named `ColumnPin`
/// rather than `Pin` to stay clear of `std::pin::Pin`.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, IdealystSchema)]
pub enum ColumnPin {
    /// Freeze against the scroller's left edge.
    Left,
    /// Freeze against the scroller's right edge.
    Right,
}

/// Row metadata tag `TableRow(draggable = true)` leaves on the built
/// row for [`Table`]'s reorder wiring to find (`table::set_row_meta` /
/// `take_row_meta`).
struct DraggableRowTag;

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
    /// Horizontal-scroll mode: columns lay out at natural width and
    /// overflow sideways inside a horizontal scroller instead of
    /// squeezing and wrapping. Required for `TableCell(pinned = …)`
    /// frozen columns (they pin against this scroller).
    // STRUCTURAL — selects the SDK's scroller wrapper at build time.
    #[prop(static)]
    pub scroll_x: bool,
    /// Row reordering by drag & drop. When `Some`, rows marked
    /// `TableRow(draggable = true)` can be picked up (long-press, or
    /// press-and-drag on pointer devices) and dropped on another
    /// draggable row; the callback receives `(from, to)` ordinals
    /// counted over the draggable rows only. Reorder your data source
    /// in the callback — the table rebuilds from it.
    pub on_reorder: Option<Rc<dyn Fn(usize, usize)>>,
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
    if let Some(on_reorder) = props.on_reorder {
        wire_reorder(&children, on_reorder);
    }
    // SDK's `table()` returns a `Bound<TableHandle>`; chain
    // `.with_style(...)` to land the themed style on the `<table>`
    // itself, then convert to Element.
    sdk_table(SdkTableProps { children, scroll_x: props.scroll_x })
        .with_style(style)
        .into_element()
}

/// Wire drag-and-drop reordering across the table's built rows.
///
/// Runs in the `Table` component body, so every signal/effect the dnd
/// objects create is owned by the table's scope. Per draggable row
/// (ordinal `i` over `TableRow(draggable = true)` rows only):
///
/// - a `Draggable<usize>` carrying `i`. Its recognizer is created ONCE
///   and fanned out to every cell (`table::set_cell_touch`) — the same
///   per-cell seam as `on_row_click`, and for the same reason: a
///   row-level surface can't receive touches that land on cell
///   content. Long-press activation so the pickup doesn't fight page
///   scrolling (vertical drag IS the scroll direction, which rules out
///   the scroll-aware hybrid).
/// - the drag's vertical offset bound to each cell's node
///   (`AnimProp::TranslateY`) so the whole row follows the finger and
///   springs back on a miss. The horizontal offset stays unbound —
///   rows only reorder vertically.
/// - a `Droppable<usize>` bound to the row's proxy surface
///   (`table::bind_row` — the SDK's row backdrop on native, the
///   `<tr>` on web), accepting any payload but its own ordinal, and
///   invoking `on_reorder(from, i)` on drop.
/// - reactive cell-style axes for feedback: `dragging` dims the
///   in-flight row, `drop_target` highlights the slot under it. Both
///   select build-time stylesheet arms (premint-compatible class
///   swaps, like `row_hovered`).
fn wire_reorder(rows: &[Element], on_reorder: Rc<dyn Fn(usize, usize)>) {
    use dnd::{Activation, DragContext, Draggable, Droppable};
    use runtime_core::animation::AnimProp;
    use runtime_core::{Ref, ViewHandle};

    /// How long a finger must hold still to pick a row up. Long enough
    /// that a scroll fling never picks up a row, short enough to feel
    /// immediate — the plain-list reorder convention.
    const PICKUP_MS: u64 = 250;
    /// Movement tolerance (px) while the pickup timer runs.
    const PICKUP_SLOP_PX: f32 = 8.0;

    let ctx: DragContext<usize> = DragContext::new();
    let mut ordinal = 0usize;
    for row in rows {
        let Some(meta) = table::take_row_meta(row) else { continue };
        if meta.downcast_ref::<DraggableRowTag>().is_none() {
            continue;
        }
        let i = ordinal;
        ordinal += 1;

        let drag = Draggable::new(&ctx, move || i).activation(Activation::LongPress {
            threshold_ms: PICKUP_MS,
            slop_px: PICKUP_SLOP_PX,
        });
        let dragging = drag.is_dragging();
        let (_, offset_y) = drag.offset();
        // One recognizer per row, cloned per cell — `handler()` builds
        // fresh recognizer state each call, and the cells must feed ONE
        // shared recognizer for the row.
        let handler = drag.handler();

        let on_reorder = on_reorder.clone();
        let drop = Droppable::new(&ctx)
            .accepts(move |from: &usize| *from != i)
            .on_drop(move |from| on_reorder(from, i));
        let over = drop.is_over();
        let row_ref: Ref<ViewHandle> = Ref::new();
        drop.bind(row_ref);
        table::bind_row(row, move |h| row_ref.fill(h));

        table::visit_row_cells(row, |cell| {
            if let Some(base) = table::cell_base_application(cell) {
                table::set_cell_style(cell, move || {
                    base.clone()
                        .with("dragging", if dragging.get() { "on" } else { "off" })
                        .with("drop_target", if over.get() { "on" } else { "off" })
                });
            }
            table::set_cell_touch(cell, handler.clone());
            let cell_ref: Ref<ViewHandle> = Ref::new();
            table::bind_cell(cell, move |h| cell_ref.fill(h));
            offset_y.bind(cell_ref, AnimProp::TranslateY);
        });
    }
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
    /// Mark this row reorderable — meaningful inside a
    /// `Table(on_reorder = …)`, where a long-press picks the row up
    /// (see the module docs). Leave `false` on header rows. A row with
    /// both `draggable` and `on_row_click` keeps only the drag
    /// recognizer (the touch slot is single-occupancy).
    // STRUCTURAL — tags the built row for the Table's wiring pass.
    #[prop(static)]
    pub draggable: bool,
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
    // Clickable + draggable both claim the cells' one touch slot; the
    // Table's reorder wiring overwrites the tap recognizer, so skip the
    // click wiring up front and keep the precedence visible here.
    if let (Some(cb), false) = (props.on_row_click, props.draggable) {
        let hovered = signal(false);
        let cells: Vec<Element> = children
            .into_iter()
            .map(|cell| make_row_cell_interactive(cell, hovered, cb.clone()))
            .collect();
        return sdk_row(SdkTableRowProps { children: cells }).into_element();
    }

    let el = sdk_row(SdkTableRowProps { children }).into_element();
    if props.draggable {
        table::set_row_meta(&el, Box::new(DraggableRowTag));
    }
    el
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

    // Reactive whole-row hover style: select the cell sheet's
    // `interactive`/`row_hovered` AXES instead of layering runtime
    // overrides. Every arm has build-time CSS, so on a premint build the
    // flip is a class swap through the reactive diversion — no engine —
    // while native resolves the same arms through the engine as always.
    // (The former `with_overrides` spelling disqualified every clickable
    // cell from preminting.) A cell without a static sheet application
    // keeps its style untouched but stays clickable.
    if let Some(base) = table::cell_base_application(&cell) {
        table::set_cell_style(&cell, move || {
            base.clone()
                .with("interactive", "on")
                .with("row_hovered", if hovered.get() { "on" } else { "off" })
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
    /// Freeze this cell's column against a scroller edge — meaningful
    /// inside a `Table(scroll_x = true)`. Pin the same cell in EVERY
    /// row (header included) or the column freezes only partially.
    // STRUCTURAL — selects the `pinned` stylesheet arm at build time.
    #[prop(static)]
    pub pinned: Option<ColumnPin>,
}

impl Default for TableCellProps {
    fn default() -> Self {
        Self {
            header: false,
            text: Reactive::Static(None),
            children: Vec::new(),
            pinned: None,
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
    // A cell's style is handed over as an EXPLICIT `StyleProp::Sheet`, not
    // as a bare application. A clickable row re-derives each cell's
    // `StyleApplication` from the built element
    // (`table::cell_base_application`) to select the `interactive` /
    // `row_hovered` axes on it, and a `--premint` build's opaque
    // `Preminted` class cannot provide one — the app must stay
    // introspectable in the payload.
    //
    // `.into_style_application()` alone used to say that and stopped:
    // `IntoStyleProp for StyleApplication` gained a preminted fast path, so
    // the application preminted anyway, `cell_base_application` returned
    // `None`, and `make_row_cell_interactive` silently skipped the whole
    // overlay — clickable rows lost their pointer cursor and their hover
    // highlight in every `--premint` build, with nothing logged. Naming the
    // variant is what actually pins the intent, and
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
    if header {
        let app = TableHeadCell().into_style_application();
        let app = match pin_arm {
            Some(arm) => app.with("pinned", arm),
            None => app,
        };
        bound.with_style(StyleProp::Sheet(Box::new(app))).into_element()
    } else {
        let app = TableBodyCell().into_style_application();
        let app = match pin_arm {
            Some(arm) => app.with("pinned", arm),
            None => app,
        };
        bound.with_style(StyleProp::Sheet(Box::new(app))).into_element()
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
                Element::Owned { element, owned } => {
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
    /// This half asserts the SEAM: the application must be recoverable.
    /// Under the default (non-premint) cfg it passes either way, so
    /// `premint_must_not_reach_table_cell_styles` guards the actual
    /// spelling.
    #[test]
    fn regression_premint_keeps_table_cells_on_the_live_engine() {
        with_test_world(|| {
            let cell = body_cell("x");
            assert!(
                table::cell_base_application(&cell).is_some(),
                "a clickable row re-derives the cell's StyleApplication to \
                 layer the pointer cursor + hover highlight over it; without \
                 one the overlay is skipped silently"
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
                draggable: false,
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
            assert!(
                resting.background.is_none(),
                "resting (row_hovered=off) leaves the body cell's background untouched"
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

    #[test]
    fn clickable_row_makes_every_cell_interactive() {
        with_test_world(|| {
            let row = TableRow(TableRowProps {
                children: vec![body_cell("a"), body_cell("b")],
                on_row_click: Some(Rc::new(|| {})),
                draggable: false,
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
            let app = table::cell_base_application(&left)
                .expect("pinned cell keeps an introspectable application");
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
            assert!(
                rules.background.is_some(),
                "pinned body cell must be opaque — content slides beneath it"
            );

            let right = TableCell(TableCellProps {
                text: Reactive::Static(Some("x".into())),
                pinned: Some(ColumnPin::Right),
                ..Default::default()
            });
            let rules = runtime_core::resolve_style(
                &table::cell_base_application(&right).expect("application"),
            );
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

    /// `Table(scroll_x = true)` lowers to the SDK's horizontal
    /// scroller wrapper — without it there is nothing for a pinned
    /// column to pin against.
    #[test]
    fn scroll_x_table_lowers_to_horizontal_scroller() {
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
            match &el {
                Element::Item { data, .. } => {
                    assert!(
                        data.downcast_ref::<runtime_vocabulary::prims::PrimCell<
                            runtime_vocabulary::prims::ScrollViewPrim,
                        >>()
                        .is_some_and(|c| c.take().horizontal),
                        "scroll_x table must be wrapped in a HORIZONTAL scroll_view"
                    );
                }
                _ => panic!("scroll_x table must lower to a scroll_view item"),
            }
        });
    }

    /// The whole reorder feature at the wiring layer: with
    /// `Table(on_reorder = …)`, a `TableRow(draggable = true)` row gets
    /// — per cell — the shared drag recognizer, a bound node handle
    /// (the vertical drag offset attaches there), and a reactive style
    /// (the `dragging` / `drop_target` feedback axes); the table's
    /// grid gains the row's proxy backdrop (drop targeting reads row
    /// geometry through it). A non-draggable header row in the same
    /// table stays untouched. If any leg regresses, rows either stop
    /// being pickable, stop following the finger, lose their feedback,
    /// or lose drop targeting — silently.
    #[test]
    fn reorderable_table_wires_drag_across_draggable_rows() {
        with_test_world(|| {
            let header = TableRow(TableRowProps {
                children: vec![TableCell(TableCellProps {
                    header: true,
                    text: Reactive::Static(Some("h".into())),
                    ..Default::default()
                })],
                ..Default::default()
            });
            let row = TableRow(TableRowProps {
                children: vec![body_cell("a"), body_cell("b")],
                draggable: true,
                ..Default::default()
            });
            let t = Table(TableProps {
                children: vec![header, row],
                on_reorder: Some(Rc::new(|_from, _to| {})),
                ..Default::default()
            });

            // Table → outer view → inner grid; the grid's children are
            // [header cell, backdrop, row cells…] (backdrop precedes
            // its row's cells so they paint above it).
            let outer = peel_owned_keepalive(t);
            let mut outer_children = match outer {
                Element::Item { children, .. } => children,
                _ => panic!("table outer must be a view item"),
            };
            let grid = outer_children.pop().expect("outer wraps the grid");
            let grid_children = match grid {
                Element::Item { children, .. } => children,
                _ => panic!("grid must be a view item"),
            };
            assert_eq!(
                grid_children.len(),
                4,
                "1 header cell + 1 backdrop + 2 draggable-row cells"
            );
            let mut grid_children = grid_children.into_iter();

            // Header cell: untouched (no drag handler, static style).
            match classify(grid_children.next().expect("header cell")) {
                P::View { on_touch, ref_fill, .. } => {
                    assert!(!on_touch, "header cell must not carry the drag recognizer");
                    assert!(!ref_fill, "header cell needs no offset anchor");
                }
                _ => panic!("header cell classifies as a View"),
            }

            // Backdrop: bound (Droppable geometry).
            match classify(grid_children.next().expect("backdrop")) {
                P::View { ref_fill, children, .. } => {
                    assert!(ref_fill, "row backdrop carries the droppable binding");
                    assert!(children.is_empty(), "backdrop is an empty proxy surface");
                }
                _ => panic!("backdrop classifies as a View"),
            }

            // Draggable row's cells: recognizer + offset anchor +
            // reactive feedback style.
            for (n, cell) in grid_children.enumerate() {
                match classify(cell) {
                    P::View { on_touch, ref_fill, style, .. } => {
                        assert!(on_touch, "cell {n} carries the shared drag recognizer");
                        assert!(ref_fill, "cell {n} anchors the vertical drag offset");
                        assert!(
                            style.expect("cell keeps a style").is_reactive(),
                            "cell {n} style is reactive (dragging / drop_target axes)"
                        );
                    }
                    _ => panic!("draggable cell classifies as a View"),
                }
            }
        });
    }

    /// Peel `Owned` wrappers, retaining the scopes (reactive styles
    /// inside read signals those scopes own).
    fn peel_owned_keepalive(mut el: Element) -> Element {
        loop {
            match el {
                Element::Owned { element, owned } => {
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
                draggable: false,
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
