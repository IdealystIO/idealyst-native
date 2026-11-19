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
//! - **Outside the table** (`Table(row_drag = …)`, [`RowDrag`]). The drag
//!   is also a `dnd` drag of the row's payload, and the row is drawn by
//!   the context's drag layer rather than in the grid: a copy of the row
//!   follows the pointer over the whole page, above everything and
//!   unclipped, while the row's own cells are hidden and the others slide
//!   to show where it would land. With the pointer over one of the
//!   context's drop targets, or outside the table, no landing is shown and
//!   the table does not scroll; released on a target, the target takes the
//!   row and `on_reorder` is not called; released outside the table on
//!   nothing, the row goes back.
//!
//! Geometry is read in window coordinates and from the grid's scroll
//! callback, never per move from layout: a cell's own coordinates move
//! with the scroll on native backends and not on the web, and a layout
//! read per pointer move forces a layout pass after every transform write.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use dnd::{Activation, DragPhase, DragPreview, DragRecognizer, ScrollAxis};
use runtime_core::animation::{AnimProp, AnimatedValue, SpringTo};
use runtime_core::primitives::virtual_grid::VirtualGridHandle;
use runtime_core::{
    effect, signal, Easing, Element, Memo, RafLoop, Ref, Signal, StyleApplication, StyleRules, Tokenized, TouchHandler,
    TouchPoint, Transition, ViewHandle,
};

use super::row_drag::{ActiveRowDrag, RowDrag};

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
    /// The row's top-left in the window at the grab, and the pointer's
    /// sideways travel since: where a [`RowDrag`]'s preview is drawn.
    origin: (f32, f32),
    pointer_dx: f32,
    /// The pointer's x in the scrollport at the grab.
    port_x0: f32,
    /// A [`RowDrag`]'s pointer is over one of its drop targets or outside
    /// the table: the table shows no landing and does not scroll.
    away: bool,
}

impl Session {
    /// How far the held row is from its slot: the pointer's travel plus
    /// the scroll since the grab.
    fn lift(&self) -> f32 {
        self.pointer_dy + (self.scroll.1 - self.scroll_y0)
    }

    /// The preview's top-left in the window: the row's, moved with the
    /// pointer on both axes.
    fn preview_at(&self) -> (f32, f32) {
        (self.origin.0 + self.pointer_dx, self.origin.1 + self.pointer_dy)
    }
}

/// What a [`RowDrag`] release does to the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowRelease {
    /// A drop target outside the table took it: nothing lands here.
    Taken,
    /// It lands in the table, in the slot under it.
    Lands,
    /// Let go outside the table on nothing: it goes back to its slot.
    Returns,
}

/// What releasing a [`RowDrag`] does: a target that `took` the row wins;
/// otherwise the row lands where it is if the pointer is `in_table`, and
/// goes back if it was carried out and dropped on nothing — a near miss of
/// a target must not reorder the list by the table's nearest edge.
pub fn row_release(took: bool, in_table: bool) -> RowRelease {
    match (took, in_table) {
        (true, _) => RowRelease::Taken,
        (false, true) => RowRelease::Lands,
        (false, false) => RowRelease::Returns,
    }
}

/// Whether a pointer at `(x, y)` in a scrollport `width` by `height` is
/// within it: where a [`RowDrag`] release still means "reorder".
pub fn within_scrollport(x: f32, y: f32, width: f32, height: f32) -> bool {
    (0.0..=width).contains(&x) && (0.0..=height).contains(&y)
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
    /// The outside context the drag joins (`Table(row_drag = …)`), the
    /// drag in flight there, and what builds the row's preview for its
    /// drag layer (set by the table once its columns are known).
    row_drag: Option<RowDrag>,
    active: RefCell<Option<ActiveRowDrag>>,
    preview: RefCell<Option<Rc<dyn Fn(usize) -> Element>>>,
    /// The held row is drawn by the drag layer, so its cells are hidden.
    ghosted: Signal<bool>,
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
        row_drag: Option<RowDrag>,
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
            row_drag,
            active: RefCell::new(None),
            preview: RefCell::new(None),
            ghosted: signal(false),
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
                // Cancels a drag still in flight in the outside context.
                drop(me.active.borrow_mut().take());
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
        // The press in the window, and within the grip cell: the grip is
        // the row's first cell, so the cell's corner is the row's.
        let origin = TouchPoint::new(0.0, 0.0);
        let pressed: Rc<Cell<(TouchPoint, TouchPoint)>> = Rc::new(Cell::new((origin, origin)));
        let at = pressed.clone();
        let recognizer = DragRecognizer::new(Activation::scroll_aware(ScrollAxis::Vertical), move |phase| {
            let Some(me) = me.upgrade() else { return };
            match phase {
                DragPhase::Began(s) => {
                    let r = runtime_core::untrack(|| row.get());
                    let (window, grab) = at.get();
                    me.begin(r, window, grab);
                    me.moved(s.window_position);
                }
                DragPhase::Moved(s) => me.moved(s.window_position),
                DragPhase::Ended { window_position, .. } => me.released(window_position),
                DragPhase::Cancelled => me.finish(false),
            }
        })
        .into_handler();
        Rc::new(move |ev: &runtime_core::TouchEvent| {
            if ev.phase == runtime_core::TouchPhase::Began {
                pressed.set((ev.window_position, ev.position));
            }
            recognizer(ev)
        })
    }

    /// What builds row `r`'s preview for a [`RowDrag`]'s drag layer.
    pub(super) fn set_preview(&self, build: Rc<dyn Fn(usize) -> Element>) {
        *self.preview.borrow_mut() = Some(build);
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

    /// `app` for a body cell showing `row`, hidden while the row is held
    /// and drawn by a [`RowDrag`]'s drag layer instead: the page shows one
    /// copy of the row, and the slot it left reads as a gap.
    pub(super) fn hidden_while_ghosted(&self, row: usize, app: StyleApplication) -> StyleApplication {
        if !(self.ghosted.get() && self.from.get() == Some(row)) {
            return app;
        }
        app.with_inline_merged(StyleRules { opacity: Some(Tokenized::Literal(0.0)), ..Default::default() })
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

    fn begin(&self, from: usize, window: TouchPoint, grab: TouchPoint) {
        let h = self.row_height;
        let scroll = self.grid.borrow().as_ref().map_or((0.0, 0.0), |g| g.scroll_offset());
        // Grid row `from + 1` (the header is row 0) sits at `(from + 1) * h`
        // in the content; the pointer is `grab.y` into it.
        let port_y0 = (from + 1) as f32 * h - scroll.1 + grab.y.clamp(0.0, h);
        let s = Session {
            from,
            window_y0: window.y,
            pointer_dy: 0.0,
            port_y0,
            scroll_y0: scroll.1,
            scroll,
            origin: (window.x - grab.x, window.y - grab.y),
            pointer_dx: 0.0,
            // The grip is frozen at the scrollport's left edge.
            port_x0: grab.x,
            away: false,
        };
        self.session.set(Some(s));
        // Join the outside context: the drag layer draws the row from
        // here on, so the grid's copy is hidden.
        let preview = self.preview.borrow().clone();
        if let (Some(drag), Some(build)) = (self.row_drag.as_ref(), preview) {
            let (x, y) = s.origin;
            let shown = DragPreview::new(move || build(from), x, y);
            *self.active.borrow_mut() = Some(drag.start(from, window, shown));
            self.ghosted.set(true);
        }
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

    fn moved(&self, window: TouchPoint) {
        let Some(mut s) = self.session.get() else { return };
        s.pointer_dx = window.x - (s.origin.0 + s.port_x0);
        s.pointer_dy = window.y - s.window_y0;
        if let Some(active) = self.active.borrow().as_ref() {
            let on_target = active.moved(window, s.preview_at());
            s.away = on_target || !self.pointer_in_table(s);
        }
        self.session.set(Some(s));
        self.update(s);
    }

    /// Whether session `s`'s pointer is inside the table's scrollport.
    fn pointer_in_table(&self, s: Session) -> bool {
        let Some(port) = self.grid.borrow().as_ref().and_then(|g| g.scrollport()) else { return true };
        within_scrollport(s.port_x0 + s.pointer_dx, s.port_y0 + s.pointer_dy, port.width, port.height)
    }

    /// The pointer was let go at window `at`. In an outside context
    /// ([`RowDrag`]) a drop target under it takes the row; otherwise the
    /// row lands in the table, or goes back if it was let go outside it.
    fn released(&self, at: TouchPoint) {
        let active = self.active.borrow_mut().take();
        let Some(active) = active else { return self.finish(true) };
        let in_table = self.session.get().is_some_and(|s| self.pointer_in_table(s));
        match row_release(active.finish(at), in_table) {
            RowRelease::Taken => self.finish_taken(),
            RowRelease::Lands => self.finish(true),
            RowRelease::Returns => self.finish(false),
        }
    }

    /// A drop target outside the table took the row: nothing lands here.
    /// No landing spring either — the app may be removing the row, and a
    /// row sliding home would be the wrong story for one that left.
    fn finish_taken(&self) {
        drop(self.autoscroll.borrow_mut().take());
        if self.session.take().is_none() {
            return;
        }
        if let Some(g) = self.grid.borrow().as_ref() {
            g.keep_row_mounted(None);
        }
        self.ghosted.set(false);
        self.landing_spring.set(0.0);
        self.landing.set(None);
        self.over.set(None);
        self.from.set(None);
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
        // Away from the table the rows rest: no slot is on offer.
        self.over.set(Some(if s.away { s.from } else { self.slot(s) }));
    }

    /// One frame of autoscroll: scroll toward the edge the pointer is near.
    fn autoscroll_frame(&self) {
        let Some(mut s) = self.session.get() else { return };
        // Over a drop target or outside the table the drag is not about
        // this list, and the edge the pointer left by must not scroll it.
        if s.away {
            return;
        }
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
        // A cancelled gesture cancels the drag in the outside context too.
        drop(self.active.borrow_mut().take());
        self.ghosted.set(false);
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

    /// A row drag offered to an outside context: the context sees the
    /// drag and the row's payload, the grid's copy of the row is hidden
    /// while the layer carries it, and a release inside the table is
    /// still a reorder. No grid here, so the pointer always reads as
    /// inside the table.
    #[test]
    fn a_row_drag_joins_the_outside_context_and_still_reorders_in_the_table() {
        idea_theme::testing::with_test_world(|| {
            let ctx: dnd::DragContext<usize> = dnd::DragContext::new();
            let moved = Rc::new(Cell::new(None));
            let out = moved.clone();
            let reorder = VirtualReorder::new(
                Rc::new(move |from, to| out.set(Some((from, to)))),
                40.0,
                Rc::new(|| 10),
                Rc::new(RefCell::new(None)),
                Some(RowDrag::new(&ctx, |row| row * 100)),
            );
            reorder.set_preview(Rc::new(|_| runtime_core::view(Vec::new()).into()));
            let at = |x: f32, y: f32| TouchPoint::new(x, y);

            reorder.begin(3, at(20.0, 170.0), at(10.0, 10.0));
            idea_theme::testing::commit();
            assert!(ctx.dragging().get(), "the context has a drag in flight");
            assert_eq!(ctx.payload(), Some(300), "carrying the row's payload");
            assert!(reorder.ghosted.get() && reorder.from.get() == Some(3));

            // Two rows down, and let go there.
            reorder.moved(at(60.0, 250.0));
            reorder.released(at(60.0, 250.0));
            idea_theme::testing::commit();
            assert_eq!(moved.get(), Some((3, 5)), "a release in the table reorders");
            assert!(!ctx.dragging().get() && ctx.payload().is_none(), "and ends the context's drag");
            assert!(!reorder.ghosted.get() && reorder.from.get().is_none());
        });
    }

    /// A cancelled gesture cancels the outside drag with it.
    #[test]
    fn a_cancelled_row_drag_leaves_no_drag_in_the_outside_context() {
        idea_theme::testing::with_test_world(|| {
            let ctx: dnd::DragContext<usize> = dnd::DragContext::new();
            let reorder = VirtualReorder::new(
                Rc::new(|_, _| panic!("a cancelled drag reorders nothing")),
                40.0,
                Rc::new(|| 10),
                Rc::new(RefCell::new(None)),
                Some(RowDrag::new(&ctx, |row| row)),
            );
            reorder.set_preview(Rc::new(|_| runtime_core::view(Vec::new()).into()));
            reorder.begin(3, TouchPoint::new(20.0, 170.0), TouchPoint::new(10.0, 10.0));
            reorder.moved(TouchPoint::new(20.0, 250.0));
            reorder.finish(false);
            idea_theme::testing::commit();
            assert!(!ctx.dragging().get() && !reorder.ghosted.get());
        });
    }

    /// A target that took the row wins; a release in the table lands; a
    /// row carried out of the table and dropped on nothing goes back.
    #[test]
    fn a_row_released_outside_the_table_on_nothing_goes_back() {
        assert_eq!(row_release(true, true), RowRelease::Taken);
        assert_eq!(row_release(true, false), RowRelease::Taken);
        assert_eq!(row_release(false, true), RowRelease::Lands);
        assert_eq!(row_release(false, false), RowRelease::Returns);
        assert!(within_scrollport(10.0, 10.0, 300.0, 200.0));
        assert!(!within_scrollport(-1.0, 10.0, 300.0, 200.0), "left of the table");
        assert!(!within_scrollport(10.0, 201.0, 300.0, 200.0), "below it");
    }

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
