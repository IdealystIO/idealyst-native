//! [`Droppable`] — a drop target that reports reactive hover state and
//! receives the payload on release.
//!
//! Build it against the same [`DragContext`] as the draggables, configure
//! which payloads it accepts and its callbacks, then [`Droppable::bind`] it to
//! the view whose window-space rect defines the drop zone. Read
//! [`Droppable::is_over`] in a `ui!`/`jsx!` body to highlight the zone while a
//! compatible payload hovers it.

use std::rc::Rc;

use runtime_core::{on_cleanup, Ref, Signal, ViewHandle};

use crate::context::{DragContext, DroppableEntry, DroppableId};

/// A drop target. Clone is cheap and shares state with the registration.
#[derive(Clone)]
pub struct Droppable<T: Clone + 'static> {
    ctx: DragContext<T>,
    id: DroppableId,
    accepts: Rc<dyn Fn(&T) -> bool>,
    is_over: Signal<bool>,
    on_enter: Option<Rc<dyn Fn(&T)>>,
    on_leave: Option<Rc<dyn Fn()>>,
    on_drop: Option<Rc<dyn Fn(T)>>,
}

impl<T: Clone + 'static> Droppable<T> {
    /// Create a drop target in `ctx`. By default it accepts every payload;
    /// narrow that with [`Droppable::accepts`]. Allocate it inside the
    /// component scope that renders the zone (its `is_over` signal lives for
    /// that scope).
    pub fn new(ctx: &DragContext<T>) -> Self {
        Self {
            ctx: ctx.clone(),
            id: DroppableId::next(),
            accepts: Rc::new(|_| true),
            is_over: Signal::new(false),
            on_enter: None,
            on_leave: None,
            on_drop: None,
        }
    }

    /// Restrict which payloads this target accepts. A payload that fails the
    /// predicate never hovers (no `is_over`, no `on_enter`) and never drops
    /// here. Replaces any previous predicate.
    pub fn accepts(mut self, pred: impl Fn(&T) -> bool + 'static) -> Self {
        self.accepts = Rc::new(pred);
        self
    }

    /// Fired when an accepted payload first hovers this target.
    pub fn on_enter(mut self, f: impl Fn(&T) + 'static) -> Self {
        self.on_enter = Some(Rc::new(f));
        self
    }

    /// Fired when an accepted payload stops hovering this target (moved off,
    /// or the drag was cancelled).
    pub fn on_leave(mut self, f: impl Fn() + 'static) -> Self {
        self.on_leave = Some(Rc::new(f));
        self
    }

    /// Fired when a payload is dropped on this target. This is the target's
    /// notification; the *source* learns the outcome via
    /// [`Draggable::on_release`](crate::Draggable::on_release).
    pub fn on_drop(mut self, f: impl Fn(T) + 'static) -> Self {
        self.on_drop = Some(Rc::new(f));
        self
    }

    /// Reactive "an accepted payload is hovering me" flag. Read it in a
    /// `ui!`/`jsx!` body to highlight the zone.
    pub fn is_over(&self) -> Signal<bool> {
        self.is_over
    }

    /// Register this target's drop zone as `target`'s window-space rect, and
    /// arrange to deregister when the surrounding scope drops. Call during
    /// render inside the active reactive scope, after configuring the
    /// callbacks. Re-binding on re-render replaces in place (stable id).
    pub fn bind(&self, target: Ref<ViewHandle>) {
        // `Ref` is `Copy`, so the rect provider can capture it and sample the
        // view's window-space rect lazily at drag time — the same read overlay
        // anchoring uses.
        let rect: Rc<dyn Fn() -> Option<runtime_core::ViewportRect>> =
            Rc::new(move || target.with(|h| h.absolute_frame()).flatten());

        self.ctx.register(DroppableEntry {
            id: self.id,
            rect,
            accepts: self.accepts.clone(),
            is_over: self.is_over,
            on_enter: self.on_enter.clone(),
            on_leave: self.on_leave.clone(),
            on_drop: self.on_drop.clone(),
        });

        // Deregister when the scope drops (component unmount / re-render),
        // mirroring how `AnimatedValue::bind` anchors its subscription.
        let ctx = self.ctx.clone();
        let id = self.id;
        on_cleanup(move || ctx.deregister(id));
    }
}
