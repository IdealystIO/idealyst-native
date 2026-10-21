//! [`TableCards`] — a table's narrow (phone) form: a virtualized list of
//! cards, one per row, laid out from the SAME [`TableColumn`] specs the
//! wide `Table` takes.
//!
//! Each column's [`CardSlot`] says where it goes: the `Title` column is
//! the card's title (the first column when none claims it), the
//! `Opposite` column sits across from it on the title line, the `Actions`
//! column is the title line's trailing control, and `Field` columns become
//! labelled cells under the title — up to `max_cells` of them. The rest,
//! and every `Detail` column, move to the row's detail sheet behind a
//! "+N more" line, so a forty-column table still reaches all forty on a
//! phone. Labels are the columns' `label`s.
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

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use runtime_core::primitives::flat_list::{flat_list, FlatListItemSize};
use runtime_core::{
    component, effect, rx, signal, tap, ui, Element, IdealystSchema, IntoElement, Reactive, Signal,
    TapRecognizer,
};

use super::select::select_row_box;
use super::{CardSlot, ColumnId, RowId, TableColumn};
use crate::components::button::Button;
use crate::components::modal::{Modal, ModalPresentation};
use crate::components::typography::Typography;
use crate::stylesheets::{
    TableCard, TableCardField, TableCardFields, TableCardLabel, TableCardMore, TableCardOpposite,
    TableCardTitle, TableCardTitleLine, TableCardsEmpty, TableCardsList, TableDetailRow,
};
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

/// Where each column goes on a card.
struct CardPlan {
    title: Option<ColumnId>,
    opposite: Option<ColumnId>,
    actions: Option<ColumnId>,
    /// Labelled cells on the card, in column order.
    shown: Vec<(ColumnId, String)>,
    /// Labelled cells in the detail sheet: every field and detail column.
    detail: Vec<(ColumnId, String)>,
}

impl CardPlan {
    fn new(columns: &[TableColumn], max_cells: usize) -> Self {
        let title = columns
            .iter()
            .find(|c| c.card == CardSlot::Title)
            .or_else(|| columns.iter().find(|c| c.card == CardSlot::Field))
            .map(|c| c.id.clone());
        let pick = |slot: CardSlot| columns.iter().find(|c| c.card == slot).map(|c| c.id.clone());
        let labelled = |c: &TableColumn| (c.id.clone(), c.label.clone().unwrap_or_else(|| c.id.to_string()));
        let fields: Vec<(ColumnId, String)> = columns
            .iter()
            .filter(|c| c.card == CardSlot::Field && Some(&c.id) != title.as_ref())
            .map(labelled)
            .collect();
        let detail = columns
            .iter()
            .filter(|c| {
                matches!(c.card, CardSlot::Field | CardSlot::Detail) && Some(&c.id) != title.as_ref()
            })
            .map(labelled)
            .collect();
        CardPlan {
            title,
            opposite: pick(CardSlot::Opposite),
            actions: pick(CardSlot::Actions),
            shown: fields.into_iter().take(max_cells).collect(),
            detail,
        }
    }

    /// How many cells the detail sheet holds that the card does not show.
    fn more(&self) -> usize {
        self.detail.len().saturating_sub(self.shown.len())
    }
}

/// Props for [`TableCards`].
#[runtime_core::props]
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
    /// Labelled cells a card shows before "+N more". Default
    /// [`CARD_CELL_CAP`].
    #[prop(static)]
    pub max_cells: usize,
    /// Called with the row index when a card is pressed — the row's own
    /// controls (its menu, its checkbox) still take their own presses.
    pub on_row_click: Option<Rc<dyn Fn(usize)>>,
    /// Fetch the next page: called as the reader nears the last card.
    pub on_end_reached: Option<Rc<dyn Fn()>>,
    /// The screen's selection, as for `Table(selection = …)`: a box on
    /// each card's title line.
    #[prop(static)]
    pub selection: Option<Signal<BTreeSet<RowId>>>,
    /// What an empty list says.
    #[prop(static)]
    pub empty: Option<String>,
}

impl Default for TableCardsProps {
    fn default() -> Self {
        Self {
            columns: Vec::new(),
            row_count: Reactive::Static(0),
            row_id: Rc::new(|i: usize| RowId::from(i)),
            cell: Rc::new(|_, _| runtime_core::view(Vec::new()).into_element()),
            max_cells: CARD_CELL_CAP,
            on_row_click: None,
            on_end_reached: None,
            selection: None,
            empty: None,
        }
    }
}

/// Everything one card reads, shared by every card of the list.
struct CardsModel {
    plan: CardPlan,
    row_id: Rc<dyn Fn(usize) -> RowId>,
    cell: Rc<dyn Fn(usize, &ColumnId) -> Element>,
    on_row_click: Option<Rc<dyn Fn(usize)>>,
    selection: Option<Signal<BTreeSet<RowId>>>,
    /// The row whose detail sheet is open.
    detail_row: Signal<Option<usize>>,
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
    let model = Rc::new(CardsModel {
        plan: CardPlan::new(&props.columns, props.max_cells),
        row_id: props.row_id.clone(),
        cell: props.cell.clone(),
        on_row_click: props.on_row_click.clone(),
        selection: props.selection,
        detail_row: signal(None),
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
    let render_model = model.clone();
    let list = flat_list::<_, _, (), _>(
        items,
        |_, item: &(RowId, u64)| row_key(&item.0),
        FlatListItemSize::Measured(Rc::new(|_, _| CARD_ESTIMATE_PX)),
        move |i, _| ui! { TableCardItem(model = render_model.clone(), index = i) },
    )
    .gap(CARD_GAP_PX)
    .end_reached_threshold(CARD_LOAD_AHEAD_PX)
    .with_style(TableCardsList());
    let list = match props.on_end_reached.clone() {
        Some(cb) => list.on_end_reached(move || cb()).into_element(),
        None => list.into_element(),
    };
    let empty_text = props.empty.clone().unwrap_or_default();
    let has_empty = props.empty.is_some();
    let count = props.row_count.clone();
    let detail_model = model.clone();
    let detail_row = model.detail_row;
    ui! {
        view(style = TableCardsList()) {
            if has_empty && count.get() == 0 {
                view(style = TableCardsEmpty()) {
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
    let rows = model.plan.detail.clone();
    ui! {
        view(style = TableCardTitle()) {
            if let Some(id) = &title {
                cell(i, id)
            }
        }
        for (id, label) in rows {
            view(style = TableDetailRow()) {
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
        Self {
            model: Rc::new(CardsModel {
                plan: CardPlan::new(&[], CARD_CELL_CAP),
                row_id: Rc::new(|i: usize| RowId::from(i)),
                cell: Rc::new(|_, _| runtime_core::view(Vec::new()).into_element()),
                on_row_click: None,
                selection: None,
                detail_row: signal(None),
            }),
            index: 0,
        }
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
    let actions = plan.actions.clone().map(|id| (model.cell)(i, &id));
    let has_fields = !plan.shown.is_empty();
    let fields: Vec<(String, Element)> =
        plan.shown.iter().map(|(id, label)| (label.clone(), (model.cell)(i, id))).collect();
    let more = plan.more();
    let select = model.selection.map(|s| select_row_box(s, (model.row_id)(i)));
    let detail_row = model.detail_row;
    let clickable = model.on_row_click.is_some();
    let card_style = TableCard().into_style_application().with("interactive", if clickable { "on" } else { "off" });
    let card = ui! {
        view(style = card_style) {
            view(style = TableCardTitleLine()) {
                select
                view(style = TableCardTitle()) { title }
                if let Some(o) = opposite {
                    view(style = TableCardOpposite()) { o }
                }
                actions
            }
            if has_fields {
                view(style = TableCardFields()) {
                    for (label, value) in fields {
                        view(style = TableCardField()) {
                            text(style = TableCardLabel()) { label }
                            value
                        }
                    }
                }
            }
            if more > 0 {
                view(style = TableCardMore()) {
                    Button(
                        label = format!("+{more} more"),
                        on_click = Rc::new(move || detail_row.set(Some(i))) as Rc<dyn Fn()>,
                        variant = crate::variant::Ghost,
                    )
                }
            }
        }
    };
    match model.on_row_click.clone() {
        Some(cb) => runtime_core::view(vec![card])
            .on_touch({
                let press = tap(TapRecognizer::new(), move || cb(i));
                move |ev| press(ev)
            })
            .into_element(),
        None => card,
    }
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
        let plan = CardPlan::new(&cols(), CARD_CELL_CAP);
        assert_eq!(plan.title.as_ref().map(ColumnId::as_str), Some("name"));
        assert_eq!(plan.opposite.as_ref().map(ColumnId::as_str), Some("status"));
        assert_eq!(plan.actions.as_ref().map(ColumnId::as_str), Some("menu"));
        let shown: Vec<_> = plan.shown.iter().map(|(id, l)| (id.as_str(), l.as_str())).collect();
        assert_eq!(shown, vec![("a", "A"), ("b", "B"), ("c", "C")]);
        let detail: Vec<_> = plan.detail.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(detail, vec!["a", "b", "c", "d", "notes"], "the detail sheet holds the whole row");
        assert_eq!(plan.more(), 2, "\"+2 more\": d and notes");
    }

    /// Without a `Title` column the first field column is the title, and
    /// it is not repeated as a labelled cell.
    #[test]
    fn card_plan_titles_the_first_column_by_default() {
        let plan = CardPlan::new(
            &[TableColumn::new("who"), TableColumn::new("hours").label("Hours")],
            CARD_CELL_CAP,
        );
        assert_eq!(plan.title.as_ref().map(ColumnId::as_str), Some("who"));
        assert_eq!(plan.shown.len(), 1);
        assert_eq!(plan.more(), 0);
    }

    /// Regression: the "+N more" button floated mid-card — a non-block
    /// Button centres itself on a column's cross axis. Its holder is a row,
    /// so it sits at the card's leading edge.
    #[test]
    fn regression_more_control_sits_at_the_leading_edge() {
        idea_theme::testing::with_test_world(|| {
            let r = runtime_core::resolve_style(&TableCardMore().into_style_application());
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
