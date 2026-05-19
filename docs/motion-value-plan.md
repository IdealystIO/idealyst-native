# Motion values — gesture-driven properties at native speed

The "Layer 2" of the touch system per [native-touch-plan.md](native-touch-plan.md).
A `Motion` is a shared scalar with dual residency (Rust + platform)
that updates a bound native property the same frame, with no
Rust↔platform round-trip per write. The piece that turns the current
"Rust handles every touch event" model from "good enough" into
"drag-a-card-at-120fps."

This doc is the design spec for that layer. Builds on top of the
raw touch system; presumes that system is fully landed on all
backends.

## Why it exists

The raw touch pipeline currently round-trips every event through
Rust:

```
native touch → Rust handler → signal write → Effect re-fire →
   style recompute → backend.apply_style → native widget update
```

The Rust call itself is cheap (~µs). The style-recompute + apply
path is not — it can take milliseconds per event. At 120Hz pan rate
that's frame budget eaten by bookkeeping, with the visible result
being the *same* number written into a transform every frame.

Reanimated solved this with shared values + animated styles: the
gesture handler writes a UI-thread-resident scalar; the bound style
reads it natively each frame; the JS thread is never on the hot
path. Same idea here, adapted to our backend-trait architecture.

The end-state shape:

```rust
let pan_x = Motion::new(0.0);

view(...)
    .on_touch({
        let pan_x = pan_x.clone();
        move |ev| {
            if ev.phase == TouchPhase::Moved {
                pan_x.write(ev.position.x);  // ← cheap; no signal
            }
            TouchResponse::CONSUMED
        }
    })
    .with_style(stylesheet! {
        transform: [translate_x(&pan_x)],    // ← native binding
    });
```

The `pan_x.write(...)` call pushes a value to the native side and
returns. No signal write, no Effect re-fire, no style recompute.
The bound `translate_x` property reads the value natively on the
next frame.

## Core abstractions

Three operations across the lifecycle of a motion value:

1. **Create** — `Motion::new(initial)` allocates a backend
   `MotionHandle` and wraps it in a Rust-side handle.
2. **Bind** — `stylesheet! { transform: [translate_x(&motion)] }`
   produces a stylesheet whose transform op references a motion
   instead of carrying a static number. Applied once via
   `Backend::bind_motion_to_property`.
3. **Write** — `motion.write(value)` updates the native scalar.
   Synchronous, lock-free, takes effect by the next render frame.

Plus two native animation primitives that ride the same handle:

4. **Animate** — `motion.animate_to(target, duration_ms, easing)`
   runs an interpolation on the native side. The platform's
   animator drives every frame.
5. **Spring** — `motion.spring_to(target, spring_config)` runs a
   native physics spring.

## Backend trait additions

```rust
pub trait Backend {
    // ...existing...

    /// Per-backend opaque handle to a motion value. The Rust-side
    /// `Motion` wraps this; multiple `Motion::clone`s share one
    /// native handle.
    type MotionHandle: Clone;

    /// Allocate a motion value on the native side. `initial` is
    /// the starting scalar.
    fn create_motion(&mut self, initial: f32) -> Self::MotionHandle;

    /// Write a value. Synchronous, lock-free, takes effect by the
    /// next render frame. No signal write, no style recompute, no
    /// effect re-fire — that's the whole point.
    fn write_motion(&mut self, handle: &Self::MotionHandle, value: f32);

    /// Snapshot the current native-side value. Used at gesture
    /// boundaries (Ended) to reflect the resting value into a
    /// Signal if the author wants reactive feedback. Cheap; safe
    /// to call from any thread the framework already runs on.
    fn read_motion(&self, handle: &Self::MotionHandle) -> f32;

    /// Bind this motion to a property on `node`. Subsequent
    /// `write_motion` calls update that property natively until
    /// the node is destroyed or rebinding replaces it.
    fn bind_motion_to_property(
        &mut self,
        node: &Self::Node,
        property: AnimProperty,
        handle: &Self::MotionHandle,
    );

    /// Native-side tween. `motion.animate_to(...)` calls this; the
    /// backend's platform animator drives interpolation each frame
    /// (CABasicAnimation / ObjectAnimator / framework Animator /
    /// WAAPI). Cancels any in-flight tween or spring on this
    /// motion before starting.
    fn animate_motion(
        &mut self,
        handle: &Self::MotionHandle,
        target: f32,
        duration_ms: u32,
        easing: Easing,
    );

    /// Native-side physics spring. Same shape as `animate_motion`
    /// but with a spring config (stiffness, damping, mass)
    /// instead of a duration.
    fn spring_motion(
        &mut self,
        handle: &Self::MotionHandle,
        target: f32,
        spring: SpringConfig,
    );

    /// Drop the native-side motion handle. Called by `Motion`'s
    /// `Drop` impl. Backends free per-platform resources here.
    fn release_motion(&mut self, handle: Self::MotionHandle);
}

#[derive(Clone, Copy)]
pub enum AnimProperty {
    TranslateX,
    TranslateY,
    Scale,
    Rotation,
    Opacity,
    // Color animation is its own future story — a `MotionColor`
    // type with three or four bound scalars, or a `MotionRgba`
    // with a different backend handle type. Out of v1 scope.
}

pub struct SpringConfig {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
}
```

All defaults are no-ops (or panic for `create_motion` /
`bind_motion_to_property` — these are required for the feature to
work at all; partial backends would yield silent broken state).

## Per-platform binding

| Platform | Storage | Write cost | Bound-property cost |
|---|---|---|---|
| **wgpu** | `Rc<Cell<f32>>` shared between Host (writer) and renderer | One cell write. Free. | Renderer reads the cell when computing the bound node's transform uniform. One read per frame. |
| **iOS** | `CALayer` keypath + a Rust-owned `f32` mirror for `read_motion` | One ObjC method call (`layer.setValue:forKeyPath:`). Doesn't invalidate layout. | Native compositor reads `CALayer.transform` on every frame anyway. Zero added cost. |
| **Android** | Direct `View.setTranslationX(...)` etc. for raw writes; `SpringAnimation` / `ObjectAnimator` for animated targets | One JNI call. Doesn't invalidate layout. | Native compositor reads the view's transform on every frame. Zero added cost. |
| **web** | CSS custom property: `--motion-{id}` written via `el.style.setProperty(...)` | One DOM style-property write. Compositor-only path; doesn't trigger layout or paint. | Bound transform uses `transform: translateX(var(--motion-{id}))`. Browser compositor reads on every frame. |

### wgpu binding detail

```
backend.create_motion(0.0) → MotionHandle(Rc<Cell<f32>>)
backend.bind_motion_to_property(node, TranslateX, &h) →
   node.motion_bindings.push((TranslateX, h.clone()))
backend.write_motion(&h, 50.0) → h.0.set(50.0)

Renderer per-frame walk:
  for (prop, h) in &node.motion_bindings {
      // Compose with style's static transform, if any.
      apply_motion_to_transform(prop, h.0.get(), &mut node_transform);
  }
```

The motion bindings on a node compose with the existing styled
transform. If the style sets `translate_x: 10` and a motion
overrides it, the motion wins at composition time — last writer
within the per-frame transform stack.

### iOS binding detail

`CALayer.translation` doesn't exist; the bound property is the
matching transform component or a separate sub-layer attribute.
Two approaches:

1. **Direct write** of `layer.transform` each `write_motion`. Use a
   stored `CATransform3D` we mutate, compose with the style's
   static transform pre-write. Simple, works.
2. **`CABasicAnimation` model layer + presentation layer** —
   trickier but standard for animated transforms. The animation's
   `fromValue` is the model layer's current value, `toValue` is
   the new write target, duration is 1 frame. UIKit interpolates.

Start with (1). It matches Reanimated's approach. Reach for (2) if
we hit jank with rapid writes.

For `animate_motion` / `spring_motion`, drop into proper
`CABasicAnimation` / `CASpringAnimation` with the target value and
duration; CALayer interpolates on the render server (off the main
thread). Cancel any in-flight tween before starting the new one
via `[layer removeAnimationForKey: ...]`.

### Android binding detail

`View.setTranslationX(v)` is the simplest direct write — invalidates
the compositor without retriggering layout. Works for `Scale` /
`Rotation` / `Alpha` similarly (`setScaleX`, `setRotation`,
`setAlpha`).

For `animate_motion`, `ObjectAnimator.ofFloat(view,
"translationX", target).setDuration(d).start()`.

For `spring_motion`, `SpringAnimation(view,
DynamicAnimation.TRANSLATION_X).setSpring(SpringForce(target)
.setStiffness(...) .setDampingRatio(...)).start()`.

Cancel via `view.animate().cancel()` before starting a new tween
on the same view.

### Web binding detail

CSS custom property approach is the cleanest:

```css
/* injected once per bound element */
.motion-bound-{id} {
    transform: translateX(var(--motion-x, 0px));
}
```

```js
// per write
element.style.setProperty('--motion-x', `${value}px`);
```

The custom-property write is composited-only — no layout, no
paint, no JS round-trip on the renderer side. Browsers handle this
extremely well.

For `animate_motion`, prefer Web Animations API:

```js
element.animate(
    { '--motion-x': [`${current}px`, `${target}px`] },
    { duration: d, easing: '...' }
)
```

For `spring_motion`, WAAPI doesn't have native spring support;
fall back to a per-frame `raf_loop` writer running the same spring
math the framework uses elsewhere. Compositor still does the cheap
work — just the spring driver is JS.

## Stylesheet binding syntax

The author-facing surface needs a way to express "this transform
operand is a motion, not a static value." Type-system trick:

```rust
pub enum TransformOp {
    TranslateX(MotionOrStatic),
    TranslateY(MotionOrStatic),
    Scale(MotionOrStatic),
    Rotation(MotionOrStatic),
}

pub enum MotionOrStatic {
    Static(f32),
    Motion(MotionRef),  // type-erased handle
}

pub fn translate_x(v: impl IntoMotionOrStatic) -> TransformOp { ... }

impl IntoMotionOrStatic for f32 { fn into(self) -> MotionOrStatic { Static(self) } }
impl IntoMotionOrStatic for &Motion { fn into(self) -> MotionOrStatic { Motion(self.to_ref()) } }
```

Then `stylesheet! { transform: [translate_x(50.0), translate_y(&pan_y)] }`
type-checks uniformly. The style-apply path dispatches:

- `TransformOp::TranslateX(Static(v))` → one-shot
  `set_transform_x(node, v)` (existing path).
- `TransformOp::TranslateX(Motion(h))` → `bind_motion_to_property(node,
  TranslateX, &h)` once at apply-time; subsequent writes update the
  property natively without re-firing the style.

Same shape for `opacity`, `scale`, `rotation` — the limited set of
properties that backends can bind natively.

## Reactivity bridge

Motion values are NOT Signals — Signal writes are the slow path
this whole feature exists to skip. To bridge motion → reactive
code, two patterns:

### Boundary-sampled signal

```rust
let pan_x = Motion::new(0.0);
let resting_pan = pan_x.signal_sampled_on_settle();
// `resting_pan` is a Signal<f32>; updates only when a tween/spring
// completes or `animate_to` / `spring_to` is called with target =
// the current value (no-op tween for "publish to signal").
```

This is the recommended path. Pan during a gesture is invisible to
Rust; at gesture-end the author calls `motion.spring_to(0.0,
default_spring)` (or however the gesture ends) and the spring's
completion writes to the signal.

### Frame-rate sampling

```rust
let pan_x = Motion::new(0.0);
let live = pan_x.signal_sampled_each_frame();
```

Reads the motion on each `raf_loop` tick and writes to a Signal if
the value changed. Defeats the no-round-trip property but useful
when you genuinely need per-frame reactivity (rare — animated
overlays whose content depends on a gesture position).

Neither version is the default — authors opt into one or the other
explicitly. Sampling is a choice, not a free behavior.

## AAS / wire compatibility

Per [project_aas_state_snapshot](../crates/framework/core/MEMORY-aas.md):
when a fresh AAS client connects mid-session, it receives a
`SceneModel` snapshot.

Motion's wire story is the hard part of this design. Two options:

### Option A: AAS graphics-degraded

Match the existing `create_graphics` policy per
[project_aas_graphics_unsupported](../crates/framework/core/MEMORY-graphics.md).
Motion-bound transforms work fully locally but degrade to "static
value at the time of write" over AAS — the binding is replaced
with a one-shot transform write per `write_motion` call.

Pro: simple, predictable, matches existing policy.
Con: AAS visualizations of gesture-driven UIs look choppy or
miss the animation entirely.

### Option B: Per-frame batched writes

The dev-side runtime batches `write_motion` calls in a
`pending_writes: HashMap<MotionId, f32>` flushed once per
`raf_loop` tick. Wire commands grow `WriteMotion { id, value }`.
Device-side AAS runtime replays writes against its own native
motion handles.

`animate_motion` and `spring_motion` ship as single wire messages
(target + duration / spring config); the device-side runs the
animation natively. This is cheap.

Pro: AAS sees smooth motion.
Con: significant wire traffic for sustained gestures (60hz × N
motion writes), plus device-side has to maintain its own animator
for tween/spring.

### Recommendation

Ship **Option A first**. It matches existing AAS-feature-parity
policy and lets us land motion locally without designing a wire
batching scheme upfront. Revisit if AAS demos of gesture
interactions become a real product need.

Either way, `SceneModel` grows:

```rust
pub motions: Vec<MotionSnapshot>,
pub motion_bindings: Vec<MotionBindingSnapshot>,
```

so fresh clients can rehydrate handle ids + current values + bound
properties.

## Hard parts

1. **Stylesheet syntax type-system trick.** `translate_x(v_or_motion)`
   uniform for static numbers and motion refs is a `From<>`/`Into<>`
   exercise but the ergonomics matter — prototype before committing.
2. **iOS animator coordination.** Starting a new `CABasicAnimation`
   while a previous one is in-flight without a visible jump
   requires reading the presentation layer and starting the new
   tween from there. Standard pattern, but easy to get wrong.
3. **`read_motion` semantics on iOS.** Model layer (`layer.transform`)
   vs. presentation layer (`layer.presentationLayer.transform`). I'd
   argue model — that's what the next write composes from. Document
   clearly.
4. **wgpu transform plumbing.** The current renderer applies styles
   into the per-node uniform at apply-time. Motion-bound transforms
   need a parallel read-each-frame path. Not deep, but the renderer
   grows a branch.
5. **Composing motion with style transform.** If `style.transform =
   translateX(10)` and the node also has a motion bound to
   `TranslateX`, what's the final value? Three answers: motion
   replaces, motion adds, motion multiplies. I'd argue **replaces**
   for v1 — simplest mental model, matches Reanimated. Compose via
   multiple motions if you want addition.

## Build order

End-to-end this is 2–3 weeks. The API can land in the first 2 days
as wgpu-only and ship incrementally.

1. **Cross-platform types** in framework-core (~½ day):
   `Motion<f32>`, `AnimProperty`, `SpringConfig`, `MotionRef`,
   stylesheet builders for `translate_x` / `translate_y` / `scale` /
   `rotation` / `opacity`, the `IntoMotionOrStatic` trait. Plus
   `Backend` trait methods (defaults: no-op for write, panic for
   create + bind).
2. **wgpu implementation** (~1 day): `MotionHandle = Rc<Cell<f32>>`;
   renderer reads cells when computing transforms. Smoke-test with
   a "drag a card 120hz" demo.
3. **iOS implementation** (~2 days): direct `CALayer` write path,
   `CABasicAnimation` / `CASpringAnimation` for animated targets.
4. **Android implementation** (~2 days): direct `View.setX` calls,
   `ObjectAnimator` / `SpringAnimation` for animated targets.
5. **Web implementation** (~1 day): CSS custom property route +
   WAAPI for animated targets.
6. **AAS** (~1 day for Option A): degrade to static writes on the
   wire backend. `SceneModel` extensions.
7. **Reactivity bridges** (~½ day): `signal_sampled_on_settle()`
   and `signal_sampled_each_frame()` as opt-in adapters.
8. **Tests** as we go — unit-testable on the framework-core side
   (motion arithmetic, animation state machines); platform binding
   smoke-tested manually + via the demo apps.

## Definition of done

- `Motion::new(...)`, `motion.write(...)`, `motion.animate_to(...)`,
  `motion.spring_to(...)` all work on all four backends.
- Binding a motion to `translate_x` / `translate_y` / `scale` /
  `rotation` / `opacity` via the stylesheet macro produces native
  per-frame updates without firing the Rust style-apply path.
- A "drag a card with finger" smoke demo runs at 120Hz on each
  platform with no perceptible Rust contribution to per-event
  latency.
- `signal_sampled_on_settle()` produces a Signal that updates only
  on tween/spring completion.
- AAS clients see motion-bound transforms (either degraded per
  Option A or smooth per Option B).

## Out of scope (for v1)

- Color motions (animated background, foreground, etc.). Future
  story: `MotionColor` with separate handle type.
- Multi-scalar motions (animated 2D position as a single value).
  Compose two `Motion<f32>`s for now.
- Native physics-based gesture composition (e.g. "this motion is
  the integral of pan velocity"). Add when concrete UI needs it.
- Bindings to non-transform properties beyond `opacity` — border
  radius, shadow blur, etc. Add per-property as concrete UIs need
  them.
