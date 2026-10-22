//! The table's column model: [`TableColumn`] specs on the `Table`, and the
//! drag-to-resize handle a resizable column's header carries.
//!
//! A column is the cells at one position in every row. With
//! `Table(columns = …)` the table owns each column's settings — width,
//! floor and cap, fill, resizable, frozen edge, alignment, truncation,
//! padding — and applies them to every cell of the column, header and
//! body alike, so the cells themselves carry none of it. Column `i` of
//! the list is the `i`-th column the AUTHOR wrote; the columns the table
//! adds itself (selection, drag grip) come before them and are never
//! counted.
//!
//! # Widths are overrides
//!
//! A column's width resolves, in order: the table's live width map
//! (`Table(column_widths = …)`, which a resize drag writes), else the
//! spec's own `width`, else none — an unsized column keeps its natural
//! width. Resetting a column (double-click its handle) removes its entry,
//! so it falls back to the spec.

use super::styles::TableStyles;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

use runtime_core::{
    component, signal, ui, Element, IdealystSchema, Memo, Reactive, Ref, Signal, TouchEvent, TouchPhase,
    TouchResponse, VariantEnum, ViewHandle,
};

use super::{CellAlign, ColumnPin};

/// The narrowest a resize drag takes a column when its spec sets no
/// `min_width`, px. Wide enough that a column never collapses past its
/// own padding and a sort arrow.
pub const COLUMN_RESIZE_MIN_PX: f32 = 48.0;

/// Two presses on a resize handle closer together than this are a
/// double-click, which resets the column to its spec width.
pub const COLUMN_RESET_DOUBLE_CLICK_NS: u64 = 400_000_000;

/// The width of the trailing actions column [`TableColumn::actions`]
/// sets up — one icon button plus the compact icon padding, px.
pub const ACTIONS_COLUMN_WIDTH_PX: f32 = 52.0;

/// A column's identity within its table — the key its live width is
/// stored under and the id `on_column_resize` reports. Cheap to clone.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, IdealystSchema)]
pub struct ColumnId(Rc<str>);

impl ColumnId {
    /// A column id from any string.
    pub fn new(id: impl Into<Rc<str>>) -> Self {
        ColumnId(id.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ColumnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ColumnId({:?})", &*self.0)
    }
}

impl fmt::Display for ColumnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ColumnId {
    fn from(s: &str) -> Self {
        ColumnId(Rc::from(s))
    }
}

impl From<String> for ColumnId {
    fn from(s: String) -> Self {
        ColumnId(Rc::from(s))
    }
}

impl From<&String> for ColumnId {
    fn from(s: &String) -> Self {
        ColumnId(Rc::from(s.as_str()))
    }
}

/// A table's live column widths, by column id — what a resize drag
/// writes and what a saved layout restores. An entry overrides the
/// column spec's `width`; a column with no entry uses its spec.
pub type ColumnWidths = BTreeMap<ColumnId, f32>;

/// A cell's horizontal padding. Give every cell of a column the same.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema, runtime_core::Remote)]
pub enum CellPadding {
    /// The theme's cell padding. The default.
    #[default]
    Standard,
    /// Compact padding for a column of icons or icon buttons — a row
    /// menu, a checkbox, a drag grip — so a narrow fixed-width column
    /// has room for its control.
    Icon,
}

// The variant string IS the stylesheet arm the value selects.
impl VariantEnum for CellPadding {
    fn as_variant_str(self) -> &'static str {
        match self {
            CellPadding::Standard => "standard",
            CellPadding::Icon => "icon",
        }
    }

    fn all_variants() -> &'static [Self] {
        &[CellPadding::Standard, CellPadding::Icon]
    }
}

/// Where a column goes in the narrow card form ([`super::TableCards`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema)]
pub enum CardSlot {
    /// A labelled cell under the card's title line, up to the card's cell
    /// cap; past the cap it moves to the row's detail sheet. The default.
    #[default]
    Field,
    /// The card's title — the row's identity. The first column is the
    /// title when no column claims it.
    Title,
    /// The value opposite the title on the title line — the one a reader
    /// scans the list for (a status, a date).
    Opposite,
    /// The trailing control on the title line — the row's menu.
    Actions,
    /// Straight to the row's detail sheet, never on the card itself.
    Detail,
    /// Not in the card form at all.
    Hidden,
}

/// One column of a [`super::Table`]'s column model — see the module docs.
///
/// ```ignore
/// Table(columns = vec![
///     TableColumn::new("name").label("Name").width(200.0).min_width(120.0)
///         .resizable().pinned(ColumnPin::Left).card(CardSlot::Title),
///     TableColumn::new("notes").label("Notes").fill().truncate(),
///     TableColumn::new("hours").label("Hours").width(80.0).align(CellAlign::Right),
///     TableColumn::actions("menu"),
/// ]) { … }
/// ```
#[derive(Clone, Debug, PartialEq, IdealystSchema)]
pub struct TableColumn {
    /// The column's identity — the key its live width is stored under.
    pub id: ColumnId,
    /// The column's name for places the header row is not — the narrow
    /// card form's labels and the row detail sheet.
    pub label: Option<String>,
    /// The column's width when the table's width map has no entry for it,
    /// px. `None` sizes it to its content.
    pub width: Option<f32>,
    /// Never narrower than this, px — and a resize drag's floor.
    pub min_width: Option<f32>,
    /// Never wider than this, px — and a resize drag's ceiling. Like
    /// `TableCell(max_width = …)`, it lays the table out as grid columns.
    pub max_width: Option<f32>,
    /// Take the width the other columns leave (a `flex` share of 1). A
    /// fill column may shrink to its `min_width`; pair it with `truncate`.
    /// In a `scroll_x` table it fills the scroller the same way, and the
    /// table overflows only once its `min_width` and the other columns no
    /// longer fit.
    /// Lays the table out as grid columns.
    pub fill: bool,
    /// Draw a drag handle on the header cell's trailing edge, revealed
    /// while the header cell is hovered: dragging it resizes the whole
    /// column live, releasing reports the width (`on_column_resize`), and
    /// a double-click resets the column to `width`.
    pub resizable: bool,
    /// Freeze the column against a scroller edge (`Table(scroll_x =
    /// true)`). The table insets later frozen columns by the widths of
    /// the ones before them, so give every frozen column but the
    /// outermost a `width`.
    pub pinned: Option<ColumnPin>,
    /// How the column's content sits in it — header and body together.
    pub align: CellAlign,
    /// Keep the content to one line ending in "…".
    pub truncate: bool,
    /// The cells' horizontal padding.
    pub padding: CellPadding,
    /// Where the column goes in the narrow card form.
    pub card: CardSlot,
}

impl TableColumn {
    /// A content-sized column with every setting at its default.
    pub fn new(id: impl Into<ColumnId>) -> Self {
        TableColumn {
            id: id.into(),
            label: None,
            width: None,
            min_width: None,
            max_width: None,
            fill: false,
            resizable: false,
            pinned: None,
            align: CellAlign::Left,
            truncate: false,
            padding: CellPadding::Standard,
            card: CardSlot::Field,
        }
    }

    /// The trailing row-actions column: [`ACTIONS_COLUMN_WIDTH_PX`] wide,
    /// frozen to the right edge, centred, with the compact icon padding —
    /// a `⋮` menu or a couple of icon buttons per row. In a plain
    /// `scroll_x` table its leading hairline shows only while columns are
    /// scrolled beneath it. In the card form it is the title line's
    /// trailing control.
    pub fn actions(id: impl Into<ColumnId>) -> Self {
        TableColumn {
            width: Some(ACTIONS_COLUMN_WIDTH_PX),
            pinned: Some(ColumnPin::Right),
            align: CellAlign::Center,
            padding: CellPadding::Icon,
            card: CardSlot::Actions,
            ..TableColumn::new(id)
        }
    }

    /// Set [`label`](Self::label).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set [`width`](Self::width).
    pub fn width(mut self, px: f32) -> Self {
        self.width = Some(px);
        self
    }

    /// Set [`min_width`](Self::min_width).
    pub fn min_width(mut self, px: f32) -> Self {
        self.min_width = Some(px);
        self
    }

    /// Set [`max_width`](Self::max_width).
    pub fn max_width(mut self, px: f32) -> Self {
        self.max_width = Some(px);
        self
    }

    /// Make this the column that takes the spare width ([`fill`](Self::fill)).
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    /// Give the header a resize handle ([`resizable`](Self::resizable)).
    pub fn resizable(mut self) -> Self {
        self.resizable = true;
        self
    }

    /// Freeze the column against an edge ([`pinned`](Self::pinned)).
    pub fn pinned(mut self, pin: ColumnPin) -> Self {
        self.pinned = Some(pin);
        self
    }

    /// Set [`align`](Self::align).
    pub fn align(mut self, align: CellAlign) -> Self {
        self.align = align;
        self
    }

    /// One line ending in "…" ([`truncate`](Self::truncate)).
    pub fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }

    /// Set [`padding`](Self::padding).
    pub fn padding(mut self, padding: CellPadding) -> Self {
        self.padding = padding;
        self
    }

    /// Set [`card`](Self::card).
    pub fn card(mut self, slot: CardSlot) -> Self {
        self.card = slot;
        self
    }

    /// The range a resize drag keeps this column in.
    pub(super) fn drag_bounds(&self) -> (f32, f32) {
        let min = self.min_width.unwrap_or(COLUMN_RESIZE_MIN_PX);
        let max = self.max_width.unwrap_or(f32::INFINITY).max(min);
        (min, max)
    }
}

impl Default for TableColumn {
    fn default() -> Self {
        TableColumn::new("")
    }
}

/// The width a column renders at now: the table's live entry, else the
/// spec's own. Reactive — read it inside a style closure.
pub(super) fn live_width(widths: Signal<ColumnWidths>, column: &TableColumn) -> Memo<Option<f32>> {
    let id = column.id.clone();
    let fallback = column.width;
    runtime_core::memo(move || widths.with(|w| w.get(&id).copied()).or(fallback))
}

/// Where a resize drag lands: the width it started at plus the pointer's
/// travel, held inside the column's bounds. The only arithmetic in the
/// gesture.
pub fn dragged_width(start: f32, delta_x: f32, bounds: (f32, f32)) -> f32 {
    (start + delta_x).clamp(bounds.0, bounds.1)
}

/// What a resize handle reports on release (`Some(width)`) or reset
/// (`None`): the column and its new width.
pub type ColumnResizeCallback = Rc<dyn Fn(ColumnId, Option<f32>)>;

/// The drag strip on a resizable column's header cell, and its gesture.
#[runtime_core::props]
#[derive(IdealystSchema)]
pub(super) struct ColumnResizeHandleProps {
    /// The column this strip resizes.
    #[prop(static)]
    pub column: TableColumn,
    /// The table's live widths — the drag writes here.
    #[prop(static)]
    pub widths: Signal<ColumnWidths>,
    /// The header cell's node, measured when a drag starts on a column
    /// that has no width yet.
    #[prop(static)]
    pub cell: Ref<ViewHandle>,
    /// Whether the pointer is over the header cell — what reveals the
    /// strip's divider.
    #[prop(static)]
    pub heading_hovered: Signal<bool>,
    /// Release / reset report.
    pub on_resize: Option<Rc<dyn Fn(ColumnId, Option<f32>)>>,
}

impl Default for ColumnResizeHandleProps {
    fn default() -> Self {
        Self {
            column: TableColumn::default(),
            widths: signal(ColumnWidths::new()),
            cell: Ref::new(),
            heading_hovered: signal(false),
            on_resize: None,
        }
    }
}

/// The drag strip. Deltas come from `window_position`, never `position`:
/// the strip sits on an edge that MOVES as the column resizes, so a
/// cell-relative coordinate would chase the very thing the drag changes
/// and the column would run away from the pointer.
///
/// ```ignore
/// ColumnResizeHandle(column = col.clone(), widths = widths, cell = cell_ref, heading_hovered = hovered)
/// ```
#[component]
pub(super) fn ColumnResizeHandle(props: &ColumnResizeHandleProps) -> Element {
    let column = props.column.clone();
    let widths = props.widths;
    let cell = props.cell.clone();
    let heading_hovered = props.heading_hovered;
    let on_resize = props.on_resize.clone();
    let id = column.id.clone();
    let bounds = column.drag_bounds();
    let spec_width = column.width;
    // In flight the divider stays revealed wherever the pointer goes — a
    // drag runs well past both the strip and the heading.
    let in_drag = signal(false);

    // Gesture-local state: nothing renders from it.
    let start_x = Rc::new(Cell::new(0.0_f32));
    let start_w = Rc::new(Cell::new(0.0_f32));
    let moved = Rc::new(Cell::new(false));
    let last_press_ns = Rc::new(Cell::new(0_u64));
    // What the column's entry was when the drag began, so a cancel puts
    // it back exactly — including back to having no entry at all.
    let restore = Rc::new(Cell::new(None::<Option<f32>>));

    // The width this drag last wrote — what release reports. Kept here
    // rather than read back from the map, whose write may still be staged
    // when the release arrives in the same turn.
    let dragged = Rc::new(Cell::new(None::<f32>));
    // `update` composes on the STAGED map, so two writes in one turn (a
    // burst of moves) keep each other.
    let set_entry = move |id: &ColumnId, w: Option<f32>| {
        widths.update(|map| {
            let mut next = map.clone();
            match w {
                Some(w) => next.insert(id.clone(), w),
                None => next.remove(id),
            };
            next
        });
    };

    let touch = move |ev: &TouchEvent| -> TouchResponse {
        match ev.phase {
            TouchPhase::Began => {
                let previous = last_press_ns.replace(ev.timestamp_ns);
                if previous != 0
                    && ev.timestamp_ns.saturating_sub(previous) < COLUMN_RESET_DOUBLE_CLICK_NS
                {
                    // Double-click: back to the spec width.
                    moved.set(false);
                    in_drag.set(false);
                    last_press_ns.set(0);
                    set_entry(&id, None);
                    if let Some(cb) = &on_resize {
                        cb(id.clone(), None);
                    }
                    return TouchResponse::CLAIMED;
                }
                // Where the column is NOW: its rendered width when the
                // node has one (an unsized column has no other answer),
                // else what it resolves to.
                let measured = cell
                    .with(|h| h.absolute_frame())
                    .flatten()
                    .map(|f| f.width)
                    .filter(|w| *w > 0.0);
                let current = widths.with_untracked(|w| w.get(&id).copied());
                restore.set(Some(current));
                dragged.set(None);
                start_w.set(measured.or(current).or(spec_width).unwrap_or(bounds.0));
                start_x.set(ev.window_position.x);
                moved.set(false);
                in_drag.set(true);
                TouchResponse::CLAIMED
            }
            TouchPhase::Moved => {
                moved.set(true);
                let w = dragged_width(start_w.get(), ev.window_position.x - start_x.get(), bounds);
                dragged.set(Some(w));
                set_entry(&id, Some(w));
                TouchResponse::CLAIMED
            }
            TouchPhase::Ended => {
                in_drag.set(false);
                // A press without movement is not a resize: leave the
                // column (and the saved layout) alone.
                if moved.replace(false) {
                    if let (Some(cb), Some(w)) = (&on_resize, dragged.get()) {
                        cb(id.clone(), Some(w.round()));
                    }
                }
                TouchResponse::CLAIMED
            }
            TouchPhase::Cancelled => {
                in_drag.set(false);
                moved.set(false);
                if let Some(previous) = restore.take() {
                    set_entry(&id, previous);
                }
                TouchResponse::CLAIMED
            }
            TouchPhase::Hovered => TouchResponse::IGNORED,
        }
    };

    let style = move || {
        TableStyles::resize_handle()
            .into_style_application()
            .with("revealed", if heading_hovered.get() || in_drag.get() { "on" } else { "off" })
    };
    ui! {
        view(style = style) {}.on_touch(touch)
    }
}

/// A column's width as the table reads it: fixed at build, or live (a
/// resizable column follows the width map).
#[derive(Clone, Copy)]
enum ColWidth {
    Static(Option<f32>),
    Live(Memo<Option<f32>>),
}

impl ColWidth {
    fn get(&self) -> Option<f32> {
        match self {
            ColWidth::Static(w) => *w,
            ColWidth::Live(m) => m.get(),
        }
    }

    fn is_live(&self) -> bool {
        matches!(self, ColWidth::Live(_))
    }
}

/// A table's column model, ready to apply to its rows: each column's
/// width source, and the frozen-column insets they add up to.
pub(super) struct ColumnModel {
    cols: Vec<TableColumn>,
    widths: Signal<ColumnWidths>,
    sizes: Vec<ColWidth>,
    /// Width of the frozen columns the table added before the author's —
    /// live while the leading edge inset is.
    lead_pin: Reactive<f32>,
}

/// The inline geometry a column puts on each of its cells, and whether it
/// moves (a live width, or an inset that sums one).
struct Geometry {
    live: bool,
    rules: Rc<dyn Fn() -> runtime_core::StyleRules>,
}

impl ColumnModel {
    pub(super) fn new(cols: Vec<TableColumn>, widths: Signal<ColumnWidths>, lead_pin: Reactive<f32>) -> Self {
        let sizes = cols
            .iter()
            .map(|c| {
                if c.resizable {
                    ColWidth::Live(live_width(widths, c))
                } else {
                    // A column that cannot be dragged reads the map once:
                    // a saved entry still applies, and its cells stay
                    // static (preminted where nothing else is set).
                    let id = c.id.clone();
                    ColWidth::Static(widths.with_untracked(|w| w.get(&id).copied()).or(c.width))
                }
            })
            .collect();
        ColumnModel { cols, widths, sizes, lead_pin }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.cols.is_empty()
    }

    /// The SDK column specs (grid-column mode): one live track per column.
    pub(super) fn specs(&self) -> Vec<Option<table::ColumnSpec>> {
        self.cols
            .iter()
            .zip(&self.sizes)
            .map(|(c, size)| {
                let (size, min, max, fill) = (*size, c.min_width, c.max_width, c.fill);
                Some(table::ColumnSpec::live(move || {
                    let w = size.get();
                    if fill {
                        // A fill column keeps filling: a dragged width
                        // becomes its floor, not a fixed track that would
                        // leave the spare width nowhere to go.
                        table::ColumnTrack { width: None, min_width: w.or(min), max_width: max, flex: Some(1.0) }
                    } else {
                        table::ColumnTrack { width: w, min_width: min, max_width: max, flex: None }
                    }
                }))
            })
            .collect()
    }

    /// Whether column `i` is frozen with another frozen column between it
    /// and the scrolling columns — its inner edge is not the frozen
    /// block's edge, so it never draws.
    fn interior(&self, i: usize) -> bool {
        match self.cols[i].pinned {
            Some(ColumnPin::Left) => self.cols[i + 1..].iter().any(|c| c.pinned == Some(ColumnPin::Left)),
            Some(ColumnPin::Right) => self.cols[..i].iter().any(|c| c.pinned == Some(ColumnPin::Right)),
            None => false,
        }
    }

    /// The inline geometry column `i` puts on its cells, if any.
    fn geometry(&self, i: usize) -> Option<Geometry> {
        use runtime_core::{Length, StyleRules, Tokenized};
        let col = &self.cols[i];
        let size = self.sizes[i];
        // The frozen columns between this one and its edge, which its
        // inset adds up.
        let before: Vec<ColWidth> = match col.pinned {
            Some(ColumnPin::Left) => (0..i)
                .filter(|j| self.cols[*j].pinned == Some(ColumnPin::Left))
                .map(|j| self.sizes[j])
                .collect(),
            Some(ColumnPin::Right) => (i + 1..self.cols.len())
                .filter(|j| self.cols[*j].pinned == Some(ColumnPin::Right))
                .map(|j| self.sizes[j])
                .collect(),
            None => Vec::new(),
        };
        if before.iter().any(|w| w.get().is_none()) {
            runtime_core::log_warn!(
                "idea-ui Table: frozen column `{}` sits beside a frozen column with no width, \
                 so its inset cannot be computed — give every frozen column but the outermost \
                 a `width`",
                col.id
            );
        }
        let lead = if col.pinned == Some(ColumnPin::Left) { self.lead_pin.clone() } else { Reactive::Static(0.0) };
        let has_inset = col.pinned.is_some() && (!matches!(lead, Reactive::Static(w) if w == 0.0) || !before.is_empty());
        let has_size = col.width.is_some() || col.min_width.is_some() || size.is_live() || size.get().is_some();
        if !has_inset && !has_size {
            return None;
        }
        let live = size.is_live() || before.iter().any(ColWidth::is_live) || !lead.is_static();
        let (pin, fill, min) = (col.pinned, col.fill, col.min_width);
        let rules = move || {
            let px = |v: f32| Tokenized::Literal(Length::Px(v));
            let w = size.get();
            let inset = (lead.get() + before.iter().filter_map(|b| b.get()).sum::<f32>()).max(0.0);
            let inset = has_inset.then(|| px(inset));
            let (width, min_width) = if fill { (None, w.or(min)) } else { (w, min) };
            StyleRules {
                // An EXACT column: `width` alone is only a floor in a
                // browser's auto table; `max-width` at the same value is
                // what holds it.
                width: width.map(px),
                max_width: width.map(px),
                min_width: min_width.map(px),
                left: inset.clone().filter(|_| pin == Some(ColumnPin::Left)),
                right: inset.filter(|_| pin == Some(ColumnPin::Right)),
                ..Default::default()
            }
        };
        Some(Geometry { live, rules: Rc::new(rules) })
    }

    /// Apply the model to one built row's cells. `resize` is `Some` on the
    /// header row that carries the resize handles (with the release
    /// callback, if any).
    pub(super) fn apply_to_row(&self, row: &mut Element, resize: Option<Option<ColumnResizeCallback>>) {
        let Some(cells) = table::row_cells_mut(row) else { return };
        let mut at = 0usize;
        for cell in cells.iter_mut() {
            let n = match table::cell_span(cell) {
                table::ColSpan::Rest => usize::MAX,
                table::ColSpan::Columns(n) => (n as usize).max(1),
            };
            if n == 1 && at < self.cols.len() {
                self.apply_to_cell(at, cell, resize.as_ref());
            }
            at = at.saturating_add(n);
        }
    }

    fn apply_to_cell(&self, i: usize, cell: &mut Element, resize: Option<&Option<ColumnResizeCallback>>) {
        let col = &self.cols[i];
        let align = col.align.as_variant_str();
        let truncate = if col.truncate { "on" } else { "off" };
        let padding = col.padding.as_variant_str();
        let pin = col.pinned.map(|p| match p {
            ColumnPin::Left => "left",
            ColumnPin::Right => "right",
        });
        let interior = self.interior(i);
        table::map_cell_application(
            cell,
            Rc::new(move |app| {
                let app = app.with("align", align).with("truncate", truncate).with("cell_padding", padding);
                let app = match pin {
                    Some(arm) => app.with("pinned", arm),
                    None => app,
                };
                if interior {
                    app.with("pin_edge", "interior")
                } else {
                    app
                }
            }),
        );
        super::reselect_content(cell, align, truncate);
        if let Some(g) = self.geometry(i) {
            let rules = g.rules.clone();
            if g.live {
                table::map_cell_style(cell, Rc::new(move |app| app.with_inline_merged(rules())));
            } else {
                let fixed = rules();
                table::map_cell_application(cell, Rc::new(move |app| app.with_inline_merged(fixed.clone())));
            }
        }
        let is_head = table::cell_application(cell)
            .is_some_and(|app| app.variants.0.contains_key("sticky_header"));
        if let (Some(on_resize), true, true) = (resize, col.resizable, is_head) {
            let hovered = signal(false);
            let cell_ref: Ref<ViewHandle> = Ref::new();
            table::bind_cell(cell, move |h| cell_ref.fill(h));
            table::set_cell_hover(cell, Rc::new(move |entering| hovered.set(entering)));
            table::map_cell_application(cell, Rc::new(|app| app.with("anchor", "on")));
            let handle = ColumnResizeHandle(&ColumnResizeHandleProps {
                column: col.clone(),
                widths: self.widths,
                cell: cell_ref,
                heading_hovered: hovered,
                on_resize: on_resize.clone(),
            });
            table::push_cell_content(cell, handle);
        }
    }
}
