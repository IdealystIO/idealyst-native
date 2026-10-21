//! The selection column `Table(selection = …)` adds: a leading checkbox
//! column whose header box selects every visible row (tri-state: none,
//! some, all) and whose row boxes tick one row each.
//!
//! The selection is a set of [`RowId`]s the SCREEN owns — a
//! `Signal<BTreeSet<RowId>>` — so it outlives a table rebuilt from a new
//! data snapshot, and a bulk action reads it directly. A row takes part
//! when it has an `id` and is `selectable`; select-all touches exactly
//! those rows of the ones the table was built with (the page on screen),
//! leaving any other ids in the set alone.

use std::collections::BTreeSet;
use std::rc::Rc;

use runtime_core::{effect, memo, signal, Element, FillRule, IconData, Reactive, Signal};

use super::RowId;
use crate::components::checkbox::{Checkbox, CheckboxProps};
use crate::components::ControlSize;

/// The selection column's width, px: a small checkbox and the compact
/// icon padding either side.
pub const SELECT_COLUMN_WIDTH_PX: f32 = 44.0;

/// The header box's mark while SOME visible rows are selected — a dash,
/// the conventional "mixed" state.
const MIXED_MARK: IconData = IconData {
    view_box: (24, 24),
    paths: &["M6 12h12"],
    fill_rule: FillRule::NonZero,
    filled: false,
};

/// How much of the visible, selectable set is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectAllState {
    /// None of them (or there are none).
    None,
    /// Some but not all.
    Some,
    /// Every one of them.
    All,
}

/// The header box's state for `selected` over the rows on screen.
pub fn select_all_state(selected: &BTreeSet<RowId>, visible: &[RowId]) -> SelectAllState {
    let n = visible.iter().filter(|id| selected.contains(*id)).count();
    match n {
        0 => SelectAllState::None,
        n if n == visible.len() => SelectAllState::All,
        _ => SelectAllState::Some,
    }
}

/// What pressing the header box does to `selected`: with every visible
/// row selected it clears them; otherwise it selects them all. Ids that
/// are not on screen are kept either way.
pub fn toggle_select_all(selected: &BTreeSet<RowId>, visible: &[RowId]) -> BTreeSet<RowId> {
    let mut next = selected.clone();
    if select_all_state(selected, visible) == SelectAllState::All {
        for id in visible {
            next.remove(id);
        }
    } else {
        next.extend(visible.iter().cloned());
    }
    next
}

/// The header row's tri-state box.
pub(super) fn select_all_box(selection: Signal<BTreeSet<RowId>>, visible: Rc<Vec<RowId>>) -> Element {
    let state = {
        let visible = visible.clone();
        memo(move || selection.with(|s| select_all_state(s, &visible)))
    };
    // The checkbox is controlled by a `Signal<bool>`; it follows the
    // derived state (checked for some or all, the dash telling them apart).
    let checked = signal(false);
    let _ = effect(move || checked.set(state.get() != SelectAllState::None));
    let nothing_to_select = visible.is_empty();
    Checkbox(&CheckboxProps {
        value: checked,
        on_change: Rc::new(move |_| {
            let next = selection.with(|s| toggle_select_all(s, &visible));
            selection.set(next);
        }),
        icon: Reactive::Dynamic(Rc::new(move || {
            (state.get() == SelectAllState::Some).then_some(MIXED_MARK)
        })),
        size: ControlSize::Sm.into(),
        disabled: Reactive::Static(nothing_to_select),
        ..Default::default()
    })
}

/// One row's box.
pub(super) fn select_row_box(selection: Signal<BTreeSet<RowId>>, id: RowId) -> Element {
    let checked = signal(false);
    {
        let id = id.clone();
        let _ = effect(move || checked.set(selection.with(|s| s.contains(&id))));
    }
    Checkbox(&CheckboxProps {
        value: checked,
        on_change: Rc::new(move |on: bool| {
            let mut next = selection.get();
            if on {
                next.insert(id.clone());
            } else {
                next.remove(&id);
            }
            selection.set(next);
        }),
        size: ControlSize::Sm.into(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<RowId> {
        v.iter().map(|s| RowId::from(*s)).collect()
    }

    fn set(v: &[&str]) -> BTreeSet<RowId> {
        ids(v).into_iter().collect()
    }

    #[test]
    fn select_all_state_is_tri_state_over_the_visible_rows() {
        let visible = ids(&["a", "b", "c"]);
        assert_eq!(select_all_state(&set(&[]), &visible), SelectAllState::None);
        assert_eq!(select_all_state(&set(&["b"]), &visible), SelectAllState::Some);
        assert_eq!(select_all_state(&set(&["a", "b", "c"]), &visible), SelectAllState::All);
        // Ids from another page do not count toward this one.
        assert_eq!(select_all_state(&set(&["x", "y"]), &visible), SelectAllState::None);
        assert_eq!(select_all_state(&set(&[]), &[]), SelectAllState::None, "nothing to select");
    }

    #[test]
    fn toggle_select_all_selects_then_clears_only_the_visible_rows() {
        let visible = ids(&["a", "b"]);
        // Some → all, keeping an off-page id.
        assert_eq!(toggle_select_all(&set(&["a", "x"]), &visible), set(&["a", "b", "x"]));
        // All → none of the visible, still keeping the off-page id.
        assert_eq!(toggle_select_all(&set(&["a", "b", "x"]), &visible), set(&["x"]));
        assert_eq!(toggle_select_all(&set(&[]), &visible), set(&["a", "b"]));
    }
}
