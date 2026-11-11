//! A virtualized [`Table`](super::Table): `Table(virtualized = Some(VirtualRows::new(…)))`.
//!
//! The rows are laid out on the framework's two-axis `virtual_grid`, which
//! mounts only the cells in view and holds the header row and the pinned
//! columns still at its edges (its frozen panes) while the rest scrolls
//! beneath them, on both axes. So a 100,000-row table builds about as many
//! cells as fit on screen, and frozen columns keep working with horizontal
//! scrolling.
//!
//! # Cell reuse
//!
//! The grid is built with a reusable renderer (`virtual_grid_recycled`):
//! as it scrolls, a cell that leaves is pointed at a row that enters, of the
//! same column, instead of being destroyed while another is built. Each body
//! cell reads its row through a memo — its style, tone, hover tint,
//! selection box and press all follow it. Content from [`VirtualRows::new`]
//! (built for one row) is rebuilt when the row changes, inside the kept box;
//! content from [`VirtualRows::reusing`] reads the row itself and is never
//! rebuilt. Header cells have a reuse class of their own.
//!
//! # Where the rows come from
//!
//! A virtualized table's rows come from [`VirtualRows`], not from the
//! children block: the grid drops a cell that scrolls away and builds it
//! again when it comes back, and a built element can only be mounted once.
//! `cell(i, column)` is row `i`'s content for one column — the same
//! renderer [`TableCards`](super::TableCards) takes, so one source feeds
//! both forms. The header row is built from the column model
//! (`TableColumn::label`, with sort state from [`VirtualRows::sort`]).
//!
//! # Columns
//!
//! Every cell of a column is one width, and a row is one height, because
//! the grid places cells from per-column widths and per-row heights — the
//! content of cells that aren't mounted can't be measured. A column with a
//! `width` keeps it; every other column shares the width the fixed ones
//! leave, never below its `min_width` (or [`COLUMN_RESIZE_MIN_PX`]) or above
//! its `max_width`. When the minimums don't fit, the grid scrolls sideways.
//! Leading `ColumnPin::Left` columns (and the selection column) are frozen
//! at the left edge, trailing `ColumnPin::Right` ones at the right edge.
//!
//! # What it supports, and doesn't
//!
//! The cell look is the same sheets' (`TableStyles::head_cell` /
//! `body_cell`, with the table's `styles` over them), as are density,
//! `variant`, the edge inset, frame slots, `selection` (select-all covers
//! every row, mounted or not), row tones, clickable rows (hover tints the
//! whole row), sort headers, resizable columns, `empty` and `scroller`.
//! Drag-to-reorder and row groups need every row mounted, so a virtualized
//! table refuses `on_reorder`, `on_group_reorder` and children with a panic
//! naming the prop.
//!
//! Backends without a two-axis grid engine (Linux, Windows, terminal, the
//! CPU renderer, SSR) show the framework's unsupported-primitive
//! placeholder in place of the grid.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use runtime_core::primitives::virtual_grid::{virtual_grid_recycled, FrozenPanes, VirtualGridHandle};
use runtime_core::{
    effect, memo, signal, tap, ui, Element, Length, Memo, Reactive, Ref, Signal, StyleApplication, StyleRules,
    TapRecognizer, Tokenized, VariantEnum, ViewHandle,
};

use super::columns::{live_width, ColumnResizeHandle};
use super::scroller::detach_on_cleanup;
use super::select::{select_all_line, select_row_box_live};
use super::styles::{TableSlotStyles, TableStyles};
use super::{
    added_column_growth, cell_text_children, select_cell_axes, side_inset, sort_indicator, CellTone, ColumnId,
    ColumnPin, ColumnWidths, InsetSide, RowId, RowTone, SortState, TableColumn, TableEmpty,
    TableInset, TableProps, TableVariant, COLUMN_RESIZE_MIN_PX, SELECT_COLUMN_WIDTH_PX,
};

/// Where a virtualized table's rows come from — see the module docs. Build
/// it with [`VirtualRows::new`] and chain the optional parts:
///
/// ```ignore
/// Table(
///     columns = columns,
///     virtualized = Some(
///         VirtualRows::new(
///             move || rows.get().len(),
///             move |i| rows.with(|r| r[i].id.clone().into()),
///             move |i, col| cell_for(i, col),
///         )
///         .on_row_click(move |i| open(i))
///         .on_end_reached(move || load_more()),
///     ),
///     selection = Some(picked),
///     max_height = Some(480.0),
/// )
/// ```
#[derive(Clone, runtime_core::Remote)]
pub struct VirtualRows {
    pub(super) count: Rc<dyn Fn() -> usize>,
    pub(super) id: Rc<dyn Fn(usize) -> RowId>,
    pub(super) cell: CellSource,
    pub(super) height: Option<f32>,
    pub(super) tone: Option<Rc<dyn Fn(usize) -> RowTone>>,
    pub(super) on_row_click: Option<Rc<dyn Fn(usize)>>,
    pub(super) selectable: Option<Rc<dyn Fn(usize) -> bool>>,
    pub(super) sort: Option<Rc<dyn Fn(&ColumnId) -> Option<SortState>>>,
    pub(super) on_sort: Option<Rc<dyn Fn(&ColumnId, SortState)>>,
    pub(super) on_end_reached: Option<Rc<dyn Fn()>>,
    pub(super) end_threshold: f32,
}

impl VirtualRows {
    /// The rows: how many there are (read reactively — a closure over the
    /// app's rows signal), row `i`'s identity (what a selection holds and
    /// what keeps a row's cells across an insert), and row `i`'s content
    /// for one column.
    pub fn new(
        count: impl Fn() -> usize + 'static,
        id: impl Fn(usize) -> RowId + 'static,
        cell: impl Fn(usize, &ColumnId) -> Element + 'static,
    ) -> Self {
        Self::with_source(Rc::new(count), Rc::new(id), CellSource::Fixed(Rc::new(cell)))
    }

    /// [`Self::new`] with REUSABLE content: `cell(row, column)` is built once
    /// per cell and reads its row through `row` (a memo) — `rx!(name_of(row.get()))`
    /// — so as the grid scrolls, a cell that leaves is pointed at a row that
    /// enters and only what reads `row` updates; nothing is rebuilt. Read
    /// your rows' data reactively in it too: the cell is never rebuilt for a
    /// data change either.
    ///
    /// With [`Self::new`] the table still keeps each cell's box and rebuilds
    /// only your content when the row changes; this goes the rest of the way,
    /// and it is what keeps a fast flick through a large table smooth.
    ///
    /// ```ignore
    /// VirtualRows::reusing(
    ///     move || rows.with(|r| r.len()),
    ///     move |i| rows.with(|r| r[i].id.clone()),
    ///     move |row, col| match col.as_str() {
    ///         "name" => ui! { Typography(content = rx!(rows.with(|r| r[row.get()].name.clone()))) },
    ///         _ => ui! { Typography(content = rx!(rows.with(|r| r[row.get()].email.clone()))) },
    ///     },
    /// )
    /// ```
    pub fn reusing(
        count: impl Fn() -> usize + 'static,
        id: impl Fn(usize) -> RowId + 'static,
        cell: impl Fn(Memo<usize>, &ColumnId) -> Element + 'static,
    ) -> Self {
        Self::with_source(Rc::new(count), Rc::new(id), CellSource::Reusable(Rc::new(cell)))
    }

    fn with_source(count: Rc<dyn Fn() -> usize>, id: Rc<dyn Fn(usize) -> RowId>, cell: CellSource) -> Self {
        Self {
            count,
            id,
            cell,
            height: None,
            tone: None,
            on_row_click: None,
            selectable: None,
            sort: None,
            on_sort: None,
            on_end_reached: None,
            end_threshold: 0.0,
        }
    }

    /// Every row's height, header included, in px. Defaults to a one-line
    /// row at the table's density, from the theme's cell padding and text
    /// line height.
    pub fn row_height(mut self, px: f32) -> Self {
        self.height = Some(px);
        self
    }

    /// Row `i`'s tint, as `TableRow(tone = …)`.
    pub fn tone(mut self, tone: impl Fn(usize) -> RowTone + 'static) -> Self {
        self.tone = Some(Rc::new(tone));
        self
    }

    /// Make every row clickable: the pointer cursor, a hover tint across
    /// the whole row, and `f(i)` on a press. A row's own controls (its
    /// selection box, a button in a cell) still take their own presses.
    pub fn on_row_click(mut self, f: impl Fn(usize) + 'static) -> Self {
        self.on_row_click = Some(Rc::new(f));
        self
    }

    /// Which rows `selection` may pick. Default: every row.
    pub fn selectable(mut self, f: impl Fn(usize) -> bool + 'static) -> Self {
        self.selectable = Some(Rc::new(f));
        self
    }

    /// Sort headers: `sort(column)` is `Some(state)` for a sortable column
    /// (read reactively, so the arrow follows the app's sort), and a press
    /// on its header calls `on_sort(column, state.next())`. The sorting
    /// itself is the app's.
    pub fn sort(
        mut self,
        sort: impl Fn(&ColumnId) -> Option<SortState> + 'static,
        on_sort: impl Fn(&ColumnId, SortState) + 'static,
    ) -> Self {
        self.sort = Some(Rc::new(sort));
        self.on_sort = Some(Rc::new(on_sort));
        self
    }

    /// Fetch the next page: called once as the reader comes within
    /// `threshold` px of the last row, and again after scrolling away and
    /// back. A screenful is the usual threshold.
    pub fn on_end_reached(mut self, threshold: f32, f: impl Fn() + 'static) -> Self {
        self.end_threshold = threshold.max(0.0);
        self.on_end_reached = Some(Rc::new(f));
        self
    }
}

/// How a body cell's content is built — see [`VirtualRows::new`] and
/// [`VirtualRows::reusing`].
#[derive(Clone)]
pub(super) enum CellSource {
    /// Built for one row; rebuilt when the cell is pointed at another.
    Fixed(Rc<dyn Fn(usize, &ColumnId) -> Element>),
    /// Built once, reading its row reactively.
    Reusable(Rc<dyn Fn(Memo<usize>, &ColumnId) -> Element>),
}

/// One column of the grid: the selection column the table adds, or one of
/// the author's.
#[derive(Clone)]
enum GridCol {
    Select,
    Author(TableColumn),
}

/// Split the column list into frozen bands: `(left, right)` frozen counts.
/// The selection column and every LEADING `ColumnPin::Left` column freeze at
/// the left; every TRAILING `ColumnPin::Right` column at the right.
///
/// # Panics
///
/// A pinned column that isn't in its edge run — a left pin after an
/// unpinned column — can't be frozen by the grid, which freezes the
/// columns at each END of the row.
fn frozen_columns(cols: &[GridCol]) -> (usize, usize) {
    let pin = |c: &GridCol| match c {
        GridCol::Select => Some(ColumnPin::Left),
        GridCol::Author(c) => c.pinned,
    };
    let left = cols.iter().take_while(|c| pin(c) == Some(ColumnPin::Left)).count();
    let right = cols[left..].iter().rev().take_while(|c| pin(c) == Some(ColumnPin::Right)).count();
    for c in &cols[left..cols.len() - right] {
        if let GridCol::Author(c) = c {
            assert!(
                c.pinned.is_none(),
                "virtualized Table: column `{}` is pinned {:?} but not at that edge of the row — \
                 a virtualized table freezes the columns at each end, so put left-pinned columns \
                 first and right-pinned ones last",
                c.id.as_str(),
                c.pinned,
            );
        }
    }
    (left, right)
}

/// Column widths for a row `available` px wide, as CSS `minmax(min, 1fr)`
/// tracks: a column with a `fixed` width keeps it; the rest come out the
/// SAME width — whatever fills the row — held between their own `(min,
/// max)`. A column held at its max leaves the rest wider, one held at its
/// min leaves them narrower. When the minimums don't fit, every flexible
/// column takes its minimum (and the grid scrolls sideways).
fn column_widths(available: f32, cols: &[(Option<f32>, f32, f32)]) -> Vec<f32> {
    let fixed: f32 = cols.iter().filter_map(|c| c.0).sum();
    let room = available - fixed;
    let filled = |w: f32| -> f32 { cols.iter().filter(|c| c.0.is_none()).map(|c| w.clamp(c.1, c.2)).sum() };
    // `filled` grows with the common width, so bisect for the one that
    // fills the room. Below every minimum it is the minimums' sum.
    let (mut lo, mut hi) = (0.0_f32, room.max(0.0));
    if filled(lo) < room {
        for _ in 0..48 {
            let mid = (lo + hi) / 2.0;
            if filled(mid) < room { lo = mid } else { hi = mid }
        }
    }
    // Exact on the columns the bisected width leaves between their bounds:
    // they split what the held ones leave, so the row fills to the pixel.
    let free = |c: &&(Option<f32>, f32, f32)| c.0.is_none() && hi > c.1 && hi < c.2;
    let n_free = cols.iter().filter(free).count();
    let w = if n_free == 0 {
        hi
    } else {
        let held: f32 = cols.iter().filter(|c| c.0.is_none() && !free(c)).map(|c| hi.clamp(c.1, c.2)).sum();
        (room - held) / n_free as f32
    };
    cols.iter().map(|(f, min, max)| f.unwrap_or_else(|| w.clamp(*min, *max))).collect()
}

/// The height of a one-line row from the resolved cell and text sheets:
/// the cell's vertical padding and divider, plus the text's line height.
fn one_line_height(cell: &StyleApplication, text: &StyleApplication) -> f32 {
    let px = |v: &Option<Tokenized<Length>>| match v.as_ref().map(|t| t.resolve()) {
        Some(Length::Px(v)) => v,
        _ => 0.0,
    };
    let c = runtime_core::resolve_style(cell);
    let t = runtime_core::resolve_style(text);
    let line = t
        .line_height
        .as_ref()
        .map(|l| l.resolve())
        .or_else(|| match t.font_size.as_ref().map(|f| f.resolve()) {
            Some(Length::Px(v)) => Some(v),
            _ => None,
        })
        .unwrap_or(0.0);
    let divider = c.border_bottom_width.as_ref().map(|w| w.resolve()).unwrap_or(0.0);
    px(&c.padding_top) + px(&c.padding_bottom) + line + divider
}

/// The grid's buffer outside the visible window, in viewport extents per
/// side — see where it's set.
const TABLE_OVERSCAN: f32 = 0.5;

fn row_key(id: &RowId, generation: u64, col: usize) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    generation.hash(&mut h);
    col.hash(&mut h);
    h.finish()
}

/// Everything a cell needs, shared by every cell the grid mounts.
struct Shared {
    cols: Vec<GridCol>,
    frozen: (usize, usize),
    rows: VirtualRows,
    /// Each row's id, plus the generation of the data it was read from —
    /// see [`virtual_table`].
    items: Signal<Vec<(RowId, u64)>>,
    styles: TableSlotStyles,
    density: &'static str,
    plain: bool,
    inset_left: Reactive<TableInset>,
    inset_right: Reactive<TableInset>,
    selection: Option<Signal<BTreeSet<RowId>>>,
    select_all: Option<runtime_core::Memo<Vec<RowId>>>,
    /// The row under the pointer and the row under a press: one signal for
    /// the whole table, since a row's cells mount independently and a
    /// signal created in one cell's scope would die with that cell.
    hovered: Signal<Option<usize>>,
    pressed: Signal<Option<usize>>,
    /// The grid's scroll offset and how far it can scroll sideways — what a
    /// plain table's frozen edges show by.
    scroll_x: Signal<(f32, f32)>,
    widths: Signal<ColumnWidths>,
    on_column_resize: Option<Rc<dyn Fn(ColumnId, Option<f32>)>>,
}

impl Shared {
    /// `pinned` arm, and `pin_edge` for a frozen column: its inner edge
    /// draws only on the last frozen column of its side, and a plain
    /// table's only while columns are scrolled beneath it.
    fn frozen_axes(&self, col: usize, app: StyleApplication) -> StyleApplication {
        let (left, right) = self.frozen;
        let n = self.cols.len();
        let (arm, inner_edge, side) = if col < left {
            ("left", col + 1 == left, 0)
        } else if col >= n - right {
            ("right", col == n - right, 1)
        } else {
            return app;
        };
        let app = app.with("pinned", arm);
        if !inner_edge {
            return app.with("pin_edge", "interior");
        }
        if !self.plain {
            return app;
        }
        let (x, max) = self.scroll_x.get();
        let behind = if side == 0 { x > 0.5 } else { x < max - 0.5 };
        app.with("pin_edge", if behind { "shown" } else { "hidden" })
    }

    /// The edge inset on a row's first and last cell.
    fn inset(&self, col: usize, app: StyleApplication) -> StyleApplication {
        let app = if col == 0 { self.inset_left.get().apply(app, InsetSide::Left) } else { app };
        if col + 1 == self.cols.len() {
            self.inset_right.get().apply(app, InsetSide::Right)
        } else {
            app
        }
    }

    fn column_axes(&self, col: usize) -> (&'static str, &'static str, &'static str) {
        match &self.cols[col] {
            GridCol::Select => ("center", "off", "icon"),
            GridCol::Author(c) => (
                c.align.as_variant_str(),
                if c.truncate { "on" } else { "off" },
                c.padding.as_variant_str(),
            ),
        }
    }
}

/// Build a virtualized table. `props.virtualized` has been taken out into
/// `rows`.
pub(super) fn virtual_table(props: TableProps, rows: VirtualRows) -> Element {
    assert!(
        !props.columns.is_empty(),
        "virtualized Table: `columns` is required — the grid sizes and labels its columns from them"
    );
    assert!(
        props.children.is_empty(),
        "virtualized Table: rows come from `VirtualRows`, not the children block (the header is \
         built from `columns`)"
    );
    assert!(
        props.on_reorder.is_none() && props.on_group_reorder.is_none(),
        "virtualized Table: drag-to-reorder needs every row mounted; it isn't available on a \
         virtualized table"
    );

    let mut cols: Vec<GridCol> = Vec::with_capacity(props.columns.len() + 1);
    if props.selection.is_some() {
        cols.push(GridCol::Select);
    }
    cols.extend(props.columns.iter().cloned().map(GridCol::Author));
    let frozen = frozen_columns(&cols);

    // Each row's id and the generation of the data it was read from. The
    // generation moves on every change to the app's rows (the `id` reads
    // subscribe), so a cell whose row changed under the same id is rebuilt
    // — its key carries the generation — as `TableCards` does.
    //
    // Seeded with the first read rather than set by the effect's first run:
    // a write staged at build would leave the grid's first fill (same turn)
    // reading an empty list.
    let read_rows = {
        let (count, id) = (rows.count.clone(), rows.id.clone());
        move |g: u64| (0..count()).map(|i| (id(i), g)).collect::<Vec<(RowId, u64)>>()
    };
    let items: Signal<Vec<(RowId, u64)>> = signal(runtime_core::untrack(|| read_rows(0)));
    {
        let generation = std::cell::Cell::new(0u64);
        let _ = effect(move || {
            let g = generation.get();
            let next = read_rows(g + 1);
            // The first run only subscribes: `items` already holds it.
            if g > 0 {
                items.set(next);
            }
            generation.set(g + 1);
        });
    }
    let select_all = props.selection.map(|_| {
        let selectable = rows.selectable.clone();
        memo(move || {
            items.with(|v| {
                v.iter()
                    .enumerate()
                    .filter(|(i, _)| selectable.as_ref().is_none_or(|f| f(*i)))
                    .map(|(_, (id, _))| id.clone())
                    .collect::<Vec<RowId>>()
            })
        })
    });

    let density = props.density.as_variant_str();
    let plain = props.variant == TableVariant::Plain;
    let row_height = rows.height.unwrap_or_else(|| {
        let cell = TableStyles::body_cell()
            .overridden(props.styles.body_cell.as_ref())
            .into_style_application()
            .with("density", density);
        let text = TableStyles::body_text().overridden(props.styles.body_text.as_ref()).into_style_application();
        one_line_height(&cell, &text)
    });
    let inset_left = side_inset(&props.inset_left, &props.inset_x);
    let inset_right = side_inset(&props.inset_right, &props.inset_x);
    let widths = props.column_widths.unwrap_or_else(|| signal(ColumnWidths::new()));
    let viewport_w = signal(0.0f32);
    let shared = Rc::new(Shared {
        cols,
        frozen,
        rows,
        items,
        styles: props.styles.clone(),
        density,
        plain,
        inset_left,
        inset_right,
        selection: props.selection,
        select_all,
        hovered: signal(None),
        pressed: signal(None),
        scroll_x: signal((0.0, 0.0)),
        widths,
        on_column_resize: props.on_column_resize.clone(),
    });

    // Column widths: live entries (a resize drag) and fixed specs keep
    // theirs; the rest share the scrollport's width. The grid re-lays out
    // when any of these reads change.
    let col_widths = {
        let shared = shared.clone();
        let live: Vec<Option<runtime_core::Memo<Option<f32>>>> = shared
            .cols
            .iter()
            .map(|c| match c {
                GridCol::Author(c) => Some(live_width(shared.widths, c)),
                GridCol::Select => None,
            })
            .collect();
        let lead_growth = {
            let inset = shared.inset_left.clone();
            move || added_column_growth(inset.get())
        };
        memo(move || {
            let specs: Vec<(Option<f32>, f32, f32)> = shared
                .cols
                .iter()
                .zip(&live)
                .enumerate()
                .map(|(i, (c, live))| match c {
                    GridCol::Select => {
                        let w = SELECT_COLUMN_WIDTH_PX + if i == 0 { lead_growth() } else { 0.0 };
                        (Some(w), w, w)
                    }
                    GridCol::Author(c) => {
                        let min = c.min_width.unwrap_or(COLUMN_RESIZE_MIN_PX);
                        let max = c.max_width.unwrap_or(f32::INFINITY).max(min);
                        (live.as_ref().and_then(|m| m.get()), min, max)
                    }
                })
                .collect();
            column_widths(viewport_w.get(), &specs)
        })
    };

    let handle: Rc<RefCell<Option<VirtualGridHandle>>> = Rc::new(RefCell::new(None));
    // The scrollport subscription lives exactly as long as the table: it
    // is filled at mount (`on_handle`) and dropped with the table's scope.
    let port_sub: Rc<RefCell<Option<runtime_core::LayoutSubscription>>> = Rc::new(RefCell::new(None));
    {
        let (port_sub, handle) = (port_sub.clone(), handle.clone());
        runtime_core::on_scope_drop(move || {
            drop(port_sub.borrow_mut().take());
            drop(handle.borrow_mut().take());
        });
    }
    if let Some(scroller) = props.scroller.clone() {
        let (handle, items) = (handle.clone(), shared.items);
        let token = scroller.attach(Rc::new(move |id: &RowId| {
            let Some(r) = items.with_untracked(|v| v.iter().position(|(i, _)| i == id)) else { return };
            if let Some(h) = handle.borrow().as_ref() {
                // Under the frozen header: row `r` is grid row `r + 1`, whose
                // offset less the header's is `r` rows.
                let (x, _) = h.scroll_offset();
                h.scroll_to(x, r as f32 * row_height);
            }
        }));
        detach_on_cleanup(scroller, token);
    }

    // `on_end_reached`: armed while the reader is further than the threshold
    // from the last row, fired once on crossing it.
    let end_armed = Rc::new(std::cell::Cell::new(true));
    let on_scroll = {
        let (shared, handle, end_armed) = (shared.clone(), handle.clone(), end_armed.clone());
        move |x: f32, y: f32| {
            let port = handle.borrow().as_ref().and_then(|h| h.scrollport());
            let (vw, vh) = port.map_or((0.0, 0.0), |p| (p.width, p.height));
            let content_w: f32 = runtime_core::untrack(|| col_widths.with(|w| w.iter().sum()));
            let max_x = (content_w - vw).max(0.0);
            if shared.plain && shared.frozen != (0, 0) {
                shared.scroll_x.set((x, max_x));
            }
            if let Some(f) = shared.rows.on_end_reached.as_ref() {
                let count = shared.items.with_untracked(|v| v.len());
                let content_h = (count + 1) as f32 * row_height;
                let near = y + vh >= content_h - shared.rows.end_threshold;
                if near && end_armed.get() {
                    end_armed.set(false);
                    f();
                } else if !near {
                    end_armed.set(true);
                }
            }
        }
    };

    // Cells are REUSED as the grid scrolls (`virtual_grid_recycled`): a
    // body cell that leaves is pointed at a row that enters, of the same
    // column (its reuse class) — its box, style and the parts of its content
    // that read the row follow; nothing is rebuilt. A header cell has a class
    // of its own, so it is never pointed at a body row.
    let reuse_class = |c: usize, r: usize| if r == 0 { u64::MAX - c as u64 } else { c as u64 };
    let render = {
        let shared = shared.clone();
        move |at: runtime_core::ReadSignal<(usize, usize)>| {
            let (c, r) = runtime_core::untrack(|| at.get());
            if r == 0 {
                head_cell(&shared, c)
            } else {
                body_cell(&shared, c, memo(move || at.get().1 - 1))
            }
        }
    };
    let key = {
        let items = shared.items;
        move |c: usize, r: usize| {
            if r == 0 {
                return row_key(&RowId::from("\u{0}head"), 0, c);
            }
            items.with(|v| v.get(r - 1).map_or(u64::MAX - c as u64, |(id, g)| row_key(id, *g, c)))
        }
    };
    let grid_style = match props.max_height {
        // Fit the rows, up to the cap: the grid reports its content size.
        Some(h) => StyleRules {
            flex_grow: Some(Tokenized::Literal(0.0)),
            flex_basis: Some(Tokenized::Literal(Length::Auto)),
            max_height: Some(Tokenized::Literal(Length::Px(h))),
            ..Default::default()
        },
        // Fill the bounded box the table sits in.
        None => StyleRules {
            flex_grow: Some(Tokenized::Literal(1.0)),
            flex_basis: Some(Tokenized::Literal(Length::Px(0.0))),
            min_height: Some(Tokenized::Literal(Length::Px(0.0))),
            ..Default::default()
        },
    };
    let count = shared.items;
    let n_cols = shared.cols.len();
    let grid = virtual_grid_recycled(
        move || n_cols,
        move || count.with(|v| v.len()) + 1,
        move |c| col_widths.with(|w| w.get(c).copied().unwrap_or(0.0)),
        move |_| row_height,
        key,
        reuse_class,
        render,
    )
    .frozen(FrozenPanes { top: 1, left: frozen.0, right: frozen.1, ..FrozenPanes::NONE })
    // Half a viewport ahead on each axis. Two-axis overscan multiplies: the
    // grid's default (a whole viewport each side) mounts about nine
    // screenfuls of cells, and every table cell is a small styled subtree.
    .overscan(TABLE_OVERSCAN)
    .on_scroll(on_scroll)
    .on_handle({
        let (handle, port_sub) = (handle.clone(), port_sub.clone());
        move |h| {
            // The scrollport's width is what flexible columns share.
            *port_sub.borrow_mut() = Some(h.on_scrollport(move |p| viewport_w.set(p.width)));
            *handle.borrow_mut() = Some(h);
        }
    })
    .style(grid_style)
    .build();

    let empty = props.empty.map(|empty| {
        let content = match empty {
            TableEmpty::Text(text) => {
                let style = TableStyles::body_text()
                    .overridden(props.styles.body_text.as_ref())
                    .into_style_application()
                    .with("text_tone", CellTone::Muted.as_variant_str());
                ui! { text(style = style) { text } }
            }
            TableEmpty::Content(content) => content,
        };
        let app = TableStyles::body_cell()
            .overridden(props.styles.body_cell.as_ref())
            .into_style_application()
            .with("density", density)
            .with("divider", "off");
        // Shown while there are no rows, collapsed to nothing otherwise —
        // not rebuilt, since an element empty state can only be mounted
        // once and the rows can come and go (a filter, a page still loading).
        let no_rows = shared.items;
        let style = move || {
            if no_rows.with(|v| v.is_empty()) {
                app.clone()
            } else {
                let zero = || Some(Tokenized::Literal(Length::Px(0.0)));
                app.clone().with_inline_merged(StyleRules {
                    max_height: zero(),
                    padding_top: zero(),
                    padding_bottom: zero(),
                    border_bottom_width: Some(Tokenized::Literal(0.0)),
                    overflow: Some(runtime_core::Overflow::Hidden),
                    ..Default::default()
                })
            }
        };
        (content, style)
    });
    let frame = TableStyles::frame()
        .overridden(props.styles.frame.as_ref())
        .into_style_application()
        .with("variant", props.variant.as_variant_str())
        .with("scroll", "fill");
    let header_slot = props.header_slot;
    let footer_slot = props.footer_slot;
    ui! {
        view(style = frame) {
            header_slot
            grid
            if let Some((content, style)) = empty {
                view(style = style) { content }
            }
            footer_slot
        }
    }
}

/// The style a grid cell starts from: its sheet with the table's override,
/// placed in the grid, at the table's density and variant, with its
/// column's alignment, truncation and padding.
fn cell_app(shared: &Shared, head: bool, col: usize) -> StyleApplication {
    let app = if head {
        TableStyles::head_cell()
            .overridden(shared.styles.head_cell.as_ref())
            .into_style_application()
            .with("sticky_header", "off")
    } else {
        TableStyles::body_cell().overridden(shared.styles.body_cell.as_ref()).into_style_application()
    };
    let (align, truncate, padding) = shared.column_axes(col);
    let app = select_cell_axes(app, None, align, truncate, padding)
        .with("placed", "grid")
        .with("density", shared.density)
        .with("variant", if shared.plain { "plain" } else { "framed" });
    // The cell fills its slot; the grid sets its size.
    app.with_inline_merged(StyleRules {
        width: Some(Tokenized::Literal(Length::Percent(100.0))),
        height: Some(Tokenized::Literal(Length::Percent(100.0))),
        ..Default::default()
    })
}

/// The header row's cell for grid column `col`.
fn head_cell(shared: &Rc<Shared>, col: usize) -> Element {
    let base = cell_app(shared, true, col);
    let column = match &shared.cols[col] {
        GridCol::Select => {
            let content = match (shared.selection, shared.select_all) {
                (Some(s), Some(all)) => Some(select_all_line(s, all, Reactive::Static(None))),
                _ => None,
            };
            let style = {
                let shared = shared.clone();
                move || shared.inset(col, shared.frozen_axes(col, base.clone()))
            };
            return ui! { view(style = style) { content } };
        }
        GridCol::Author(c) => c.clone(),
    };
    let (align, truncate, _) = shared.column_axes(col);
    let sort: Option<Reactive<Option<SortState>>> = shared.rows.sort.as_ref().map(|f| {
        let (f, id) = (f.clone(), column.id.clone());
        Reactive::Dynamic(Rc::new(move || f(&id)))
    });
    let sortable = sort.as_ref().is_some_and(|s| s.get().is_some());
    let mut content = cell_text_children(
        true,
        Reactive::Static(column.label.clone()),
        align,
        truncate,
        Reactive::Static(CellTone::None),
        &shared.styles,
    );
    if let (true, Some(sort)) = (sortable, sort.clone()) {
        content.push(sort_indicator(sort));
    }
    let inner = TableStyles::cell_inner()
        .overridden(shared.styles.cell_inner.as_ref())
        .into_style_application()
        .with("align", align);
    let anchor = column.resizable;
    let style = {
        let shared = shared.clone();
        move || {
            let app = shared.inset(col, shared.frozen_axes(col, base.clone()));
            let app = if sortable { app.with("interactive", "on") } else { app };
            if anchor { app.with("anchor", "on") } else { app }
        }
    };
    let press = match (sortable, sort, shared.rows.on_sort.clone()) {
        (true, Some(sort), Some(on_sort)) => {
            let id = column.id.clone();
            let press = tap(TapRecognizer::new(), move || on_sort(&id, sort.get().unwrap_or_default().next()));
            Some(Rc::new(move |ev: &runtime_core::TouchEvent| press(ev)) as runtime_core::TouchHandler)
        }
        _ => None,
    };
    let cell_ref: Ref<ViewHandle> = Ref::new();
    let heading_hovered = signal(false);
    let handle = column.resizable.then(|| {
        let on_resize = shared.on_column_resize.clone();
        ui! {
            ColumnResizeHandle(
                column = column.clone(),
                widths = shared.widths,
                cell = cell_ref.clone(),
                heading_hovered = heading_hovered,
                on_resize = on_resize,
            )
        }
    });
    let hover = move |entering: bool| heading_hovered.set(entering);
    match press {
        Some(press) => ui! {
            view(style = style, bind = cell_ref, on_hover = hover, on_touch = move |ev| press(ev)) {
                view(style = inner) { content }
                handle
            }
        },
        None => ui! {
            view(style = style, bind = cell_ref, on_hover = hover) {
                view(style = inner) { content }
                handle
            }
        },
    }
}

/// A body cell of grid column `col`, showing body row `row` — a memo, since
/// the grid reuses the cell for other rows as it scrolls. Everything that
/// varies by row reads it: the content, the selection box, the tone, the
/// hover tint and the press.
fn body_cell(shared: &Rc<Shared>, col: usize, row: Memo<usize>) -> Element {
    let base = cell_app(shared, false, col);
    let items = shared.items;
    let content: Element = match &shared.cols[col] {
        GridCol::Select => match shared.selection {
            Some(selection) => {
                let selectable = shared.rows.selectable.clone();
                let id: Rc<dyn Fn() -> Option<RowId>> =
                    Rc::new(move || items.with(|v| v.get(row.get()).map(|(id, _)| id.clone())));
                let shown = {
                    let id = id.clone();
                    move || id().is_some() && selectable.as_ref().is_none_or(|f| f(row.get()))
                };
                ui! {
                    if shown() {
                        select_row_box_live(selection, id.clone())
                    }
                }
            }
            None => ui! { view() },
        },
        GridCol::Author(c) => match &shared.rows.cell {
            CellSource::Reusable(f) => f(row, &c.id),
            CellSource::Fixed(f) => {
                // Content built for one row: rebuilt when the cell is pointed
                // at another, or that row's data changes — the cell's box
                // around it is kept.
                let (f, id) = (f.clone(), c.id.clone());
                // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape
                runtime_core::switch(
                    move || {
                        let r = row.get();
                        (r, items.with(|v| v.get(r).map(|(_, g)| *g)))
                    },
                    move |&(r, generation)| match generation {
                        Some(_) => f(r, &id),
                        None => ui! { view() },
                    },
                )
            }
        },
    };
    let (align, _, _) = shared.column_axes(col);
    let inner = TableStyles::cell_inner()
        .overridden(shared.styles.cell_inner.as_ref())
        .into_style_application()
        .with("align", align);
    let tone = shared.rows.tone.clone();
    let clickable = shared.rows.on_row_click.is_some();
    // Whether THIS row is lit (hovered or pressed). The style reads this
    // memo, not the table-wide signals: a memo re-runs on every pointer move
    // but only re-runs the style when its own answer changes, so moving the
    // pointer one row restyles that row and the row it left — not every
    // mounted cell (each restyle also dirties the cell's layout on native).
    let lit = clickable.then(|| {
        let (hovered, pressed) = (shared.hovered, shared.pressed);
        memo(move || {
            let r = Some(row.get());
            hovered.get() == r || pressed.get() == r
        })
    });
    let style = {
        let shared = shared.clone();
        move || {
            let app = shared.inset(col, shared.frozen_axes(col, base.clone()));
            let app = match tone.as_ref().map(|f| f(row.get())) {
                Some(t) if t != RowTone::None => app.with("tone", t.as_variant_str()),
                _ => app,
            };
            match lit {
                None => app,
                Some(lit) => app.with("interactive", "on").with("row_hovered", if lit.get() { "on" } else { "off" }),
            }
        }
    };
    let here = move || runtime_core::untrack(|| row.get());
    match shared.rows.on_row_click.clone() {
        Some(cb) => {
            let (hovered, pressed) = (shared.hovered, shared.pressed);
            let press = runtime_core::tap_with_press(
                TapRecognizer::new(),
                move || cb(here()),
                move |down| pressed.set(down.then(here)),
            );
            let hover = move |entering: bool| {
                if entering {
                    hovered.set(Some(here()));
                } else if hovered.peek() == Some(here()) {
                    hovered.set(None);
                }
            };
            ui! {
                view(style = style, on_hover = hover, on_touch = move |ev| press(ev)) {
                    view(style = inner) { content }
                }
            }
        }
        None => ui! {
            view(style = style) {
                view(style = inner) { content }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed columns keep their width; the rest share the remainder
    /// equally; a capped column gives its surplus to the others.
    #[test]
    fn flexible_columns_share_the_width_the_fixed_ones_leave() {
        let w = column_widths(600.0, &[(Some(100.0), 100.0, 100.0), (None, 50.0, f32::INFINITY), (None, 50.0, f32::INFINITY)]);
        assert_eq!(w, vec![100.0, 250.0, 250.0]);
        let w = column_widths(600.0, &[(None, 50.0, 120.0), (None, 50.0, f32::INFINITY)]);
        assert_eq!(w, vec![120.0, 480.0], "the capped column's surplus goes to the other");
        let w = column_widths(600.0, &[(None, 48.0, f32::INFINITY), (None, 120.0, f32::INFINITY)]);
        assert_eq!(w, vec![300.0, 300.0], "different minimums, the same width once both fit");
        let w = column_widths(300.0, &[(None, 48.0, f32::INFINITY), (None, 200.0, f32::INFINITY)]);
        assert_eq!(w, vec![100.0, 200.0], "a column held at its minimum leaves the rest narrower");
    }

    /// When the minimums don't fit, every flexible column takes its minimum
    /// — the grid scrolls sideways rather than squeezing columns to nothing.
    #[test]
    fn columns_hold_their_minimum_when_the_row_is_too_narrow() {
        let w = column_widths(100.0, &[(Some(80.0), 80.0, 80.0), (None, 60.0, f32::INFINITY), (None, 70.0, 200.0)]);
        assert_eq!(w, vec![80.0, 60.0, 70.0]);
        let w = column_widths(0.0, &[(None, 60.0, f32::INFINITY)]);
        assert_eq!(w, vec![60.0], "before the scrollport is known");
    }

    fn col(id: &str, pin: Option<ColumnPin>) -> GridCol {
        let c = TableColumn::new(id);
        GridCol::Author(match pin {
            Some(p) => c.pinned(p),
            None => c,
        })
    }

    #[test]
    fn edge_pinned_columns_and_the_selection_column_freeze() {
        let cols = [GridCol::Select, col("a", Some(ColumnPin::Left)), col("b", None), col("c", Some(ColumnPin::Right))];
        assert_eq!(frozen_columns(&cols), (2, 1));
        assert_eq!(frozen_columns(&[col("a", None), col("b", None)]), (0, 0));
    }

    #[test]
    #[should_panic(expected = "pinned")]
    fn a_pinned_column_away_from_its_edge_is_refused() {
        frozen_columns(&[col("a", None), col("b", Some(ColumnPin::Left)), col("c", None)]);
    }
}
