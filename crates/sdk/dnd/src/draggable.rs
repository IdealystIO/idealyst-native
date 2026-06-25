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
use runtime_core::{effect, Bound, Element, Ref, Signal, ViewHandle};

use crate::context::{DragContext, PreviewBuilder};
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
    /// Reactive "this specific source is being dragged right now". The wrapped
    /// component reads it to restyle itself (dim/hide while its ghost flies) —
    /// the equivalent of react-dnd's `isDragging` from `useDrag`.
    is_dragging: Signal<bool>,
    on_start: Option<StartCb>,
    on_release: Option<ReleaseCb>,
    /// When set, the drag uses the **drag-layer** model: a ghost built by this
    /// closure follows the pointer in a top-level overlay (see
    /// [`crate::drag_layer`]) and the source element stays put. When `None`,
    /// the source element itself translates (the in-place model).
    preview: Option<PreviewBuilder>,
    /// Opacity to fade the source element to while it is being dragged (so a
    /// drag-layer ghost reads as "the live copy"). Applied by [`Draggable::attach`].
    dim: Option<f32>,
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
            is_dragging: Signal::new(false),
            on_start: None,
            on_release: None,
            preview: None,
            dim: None,
        }
    }

    /// Reactive flag: `true` while THIS source is being dragged. Read it in the
    /// wrapped component to restyle the source while its drag is in flight
    /// (e.g. dim it to `opacity: 0.4`, or hide it so only the ghost shows).
    /// The react-dnd `isDragging` equivalent.
    pub fn is_dragging(&self) -> Signal<bool> {
        self.is_dragging
    }

    /// Opt into the **drag-layer** model: `build` produces a ghost that follows
    /// the pointer in a top-level overlay, rendered above all content and never
    /// clipped by an ancestor. The source element stays in place (dim it via
    /// [`DragContext::dragging`] if you like). Requires [`crate::drag_layer`]
    /// to be mounted once near the app root. This is the robust default for
    /// cross-container drag; without it the source element translates in place,
    /// which is constrained to its own stacking context.
    pub fn preview(mut self, build: impl Fn() -> Element + 'static) -> Self {
        self.preview = Some(std::rc::Rc::new(build));
        self
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

    /// Fade the source element to `opacity` while it is being dragged, so a
    /// drag-layer ghost reads as the live copy and the original as parked.
    /// Applied by [`Draggable::attach`]. (For full control instead, read
    /// [`Draggable::is_dragging`] in your own view.)
    pub fn dim_source(mut self, opacity: f32) -> Self {
        self.dim = Some(opacity);
        self
    }

    /// Wire this draggable onto `view` and return the finished element — the
    /// one-call form of "make a ref, install the handler, bind it". Installs
    /// the touch handler, owns the ref, binds the in-place offset, and applies
    /// [`Draggable::dim_source`] if set. Use [`Draggable::handler`] +
    /// [`Draggable::bind`] directly when you need the ref yourself (to bind
    /// other animated props to the node) or to compose in a `GestureGroup`.
    pub fn attach(self, view: Bound<ViewHandle>) -> Element {
        let r: Ref<ViewHandle> = Ref::new();
        // Offset bind is harmless in the drag-layer model (the element never
        // translates) and required in the in-place model.
        self.bind(r);
        if let Some(opacity) = self.dim {
            let dim_av = AnimatedValue::new(1.0);
            dim_av.bind(r, AnimProp::Opacity);
            let is_dragging = self.is_dragging;
            // Bridge the per-source drag flag to the bound opacity. Built
            // during render, so the surrounding component scope owns it.
            effect!({
                dim_av.set(if is_dragging.get() { opacity } else { 1.0 });
            });
        }
        let handler = self.handler();
        view.on_touch(move |ev| handler(ev)).bind(r).into()
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
        // In drag-layer mode `base` instead holds the grab offset (where in the
        // element the finger landed), so the ghost sits under the finger.
        let base = self.base.clone();
        let snap_back = self.snap_back;
        let is_dragging = self.is_dragging;
        let on_start = self.on_start.clone();
        let on_release = self.on_release.clone();
        let preview = self.preview.clone();

        move |phase: DragPhase| match phase {
            DragPhase::Began(sample) => {
                is_dragging.set(true);
                if let Some(builder) = &preview {
                    // Drag-layer model: the ghost moves, the source stays put.
                    // Stash the grab offset so the ghost tracks under the finger.
                    base.set((sample.view_position.x, sample.view_position.y));
                    ctx.set_preview(
                        builder.clone(),
                        sample.window_position.x - sample.view_position.x,
                        sample.window_position.y - sample.view_position.y,
                    );
                } else {
                    // In-place model: the source element translates. Snapshot
                    // the rest position so a miss springs back exactly here.
                    x.cancel();
                    y.cancel();
                    base.set((x.get(), y.get()));
                }
                ctx.begin((payload)());
                ctx.update(sample.window_position);
                if let Some(cb) = &on_start {
                    cb();
                }
            }
            DragPhase::Moved(sample) => {
                if preview.is_some() {
                    let (gx, gy) = base.get();
                    ctx.move_preview(sample.window_position.x - gx, sample.window_position.y - gy);
                } else {
                    let (bx, by) = base.get();
                    x.set(bx + sample.delta.x);
                    y.set(by + sample.delta.y);
                }
                ctx.update(sample.window_position);
            }
            DragPhase::Ended {
                window_position, ..
            } => {
                is_dragging.set(false);
                let landed = ctx.finish(window_position);
                let outcome = if landed {
                    DropOutcome::Landed
                } else {
                    // In-place mode springs the element back; the ghost just
                    // vanishes when `dragging` flips false (the drag layer
                    // unmounts it), so there is nothing to restore.
                    if preview.is_none() && snap_back {
                        spring_back(&x, &y, base.get());
                    }
                    DropOutcome::Missed
                };
                if let Some(cb) = &on_release {
                    cb(outcome);
                }
            }
            DragPhase::Cancelled => {
                is_dragging.set(false);
                ctx.cancel();
                if preview.is_none() && snap_back {
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
