//! [`TableScroller`]: scroll a table, in either form, to a row.
//!
//! The wide `Table` and the narrow `TableCards` reach a row differently, and
//! an app holds one of these across the breakpoint switch that swaps them, so
//! it doesn't care which is on screen.
//!
//! - `TableCards` is a virtualizer, so a row that isn't mounted yet is still
//!   reachable by index (`VirtualizerHandle::scroll_to_index`).
//! - `Table(scroll_y = true)` moves its own scroller. The row's place in the
//!   scrolled content is its window `y` less the first body row's, both read
//!   from the rows' proxy surfaces (`table::bind_row`): a difference, so it
//!   holds whatever the scroller's current offset is, and it puts the row
//!   directly under a pinned head.
//!
//! A `Table` without `scroll_y` has no vertical scroller of its own: the page
//! scrolls it, and the runtime offers no app-facing way to move an ancestor
//! scroller to a node. There the call does nothing.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::primitives::portal::ViewportRect;

use super::RowId;

type ScrollFn = Rc<dyn Fn(&RowId)>;

/// Scrolls the `Table` or `TableCards` it is handed to, to a row:
///
/// ```ignore
/// let scroller = TableScroller::new();
/// ui! {
///     if wide { Table(scroll_y = true, scroller = Some(scroller.clone())) { … } }
///     else { TableCards(columns = columns, cell = cell, scroller = Some(scroller.clone())) }
/// }
/// // later
/// scroller.scroll_to_row(&RowId::from("row-42"));
/// ```
///
/// The row lands at the top of the scroller, as far as its offset range allows.
/// Nothing happens for an unknown id or while no table is mounted. Rows
/// are matched by `TableRow(id = …)` on the wide form and `row_id` on the cards.
#[derive(Clone, Default, runtime_core::Remote)]
pub struct TableScroller(Rc<TableScrollerInner>);

#[derive(Default)]
struct TableScrollerInner {
    target: RefCell<Option<(u64, ScrollFn)>>,
    next_token: Cell<u64>,
}

impl TableScroller {
    /// A scroller no table is using yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scroll the mounted table to the row with this id.
    pub fn scroll_to_row(&self, id: &RowId) {
        // Cloned out: the host call must not run with the slot borrowed.
        let target = self.0.target.borrow().as_ref().map(|(_, f)| f.clone());
        if let Some(scroll) = target {
            scroll(id);
        }
    }

    /// Point this scroller at a newly mounted table. The token keeps a table
    /// leaving later from clearing the one that replaced it (a breakpoint
    /// switch mounts one form before the other's cleanup runs).
    pub(super) fn attach(&self, scroll: ScrollFn) -> u64 {
        let token = self.0.next_token.get();
        self.0.next_token.set(token + 1);
        *self.0.target.borrow_mut() = Some((token, scroll));
        token
    }

    fn detach(&self, token: u64) {
        let mut target = self.0.target.borrow_mut();
        if target.as_ref().is_some_and(|(t, _)| *t == token) {
            *target = None;
        }
    }
}

/// Detach when the calling component's scope ends, so a scroller outliving the
/// table doesn't reach into handles it unmounted.
pub(super) fn detach_on_cleanup(scroller: TableScroller, token: u64) {
    runtime_core::on_scope_drop(move || scroller.detach(token));
}

/// [`detach_on_cleanup`] for a table that attaches later, at mount.
pub(super) fn detach_on_cleanup_cell(scroller: TableScroller, token: Rc<Cell<Option<u64>>>) {
    runtime_core::on_scope_drop(move || {
        if let Some(token) = token.get() {
            scroller.detach(token);
        }
    });
}

/// The scroll offset that brings `target` to the top of a table's rows: its
/// window `y` less the first body row's. `rows` is every body row in order;
/// `None` when the row is unknown or isn't laid out.
pub(super) fn row_offset(rows: &[(Option<&RowId>, Option<ViewportRect>)], target: &RowId) -> Option<f32> {
    let first = rows.first()?.1?;
    let (_, frame) = rows.iter().find(|(id, _)| *id == Some(target))?;
    Some(frame.as_ref()?.y - first.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(y: f32) -> Option<ViewportRect> {
        Some(ViewportRect { x: 0.0, y, width: 100.0, height: 40.0 })
    }

    /// The frames are window coordinates, which already include whatever the
    /// scroller's current offset is: the difference doesn't move with it.
    #[test]
    fn row_offset_is_the_distance_from_the_first_body_row() {
        let (a, b) = (RowId::from("a"), RowId::from("b"));
        let at_rest = [(Some(&a), rect(100.0)), (Some(&b), rect(180.0))];
        assert_eq!(row_offset(&at_rest, &b), Some(80.0));
        assert_eq!(row_offset(&at_rest, &a), Some(0.0));
        let scrolled = [(Some(&a), rect(40.0)), (Some(&b), rect(120.0))];
        assert_eq!(row_offset(&scrolled, &b), Some(80.0), "already scrolled by 60");
    }

    /// A band has no id, but it is still what the scroller starts with.
    #[test]
    fn row_offset_counts_an_unidentified_leading_row() {
        let b = RowId::from("b");
        assert_eq!(row_offset(&[(None, rect(0.0)), (Some(&b), rect(30.0))], &b), Some(30.0));
    }

    #[test]
    fn row_offset_needs_a_known_laid_out_row() {
        let a = RowId::from("a");
        assert_eq!(row_offset(&[], &a), None);
        assert_eq!(row_offset(&[(None, rect(0.0))], &a), None, "unknown id");
        assert_eq!(row_offset(&[(None, rect(0.0)), (Some(&a), None)], &a), None, "not laid out");
        assert_eq!(row_offset(&[(None, None), (Some(&a), rect(5.0))], &a), None, "no anchor");
    }

    /// One form replacing the other: the old table's late cleanup must not
    /// clear the new one's target.
    #[test]
    fn a_stale_detach_does_not_clear_the_new_target() {
        let scroller = TableScroller::new();
        let hits = Rc::new(Cell::new(0));
        let (a, b) = (hits.clone(), hits.clone());
        let old = scroller.attach(Rc::new(move |_| a.set(a.get() + 1)));
        let new = scroller.attach(Rc::new(move |_| b.set(b.get() + 10)));
        scroller.detach(old);
        scroller.scroll_to_row(&RowId::from("x"));
        assert_eq!(hits.get(), 10);
        scroller.detach(new);
        scroller.scroll_to_row(&RowId::from("x"));
        assert_eq!(hits.get(), 10, "no table mounted: nothing happens");
    }
}
