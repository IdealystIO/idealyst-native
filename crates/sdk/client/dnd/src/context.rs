//! [`DragContext`] — the shared registry every [`Draggable`](crate::Draggable)
//! and [`Droppable`](crate::Droppable) in a drag scope reads.
//!
//! It owns two things: the **live drag session** (is a drag active, and what
//! payload is in flight) and the **set of registered drop targets** (each with
//! its window-space rect provider, accept predicate, reactive hover signal,
//! and callbacks). Draggables drive it (`begin` / `update` / `finish` /
//! `cancel`); droppables register into it (`register` / `deregister`) and read
//! their own hover state. A context is cheap to clone — it is one `Rc` — so
//! clone it into every draggable and droppable that should share a scope.
//!
//! Hit-testing happens in window coordinates: on each move the pointer's
//! `window_position` is tested against every registered droppable's
//! [`ViewHandle::absolute_frame`](runtime_core::ViewHandle::absolute_frame),
//! filtered by each target's accept predicate. When targets nest, the
//! **smallest-area** match wins — the most specific (innermost) target — which
//! is the intuitive result for a target inside a target.
//!
//! ## A drag outlives its source
//!
//! The source's own view delivers a drag's moves and release. A scroller
//! can unmount that view mid-drag — a virtualized list scrolled far
//! enough (by an auto-scroll near its edge, say) drops the source's
//! item — and its gesture stream ends with it, release included. When the
//! source's scope drops with its drag in flight, the context takes the
//! drag over from the app-wide pointer
//! ([`add_pointer_listener`](runtime_core::add_pointer_listener)), which
//! it listens to for its whole life so it knows whether the primary
//! button is down at that moment:
//!
//! - button down (a mouse or pen drag): moves keep moving the ghost and
//!   updating hover, and the release drops the payload on the target under
//!   it, as the source would have;
//! - otherwise (a finger, which has no app-wide channel, or a release
//!   nothing reported): the drag is cancelled. It never sticks.
//!
//! The gone source's [`Draggable::on_release`](crate::Draggable::on_release)
//! is not called: its scope, and whatever its callback would write, is
//! gone.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::animation::{AnimProp, AnimatedValue};
use runtime_core::{
    add_pointer_listener, pointer_root_origin, signal, AppPointerEvent, Element, MouseButton,
    PointerListener, PointerOutcome, PointerPhase, Ref, Signal, TouchPoint, ViewHandle,
    ViewportRect,
};

/// Builds the visual that follows the pointer during a drag (the "drag
/// preview" / ghost). Snapshotted once at drag start.
pub(crate) type PreviewBuilder = Rc<dyn Fn() -> Element>;

runtime_tls::thread_local! {
    /// Process-wide monotonic id source for droppables. Not time/random, so
    /// it is safe in workflow/replay contexts.
    static NEXT_DROPPABLE_ID: Cell<u64> = const { Cell::new(1) };
}

/// Opaque identity of a registered [`Droppable`](crate::Droppable). Stable for
/// the life of that droppable handle, so re-registration on re-render replaces
/// rather than duplicates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DroppableId(u64);

impl DroppableId {
    /// Mint a fresh id.
    pub(crate) fn next() -> Self {
        Self(NEXT_DROPPABLE_ID.with(|c| {
            let v = c.get();
            c.set(v + 1);
            v
        }))
    }
}

/// The configured registration a droppable hands the context. Boxed closures,
/// so the context can hold targets of one payload type `T` uniformly.
pub(crate) struct DroppableEntry<T> {
    pub(crate) id: DroppableId,
    /// Window-space rect of the target, sampled lazily at drag time. `None`
    /// when the target is not laid out / not mounted.
    pub(crate) rect: Rc<dyn Fn() -> Option<ViewportRect>>,
    /// Whether this target accepts the given payload. Targets that return
    /// `false` are skipped during hit-testing (no hover, no drop).
    pub(crate) accepts: Rc<dyn Fn(&T) -> bool>,
    /// Reactive hover state for this target — `true` while the pointer is over
    /// it *and* it accepts the in-flight payload.
    pub(crate) is_over: Signal<bool>,
    pub(crate) on_enter: Option<Rc<dyn Fn(&T)>>,
    pub(crate) on_leave: Option<Rc<dyn Fn()>>,
    pub(crate) on_drop: Option<Rc<dyn Fn(T)>>,
}

struct Inner<T> {
    /// `true` while any drag in this scope is active. Reactive, so the whole
    /// scope can react to "a drag is happening" (dim non-targets, show a
    /// trash zone, etc.).
    dragging: Signal<bool>,
    /// The payload of the in-flight drag, cloned from the source at `begin`.
    payload: Option<T>,
    droppables: Vec<DroppableEntry<T>>,
    /// Drop-zone rects **snapshotted once at `begin`**, not re-read per move.
    /// Reading a node's rect is a synchronous layout flush on web
    /// (`getBoundingClientRect`); doing it per pointermove — interleaved with
    /// the transform writes that move the dragged element — thrashes layout and
    /// stutters the drag. Only the app knows when its targets move, so it says
    /// so ([`DragContext::scrolled_by`], [`DragContext::refresh_rects`]). Empty
    /// when no drag is active.
    session_rects: Vec<(DroppableId, ViewportRect)>,
    /// Where the pointer last was in this drag, so a refresh can re-test hover
    /// without waiting for it to move.
    last_pointer: Option<TouchPoint>,
    /// Moves an in-place drag source by a scroll's delta, so it stays under the
    /// pointer while its scroller carries it away. `None` for a drag-layer ghost,
    /// which is positioned in window coordinates.
    follow_scroll: Option<Rc<dyn Fn(f32, f32)>>,
    /// Which droppable the pointer is currently over (drives enter/leave edge
    /// detection), if any.
    over: Option<DroppableId>,
    /// The drag-layer preview ("ghost") that follows the pointer when a
    /// [`Draggable`](crate::Draggable) opts into one. `ghost_x`/`ghost_y` are
    /// its top-left in window coordinates; `preview` builds its content. The
    /// ghost lives in a top-level overlay ([`crate::drag_layer`]) so it renders
    /// above ALL content and is never clipped by an ancestor — the real fix for
    /// "the dragged element renders behind / gets clipped", instead of leaning
    /// on sibling paint order.
    /// The ghost's top-left in window coordinates, bound to its `translate`
    /// (the drag layer pins it at `top:0; left:0`). Translate-only so the ghost
    /// never affects layout — a real `top`/`left` offset that grows with the
    /// drag can expand the container the overlay lowers into.
    ghost_x: AnimatedValue<f32>,
    ghost_y: AnimatedValue<f32>,
    preview: Option<PreviewBuilder>,
    /// Counts sessions, so a source can tell its own drag from a later one
    /// ([`owns`](DragContext::owns)).
    session: u64,
    /// Set while the context drives a drag whose source unmounted: the
    /// ghost's grab offset (pointer minus ghost top-left), `None` for an
    /// in-place drag. See the module docs, "A drag outlives its source".
    orphan: Option<Option<(f32, f32)>>,
    /// The app-wide pointer, heard for the context's life (module docs).
    /// Its closure holds the context weakly, so this does not keep it alive.
    pointer: Option<PointerListener>,
}

/// Shared drag/drop registry for one scope. Clone freely — clones share state.
pub struct DragContext<T> {
    inner: Rc<RefCell<Inner<T>>>,
}

impl<T> Clone for DragContext<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: Clone + 'static> Default for DragContext<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + 'static> DragContext<T> {
    /// Create a context. Call this inside the component scope that owns the
    /// drag area; the `dragging` signal it allocates lives for that scope.
    /// It listens to the app-wide pointer while it lives (module docs, "A
    /// drag outlives its source").
    pub fn new() -> Self {
        let ctx = Self {
            inner: Rc::new(RefCell::new(Inner {
                dragging: signal(false),
                payload: None,
                droppables: Vec::new(),
                session_rects: Vec::new(),
                last_pointer: None,
                follow_scroll: None,
                over: None,
                ghost_x: AnimatedValue::new(0.0),
                ghost_y: AnimatedValue::new(0.0),
                preview: None,
                session: 0,
                orphan: None,
                pointer: None,
            })),
        };
        let weak = Rc::downgrade(&ctx.inner);
        let pointer = add_pointer_listener(move |e| {
            if let Some(inner) = weak.upgrade() {
                DragContext { inner }.follow_orphan(e);
            }
            PointerOutcome::Default
        });
        ctx.inner.borrow_mut().pointer = Some(pointer);
        ctx
    }

    /// Reactive "a drag is in flight" flag. Read it in a `ui!`/`jsx!` body to
    /// react to drags starting and ending.
    pub fn dragging(&self) -> Signal<bool> {
        self.inner.borrow().dragging
    }

    /// The payload of the in-flight drag, if any. Cloned out.
    pub fn payload(&self) -> Option<T> {
        self.inner.borrow().payload.clone()
    }

    // ---- droppable registration (called by Droppable::bind) --------------

    /// Register (or, by id, replace) a drop target. Idempotent per id, so a
    /// droppable that re-binds on re-render updates in place.
    pub(crate) fn register(&self, entry: DroppableEntry<T>) {
        let mut inner = self.inner.borrow_mut();
        if let Some(slot) = inner.droppables.iter_mut().find(|e| e.id == entry.id) {
            *slot = entry;
        } else {
            inner.droppables.push(entry);
        }
    }

    /// Remove a drop target (called from the droppable's scope cleanup).
    pub(crate) fn deregister(&self, id: DroppableId) {
        let mut inner = self.inner.borrow_mut();
        inner.droppables.retain(|e| e.id != id);
        if inner.over == Some(id) {
            inner.over = None;
        }
    }

    // ---- a drag driven by the caller's own gesture ----------------------

    /// Open a drag from a gesture the caller recognises itself, for a
    /// component that already owns its drag (a table's row grip, a
    /// calendar's event) and wants this context's drop targets to see it.
    /// [`Draggable`](crate::Draggable) is the usual way in; this is the
    /// same session without its recognizer.
    ///
    /// `at` is the pointer in window coordinates. With a `preview`, the
    /// [`drag_layer`](crate::drag_layer) draws it until the session ends.
    pub fn start_drag(&self, payload: T, at: TouchPoint, preview: Option<DragPreview>) -> DragSession<T> {
        if let Some(DragPreview { build, x, y }) = preview {
            self.set_preview(build, x, y);
        }
        self.begin(payload);
        self.update(at);
        DragSession { ctx: self.clone(), open: true }
    }

    /// Whether a drop target that accepts the in-flight payload is under
    /// the pointer, as of the last move.
    pub fn is_over_target(&self) -> bool {
        self.inner.borrow().over.is_some()
    }

    /// Where the pointer is in the drag in flight, in window coordinates,
    /// as of its last move; `None` with no drag in flight. Read it from a
    /// frame loop to auto-scroll near an edge from the pointer itself.
    pub fn pointer_position(&self) -> Option<TouchPoint> {
        let inner = self.inner.borrow();
        inner.payload.as_ref().and(inner.last_pointer)
    }

    /// Abandon the drag in flight: nothing is delivered, the hovered
    /// target hears `on_leave`, and the ghost goes. A
    /// [`Draggable`](crate::Draggable) whose drag this ends reports
    /// [`DropOutcome::Cancelled`](crate::DropOutcome::Cancelled) when its
    /// pointer lifts. No-op with no drag in flight.
    pub fn cancel(&self) {
        self.end_cancelled();
        self.clear_preview();
    }

    /// Take the preview down, so a later in-place drag draws no ghost.
    pub(crate) fn clear_preview(&self) {
        self.inner.borrow_mut().preview = None;
    }

    // ---- drag session (called by Draggable's recognizer callback) --------

    /// Open a drag session with `payload`. Sets `dragging` true and clears any
    /// stale hover. Returns the session's token for [`owns`](Self::owns).
    pub(crate) fn begin(&self, payload: T) -> u64 {
        let (dragging, session) = {
            let mut inner = self.inner.borrow_mut();
            inner.payload = Some(payload);
            inner.over = None;
            inner.last_pointer = None;
            inner.orphan = None;
            inner.session += 1;
            // Snapshot every drop zone's rect here at drag start, so the
            // per-move hit-test reads no geometry (see `session_rects`).
            inner.session_rects = read_rects(&inner.droppables);
            (inner.dragging, inner.session)
        };
        dragging.set(true);
        session
    }

    /// Whether the drag `session` (a [`begin`](Self::begin) token) is still
    /// the one in flight — not finished, cancelled, or replaced.
    pub(crate) fn owns(&self, session: u64) -> bool {
        let inner = self.inner.borrow();
        inner.payload.is_some() && inner.session == session
    }

    /// The source of the drag in flight unmounted. Take the drag over from
    /// the app-wide pointer while the primary button is down, else cancel
    /// it. `grab` is the pointer's offset from the ghost's top-left, `None`
    /// for an in-place drag. See the module docs, "A drag outlives its
    /// source".
    pub(crate) fn orphan(&self, grab: Option<(f32, f32)>) {
        if self.inner.borrow().payload.is_none() {
            return;
        }
        // The context's own listener has tracked the buttons since it was
        // made, so this is the button state now, not since some later point.
        if runtime_shared::pointer_input::is_button_down(MouseButton::Primary) {
            let mut inner = self.inner.borrow_mut();
            inner.orphan = Some(grab);
            // It moved the source's offset, and the source is gone.
            inner.follow_scroll = None;
        } else {
            self.cancel();
        }
    }

    /// Drive an orphaned drag from one app-wide pointer event.
    fn follow_orphan(&self, e: &AppPointerEvent) {
        let Some(grab) = self.inner.borrow().orphan else {
            return;
        };
        // Pointer events are relative to the app root, drags to the window.
        let (ox, oy) = pointer_root_origin();
        let at = TouchPoint::new(e.x + ox, e.y + oy);
        match e.phase {
            PointerPhase::Move if e.is_held(MouseButton::Primary) => {
                if let Some((gx, gy)) = grab {
                    self.move_preview(at.x - gx, at.y - gy);
                }
                self.update(at);
            }
            // The release — or, when the window lost the mouse, the Up the
            // dispatcher sends for every held button.
            PointerPhase::Up if !e.is_held(MouseButton::Primary) => {
                self.finish(at);
                self.clear_preview();
            }
            _ => {}
        }
    }

    /// Update hover state for the pointer at `window` (in window coordinates),
    /// firing `on_enter` / `on_leave` and flipping `is_over` signals on the
    /// edge. No-op if no session is open.
    pub(crate) fn update(&self, window: TouchPoint) {
        self.inner.borrow_mut().last_pointer = Some(window);
        // Resolve everything we need while holding the borrow, then run
        // callbacks after releasing it (callbacks may read the context).
        enum Edge<T> {
            None,
            // (leave signal, leave cb) — previous target, if any.
            // (enter signal, enter cb, payload) — new target, if any.
            Change {
                leave: Option<(Signal<bool>, Option<Rc<dyn Fn()>>)>,
                enter: Option<(Signal<bool>, Option<Rc<dyn Fn(&T)>>, T)>,
                new_over: Option<DroppableId>,
            },
        }

        let edge = {
            let inner = self.inner.borrow();
            let Some(payload) = inner.payload.clone() else {
                return;
            };
            let hit = hit_test(&inner.droppables, &inner.session_rects, window, &payload);
            if hit == inner.over {
                Edge::None
            } else {
                let leave = inner.over.and_then(|id| {
                    inner
                        .droppables
                        .iter()
                        .find(|e| e.id == id)
                        .map(|e| (e.is_over, e.on_leave.clone()))
                });
                let enter = hit.and_then(|id| {
                    inner
                        .droppables
                        .iter()
                        .find(|e| e.id == id)
                        .map(|e| (e.is_over, e.on_enter.clone(), payload.clone()))
                });
                Edge::Change {
                    leave,
                    enter,
                    new_over: hit,
                }
            }
        };

        if let Edge::Change {
            leave,
            enter,
            new_over,
        } = edge
        {
            self.inner.borrow_mut().over = new_over;
            if let Some((sig, cb)) = leave {
                sig.set(false);
                if let Some(cb) = cb {
                    cb();
                }
            }
            if let Some((sig, cb, payload)) = enter {
                sig.set(true);
                if let Some(cb) = cb {
                    cb(&payload);
                }
            }
        }
    }

    /// Finish a drag at `window`. Delivers the payload to the target under the
    /// pointer (if any accepts it) and returns whether a drop landed. Closes
    /// the session and clears hover regardless.
    pub(crate) fn finish(&self, window: TouchPoint) -> bool {
        // Pull out the drop target + payload + reset, holding the borrow only
        // for the bookkeeping. Run on_drop afterward.
        let (drop_cb, leave_cb, payload, over_sig, dragging) = {
            let mut inner = self.inner.borrow_mut();
            let payload = inner.payload.take();
            let dragging = inner.dragging;
            let over = inner.over.take();
            inner.last_pointer = None;
            inner.follow_scroll = None;
            inner.orphan = None;
            let Some(payload) = payload else {
                inner.session_rects.clear();
                dragging.set(false);
                return false;
            };
            // Re-hit-test at the release point so a fast lift between moves
            // still resolves against the right target.
            let hit = hit_test(&inner.droppables, &inner.session_rects, window, &payload);
            let entry = hit.and_then(|id| inner.droppables.iter().find(|e| e.id == id));
            let drop_cb = entry.and_then(|e| e.on_drop.clone());
            // The previously-hovered target: clear its `is_over` signal AND fire
            // its `on_leave` — the drag is ending, so any hover visual (signal-
            // OR callback-driven, e.g. an animated highlight) must reset. Drop
            // does not skip leave.
            let over_entry = over.and_then(|id| inner.droppables.iter().find(|e| e.id == id));
            let leave_cb = over_entry.and_then(|e| e.on_leave.clone());
            let over_sig = over_entry
                .map(|e| e.is_over)
                // …and also the freshly-hit target's signal, in case it differs
                // from the last-hovered one.
                .or_else(|| entry.map(|e| e.is_over));
            inner.session_rects.clear();
            (drop_cb, leave_cb, payload, over_sig, dragging)
        };

        if let Some(sig) = over_sig {
            sig.set(false);
        }
        if let Some(cb) = leave_cb {
            cb();
        }
        dragging.set(false);
        if let Some(cb) = drop_cb {
            cb(payload);
            true
        } else {
            false
        }
    }

    /// Close the session without delivering the payload. Fires `on_leave`
    /// for the hovered target. [`cancel`](Self::cancel) also takes the
    /// preview down.
    fn end_cancelled(&self) {
        let (leave, dragging) = {
            let mut inner = self.inner.borrow_mut();
            inner.payload = None;
            inner.session_rects.clear();
            inner.last_pointer = None;
            inner.follow_scroll = None;
            inner.orphan = None;
            let dragging = inner.dragging;
            let leave = inner.over.take().and_then(|id| {
                inner
                    .droppables
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| (e.is_over, e.on_leave.clone()))
            });
            (leave, dragging)
        };
        if let Some((sig, cb)) = leave {
            sig.set(false);
            if let Some(cb) = cb {
                cb();
            }
        }
        dragging.set(false);
    }

    /// Re-read every drop zone's rect during a drag and re-test hover at the
    /// pointer. Call it when targets moved, or new ones mounted (a virtualized
    /// list scrolling them in): the hit-test only sees rects read at drag start
    /// or here. No-op with no drag in flight. Costs one rect read per target, so
    /// call it when something moved, not on every pointer move.
    pub fn refresh_rects(&self) {
        let pointer = {
            let mut inner = self.inner.borrow_mut();
            if inner.payload.is_none() {
                return;
            }
            inner.session_rects = read_rects(&inner.droppables);
            inner.last_pointer
        };
        if let Some(pointer) = pointer {
            self.update(pointer);
        }
    }

    /// A scroller under the drag moved its content by `(dx, dy)`, the change of
    /// its scroll offset: refresh the drop zones ([`refresh_rects`](Self::refresh_rects))
    /// and keep an in-place drag source under the pointer. Call it from the
    /// scroller's `on_scroll`, or the loop that auto-scrolls near an edge. No-op
    /// with no drag in flight.
    pub fn scrolled_by(&self, dx: f32, dy: f32) {
        let follow = self.inner.borrow().follow_scroll.clone();
        if let Some(follow) = follow {
            follow(dx, dy);
        }
        self.refresh_rects();
    }

    /// Set what [`scrolled_by`](Self::scrolled_by) moves for this drag.
    pub(crate) fn set_follow_scroll(&self, follow: Rc<dyn Fn(f32, f32)>) {
        self.inner.borrow_mut().follow_scroll = Some(follow);
    }

    // ---- drag-layer ghost (Model B) --------------------------------------

    /// Install the preview builder + place the ghost at window-space `(x, y)`
    /// (its top-left). Called at drag start *before* `begin` flips `dragging`.
    ///
    /// The ghost is positioned purely by **translate** (the drag layer pins it
    /// at `top: 0; left: 0` and these values drive its transform). Translate is
    /// the only positioning that NEVER affects layout on any backend — a real
    /// `top`/`left` offset that grows as you drag can expand whatever container
    /// the overlay lowers into. The ghost is not mounted yet when this runs, so
    /// the bound translate can't reach it here: [`crate::drag_layer`] applies
    /// the current position as the ghost mounts ([`Self::place_ghost`]).
    pub(crate) fn set_preview(&self, builder: PreviewBuilder, x: f32, y: f32) {
        let (gx, gy) = {
            let mut inner = self.inner.borrow_mut();
            inner.preview = Some(builder);
            (inner.ghost_x.clone(), inner.ghost_y.clone())
        };
        gx.cancel();
        gy.cancel();
        gx.set(x);
        gy.set(y);
    }

    /// The ghost's current top-left in **window** coordinates (the value last
    /// fed to [`set_preview`](Self::set_preview) / [`move_preview`](Self::move_preview)).
    ///
    /// Read it from a [`Draggable::on_release`](crate::Draggable::on_release)
    /// callback to drive a *drop animation*: reveal the real (previously hidden)
    /// source element at exactly the point the ghost was let go, then spring it
    /// into its resting slot. Without this the source reveals wherever its own
    /// hidden animation happened to be — mid-flight on a fast drag (a visible
    /// jerk), already-settled on a slow one (no animation at all) — because the
    /// ghost and the source are decoupled. Anchoring the reveal to the ghost's
    /// release point makes the hand-off seamless and speed-independent. The
    /// value persists after the drag ends (it is only overwritten by the next
    /// `set_preview`), so reading it during `on_release` is well-defined.
    pub fn ghost_position(&self) -> (f32, f32) {
        let inner = self.inner.borrow();
        (inner.ghost_x.get(), inner.ghost_y.get())
    }

    /// Move the ghost to window-space `(x, y)` (its top-left) on each drag move.
    pub(crate) fn move_preview(&self, x: f32, y: f32) {
        let (gx, gy) = {
            let inner = self.inner.borrow();
            (inner.ghost_x.clone(), inner.ghost_y.clone())
        };
        gx.set(x);
        gy.set(y);
    }

    /// Whether a drag preview is currently installed.
    pub(crate) fn has_preview(&self) -> bool {
        self.inner.borrow().preview.is_some()
    }

    /// Build the current preview's content (empty fragment if none).
    pub fn build_preview(&self) -> Element {
        let builder = self.inner.borrow().preview.clone();
        match builder {
            Some(b) => b(),
            None => runtime_core::fragment(Vec::new()),
        }
    }

    /// Write the ghost's current window position to `ghost`'s translate.
    /// [`crate::drag_layer`] calls it as the ghost mounts, before its first
    /// paint: [`bind_ghost`](Self::bind_ghost)'s binding reaches the node
    /// only on the position's next change, or after mount, and in between
    /// the ghost painted a frame at the window's top-left.
    pub fn place_ghost(&self, ghost: &ViewHandle) {
        let (x, y) = self.ghost_position();
        ghost.set_animated_f32(AnimProp::TranslateX, x);
        ghost.set_animated_f32(AnimProp::TranslateY, y);
    }

    /// Bind the ghost's window position to `target`'s translate. Used by
    /// [`crate::drag_layer`] on the overlay view that holds the preview.
    pub fn bind_ghost(&self, target: Ref<ViewHandle>) {
        let (gx, gy) = {
            let inner = self.inner.borrow();
            (inner.ghost_x.clone(), inner.ghost_y.clone())
        };
        gx.bind(target, AnimProp::TranslateX);
        gy.bind(target, AnimProp::TranslateY);
    }
}

/// A drag preview for [`DragContext::start_drag`]: what the
/// [`drag_layer`](crate::drag_layer) draws, and where its top-left starts,
/// in window coordinates.
pub struct DragPreview {
    build: PreviewBuilder,
    x: f32,
    y: f32,
}

impl DragPreview {
    /// `build`'s element, its top-left at window `(x, y)`.
    pub fn new(build: impl Fn() -> Element + 'static, x: f32, y: f32) -> Self {
        Self { build: Rc::new(build), x, y }
    }
}

/// A drag in flight that the caller drives from its own gesture — see
/// [`DragContext::start_drag`]. End it with [`finish`](Self::finish) or
/// [`cancel`](Self::cancel); a session dropped without either is cancelled.
pub struct DragSession<T: Clone + 'static> {
    ctx: DragContext<T>,
    open: bool,
}

impl<T: Clone + 'static> DragSession<T> {
    /// The pointer is at window `at`; with a preview, its top-left moves to
    /// `preview_at`. Returns whether a drop target that accepts the payload
    /// is under the pointer.
    pub fn moved(&self, at: TouchPoint, preview_at: Option<(f32, f32)>) -> bool {
        if let Some((x, y)) = preview_at {
            self.ctx.move_preview(x, y);
        }
        self.ctx.update(at);
        self.ctx.is_over_target()
    }

    /// Release at window `at`: the target under it, if one accepts the
    /// payload, receives it through its `on_drop`. Returns whether one did.
    pub fn finish(mut self, at: TouchPoint) -> bool {
        self.open = false;
        let landed = self.ctx.finish(at);
        self.ctx.clear_preview();
        landed
    }

    /// Abandon the drag: nothing is delivered.
    pub fn cancel(mut self) {
        self.close();
    }

    fn close(&mut self) {
        if std::mem::replace(&mut self.open, false) {
            self.ctx.cancel();
            self.ctx.clear_preview();
        }
    }
}

impl<T: Clone + 'static> Drop for DragSession<T> {
    fn drop(&mut self) {
        self.close();
    }
}

/// Every drop zone's current rect. A target not laid out yet has none.
fn read_rects<T>(droppables: &[DroppableEntry<T>]) -> Vec<(DroppableId, ViewportRect)> {
    droppables.iter().filter_map(|e| (e.rect)().map(|r| (e.id, r))).collect()
}

/// True if `p` (window coords) lies within `rect`.
fn rect_contains(rect: ViewportRect, p: TouchPoint) -> bool {
    p.x >= rect.x
        && p.x < rect.x + rect.width
        && p.y >= rect.y
        && p.y < rect.y + rect.height
}

/// Find the droppable under `window` that accepts `payload`, preferring the
/// smallest-area (most specific / innermost) match when several overlap.
/// Tests against the rects in the session snapshot — never re-reads geometry.
fn hit_test<T>(
    droppables: &[DroppableEntry<T>],
    rects: &[(DroppableId, ViewportRect)],
    window: TouchPoint,
    payload: &T,
) -> Option<DroppableId> {
    let mut best: Option<(DroppableId, f32)> = None;
    for (id, rect) in rects {
        if !rect_contains(*rect, window) {
            continue;
        }
        let Some(e) = droppables.iter().find(|e| e.id == *id) else {
            continue;
        };
        if !(e.accepts)(payload) {
            continue;
        }
        let area = rect.width * rect.height;
        match best {
            Some((_, best_area)) if best_area <= area => {}
            _ => best = Some((*id, area)),
        }
    }
    best.map(|(id, _)| id)
}
