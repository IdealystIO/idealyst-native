//! Drag-to-reorder rows — what `Table(on_reorder = …)` adds: a leading
//! grip column, the lifted row riding above its neighbours, the others
//! sliding aside to open the landing gap, and `on_reorder(from, to)` on
//! drop. Built on the `dnd` SDK.
//!
//! - **Ordinals.** `from` / `to` count the DRAGGABLE body rows only, in
//!   order — a row with `draggable = false` (a group title, a locked row)
//!   is skipped, so the numbers index the list the app reorders. The app
//!   owns the move ([`move_in_place`]) and the persist.
//! - **Groups.** A row only lands among rows of its own
//!   `TableRow(reorder_group = …)`; across groups the gap stays put.
//! - **The grab.** Only the grip cell takes the drag recognizer, so
//!   everything else in the row (menus, checkboxes, a clickable row's tap)
//!   stays plainly clickable. Activation is scroll-aware: a mouse grabs at
//!   once; on touch, vertical motion scrolls and a hold picks the row up.
//! - **Frozen columns stay frozen.** The lift is the cell sheets' `drag`
//!   axis — `relative` and a z-index on the lifted row — which sorts
//!   before `pinned`, so a frozen cell keeps `Sticky` (and its pin)
//!   throughout the drag instead of being overridden to `relative`.
//!   Drop targets are the rows' proxy surfaces (`table::bind_row`), whose
//!   frames stay at their layout position while the cells translate, so
//!   hit-testing never oscillates with the slide.

use std::cell::RefCell;
use std::rc::Rc;

use dnd::{Activation, DragContext, Draggable, Droppable, ScrollAxis};
use runtime_core::animation::{AnimProp, AnimatedValue, SpringTo};
use runtime_core::{effect, signal, Element, FillRule, IconData, Ref, Signal, TouchHandler, ViewHandle};

use super::RowId;

/// The grip column's width, px: the grip glyph and the compact icon
/// padding either side.
pub const GRIP_COLUMN_WIDTH_PX: f32 = 36.0;

/// The grip glyph's square size, px.
pub(super) const GRIP_ICON_PX: f32 = 16.0;

/// Stiffness / damping of the spring the displaced rows slide on.
const SLIDE_STIFFNESS: f32 = 460.0;
const SLIDE_DAMPING: f32 = 40.0;

/// Six dots in two columns — the conventional drag grip.
pub(super) const GRIP_MARK: IconData = IconData {
    view_box: (24, 24),
    paths: &[
        "M10 5a1 1 0 1 1-2 0a1 1 0 1 1 2 0Z",
        "M10 12a1 1 0 1 1-2 0a1 1 0 1 1 2 0Z",
        "M10 19a1 1 0 1 1-2 0a1 1 0 1 1 2 0Z",
        "M16 5a1 1 0 1 1-2 0a1 1 0 1 1 2 0Z",
        "M16 12a1 1 0 1 1-2 0a1 1 0 1 1 2 0Z",
        "M16 19a1 1 0 1 1-2 0a1 1 0 1 1 2 0Z",
    ],
    fill_rule: FillRule::NonZero,
    filled: false,
};

/// The list mutation an `on_reorder` starts with: move `from` to `to`,
/// remove-then-insert — exactly where the drag's gap showed the row
/// landing. Returns `false` (and leaves the list alone) for a no-op or an
/// out-of-range index, so a caller can skip the persist.
pub fn move_in_place<T>(list: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from == to || from >= list.len() || to >= list.len() {
        return false;
    }
    let item = list.remove(from);
    list.insert(to, item);
    true
}

/// How far draggable row `i` slides while row `from` is held over slot
/// `over`: the rows between the vacated slot and the landing slot shift by
/// the dragged row's height; everything else holds still (the dragged row
/// itself follows the pointer). Matches [`move_in_place`] exactly.
pub fn slide_offset(i: usize, from: usize, over: usize, heights: &[f32]) -> f32 {
    let h = heights.get(from).copied().unwrap_or(0.0);
    if from < over && i > from && i <= over {
        -h
    } else if over < from && i >= over && i < from {
        h
    } else {
        0.0
    }
}

/// One draggable row's half of the gesture.
struct DragRow {
    handler: TouchHandler,
    offset_y: AnimatedValue<f32>,
    row_ref: Ref<ViewHandle>,
}

/// A table's reorder wiring: built before the grip cells are added (each
/// grip takes its row's recognizer), then applied to each body row once
/// every cell is in place ([`Reorder::wire_row`]).
pub(super) struct Reorder {
    ctx: DragContext<usize>,
    /// The held row and the slot it would land in, as ordinals.
    drag_from: Signal<Option<usize>>,
    over_slot: Signal<Option<usize>>,
    /// Row heights, sampled at drag start: the gap is the dragged row's.
    heights: Rc<RefCell<Vec<f32>>>,
    /// Ordinal per body row (`None`: not draggable).
    ordinals: Vec<Option<usize>>,
    groups: Rc<Vec<Option<RowId>>>,
    rows: Vec<DragRow>,
    on_reorder: Rc<dyn Fn(usize, usize)>,
}

impl Reorder {
    /// Wiring for body rows described by `(draggable, group)`, in order.
    pub(super) fn new(body: &[(bool, Option<RowId>)], on_reorder: Rc<dyn Fn(usize, usize)>) -> Self {
        let ctx: DragContext<usize> = DragContext::new();
        let drag_from: Signal<Option<usize>> = signal(None);
        let over_slot: Signal<Option<usize>> = signal(None);
        let mut ordinals = Vec::with_capacity(body.len());
        let mut groups = Vec::new();
        for (draggable, group) in body {
            ordinals.push(draggable.then(|| {
                groups.push(group.clone());
                groups.len() - 1
            }));
        }
        let count = groups.len();
        let heights = Rc::new(RefCell::new(vec![0.0_f32; count]));
        let row_refs: Rc<Vec<Ref<ViewHandle>>> = Rc::new((0..count).map(|_| Ref::new()).collect());
        let rows = (0..count)
            .map(|k| {
                let start_heights = heights.clone();
                let start_refs = row_refs.clone();
                let drag = Draggable::new(&ctx, move || k)
                    .activation(Activation::scroll_aware(ScrollAxis::Vertical))
                    .on_start(move || {
                        {
                            let mut hs = start_heights.borrow_mut();
                            for (j, r) in start_refs.iter().enumerate() {
                                hs[j] = r
                                    .with(|h| h.absolute_frame())
                                    .flatten()
                                    .map(|f| f.height)
                                    .unwrap_or(0.0);
                            }
                        }
                        drag_from.set(Some(k));
                        // No gap until the pointer crosses another row.
                        over_slot.set(Some(k));
                    })
                    .on_release(move |_| {
                        drag_from.set(None);
                        over_slot.set(None);
                    });
                DragRow {
                    handler: drag.handler(),
                    offset_y: drag.offset().1,
                    row_ref: row_refs[k],
                }
            })
            .collect();
        Reorder {
            ctx,
            drag_from,
            over_slot,
            heights,
            ordinals,
            groups: Rc::new(groups),
            rows,
            on_reorder,
        }
    }

    /// The drag recognizer body row `body_index`'s grip installs, if the
    /// row is draggable.
    pub(super) fn grip_handler(&self, body_index: usize) -> Option<TouchHandler> {
        let k = self.ordinals.get(body_index).copied().flatten()?;
        Some(self.rows[k].handler.clone())
    }

    /// Wire body row `body_index` — every cell in place, grip included:
    /// the drop target on its proxy surface, the in-flight cell styles
    /// (lifted / behind), and the finger-follow and slide offsets.
    pub(super) fn wire_row(&self, body_index: usize, row: &Element) {
        let ordinal = self.ordinals.get(body_index).copied().flatten();
        let drag_from = self.drag_from;
        let over_slot = self.over_slot;
        let mut offsets: Vec<AnimatedValue<f32>> = Vec::new();
        if let Some(k) = ordinal {
            let row_ref = self.rows[k].row_ref;
            table::bind_row(row, move |h| row_ref.fill(h));
            let groups = self.groups.clone();
            let on_reorder = self.on_reorder.clone();
            let drop = Droppable::new(&self.ctx)
                .accepts(move |from: &usize| *from != k && groups.get(*from) == groups.get(k))
                .on_enter(move |_| over_slot.set(Some(k)))
                .on_drop(move |from| on_reorder(from, k));
            drop.bind(row_ref);

            // The gap: displaced rows spring aside by the dragged row's
            // height. A spring already at its target never ticks, so this
            // idle 0 never fights the held row's finger-follow offset.
            let slide = AnimatedValue::new(0.0);
            let heights = self.heights.clone();
            let e_slide = slide.clone();
            let _ = effect(move || {
                let target = match (drag_from.get(), over_slot.get()) {
                    (Some(f), Some(o)) => slide_offset(k, f, o, &heights.borrow()),
                    _ => 0.0,
                };
                e_slide.animate(SpringTo::new(target).stiffness(SLIDE_STIFFNESS).damping(SLIDE_DAMPING));
            });
            offsets.push(self.rows[k].offset_y.clone());
            offsets.push(slide);
        }
        table::visit_row_cells(row, |cell| {
            table::map_cell_style(
                cell,
                Rc::new(move |app| match drag_from.get() {
                    None => app,
                    Some(f) if Some(f) == ordinal => app.with("drag", "lifted").with("row_hovered", "on"),
                    Some(_) => app.with("drag", "behind"),
                }),
            );
            if !offsets.is_empty() {
                let cell_ref: Ref<ViewHandle> = Ref::new();
                table::bind_cell(cell, move |h| cell_ref.fill(h));
                for value in &offsets {
                    value.bind(cell_ref, AnimProp::TranslateY);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::table::{ColumnPin, TableCell, TableCellProps, TableRow, TableRowProps};
    use idea_theme::testing::with_test_world;
    use runtime_core::Reactive;

    fn row(pinned: bool) -> Element {
        TableRow(TableRowProps {
            children: vec![
                TableCell(TableCellProps {
                    text: Reactive::Static(Some("frozen".into())),
                    pinned: pinned.then_some(ColumnPin::Left),
                    ..Default::default()
                }),
                TableCell(TableCellProps { text: Reactive::Static(Some("x".into())), ..Default::default() }),
            ],
            ..Default::default()
        })
    }

    fn cell_rules(row: &Element) -> Vec<Rc<runtime_core::StyleRules>> {
        let mut out = Vec::new();
        table::visit_row_cells(row, |c| {
            out.push(runtime_core::resolve_style(&table::cell_application(c).expect("a sheet")));
        });
        out
    }

    /// CrewForge (item 3): lifting a row overrode its cells' `position`
    /// to `relative` so a z-index applied — which unfroze the pinned
    /// column for the length of the drag. The lift is now the `drag`
    /// axis, which sorts before `pinned`: the lifted row's frozen cell
    /// stays `Sticky` and rides above the row's scrolling cells, its
    /// other cells are raised `relative`, and every other row goes
    /// opaque so the sliding rows never show through each other.
    #[test]
    fn regression_lifted_row_keeps_its_frozen_column_frozen() {
        with_test_world(|| {
            let reorder = Reorder::new(&[(true, None), (true, None)], Rc::new(|_, _| {}));
            let (held, other) = (row(true), row(true));
            reorder.wire_row(0, &held);
            reorder.wire_row(1, &other);
            let rest = cell_rules(&held);
            assert_eq!(rest[0].position, Some(runtime_core::Position::Sticky));
            assert_eq!(rest[1].position, None, "nothing moves until a drag starts");

            reorder.drag_from.set(Some(0));
            idea_theme::testing::commit();
            let lifted = cell_rules(&held);
            assert_eq!(lifted[0].position, Some(runtime_core::Position::Sticky), "still frozen");
            assert_eq!(lifted[0].z_index, Some(3));
            assert_eq!(lifted[1].position, Some(runtime_core::Position::Relative));
            assert_eq!(lifted[1].z_index, Some(2));
            let behind = cell_rules(&other);
            assert_eq!(behind[0].position, Some(runtime_core::Position::Sticky));
            assert_eq!(
                behind[1].background.as_ref().and_then(|b| b.name().map(str::to_string)).as_deref(),
                Some("color-surface"),
                "a row behind the drag is opaque"
            );
        });
    }

    /// Only draggable rows take part, ordinals skip the rest, and a row
    /// accepts drops from its own group only.
    #[test]
    fn ordinals_skip_rows_that_do_not_drag() {
        with_test_world(|| {
            let reorder = Reorder::new(
                &[(true, Some("a".into())), (false, None), (true, Some("b".into()))],
                Rc::new(|_, _| {}),
            );
            assert!(reorder.grip_handler(0).is_some());
            assert!(reorder.grip_handler(1).is_none(), "a locked row has no grip");
            assert!(reorder.grip_handler(2).is_some());
            assert_eq!(reorder.ordinals, vec![Some(0), None, Some(1)]);
            assert_ne!(reorder.groups.first(), reorder.groups.get(1), "different groups");
        });
    }

    #[test]
    fn move_in_place_moves_down_and_up() {
        let mut list = vec!["a", "b", "c", "d"];
        assert!(move_in_place(&mut list, 0, 2));
        assert_eq!(list, vec!["b", "c", "a", "d"]);
        assert!(move_in_place(&mut list, 3, 0));
        assert_eq!(list, vec!["d", "b", "c", "a"]);
    }

    #[test]
    fn move_in_place_rejects_noop_and_out_of_range() {
        let mut list = vec!["a", "b"];
        assert!(!move_in_place(&mut list, 1, 1));
        assert!(!move_in_place(&mut list, 2, 0));
        assert!(!move_in_place(&mut list, 0, 9));
        assert_eq!(list, vec!["a", "b"]);
    }

    /// The gap previews exactly where `move_in_place` lands the row.
    #[test]
    fn slide_offset_opens_the_landing_gap() {
        let heights = [10.0, 20.0, 30.0, 40.0];
        assert_eq!(slide_offset(0, 0, 2, &heights), 0.0);
        assert_eq!(slide_offset(1, 0, 2, &heights), -10.0);
        assert_eq!(slide_offset(2, 0, 2, &heights), -10.0);
        assert_eq!(slide_offset(3, 0, 2, &heights), 0.0);
        assert_eq!(slide_offset(1, 3, 1, &heights), 40.0);
        assert_eq!(slide_offset(2, 3, 1, &heights), 40.0);
        assert_eq!(slide_offset(0, 3, 1, &heights), 0.0);
        for i in 0..4 {
            assert_eq!(slide_offset(i, 1, 1, &heights), 0.0, "over its own slot nothing moves");
        }
    }
}
