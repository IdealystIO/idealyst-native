//! [`Draggable`] — a drag source that carries a typed payload.
//!
//! Wire it like [`pan::Pan`](https://docs.rs/pan): build it against a
//! [`DragContext`], install its [`Draggable::handler`] on the view's
//! `on_touch` slot, and [`Draggable::bind`] its offset to the same view so the
//! element follows the finger. On release the payload is delivered to whatever
//! [`Droppable`](crate::Droppable) is under the pointer; if none accepts it the
//! element springs back (unless you opt out).

use std::cell::Cell;
use std::rc::Rc;

use runtime_core::animation::{AnimProp, AnimatedValue, SpringTo};
use runtime_core::{Ref, ViewHandle};

use crate::context::DragContext;
use crate::recognizer::{Activation, DragPhase, DragRecognizer};

/// Spring stiffness used for the snap-back when a drag misses every target.
/// Matches the snappy default the animation examples use for return-to-rest.
pub const SNAP_BACK_STIFFNESS: f32 = 140.0;
/// Spring damping for the snap-back — critically-ish damped, no overshoot.
pub const SNAP_BACK_DAMPING: f32 = 20.0;

/// How a drag ended, delivered to [`Draggable::on_release`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropOutcome {
    /// The payload landed on a target that accepted it.
    Landed,
    /// The finger lifted over no accepting target. The element snaps back
    /// (unless [`Draggable::snap_back`] was set `false`).
    Missed,
    /// The platform interrupted the drag (system gesture, view detach).
    Cancelled,
}

type StartCb = Rc<dyn Fn()>;
type ReleaseCb = Rc<dyn Fn(DropOutcome)>;

/// A drag source. Clone is cheap and shares the offset + context.
#[derive(Clone)]
pub struct Draggable<T: Clone + 'static> {
    ctx: DragContext<T>,
    payload: Rc<dyn Fn() -> T>,
    activation: Activation,
    x: AnimatedValue<f32>,
    y: AnimatedValue<f32>,
    /// Offset captured at the start of the in-flight drag, so a missed drop
    /// springs back to exactly where the element rested before the grab.
    base: Rc<Cell<(f32, f32)>>,
    snap_back: bool,
    on_start: Option<StartCb>,
    on_release: Option<ReleaseCb>,
}

impl<T: Clone + 'static> Draggable<T> {
    /// Create a draggable in `ctx` whose dropped payload is produced by
    /// `payload` (sampled fresh at each drag start, so it can reflect current
    /// state). Activation defaults to [`Activation::platform_default`] —
    /// long-press on touch, immediate on desktop.
    pub fn new(ctx: &DragContext<T>, payload: impl Fn() -> T + 'static) -> Self {
        Self {
            ctx: ctx.clone(),
            payload: Rc::new(payload),
            activation: Activation::platform_default(),
            x: AnimatedValue::new(0.0),
            y: AnimatedValue::new(0.0),
            base: Rc::new(Cell::new((0.0, 0.0))),
            snap_back: true,
            on_start: None,
            on_release: None,
        }
    }

    /// Override when the drag commits. See [`Activation`].
    pub fn activation(mut self, activation: Activation) -> Self {
        self.activation = activation;
        self
    }

    /// Whether a missed drop springs the element back to its pre-drag
    /// position. Default `true`. Set `false` when the element is a transient
    /// ghost the app removes itself.
    pub fn snap_back(mut self, on: bool) -> Self {
        self.snap_back = on;
        self
    }

    /// Fired once when a drag commits. Replaces any previous callback.
    pub fn on_start(mut self, f: impl Fn() + 'static) -> Self {
        self.on_start = Some(Rc::new(f));
        self
    }

    /// Fired once when the drag ends, with how it ended. Replaces any
    /// previous callback. (The *target* is notified separately via
    /// [`Droppable::on_drop`](crate::Droppable::on_drop); this is the
    /// *source's* notification.)
    pub fn on_release(mut self, f: impl Fn(DropOutcome) + 'static) -> Self {
        self.on_release = Some(Rc::new(f));
        self
    }

    /// The live `(x, y)` ghost offset as two [`AnimatedValue`]s. Bind them
    /// yourself for a custom transform, or use [`Draggable::bind`].
    pub fn offset(&self) -> (AnimatedValue<f32>, AnimatedValue<f32>) {
        (self.x.clone(), self.y.clone())
    }

    /// Bind the ghost offset to `target`'s translate (`x → TranslateX`,
    /// `y → TranslateY`) so the element follows the finger during a drag.
    /// Call during render inside the active reactive scope.
    pub fn bind(&self, target: Ref<ViewHandle>) {
        self.x.bind(target, AnimProp::TranslateX);
        self.y.bind(target, AnimProp::TranslateY);
    }

    /// The installable [`TouchHandler`](runtime_core::TouchHandler) for the
    /// view's `on_touch` slot.
    pub fn handler(&self) -> runtime_core::TouchHandler {
        DragRecognizer::new(self.activation, self.drag_callback()).into_handler()
    }

    /// The underlying [`DragRecognizer`] for composing in a
    /// [`gesture::GestureGroup`] alongside other recognizers (e.g. a tap that
    /// must fail before the drag begins). The recognizer it returns shares no
    /// state with [`Draggable::handler`]'s — use one path or the other.
    pub fn recognizer(&self) -> DragRecognizer {
        DragRecognizer::new(self.activation, self.drag_callback())
    }

    /// The `DragPhase` → context/offset closure shared by `handler` and
    /// `recognizer`. Captures fresh clones so the two paths stay independent.
    fn drag_callback(&self) -> impl Fn(DragPhase) + 'static {
        let ctx = self.ctx.clone();
        let payload = self.payload.clone();
        let x = self.x.clone();
        let y = self.y.clone();
        let base = self.base.clone();
        let snap_back = self.snap_back;
        let on_start = self.on_start.clone();
        let on_release = self.on_release.clone();

        move |phase: DragPhase| match phase {
            DragPhase::Began(sample) => {
                // Take over any running animation and snapshot the rest
                // position so a miss springs back exactly here.
                x.cancel();
                y.cancel();
                base.set((x.get(), y.get()));
                ctx.begin((payload)());
                ctx.update(sample.window_position);
                if let Some(cb) = &on_start {
                    cb();
                }
            }
            DragPhase::Moved(sample) => {
                let (bx, by) = base.get();
                x.set(bx + sample.delta.x);
                y.set(by + sample.delta.y);
                ctx.update(sample.window_position);
            }
            DragPhase::Ended {
                window_position, ..
            } => {
                let landed = ctx.finish(window_position);
                let outcome = if landed {
                    DropOutcome::Landed
                } else {
                    if snap_back {
                        spring_back(&x, &y, base.get());
                    }
                    DropOutcome::Missed
                };
                if let Some(cb) = &on_release {
                    cb(outcome);
                }
            }
            DragPhase::Cancelled => {
                ctx.cancel();
                if snap_back {
                    spring_back(&x, &y, base.get());
                }
                if let Some(cb) = &on_release {
                    cb(DropOutcome::Cancelled);
                }
            }
        }
    }
}

/// Spring both axes back to `(tx, ty)` — the pre-drag rest position.
fn spring_back(x: &AnimatedValue<f32>, y: &AnimatedValue<f32>, (tx, ty): (f32, f32)) {
    x.animate(
        SpringTo::new(tx)
            .stiffness(SNAP_BACK_STIFFNESS)
            .damping(SNAP_BACK_DAMPING),
    );
    y.animate(
        SpringTo::new(ty)
            .stiffness(SNAP_BACK_STIFFNESS)
            .damping(SNAP_BACK_DAMPING),
    );
}
