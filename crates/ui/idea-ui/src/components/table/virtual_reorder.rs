//! Drag-to-reorder rows on a virtualized [`Table`](super::Table) —
//! `Table(on_reorder = …, virtualized = …)`.
//!
//! The plain table's reorder ([`super::reorder`]) measures every row and
//! hit-tests each one as a drop target. A virtualized table has most of its
//! rows unmounted and every row one height, so this one works from geometry
//! alone:
//!
//! - **The held row** follows the pointer: its cells translate by the
//!   pointer's travel plus however far the grid has scrolled since the
//!   grab. The landing slot is the row under its centre ([`landing_slot`]);
//!   the rows between the held row's slot and the landing slot slide aside
//!   by one row ([`uniform_slide`]).
//! - **Cells are reused** as the grid scrolls, so nothing here belongs to a
//!   row: every body cell reads its row through its memo and computes its
//!   own offset from the shared drag state. A cell pointed at a new row
//!   mid-drag jumps to that row's offset instead of springing from the
//!   last one's.
//! - **The held row stays mounted.** The drag recognizer lives on the
//!   row's grip cell, and a cell that leaves the grid's window is parked
//!   and reused elsewhere — the gesture would end with it (on the web,
//!   re-inserting the node drops its pointer capture). The grid keeps the
//!   row's cells mounted while it is held
//!   (`VirtualGridHandle::keep_row_mounted`).
//! - **Autoscroll.** Held within [`AUTOSCROLL_EDGE_PX`] of the top (under
//!   the header) or the bottom of the scrollport, the grid scrolls each
//!   frame, faster the nearer the edge ([`autoscroll_step`]), so a row can
//!   travel the whole table.
//! - **The drop.** `on_reorder(from, to)` — row indices, the same `i`
//!   [`VirtualRows`](super::VirtualRows) reads — and the row LANDS: it stays
//!   lifted (tinted, above its neighbours) while one table-wide spring
//!   carries it from where it was let go into slot `to`. Apply the move in
//!   the callback ([`move_in_place`](super::move_in_place)): the cells show
//!   the rows' new order from the same frame.
//! - **No background fade while a row is held or landing.** Cells are
//!   positional: on the drop the cells at the held row's old index show the
//!   next row at once, and a 150 ms tint fade-out there read as the
//!   highlight jumping to the row that took the index. With the fade off,
//!   the tint leaves those cells instantly and stays on the moved row; the
//!   fade comes back as it lands, so its tint fades out in its new slot.
//!
//! Geometry is read in window coordinates and from the grid's scroll
//! callback, never per move from layout: a cell's own coordinates move
//! with the scroll on native backends and not on the web, and a layout
//! read per pointer move forces a layout pass after every transform write.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use dnd::{Activation, DragPhase, DragRecognizer, ScrollAxis};
use runtime_core::animation::{AnimProp, AnimatedValue, SpringTo};
use runtime_core::primitives::virtual_grid::VirtualGridHandle;
use runtime_core::{
    effect, signal, Easing, Memo, RafLoop, Ref, Signal, StyleApplication, StyleRules, TouchHandler, Transition,
    ViewHandle,
};

/// How close to the scrollport's top (under the header) or bottom edge the
/// pointer must be for the grid to autoscroll, px: about one row at the
/// comfortable density, a target a finger can hold.
pub const AUTOSCROLL_EDGE_PX: f32 = 48.0;

/// The fastest autoscroll, px per frame (reached at the edge itself): about
/// a 40 px row every two frames, quick enough to cross a long table and
/// slow enough to stop on the row you want.
pub const AUTOSCROLL_MAX_STEP_PX: f32 = 18.0;

/// Stiffness / damping of the spring the displaced rows slide on and the
/// dropped row settles on — the plain table's.
const SLIDE_STIFFNESS: f32 = 460.0;
const SLIDE_DAMPING: f32 = 40.0;

/// The slot a held row lands in: the row under its centre. `from` is the
/// held row, `lift` how far it has moved from its slot (px, down is
/// positive), `h` the row height and `count` the rows. Clamped to the rows.
pub fn landing_slot(from: usize, lift: f32, h: f32, count: usize) -> usize {
    if count == 0 || h <= 0.0 {
        return from;
    }
    let centre = from as f32 * h + h / 2.0 + lift;
    ((centre / h).floor().max(0.0) as usize).min(count - 1)
}

/// How far row `i` slides while row `from` is held over slot `over`, with
/// every row `h` tall: the rows between the vacated slot and the landing
/// slot move one row toward the gap, the rest hold still.
/// [`slide_offset`](super::slide_offset) with uniform heights.
pub fn uniform_slide(i: usize, from: usize, over: usize, h: f32) -> f32 {
    if from < over && i > from && i <= over {
        -h
    } else if over < from && i >= over && i < from {
        h
    } else {
        0.0
    }
}

/// This frame's autoscroll, px (negative scrolls up): `pointer_y` against a
/// band of rows from `top` to `bottom` (all in the scrollport's coordinates).
/// Zero away from the edges, ramping to [`AUTOSCROLL_MAX_STEP_PX`] at them
/// and beyond.
pub fn autoscroll_step(pointer_y: f32, top: f32, bottom: f32) -> f32 {
    let edge = AUTOSCROLL_EDGE_PX.min((bottom - top).max(0.0) / 2.0);
    if edge <= 0.0 {
        return 0.0;
    }
    if pointer_y < top + edge {
        -AUTOSCROLL_MAX_STEP_PX * ((top + edge - pointer_y) / edge).min(1.0)
    } else if pointer_y > bottom - edge {
        AUTOSCROLL_MAX_STEP_PX * ((pointer_y - (bottom - edge)) / edge).min(1.0)
    } else {
        0.0
    }
}

/// The gesture in flight.
#[derive(Clone, Copy, Debug)]
struct Session {
    from: usize,
    /// The pointer's window y at the grab, and its travel since.
    window_y0: f32,
    pointer_dy: f32,
    /// The pointer's y in the scrollport at the grab.
    port_y0: f32,
    /// The grid's scroll at the grab, and now (from its scroll callback).
    scroll_y0: f32,
    scroll: (f32, f32),
}

impl Session {
    /// How far the held row is from its slot: the pointer's travel plus
    /// the scroll since the grab.
    fn lift(&self) -> f32 {
        self.pointer_dy + (self.scroll.1 - self.scroll_y0)
    }
}

/// A virtualized table's reorder state — one per table, shared by every
/// cell the grid mounts (a signal made in one cell's scope would die with
/// the cell).
pub(super) struct VirtualReorder {
    on_reorder: Rc<dyn Fn(usize, usize)>,
    row_height: f32,
    /// The row count, read untracked.
    count: Rc<dyn Fn() -> usize>,
    grid: Rc<RefCell<Option<VirtualGridHandle>>>,
    /// The held row.
    pub(super) from: Signal<Option<usize>>,
    /// The slot it would land in.
    over: Signal<Option<usize>>,
    /// How far the held row is from its slot, px.
    lift: Signal<f32>,
    /// The row a drop moved, while it lands, and how far it still is from
    /// its slot — `landing_spring`'s value, published for the cells.
    landing: Signal<Option<usize>>,
    landing_y: Signal<f32>,
    landing_spring: AnimatedValue<f32>,
    landing_sub: RefCell<Option<runtime_core::animation::Subscription<f32>>>,
    session: Cell<Option<Session>>,
    autoscroll: RefCell<Option<RafLoop>>,
    me: Weak<VirtualReorder>,
}

impl VirtualReorder {
    /// Reorder state for rows `row_height` tall, `count` of them, laid out
    /// on the grid `grid` fills at mount. Create it in the table's scope.
    pub(super) fn new(
        on_reorder: Rc<dyn Fn(usize, usize)>,
        row_height: f32,
        count: Rc<dyn Fn() -> usize>,
        grid: Rc<RefCell<Option<VirtualGridHandle>>>,
    ) -> Rc<Self> {
        let me = Rc::new_cyclic(|me| VirtualReorder {
            on_reorder,
            row_height,
            count,
            grid,
            from: signal(None),
            over: signal(None),
            lift: signal(0.0),
            landing: signal(None),
            landing_y: signal(0.0),
            landing_spring: AnimatedValue::new(0.0),
            landing_sub: RefCell::new(None),
            session: Cell::new(None),
            autoscroll: RefCell::new(None),
            me: me.clone(),
        });
        // The landing spring publishes each step, and ends the landing when
        // it comes to rest (a spring snaps to its target, velocity zero).
        let (landing, landing_y) = (me.landing, me.landing_y);
        let sub = me.landing_spring.subscribe(move |y, v| {
            landing_y.set(*y);
            if *y == 0.0 && *v == 0.0 {
                landing.set(None);
            }
        });
        *me.landing_sub.borrow_mut() = Some(sub);
        // A table unmounted mid-drag or mid-landing stops its frame loop and
        // spring with it: both would otherwise write its disposed signals.
        let weak = Rc::downgrade(&me);
        runtime_core::on_scope_drop(move || {
            if let Some(me) = weak.upgrade() {
                drop(me.autoscroll.borrow_mut().take());
                drop(me.landing_sub.borrow_mut().take());
                me.landing_spring.cancel();
            }
        });
        me
    }

    /// The drag recognizer a grip cell installs. `row` is the cell's row,
    /// read when the drag starts — the cell may have been pointed at
    /// another since it was built.
    ///
    /// The drag commits once the pointer has travelled the activation slop
    /// (or after a hold), but the row is measured from the PRESS: measured
    /// from the commit point it would trail the pointer by the slop.
    pub(super) fn grip_handler(&self, row: Memo<usize>) -> TouchHandler {
        let me = self.me.clone();
        // The press's window y and its y within the row.
        let pressed: Rc<Cell<(f32, f32)>> = Rc::new(Cell::new((0.0, 0.0)));
        let at = pressed.clone();
        let recognizer = DragRecognizer::new(Activation::scroll_aware(ScrollAxis::Vertical), move |phase| {
            let Some(me) = me.upgrade() else { return };
            match phase {
                DragPhase::Began(s) => {
                    let r = runtime_core::untrack(|| row.get());
                    let (window_y, grab_y) = at.get();
                    me.begin(r, window_y, grab_y);
                    me.moved(s.window_position.y);
                }
                DragPhase::Moved(s) => me.moved(s.window_position.y),
                DragPhase::Ended { .. } => me.finish(true),
                DragPhase::Cancelled => me.finish(false),
            }
        })
        .into_handler();
        Rc::new(move |ev: &runtime_core::TouchEvent| {
            if ev.phase == runtime_core::TouchPhase::Began {
                pressed.set((ev.window_position.y, ev.position.y));
            }
            recognizer(ev)
        })
    }

    /// The `drag` arm for a cell showing `row`: `lifted` for the held row
    /// and for a dropped row while it lands, `behind` for the rest meanwhile.
    pub(super) fn drag_arm(&self, row: usize) -> Option<&'static str> {
        match self.from.get().or(self.landing.get()) {
            None => None,
            Some(f) if f == row => Some("lifted"),
            Some(_) => Some("behind"),
        }
    }

    /// `app` for a body cell, with its background fade off while a row is
    /// held or landing — see the module docs, "No background fade".
    pub(super) fn quiet_background(&self, app: StyleApplication) -> StyleApplication {
        if self.from.get().is_none() && self.landing.get().is_none() {
            return app;
        }
        app.with_inline_merged(StyleRules {
            background_transition: Some(Transition::new(0, Easing::Linear)),
            ..Default::default()
        })
    }

    /// Whether `row` is the held row.
    pub(super) fn is_held(&self, row: usize) -> bool {
        self.from.get() == Some(row)
    }

    /// Drive the translate of a body cell showing `row` (bound through
    /// `cell`): the held row follows the pointer, the rows it passes slide
    /// aside, and after a drop the moved row lands in its slot.
    pub(super) fn bind_cell(&self, row: Memo<usize>, cell: Ref<ViewHandle>) {
        let offset = AnimatedValue::new(0.0_f32);
        offset.bind(cell, AnimProp::TranslateY);
        let (from, over, lift, h) = (self.from, self.over, self.lift, self.row_height);
        let (landing, landing_y) = (self.landing, self.landing_y);
        let shown = Cell::new(None::<usize>);
        let spring = |to: f32| SpringTo::new(to).stiffness(SLIDE_STIFFNESS).damping(SLIDE_DAMPING);
        let _ = effect(move || {
            let r = row.get();
            // Pointed at another row: take that row's offset at once — a
            // spring from the last row's offset would show the cell moving
            // between two rows it never sat in.
            let repointed = shown.replace(Some(r)).is_some_and(|was| was != r);
            match from.get() {
                Some(f) if f == r => offset.set(lift.get()),
                Some(f) => {
                    let to = over.get().map_or(0.0, |o| uniform_slide(r, f, o, h));
                    if repointed {
                        offset.set(to);
                    } else {
                        offset.animate(spring(to));
                    }
                }
                // The landing row follows the table's landing spring; every
                // other row is in place at once — the rows that slid aside
                // now hold the data that moved into their slots.
                None if landing.get() == Some(r) => offset.set(landing_y.get()),
                None => offset.set(0.0),
            }
        });
    }

    /// The grid scrolled to `(x, y)` — its own scroll callback, so this sees
    /// a wheel or a native fling as well as the autoscroll.
    pub(super) fn scrolled(&self, x: f32, y: f32) {
        let Some(mut s) = self.session.get() else { return };
        s.scroll = (x, y);
        self.session.set(Some(s));
        self.update(s);
    }

    fn begin(&self, from: usize, window_y: f32, grab_y: f32) {
        let h = self.row_height;
        let scroll = self.grid.borrow().as_ref().map_or((0.0, 0.0), |g| g.scroll_offset());
        // Grid row `from + 1` (the header is row 0) sits at `(from + 1) * h`
        // in the content; the pointer is `grab_y` into it.
        let port_y0 = (from + 1) as f32 * h - scroll.1 + grab_y.clamp(0.0, h);
        let s = Session { from, window_y0: window_y, pointer_dy: 0.0, port_y0, scroll_y0: scroll.1, scroll };
        self.session.set(Some(s));
        if let Some(g) = self.grid.borrow().as_ref() {
            g.keep_row_mounted(Some(from + 1));
        }
        // A row still landing from the last drop is put down.
        self.landing_spring.set(0.0);
        self.landing.set(None);
        self.lift.set(0.0);
        self.over.set(Some(from));
        self.from.set(Some(from));
        let me = self.me.clone();
        *self.autoscroll.borrow_mut() = Some(runtime_core::raf_loop(move || {
            if let Some(me) = me.upgrade() {
                me.autoscroll_frame();
            }
        }));
    }

    fn moved(&self, window_y: f32) {
        let Some(mut s) = self.session.get() else { return };
        s.pointer_dy = window_y - s.window_y0;
        self.session.set(Some(s));
        self.update(s);
    }

    /// The held row's landing slot for session `s`.
    ///
    /// Computed from the session, never read back from `lift` / `over`: a
    /// signal write stages until the turn's flush, and a move and a release
    /// can arrive in the same turn — a read-back there sees the last
    /// frame's slot.
    fn slot(&self, s: Session) -> usize {
        landing_slot(s.from, s.lift(), self.row_height, (self.count)())
    }

    /// Publish the held row's lift and landing slot for session `s`. The
    /// signals' own equality check drops a write of the unchanged slot.
    fn update(&self, s: Session) {
        self.lift.set(s.lift());
        self.over.set(Some(self.slot(s)));
    }

    /// One frame of autoscroll: scroll toward the edge the pointer is near.
    fn autoscroll_frame(&self) {
        let Some(mut s) = self.session.get() else { return };
        let Some(grid) = self.grid.borrow().clone() else { return };
        let Some(port) = grid.scrollport() else { return };
        let h = self.row_height;
        let pointer_y = s.port_y0 + s.pointer_dy;
        let step = autoscroll_step(pointer_y, h, port.height);
        if step == 0.0 {
            return;
        }
        let content = ((self.count)() + 1) as f32 * h;
        let max = (content - port.height).max(0.0);
        let y = (s.scroll.1 + step).clamp(0.0, max);
        if y == s.scroll.1 {
            return;
        }
        // Ahead of the grid's scroll callback (the web's arrives a frame
        // later), so the next frame steps on from here, not from the old
        // offset; the callback, when it lands, carries the true offset.
        s.scroll.1 = y;
        self.session.set(Some(s));
        self.update(s);
        grid.scroll_to(s.scroll.0, y);
    }

    /// End the drag: report the move if it `landed` (a cancelled drag goes
    /// back to its own slot) and settle the moved row from where it was
    /// let go.
    fn finish(&self, landed: bool) {
        drop(self.autoscroll.borrow_mut().take());
        let Some(s) = self.session.take() else { return };
        if let Some(g) = self.grid.borrow().as_ref() {
            g.keep_row_mounted(None);
        }
        let to = if landed { self.slot(s) } else { s.from };
        // The row now in slot `to` is the moved one; it was let go
        // `(from - to)` rows plus its lift away from that slot.
        let offset = (s.from as f32 - to as f32) * self.row_height + s.lift();
        // Even a row let go exactly over its slot lands: its tint must leave
        // the cells at the old index with the fade off. A spring started at
        // its target finishes on its first frame, which ends the landing.
        self.landing_spring.set(offset);
        self.landing.set(Some(to));
        self.landing_spring.animate(SpringTo::new(0.0).stiffness(SLIDE_STIFFNESS).damping(SLIDE_DAMPING));
        self.over.set(None);
        self.from.set(None);
        if to != s.from {
            (self.on_reorder)(s.from, to);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_landing_slot_is_the_row_under_the_held_rows_centre() {
        // Row 5 of 40 px rows: its centre crosses into row 6 at +20 px.
        assert_eq!(landing_slot(5, 0.0, 40.0, 100), 5);
        assert_eq!(landing_slot(5, 19.0, 40.0, 100), 5);
        assert_eq!(landing_slot(5, 20.0, 40.0, 100), 6);
        assert_eq!(landing_slot(5, -21.0, 40.0, 100), 4);
        assert_eq!(landing_slot(5, 4000.0, 40.0, 100), 99, "clamped to the last row");
        assert_eq!(landing_slot(5, -4000.0, 40.0, 100), 0, "and the first");
    }

    /// The slide matches the move `move_in_place` makes, as the plain
    /// table's does.
    #[test]
    fn uniform_slide_opens_the_gap_move_in_place_fills() {
        let h = 40.0;
        // 2 held over 5: rows 3..=5 move up one.
        let slid: Vec<f32> = (0..7).map(|i| uniform_slide(i, 2, 5, h)).collect();
        assert_eq!(slid, vec![0.0, 0.0, 0.0, -h, -h, -h, 0.0]);
        // 5 held over 2: rows 2..=4 move down one.
        let slid: Vec<f32> = (0..7).map(|i| uniform_slide(i, 5, 2, h)).collect();
        assert_eq!(slid, vec![0.0, 0.0, h, h, h, 0.0, 0.0]);
        assert!((0..7).all(|i| uniform_slide(i, 3, 3, h) == 0.0), "over its own slot nothing moves");
        // Agrees with the measured-heights rule.
        let heights = vec![h; 7];
        for (from, over) in [(2, 5), (5, 2), (0, 6), (6, 0)] {
            for i in 0..7 {
                assert_eq!(uniform_slide(i, from, over, h), super::super::slide_offset(i, from, over, &heights));
            }
        }
    }

    #[test]
    fn autoscroll_ramps_up_toward_each_edge() {
        let (top, bottom) = (40.0, 440.0);
        assert_eq!(autoscroll_step(240.0, top, bottom), 0.0, "mid-table: still");
        assert_eq!(autoscroll_step(top + AUTOSCROLL_EDGE_PX, top, bottom), 0.0, "the band's inner edge");
        let near = autoscroll_step(top + AUTOSCROLL_EDGE_PX / 2.0, top, bottom);
        assert!(near < 0.0 && near > -AUTOSCROLL_MAX_STEP_PX, "half way into the top band: {near}");
        assert_eq!(autoscroll_step(top - 100.0, top, bottom), -AUTOSCROLL_MAX_STEP_PX, "over the header: full speed up");
        assert_eq!(autoscroll_step(bottom + 10.0, top, bottom), AUTOSCROLL_MAX_STEP_PX, "past the bottom: full speed down");
        // A scrollport too short for two bands splits it.
        assert_eq!(autoscroll_step(60.0, 40.0, 80.0), 0.0);
        assert!(autoscroll_step(45.0, 40.0, 80.0) < 0.0);
    }
}
