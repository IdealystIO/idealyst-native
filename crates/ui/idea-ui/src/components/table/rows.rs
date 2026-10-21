//! Row identity and the row-level settings a [`super::TableRow`] hands to
//! its [`super::Table`].
//!
//! A table's body runs after its rows are built, and the rows run before
//! the table's settings are known — so a row cannot add the table's
//! built-in cells (selection, drag grip) itself. It records what the table
//! needs to build them ([`RowMeta`]) on its payload (`table::set_row_data`)
//! instead, including its tone and its click wiring, so a cell the table
//! adds tints and answers the pointer exactly like the row's own cells.

use std::fmt;
use std::rc::Rc;

use runtime_core::{Element, IdealystSchema, Reactive, Signal, TouchHandler, VariantEnum};

use super::{RowTone, TableDensity};

/// A row's identity — what a selection holds and a reorder reports.
/// Cheap to clone; built from strings and integers.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, IdealystSchema)]
pub struct RowId(Rc<str>);

impl RowId {
    /// A row id from any string.
    pub fn new(id: impl Into<Rc<str>>) -> Self {
        RowId(id.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RowId({:?})", &*self.0)
    }
}

impl fmt::Display for RowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for RowId {
    fn from(s: &str) -> Self {
        RowId(Rc::from(s))
    }
}

impl From<String> for RowId {
    fn from(s: String) -> Self {
        RowId(Rc::from(s))
    }
}

impl From<&String> for RowId {
    fn from(s: &String) -> Self {
        RowId(Rc::from(s.as_str()))
    }
}

macro_rules! row_id_from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for RowId {
            fn from(n: $t) -> Self {
                RowId(Rc::from(n.to_string()))
            }
        }
    )*};
}
row_id_from_int!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize);

/// A clickable row's wiring, shared by every cell of the row.
#[derive(Clone)]
pub(super) struct RowInteraction {
    pub hovered: Signal<bool>,
    pub pressed: Signal<bool>,
    pub on_click: Rc<dyn Fn()>,
}

/// What a row tells its table — see the module docs.
#[derive(Clone)]
pub(super) struct RowMeta {
    pub id: Option<RowId>,
    pub selectable: bool,
    pub draggable: bool,
    /// The reorder scope (`TableRow(reorder_group = …)`, or its
    /// `TableGroup`'s id).
    pub group: Option<RowId>,
    pub tone: Reactive<RowTone>,
    pub interaction: Option<RowInteraction>,
    /// The row's own density, over the table's (`TableRow(density = …)`).
    pub density: Option<TableDensity>,
    /// Set on a `TableGroup`'s band row.
    pub band: Option<BandMeta>,
}

/// What a group's band row tells its table.
#[derive(Clone)]
pub(super) struct BandMeta {
    /// Whether the group moves as a whole in `Table(on_group_reorder = …)`.
    pub draggable: bool,
    /// The band content's left pin — the width of the frozen columns the
    /// table adds before it — and, when the band's cell is the row's first,
    /// the table's leading edge inset. The table sets both while it
    /// builds; the band's style reads them when it resolves.
    pub lead: Rc<std::cell::RefCell<BandLead>>,
}

/// See [`BandMeta::lead`].
#[derive(Clone)]
pub(super) struct BandLead {
    pub pin: f32,
    pub inset: Reactive<super::TableInset>,
}

impl Default for BandLead {
    fn default() -> Self {
        BandLead { pin: 0.0, inset: Reactive::Static(super::TableInset::None) }
    }
}

impl RowMeta {
    /// A copy with `f` applied, re-attached to `row`.
    pub(super) fn update(row: &Element, f: impl FnOnce(&mut RowMeta)) {
        if let Some(meta) = row_meta(row) {
            let mut meta = (*meta).clone();
            f(&mut meta);
            table::set_row_data(row, Rc::new(meta));
        }
    }
}

/// The [`RowMeta`] a built row carries, if it is a `TableRow`.
pub(super) fn row_meta(row: &Element) -> Option<Rc<RowMeta>> {
    table::row_data(row).and_then(|d| d.downcast::<RowMeta>().ok())
}

/// Tint `cell` with a row's tone — a static selection for a static tone
/// (the sheet stays preminted), a live one for a reactive tone.
pub(super) fn apply_row_tone(cell: &Element, tone: &Reactive<RowTone>) {
    match tone {
        Reactive::Static(RowTone::None) => {}
        Reactive::Static(tone) => {
            let arm = tone.as_variant_str();
            table::map_cell_application(cell, Rc::new(move |app| app.with("tone", arm)));
        }
        Reactive::Dynamic(f) => {
            let f = f.clone();
            table::map_cell_style(cell, Rc::new(move |app| app.with("tone", f().as_variant_str())));
        }
    }
}

/// Give a cell the table added the row's own treatment: its tone, and —
/// on a clickable row — the shared hover tint, the pointer cursor and the
/// row's tap. `touch` replaces the row's tap on this cell (the drag grip
/// takes the drag recognizer instead) while keeping the hover.
pub(super) fn decorate_added_cell(cell: Element, meta: &RowMeta, touch: Option<TouchHandler>) -> Element {
    apply_row_tone(&cell, &meta.tone);
    match (&meta.interaction, touch) {
        (Some(i), None) => {
            super::make_row_cell_interactive(cell, i.hovered, i.pressed, i.on_click.clone())
        }
        (Some(i), Some(touch)) => {
            let (hovered, pressed) = (i.hovered, i.pressed);
            table::map_cell_style(
                &cell,
                Rc::new(move |app| {
                    app.with("row_hovered", if hovered.get() || pressed.get() { "on" } else { "off" })
                }),
            );
            table::set_cell_interaction(&cell, touch, Rc::new(move |entering| hovered.set(entering)));
            cell
        }
        (None, Some(touch)) => {
            table::set_cell_touch(&cell, touch);
            cell
        }
        (None, None) => cell,
    }
}
