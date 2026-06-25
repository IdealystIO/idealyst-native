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

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::animation::{AnimProp, AnimatedValue};
use runtime_core::{Element, Ref, Signal, TouchPoint, ViewHandle, ViewportRect};

/// Builds the visual that follows the pointer during a drag (the "drag
/// preview" / ghost). Snapshotted once at drag start.
pub(crate) type PreviewBuilder = Rc<dyn Fn() -> Element>;

thread_local! {
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
    /// stutters the drag. Targets don't move during a drag, so one snapshot at
    /// drag start is both correct and cheap. Empty when no drag is active.
    session_rects: Vec<(DroppableId, ViewportRect)>,
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
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                dragging: Signal::new(false),
                payload: None,
                droppables: Vec::new(),
                session_rects: Vec::new(),
                over: None,
                ghost_x: AnimatedValue::new(0.0),
                ghost_y: AnimatedValue::new(0.0),
                preview: None,
            })),
        }
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

    // ---- drag session (called by Draggable's recognizer callback) --------

    /// Open a drag session with `payload`. Sets `dragging` true and clears any
    /// stale hover.
    pub(crate) fn begin(&self, payload: T) {
        let dragging = {
            let mut inner = self.inner.borrow_mut();
            inner.payload = Some(payload);
            inner.over = None;
            // Snapshot every drop zone's rect ONCE, here at drag start, so the
            // per-move hit-test reads no geometry (see `session_rects`).
            let rects: Vec<(DroppableId, ViewportRect)> = inner
                .droppables
                .iter()
                .filter_map(|e| (e.rect)().map(|r| (e.id, r)))
                .collect();
            inner.session_rects = rects;
            inner.dragging
        };
        dragging.set(true);
    }

    /// Update hover state for the pointer at `window` (in window coordinates),
    /// firing `on_enter` / `on_leave` and flipping `is_over` signals on the
    /// edge. No-op if no session is open.
    pub(crate) fn update(&self, window: TouchPoint) {
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

    /// Abort the in-flight drag without delivering the payload (platform
    /// cancel). Fires `on_leave` for the hovered target and closes the session.
    pub(crate) fn cancel(&self) {
        let (leave, dragging) = {
            let mut inner = self.inner.borrow_mut();
            inner.payload = None;
            inner.session_rects.clear();
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

    // ---- drag-layer ghost (Model B) --------------------------------------

    /// Install the preview builder + place the ghost at window-space `(x, y)`
    /// (its top-left). Called at drag start *before* `begin` flips `dragging`.
    ///
    /// The ghost is positioned purely by **translate** (the drag layer pins it
    /// at `top: 0; left: 0` and these values drive its transform). Translate is
    /// the only positioning that NEVER affects layout on any backend — a real
    /// `top`/`left` offset that grows as you drag can expand whatever container
    /// the overlay lowers into. The cost is that `AnimatedValue::bind` only
    /// applies on the *next* change after the node mounts, so we'd flash at the
    /// origin for one frame; to avoid that we re-apply the current value on a
    /// microtask, which lands after the overlay has mounted (and its ref
    /// filled) but before paint.
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
        // Re-apply the *current* value once the ghost has mounted, so the bound
        // translate isn't stuck unapplied for the first frame.
        let (gx2, gy2) = (gx, gy);
        runtime_core::schedule_microtask(move || {
            gx2.set(gx2.get());
            gy2.set(gy2.get());
        });
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

/// True if `p` (window coords) lies within `rect`.
fn rect_contains(rect: ViewportRect, p: TouchPoint) -> bool {
    p.x >= rect.x
        && p.x < rect.x + rect.width
        && p.y >= rect.y
        && p.y < rect.y + rect.height
}

/// Find the droppable under `window` that accepts `payload`, preferring the
/// smallest-area (most specific / innermost) match when several overlap.
/// Tests against the rects snapshotted at `begin` — never re-reads geometry.
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
