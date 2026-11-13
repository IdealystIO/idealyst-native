//! [`TableCards`] — a table's narrow (phone) form: a virtualized list of
//! cards, one per row, laid out from the SAME [`TableColumn`] specs the
//! wide `Table` takes.
//!
//! Each column's [`CardSlot`] says where it goes: the `Title` column is
//! the card's title (the first column when none claims it), the
//! `Opposite` column sits across from it on the title line, the `Actions`
//! column is the title line's trailing control, and `Field` columns become
//! labelled cells under the title — up to `max_cells` of them, a `FieldWide`
//! taking a whole line. The rest, and every `Detail` column, move to the row's
//! detail sheet behind a "+N more" line, so a forty-column table still reaches
//! all forty on a phone. Labels are the columns' `label`s, unless `cell_caption`
//! names one for a row; `cell_empty` drops a row's empty cells instead of
//! drawing a label over a dash, and an empty cell doesn't use up a place.
//!
//! `presentation = CardPresentation::Hairline` draws one line per record
//! instead of a framed card: the title over one meta line (the `Opposite` and
//! `Meta` columns), the `Trailing` chip and the `Actions` control at the end,
//! the theme's border between rows. `Field` and `Detail` columns live in the
//! detail sheet, which a press on the row opens unless `on_row_click` claims it.
//!
//! `on_row_menu` opens a row's menu on a hold or a right-click. It rides the
//! row's own press handler: a `long_press` wrapped around the list never sees
//! a hold, because the row's press takes the touch.
//!
//! `scroller` scrolls the list to a row ([`TableScroller`]); hand the same one
//! to the wide form.
//!
//! With a `selection`, a pinned line above the rows selects them all (tri-state)
//! and says how many are selected. `selection_header = false` leaves it out, for
//! a screen with its own.
//!
//! The list is virtualized with MEASURED heights: each card is laid out
//! and measured as it mounts, so a card's height is whatever its content
//! is — no height model to keep in step with the sheets. `on_end_reached`
//! fetches the next page as the reader nears the end.
//!
//! The wide and narrow forms are two components because a table's rows
//! are built before the table runs and cannot be rebuilt as cards — so
//! the app switches on the breakpoint and hands both forms the same
//! column specs and the same per-cell renderer:
//!
//! ```ignore
//! let cell = Rc::new(move |i: usize, col: &ColumnId| cell_for(&rows.get()[i], col));
//! ui! {
//!     if current_breakpoint().get().is_at_least(Breakpoint::Sm) {
//!         Table(columns = columns.clone()) { … rows built with `cell` … }
//!     } else {
//!         TableCards(
//!             columns = columns.clone(),
//!             row_count = rx!(rows.get().len()),
//!             row_id = Rc::new(move |i| rows.get()[i].id.clone().into()),
//!             cell = cell.clone(),
//!             on_end_reached = Some(load_more),
//!         )
//!     }
//! }
//! ```

use super::styles::{TableSlotStyles, TableSpan, TableStyles};
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use runtime_core::primitives::flat_list::{flat_list, FlatListItemSize};
use runtime_core::{
    component, effect, memo, rx, signal, ui, Element, IdealystSchema, IntoElement, Reactive, Signal, TouchPoint,
};

use super::row_press::{row_press, RowMenu};
use super::scroller::{detach_on_cleanup_cell, TableScroller};
use super::select::{select_all_line, select_row_box};
use super::{CardSlot, ColumnId, RowId, TableColumn};
use crate::components::button::Button;
use crate::components::modal::{Modal, ModalPresentation};
use crate::components::typography::Typography;
use crate::typography_kind;

/// How many labelled cells a card shows before the rest move behind
/// "+N more". A fourth is a sign the columns need ranking, not a smaller
/// card.
pub const CARD_CELL_CAP: usize = 3;

/// A card's height before it is measured, px — the virtualizer's first
/// guess, replaced by the real height as each card mounts.
pub const CARD_ESTIMATE_PX: f32 = 132.0;

/// Between two cards, px.
pub const CARD_GAP_PX: f32 = 10.0;

/// How far before the last card the next page is asked for, px — about
/// half a phone screen, so the page arrives before the reader does.
pub const CARD_LOAD_AHEAD_PX: f32 = 400.0;

/// A hairline row's minimum height, px: a comfortable tap target with room
/// for a title over a meta line.
pub const CARD_ROW_MIN_HEIGHT_PX: f32 = 64.0;

/// A hairline row's height before it is measured, px.
const CARD_ROW_ESTIMATE_PX: f32 = CARD_ROW_MIN_HEIGHT_PX;

/// How a [`TableCards`] list draws a record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema)]
pub enum CardPresentation {
    /// A framed card per record: a title line over labelled cells. The default.
    #[default]
    Cards,
    /// One line per record, edge to edge, the theme's border between rows: the
    /// title over one meta line, a trailing chip and the row's actions.
    Hairline,
}

/// A labelled cell: where a column's value goes, under what caption.
#[derive(Clone, Debug, PartialEq)]
struct CardCell {
    id: ColumnId,
    /// The column's `label`, or its id.
    label: String,
    /// `CardSlot::FieldWide`: takes the whole line.
    wide: bool,
}

/// Where each column goes on a card.
struct CardPlan {
    title: Option<ColumnId>,
    /// The title line's opposite value. A hairline row reads it through `meta`.
    opposite: Option<ColumnId>,
    /// A hairline row's meta line, in column order. Empty on a framed card.
    meta: Vec<ColumnId>,
    trailing: Option<ColumnId>,
    actions: Option<ColumnId>,
    /// Candidates for the labelled cells on the card, in column order. Empty on
    /// a hairline row.
    fields: Vec<CardCell>,
    /// How many labelled cells a card shows.
    max_cells: usize,
    /// Labelled cells in the detail sheet: every field and detail column.
    detail: Vec<CardCell>,
}

impl CardPlan {
    fn new(columns: &[TableColumn], max_cells: usize, presentation: CardPresentation) -> Self {
        let hairline = presentation == CardPresentation::Hairline;
        let title = columns
            .iter()
            .find(|c| c.card == CardSlot::Title)
            .or_else(|| columns.iter().find(|c| matches!(c.card, CardSlot::Field | CardSlot::FieldWide)))
            .map(|c| c.id.clone());
        let pick = |slot: CardSlot| columns.iter().find(|c| c.card == slot).map(|c| c.id.clone());
        let is_title = |c: &TableColumn| Some(&c.id) == title.as_ref();
        let cell = |c: &TableColumn| CardCell {
            id: c.id.clone(),
            label: c.label.clone().unwrap_or_else(|| c.id.to_string()),
            wide: c.card == CardSlot::FieldWide,
        };
        let is_field = |c: &TableColumn| match c.card {
            CardSlot::Field | CardSlot::FieldWide => true,
            CardSlot::Meta => !hairline,
            _ => false,
        };
        let fields: Vec<CardCell> = if hairline {
            Vec::new()
        } else {
            columns.iter().filter(|c| is_field(c) && !is_title(c)).map(cell).collect()
        };
        let meta = if hairline {
            columns
                .iter()
                .filter(|c| matches!(c.card, CardSlot::Opposite | CardSlot::Meta))
                .map(|c| c.id.clone())
                .collect()
        } else {
            Vec::new()
        };
        let detail = columns
            .iter()
            .filter(|c| (is_field(c) || c.card == CardSlot::Detail) && !is_title(c))
            .map(cell)
            .collect();
        CardPlan {
            title,
            opposite: if hairline { None } else { pick(CardSlot::Opposite) },
            meta,
            trailing: pick(CardSlot::Trailing),
            actions: pick(CardSlot::Actions),
            fields,
            max_cells,
            detail,
        }
    }

    /// The labelled cells one row shows, and how many of its other cells only
    /// the detail sheet holds. A cell `is_empty` reports doesn't take a place on
    /// the card or count toward "+N more".
    fn row(&self, is_empty: impl Fn(&ColumnId) -> bool) -> (Vec<&CardCell>, usize) {
        let shown: Vec<&CardCell> =
            self.fields.iter().filter(|c| !is_empty(&c.id)).take(self.max_cells).collect();
        let held = self.detail.iter().filter(|c| !is_empty(&c.id)).count();
        let more = held.saturating_sub(shown.len());
        (shown, more)
    }
}

/// Props for [`TableCards`].
#[runtime_core::props(styles = TableStyles)]
#[derive(IdealystSchema)]
pub struct TableCardsProps {
    /// The table's column specs — the same list the wide `Table` takes.
    /// Each column's `card` slot and `label` place it on the card.
    #[prop(static)]
    pub columns: Vec<TableColumn>,
    /// How many rows there are. Reactive: pass `rx!(rows.get().len())`.
    pub row_count: usize,
    /// Row `i`'s identity — the virtualizer's key, and what a selection
    /// holds.
    pub row_id: Rc<dyn Fn(usize) -> RowId>,
    /// Row `i`'s content for one column — the same renderer the wide form
    /// builds its cells from.
    pub cell: Rc<dyn Fn(usize, &ColumnId) -> Element>,
    /// How a record is drawn: a framed card, or one line in a hairline list.
    /// Default [`CardPresentation::Cards`].
    #[prop(static)]
    pub presentation: CardPresentation,
    /// Labelled cells a card shows before "+N more". Default
    /// [`CARD_CELL_CAP`].
    #[prop(static)]
    pub max_cells: usize,
    /// Is row `i`'s value empty for this column? When it says so the cell is
    /// not drawn, on the card, on a hairline row's meta line or in the detail
    /// sheet, so a usually-empty column doesn't put a label over a dash on
    /// every card. `None` (the default) draws every cell.
    pub cell_empty: Option<Rc<dyn Fn(usize, &ColumnId) -> bool>>,
    /// Row `i`'s caption for a labelled cell, when it differs per row. `None`
    /// from the closure, or no closure (the default), uses the column's `label`.
    pub cell_caption: Option<Rc<dyn Fn(usize, &ColumnId) -> Option<String>>>,
    /// Called with the row index when a card is pressed — the row's own
    /// controls (its menu, its checkbox) still take their own presses.
    pub on_row_click: Option<Rc<dyn Fn(usize)>>,
    /// Open row `i`'s menu: called with the press point (window
    /// coordinates) on a hold (a long press, on touch) or a secondary
    /// press. It shares the row's press handler with `on_row_click`, so a
    /// hold opens the menu and not the row. A `long_press` wrapped around
    /// the list never sees a hold: the row's own press takes the touch.
    pub on_row_menu: Option<Rc<dyn Fn(usize, TouchPoint)>>,
    /// Fetch the next page: called as the reader nears the last card.
    pub on_end_reached: Option<Rc<dyn Fn()>>,
    /// The screen's selection, as for `Table(selection = …)`: a box on each
    /// card's title line, and a pinned line above the list with a tri-state
    /// select-all over the list's rows and "N selected". As on the wide form,
    /// ids that aren't in the list stay selected.
    #[prop(static)]
    pub selection: Option<Signal<BTreeSet<RowId>>>,
    /// The select-all line's text for a selected-row count. Defaults to "Select
    /// all" at zero and "N selected" otherwise.
    pub selection_label: Option<Rc<dyn Fn(usize) -> String>>,
    /// Whether a `selection` draws the select-all line. Default `true`; turn it
    /// off when the screen has its own select-all header, so a phone doesn't
    /// show two. Each card keeps its box.
    #[prop(static)]
    pub selection_header: bool,
    /// What an empty list says.
    #[prop(static)]
    pub empty: Option<String>,
    /// Scrolls this list to a row: see [`TableScroller`]. Only rows it has been
    /// given can be reached; an id past the last loaded page does nothing.
    #[prop(static)]
    pub scroller: Option<TableScroller>,
}

impl Default for TableCardsProps {
    fn default() -> Self {
        Self {
            columns: Vec::new(),
            row_count: Reactive::Static(0),
            row_id: Rc::new(|i: usize| RowId::from(i)),
            cell: Rc::new(|_, _| runtime_core::view(Vec::new()).into_element()),
            presentation: CardPresentation::default(),
            max_cells: CARD_CELL_CAP,
            cell_empty: None,
            cell_caption: None,
            on_row_click: None,
            on_row_menu: None,
            on_end_reached: None,
            selection: None,
            selection_label: None,
            selection_header: true,
            empty: None,
            scroller: None,
            styles: Default::default(),
        }
    }
}

/// The select-all line's text when the host names none.
fn default_selection_label(count: usize) -> String {
    match count {
        0 => "Select all".to_string(),
        n => format!("{n} selected"),
    }
}

/// Everything one card reads, shared by every card of the list.
struct CardsModel {
    plan: CardPlan,
    row_id: Rc<dyn Fn(usize) -> RowId>,
    cell: Rc<dyn Fn(usize, &ColumnId) -> Element>,
    cell_empty: Option<Rc<dyn Fn(usize, &ColumnId) -> bool>>,
    cell_caption: Option<Rc<dyn Fn(usize, &ColumnId) -> Option<String>>>,
    on_row_click: Option<Rc<dyn Fn(usize)>>,
    on_row_menu: Option<Rc<dyn Fn(usize, TouchPoint)>>,
    selection: Option<Signal<BTreeSet<RowId>>>,
    presentation: CardPresentation,
    /// The row whose detail sheet is open.
    detail_row: Signal<Option<usize>>,
    /// The instance's per-slot overrides (the `card*` / `cards_*` /
    /// `detail_row` slots).
    styles: TableSlotStyles,
}

impl Default for CardsModel {
    fn default() -> Self {
        CardsModel {
            plan: CardPlan::new(&[], CARD_CELL_CAP, CardPresentation::default()),
            row_id: Rc::new(|i: usize| RowId::from(i)),
            cell: Rc::new(|_, _| runtime_core::view(Vec::new()).into_element()),
            cell_empty: None,
            cell_caption: None,
            on_row_click: None,
            on_row_menu: None,
            selection: None,
            presentation: CardPresentation::default(),
            detail_row: signal(None),
            styles: Default::default(),
        }
    }
}

impl CardsModel {
    fn is_empty(&self, i: usize, id: &ColumnId) -> bool {
        self.cell_empty.as_ref().is_some_and(|f| f(i, id))
    }

    /// Row `i`'s menu callback, when the list has one.
    fn row_menu(&self, i: usize) -> Option<RowMenu> {
        self.on_row_menu.clone().map(|f| Rc::new(move |at| f(i, at)) as RowMenu)
    }

    /// A labelled cell's caption on row `i`.
    fn caption(&self, i: usize, cell: &CardCell) -> String {
        self.cell_caption.as_ref().and_then(|f| f(i, &cell.id)).unwrap_or_else(|| cell.label.clone())
    }
}

/// A virtualizer key for a row id: stable across inserts and reorders,
/// unlike an index.
fn row_key(id: &RowId) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

/// A table's narrow form — see the module docs.
#[component]
pub fn TableCards(props: &TableCardsProps) -> Element {
    let hairline = props.presentation == CardPresentation::Hairline;
    let model = Rc::new(CardsModel {
        plan: CardPlan::new(&props.columns, props.max_cells, props.presentation),
        row_id: props.row_id.clone(),
        cell: props.cell.clone(),
        cell_empty: props.cell_empty.clone(),
        cell_caption: props.cell_caption.clone(),
        on_row_click: props.on_row_click.clone(),
        on_row_menu: props.on_row_menu.clone(),
        selection: props.selection,
        presentation: props.presentation,
        detail_row: signal(None),
        styles: props.styles.clone(),
    });
    // The list's data: each row's id, plus the generation of the data it
    // was read from. The generation moves on every change to the app's
    // rows (the `row_id` reads subscribe), so a card whose row changed
    // under the same id re-renders, while the key — the id alone — keeps
    // the mounted cards and the scroll position.
    let items: Signal<Vec<(RowId, u64)>> = signal(Vec::new());
    {
        let (count, row_id) = (props.row_count.clone(), props.row_id.clone());
        let generation = std::cell::Cell::new(0u64);
        let _ = effect(move || {
            let g = generation.get() + 1;
            generation.set(g);
            items.set((0..count.get()).map(|i| (row_id(i), g)).collect());
        });
    }
    let header = props.selection.filter(|_| props.selection_header).map(|selection| {
        let (count, row_id) = (props.row_count.clone(), props.row_id.clone());
        let visible = memo(move || (0..count.get()).map(|i| row_id(i)).collect::<Vec<RowId>>());
        let name = props.selection_label.clone();
        let label = Reactive::derive(move || {
            let count = selection.with(|s| s.len());
            Some(name.as_ref().map_or_else(|| default_selection_label(count), |f| f(count)))
        });
        select_all_line(selection, visible, label)
    });
    let header_style = TableStyles::cards_header().overridden(props.styles.cards_header.as_ref());
    let render_model = model.clone();
    let estimate = if hairline { CARD_ROW_ESTIMATE_PX } else { CARD_ESTIMATE_PX };
    let list = flat_list::<_, _, (), _>(
        items,
        |_, item: &(RowId, u64)| row_key(&item.0),
        FlatListItemSize::Measured(Rc::new(move |_, _| estimate)),
        move |i, _| {
            if hairline {
                ui! { TableCardRow(model = render_model.clone(), index = i) }
            } else {
                ui! { TableCardItem(model = render_model.clone(), index = i) }
            }
        },
    )
    // Hairline rows sit flush: the separator is their border.
    .gap(if hairline { 0.0 } else { CARD_GAP_PX })
    .end_reached_threshold(CARD_LOAD_AHEAD_PX)
    .with_style(TableStyles::cards_list().overridden(props.styles.cards_list.as_ref()));
    let list = match props.scroller.clone() {
        Some(scroller) => {
            let attached = scroller.clone();
            let token = std::rc::Rc::new(std::cell::Cell::new(None));
            let filled = token.clone();
            detach_on_cleanup_cell(scroller, token);
            list.on_handle(move |h| {
                filled.set(Some(attached.attach(Rc::new(move |id: &RowId| {
                    if let Some(i) = items.with(|rows| rows.iter().position(|(row, _)| row == id)) {
                        h.scroll_to_index(i);
                    }
                }))));
            })
        }
        None => list,
    };
    let list = match props.on_end_reached.clone() {
        Some(cb) => list.on_end_reached(move || cb()).into_element(),
        None => list.into_element(),
    };
    let empty_text = props.empty.clone().unwrap_or_default();
    let has_empty = props.empty.is_some();
    let count = props.row_count.clone();
    let detail_model = model.clone();
    let detail_row = model.detail_row;
    let list_style = TableStyles::cards_list().overridden(props.styles.cards_list.as_ref());
    let empty_override = props.styles.cards_empty.clone();
    let empty_style = move || TableStyles::cards_empty().overridden(empty_override.as_ref());
    ui! {
        view(style = list_style) {
            if let Some(header) = header {
                view(style = header_style) { header }
            }
            if has_empty && count.get() == 0 {
                view(style = empty_style()) {
                    Typography(content = empty_text.clone(), kind = typography_kind::Body, muted = true)
                }
            }
            list
            Modal(
                open = rx!(detail_row.get().is_some()),
                presentation = ModalPresentation::Sheet,
                on_dismiss = Some(Rc::new(move || detail_row.set(None)) as Rc<dyn Fn()>),
                content = move || detail_sheet(&detail_model),
            )
        }
    }
}

/// The detail sheet for the open row: its title, then every labelled
/// cell.
fn detail_sheet(model: &Rc<CardsModel>) -> Element {
    let Some(i) = model.detail_row.get() else {
        return runtime_core::view(Vec::new()).into_element();
    };
    let cell = model.cell.clone();
    let title = model.plan.title.clone();
    let title_style = TableStyles::card_title().overridden(model.styles.card_title.as_ref());
    let rows: Vec<_> = model
        .plan
        .detail
        .iter()
        .filter(|c| !model.is_empty(i, &c.id))
        .map(|c| {
            (c.id.clone(), model.caption(i, c), TableStyles::detail_row().overridden(model.styles.detail_row.as_ref()))
        })
        .collect();
    ui! {
        view(style = title_style) {
            if let Some(id) = &title {
                cell(i, id)
            }
        }
        for (id, label, row_style) in rows {
            view(style = row_style) {
                Typography(content = label.clone(), kind = typography_kind::Caption, muted = true)
                cell(i, &id)
            }
        }
    }
}

/// Props for one card.
#[runtime_core::props]
#[derive(IdealystSchema)]
struct TableCardItemProps {
    /// The list's shared model.
    #[prop(static)]
    pub model: Rc<CardsModel>,
    /// The row this card shows.
    #[prop(static)]
    pub index: usize,
}

impl Default for TableCardItemProps {
    fn default() -> Self {
        Self { model: Rc::new(CardsModel::default()), index: 0 }
    }
}

/// Props for one hairline row.
#[runtime_core::props]
#[derive(IdealystSchema)]
struct TableCardRowProps {
    /// The list's shared model.
    #[prop(static)]
    pub model: Rc<CardsModel>,
    /// The row this line shows.
    #[prop(static)]
    pub index: usize,
}

impl Default for TableCardRowProps {
    fn default() -> Self {
        Self { model: Rc::new(CardsModel::default()), index: 0 }
    }
}

/// One row's card.
///
/// ```ignore
/// TableCardItem(model = model.clone(), index = i)
/// ```
#[component]
fn TableCardItem(props: &TableCardItemProps) -> Element {
    let model = props.model.clone();
    let i = props.index;
    let plan = &model.plan;
    let title = plan.title.clone().map(|id| (model.cell)(i, &id));
    let opposite = plan.opposite.clone().map(|id| (model.cell)(i, &id));
    let trailing = plan.trailing.clone().map(|id| (model.cell)(i, &id));
    let actions = plan.actions.clone().map(|id| (model.cell)(i, &id));
    let (shown, more) = plan.row(|id| model.is_empty(i, id));
    let has_fields = !shown.is_empty();
    // Each section's slot style, built here because the branches below
    // each take their own.
    let styles = &model.styles;
    let opposite = opposite
        .map(|o| (o, TableStyles::card_opposite().overridden(styles.card_opposite.as_ref())));
    let fields: Vec<_> = shown
        .iter()
        .map(|c| {
            (
                model.caption(i, c),
                (model.cell)(i, &c.id),
                TableStyles::card_field()
                    .set(if c.wide { TableSpan::Wide } else { TableSpan::Half })
                    .overridden(styles.card_field.as_ref()),
                TableStyles::card_label().overridden(styles.card_label.as_ref()),
            )
        })
        .collect();
    let fields_style = TableStyles::card_fields().overridden(styles.card_fields.as_ref());
    let trailing = trailing.map(|t| (t, TableStyles::card_trailing().overridden(styles.card_trailing.as_ref())));
    let more_style = TableStyles::card_more().overridden(styles.card_more.as_ref());
    let title_line_style = TableStyles::card_title_line().overridden(styles.card_title_line.as_ref());
    let title_style = TableStyles::card_title().overridden(styles.card_title.as_ref());
    let select = model.selection.map(|s| select_row_box(s, (model.row_id)(i)));
    let detail_row = model.detail_row;
    let clickable = model.on_row_click.is_some();
    let card_style = TableStyles::card()
        .overridden(styles.card.as_ref())
        .into_style_application()
        .with("interactive", if clickable { "on" } else { "off" });
    let card = ui! {
        view(style = card_style) {
            view(style = title_line_style) {
                select
                view(style = title_style) { title }
                if let Some((o, opposite_style)) = opposite {
                    view(style = opposite_style) { o }
                }
                if let Some((t, trailing_style)) = trailing {
                    view(style = trailing_style) { t }
                }
                actions
            }
            if has_fields {
                view(style = fields_style) {
                    for (label, value, field_style, label_style) in fields {
                        view(style = field_style) {
                            text(style = label_style) { label }
                            value
                        }
                    }
                }
            }
            if more > 0 {
                view(style = more_style) {
                    Button(
                        label = format!("+{more} more"),
                        on_click = Rc::new(move || detail_row.set(Some(i))) as Rc<dyn Fn()>,
                        variant = crate::variant::Ghost,
                    )
                }
            }
        }
    };
    let on_tap = model.on_row_click.clone().map(|cb| Rc::new(move || cb(i)) as Rc<dyn Fn()>);
    with_row_press(card, on_tap, model.row_menu(i))
}

/// `content` inside the row's press surface, when it has a tap or a menu.
fn with_row_press(content: Element, on_tap: Option<Rc<dyn Fn()>>, on_menu: Option<RowMenu>) -> Element {
    match row_press(on_tap, on_menu, |_| {}) {
        Some(press) => runtime_core::view(vec![content]).on_touch(move |ev| press(ev)).into_element(),
        None => content,
    }
}

/// One record of a [`CardPresentation::Hairline`] list.
///
/// ```ignore
/// TableCardRow(model = model.clone(), index = i)
/// ```
#[component]
fn TableCardRow(props: &TableCardRowProps) -> Element {
    let model = props.model.clone();
    let i = props.index;
    let plan = &model.plan;
    let styles = &model.styles;
    let title = plan.title.clone().map(|id| (model.cell)(i, &id));
    let trailing = plan
        .trailing
        .clone()
        .filter(|id| !model.is_empty(i, id))
        .map(|id| ((model.cell)(i, &id), TableStyles::card_trailing().overridden(styles.card_trailing.as_ref())));
    let actions = plan.actions.clone().map(|id| (model.cell)(i, &id));
    let meta: Vec<_> = plan
        .meta
        .iter()
        .filter(|id| !model.is_empty(i, id))
        .enumerate()
        .map(|(n, id)| {
            let separator = (n > 0)
                .then(|| TableStyles::card_meta_separator().overridden(styles.card_meta_separator.as_ref()));
            (separator, (model.cell)(i, id))
        })
        .collect();
    let has_meta = !meta.is_empty();
    let select = model.selection.map(|s| select_row_box(s, (model.row_id)(i)));
    let body_style = TableStyles::card_row_body().overridden(styles.card_row_body.as_ref());
    let title_style = TableStyles::card_row_title().overridden(styles.card_row_title.as_ref());
    let meta_style = TableStyles::card_meta().overridden(styles.card_meta.as_ref());
    // A press belongs to the app's handler. Without one it opens the detail
    // sheet, the only way to the columns a one-line row doesn't draw.
    let detail_row = model.detail_row;
    let (_, more) = plan.row(|id| model.is_empty(i, id));
    let press: Option<Rc<dyn Fn()>> = match model.on_row_click.clone() {
        Some(cb) => Some(Rc::new(move || cb(i))),
        None if more > 0 => Some(Rc::new(move || detail_row.set(Some(i)))),
        None => None,
    };
    let row_style = TableStyles::card_row()
        .overridden(styles.card_row.as_ref())
        .into_style_application()
        .with("interactive", if press.is_some() { "on" } else { "off" });
    let row = ui! {
        view(style = row_style) {
            select
            view(style = body_style) {
                view(style = title_style) { title }
                if has_meta {
                    view(style = meta_style) {
                        for (separator, piece) in meta {
                            if let Some(separator_style) = separator {
                                text(style = separator_style) { "·" }
                            }
                            piece
                        }
                    }
                }
            }
            if let Some((chip, trailing_style)) = trailing {
                view(style = trailing_style) { chip }
            }
            actions
        }
    };
    with_row_press(row, press, model.row_menu(i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols() -> Vec<TableColumn> {
        vec![
            TableColumn::new("name").label("Name").card(CardSlot::Title),
            TableColumn::new("status").label("Status").card(CardSlot::Opposite),
            TableColumn::new("a").label("A"),
            TableColumn::new("b").label("B"),
            TableColumn::new("c").label("C"),
            TableColumn::new("d").label("D"),
            TableColumn::new("notes").label("Notes").card(CardSlot::Detail),
            TableColumn::new("secret").card(CardSlot::Hidden),
            TableColumn::actions("menu"),
        ]
    }

    /// The card's shape comes from the column specs: title, opposite and
    /// actions on the title line, up to the cap of labelled cells, and the
    /// rest — plus every detail column — behind "+N more". Nothing a column
    /// marks `Hidden` appears anywhere.
    #[test]
    fn card_plan_places_every_column_from_its_slot() {
        let plan = CardPlan::new(&cols(), CARD_CELL_CAP, CardPresentation::Cards);
        assert_eq!(plan.title.as_ref().map(ColumnId::as_str), Some("name"));
        assert_eq!(plan.opposite.as_ref().map(ColumnId::as_str), Some("status"));
        assert_eq!(plan.actions.as_ref().map(ColumnId::as_str), Some("menu"));
        let (shown, more) = plan.row(|_| false);
        let shown: Vec<_> = shown.iter().map(|c| (c.id.as_str(), c.label.as_str())).collect();
        assert_eq!(shown, vec![("a", "A"), ("b", "B"), ("c", "C")]);
        let detail: Vec<_> = plan.detail.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(detail, vec!["a", "b", "c", "d", "notes"], "the detail sheet holds the whole row");
        assert_eq!(more, 2, "\"+2 more\": d and notes");
    }

    /// want_1749af8b: a `FieldWide` column is a field that knows it takes the
    /// line, shown and capped like any other field.
    #[test]
    fn card_plan_marks_a_wide_field() {
        let plan = CardPlan::new(
            &[
                TableColumn::new("name").card(CardSlot::Title),
                TableColumn::new("hours").label("Hours"),
                TableColumn::new("scopes").label("Scopes").card(CardSlot::FieldWide),
            ],
            CARD_CELL_CAP,
            CardPresentation::Cards,
        );
        let (shown, _) = plan.row(|_| false);
        let wide: Vec<_> = shown.iter().map(|c| (c.id.as_str(), c.wide)).collect();
        assert_eq!(wide, vec![("hours", false), ("scopes", true)]);
    }

    /// want_dafe7ce3: an empty cell isn't drawn, doesn't use up a place on the
    /// card (the next column takes it) and isn't counted in "+N more".
    #[test]
    fn card_plan_drops_a_rows_empty_cells() {
        let plan = CardPlan::new(&cols(), CARD_CELL_CAP, CardPresentation::Cards);
        let (shown, more) = plan.row(|id| id.as_str() == "b");
        let shown: Vec<_> = shown.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(shown, vec!["a", "c", "d"], "d moves up into b's place");
        assert_eq!(more, 1, "only notes is left");
        let (shown, more) = plan.row(|_| true);
        assert!(shown.is_empty());
        assert_eq!(more, 0, "nothing to reach: no \"+N more\"");
    }

    /// want_5e506f37 / want_db31df82: a hairline row is the title, one meta line
    /// from the Opposite and Meta columns in column order, the trailing chip and
    /// the actions. Everything else goes to the sheet; nothing is a labelled cell
    /// on the row.
    #[test]
    fn card_plan_hairline_builds_a_meta_line_and_no_cells() {
        let plan = CardPlan::new(
            &[
                TableColumn::new("name").card(CardSlot::Title),
                TableColumn::new("role").card(CardSlot::Meta),
                TableColumn::new("status").card(CardSlot::Opposite),
                TableColumn::new("alert").card(CardSlot::Trailing),
                TableColumn::new("hours").label("Hours"),
                TableColumn::new("notes").card(CardSlot::Detail),
                TableColumn::actions("menu"),
            ],
            CARD_CELL_CAP,
            CardPresentation::Hairline,
        );
        let meta: Vec<_> = plan.meta.iter().map(ColumnId::as_str).collect();
        assert_eq!(meta, vec!["role", "status"]);
        assert_eq!(plan.trailing.as_ref().map(ColumnId::as_str), Some("alert"));
        assert_eq!(plan.opposite, None, "the opposite value moved to the meta line");
        let (shown, more) = plan.row(|_| false);
        assert!(shown.is_empty());
        assert_eq!(more, 2, "hours and notes are in the sheet");
        let detail: Vec<_> = plan.detail.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(detail, vec!["hours", "notes"], "a meta piece is on the row, not repeated");
    }

    /// The presentation is opt-in: a table that names none keeps its cards, and
    /// a `Meta` column on a framed card is a plain labelled cell.
    #[test]
    fn card_plan_framed_card_treats_meta_as_a_field() {
        assert_eq!(TableCardsProps::default().presentation, CardPresentation::Cards);
        let plan = CardPlan::new(
            &[TableColumn::new("name").card(CardSlot::Title), TableColumn::new("role").card(CardSlot::Meta)],
            CARD_CELL_CAP,
            CardPresentation::Cards,
        );
        assert!(plan.meta.is_empty());
        let (shown, _) = plan.row(|_| false);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].id.as_str(), "role");
    }

    #[test]
    fn selection_label_counts_the_selected_rows() {
        assert_eq!(default_selection_label(0), "Select all");
        assert_eq!(default_selection_label(3), "3 selected");
    }

    /// Without a `Title` column the first field column is the title, and
    /// it is not repeated as a labelled cell.
    #[test]
    fn card_plan_titles_the_first_column_by_default() {
        let plan = CardPlan::new(
            &[TableColumn::new("who"), TableColumn::new("hours").label("Hours")],
            CARD_CELL_CAP,
            CardPresentation::Cards,
        );
        assert_eq!(plan.title.as_ref().map(ColumnId::as_str), Some("who"));
        let (shown, more) = plan.row(|_| false);
        assert_eq!(shown.len(), 1);
        assert_eq!(more, 0);
    }

    /// Regression: the "+N more" button floated mid-card. Its holder is a row,
    /// so it sits at the card's leading edge.
    #[test]
    fn regression_more_control_sits_at_the_leading_edge() {
        idea_theme::testing::with_test_world(|| {
            let r = runtime_core::resolve_style(&TableStyles::card_more().into_style_application());
            assert_eq!(r.flex_direction, Some(runtime_core::FlexDirection::Row));
            assert_eq!(r.justify_content, Some(runtime_core::JustifyContent::FlexStart));
        });
    }

    #[test]
    fn row_keys_follow_the_id_not_the_index() {
        assert_eq!(row_key(&RowId::from("a")), row_key(&RowId::from("a")));
        assert_ne!(row_key(&RowId::from("a")), row_key(&RowId::from("b")));
    }
}
