# Animation

The framework's animation system is a value-handle + animator + clock
trio that drives per-frame updates from a single platform-agnostic
core. It runs on the UI thread, ticks only when work is in flight,
and dispatches finished values to the backend via the
`Backend::set_animated_*` family.

It's distinct from — and complementary to — the style-level
[`Transition`](styling.md) system. Transitions are declarative ("when
a style property changes, interpolate over N ms"); the animation
system is imperative and ownable ("I hold this value, drive it with
this spring, feed it to this property"). Use transitions for
hover/focus chrome. Use the animation system for gestures, springs,
custom motion, and anything that needs interruption with velocity
preservation.

Implementation: [`framework_core::animation`](../crates/framework/core/src/animation/).

---

## The end-to-end shape

```rust
use framework_core::animation::*;
use std::time::Duration;

let scale = AnimatedValue::new(1.0_f32);

// Subscribe the value to a backend property. Fires once
// immediately, then again every frame the clock advances.
let _sub = scale.subscribe_and_apply({
    let backend = backend.clone();
    let node = node.clone();
    move |v, _vel| {
        backend
            .borrow_mut()
            .set_animated_f32(&node, AnimProp::Scale, *v);
    }
});

// Press: tween toward 1.1 with ease-out.
scale.animate(TweenTo::new(1.1, Duration::from_millis(120)).ease_out());

// Release mid-flight: hand off to a spring. The spring inherits
// the tween's current (finite-difference) velocity, so motion
// continues smoothly across the swap.
scale.animate(SpringTo::new(1.0).stiffness(280).damping(22));
```

The `_sub` plumbing is by design — it lives in author code (or a
peripheral builder library) rather than core. The core surface is
the value handle and the animator factories; backend wiring is the
seam you control.

---

## Core abstractions

### `Animatable`

A trait describing values that can be interpolated and integrated:

```rust
pub trait Animatable: Clone + 'static {
    fn add_scaled(base: &Self, delta: &Self, scale: f32) -> Self;
    fn sub(a: &Self, b: &Self) -> Self;
    fn norm_sq(value: &Self) -> f32;
    fn zero() -> Self;
    fn lerp(a: &Self, b: &Self, t: f32) -> Self;  // default impl
}
```

`add_scaled` and `sub` are the two operations every animator needs:
`add_scaled(base, d, k) = base + d * k` (spring integration step and
tween interpolation) and `sub(a, b) = a - b` (displacement for spring
force, delta for tween lerp). `norm_sq` is the squared magnitude,
compared against a squared threshold so springs don't pay a `sqrt`
every frame.

Implementations ship for `f32`, the `f32` tuple shapes
(`(f32, f32)`, `(f32, f32, f32)`, `(f32, f32, f32, f32)`), and
const-generic `[f32; N]` arrays. Implementing for a custom struct
is mechanical — wire up the four required ops component-wise.

### `Animator<T>` and `Sample<T>`

```rust
pub trait Animator<T: Animatable>: 'static {
    fn sample(&mut self, dt: Duration) -> Sample<T>;
}

pub struct Sample<T: Animatable> {
    pub value: T,
    pub velocity: T,
    pub finished: bool,
}
```

An animator advances its own state by `dt` and reports the new
value, the instantaneous velocity in `T`-per-second, and whether
it's done. Velocity is the load-bearing detail: it's what makes
gesture handoff feel right.

After `finished: true` an animator must remain idempotent — repeated
samples return the same resting `(value, T::zero(), true)`.

### `AnimatorFactory<T>`

```rust
pub trait AnimatorFactory<T: Animatable> {
    fn build(self, current: T, velocity: T) -> Box<dyn Animator<T>>;
}
```

What authors construct at the call site. A factory describes the
*intent* (target + how to get there); the framework supplies the
current value and velocity at *attachment* time. This split is
what enables velocity-preserving handoff between animators.

### `AnimatedValue<T>`

The user-facing value handle. `Clone`-able; clones share state.

```rust
impl<T: Animatable> AnimatedValue<T> {
    pub fn new(initial: T) -> Self;
    pub fn get(&self) -> T;
    pub fn velocity(&self) -> T;
    pub fn set(&self, value: T);                       // snap; zero velocity
    pub fn animate<F: AnimatorFactory<T>>(&self, f: F);
    pub fn cancel(&self);                              // stop; preserve velocity
    pub fn is_animating(&self) -> bool;
    pub fn subscribe<F: FnMut(&T, &T) + 'static>(&self, f: F) -> Subscription<T>;
    pub fn subscribe_and_apply<F>(&self, f: F) -> Subscription<T>;
}
```

`set` snaps to a value, zeroes velocity, and cancels any in-flight
animator — use it for gesture-drag updates. `cancel` stops the
animator but preserves velocity, so a subsequent `animate` can
hand off cleanly. `subscribe_and_apply` fires the listener once
with the current state before registering — closes the mount-to-
first-tick gap when wiring to backends.

Listeners receive `(value, velocity)`. They may re-enter the same
value's API (`get`, `set`, `animate`, `cancel`, `subscribe`) — the
dispatch loop snapshots listener handles and uses `try_borrow_mut`
to skip recursive self-invocation silently. The only constraint
is that a listener can't invoke *itself* recursively.

### `AnimationClock`

Thread-local registry of live tick closures, in
[`animation::clock`](../crates/framework/core/src/animation/clock.rs).
Each `AnimatedValue` registers a tick closure on `animate(...)`; the
clock installs a single `Scheduler::raf_loop` and walks the closures
once per frame, unregistering each when its animator reports
`finished`. When the registry empties the `raf_loop` handle is
dropped — the system idles to zero per-frame work.

For tests there's `tick_for_test(dt)` that synchronously drives
the closures without a scheduler installed.

---

## Built-in animators

### `TweenTo` — duration + curve

```rust
TweenTo::new(target, Duration::from_millis(150))
    .ease_out()
```

Linear interpolation under a [`Curve`](#curves). Velocity is
computed by finite difference between consecutive samples, so a
tween handing off to a spring still produces correct momentum.
Does *not* preserve velocity on entry (the curve dictates motion);
for that, use `SpringTo`.

### `SpringTo` — damped harmonic oscillator

```rust
SpringTo::new(target)
    .stiffness(280)
    .damping(22)
```

Semi-implicit Euler integration. Inherits the value handle's
current velocity on `build` unless overridden via
`.initial_velocity(...)`. Defaults are `(stiffness: 170, damping:
26, mass: 1.0)` — the React Spring / Framer Motion default with
just-under-critical damping, slight overshoot, "alive" feel.

Settling thresholds (`rest_displacement`, `rest_velocity`) are
configurable. Defaults assume normalized 0..1 ranges; bump them for
pixel-space work.

### `DecayFrom` — velocity-driven fling

```rust
DecayFrom::new(release_velocity)
    .friction(3.0)
```

Closed-form exponential decay. No target — the resting value is
wherever momentum carries before friction wins. The pattern for
flick-scroll, toss-to-dismiss, swipe-decelerate. Frame-rate
independent (closed-form, not Euler-approximated).

---

## Composition

### `SequenceFactory.then(...)`

Run factories back-to-back. Velocity flows across segment
boundaries — a tween into a spring continues smoothly.

```rust
SequenceFactory::new()
    .then(TweenTo::new(1.0, Duration::from_millis(48)).linear())
    .then(SpringTo::new(1.0).stiffness(180))
```

When a segment finishes mid-frame, the sequence advances to the
next segment with the same `dt` slice. The advance loop caps at
`MAX_SEGMENTS_PER_FRAME` (64) to keep zero-duration segment chains
finite.

### `LoopFactory` + `Repeat`

Replay an inner factory `Times(N)` or `Forever`.

```rust
LoopFactory::new(
    SequenceFactory::new()
        .then(TweenTo::new(1.1, Duration::from_millis(120)).ease_out())
        .then(TweenTo::new(1.0, Duration::from_millis(120)).ease_in()),
    Repeat::Forever,
)
```

No autoreverse flag — express ping-pong as a two-segment sequence
inside a loop. Springs and decays have no canonical reverse, so
the explicit form is the only one that works for all factory
types.

### `KeyframesTo` — multi-stop waypoints

```rust
KeyframesTo::new(Duration::from_millis(400))
    .stop(0.0, 0.0)
    .stop(0.6, 1.1)
    .stop(1.0, 1.0)
    .curve(Easing::EaseOut)
```

Stops are `(offset, value)` pairs with offset in `0..=1`. The
framework sorts defensively. Unspecified `(0.0, seed)` is implicit
— the value handle's current value at build time is the starting
point — so `KeyframesTo::new(d).stop(1.0, target)` reads as
"tween to target via the curve."

### `Wait` and `SnapTo`

The connective tissue:

```rust
Wait::new(Duration::from_millis(200))    // hold value, finish after duration
SnapTo::new(0.0_f32)                     // instant set, finish in zero ms
```

Both compose inside sequences and loops. `SnapTo` is the rewind
primitive when you want every loop iteration to start from a known
value; `Wait` is how you express delays without separate API.

### `stagger(values, step_delay, factory_fn)`

Apply a per-index delay to a collection of values:

```rust
stagger(&card_scales, Duration::from_millis(40), |_i| {
    SpringTo::new(1.0).stiffness(220).damping(20)
});
```

Internally just a `for` over `(i, value)` that prepends
`Wait::new(step_delay * i)` to each factory.

---

## Curves

```rust
pub enum Easing {
    Linear, Ease, EaseIn, EaseOut, EaseInOut,
    CubicBezier(f32, f32, f32, f32),
}
```

Same vocabulary as the style-level transition system — same
file, in fact. The cubic-Bézier solver
([`animation::curve`](../crates/framework/core/src/animation/curve.rs))
is the canonical UI-grade Newton-Raphson approximation, hoisted
out of the wgpu renderer so every backend and the style system
agree on what `Easing::Ease` actually looks like.

Springs and decays are physics-driven and don't go through `Curve`.

---

## Velocity-preserving handoff

The behavioural feature that makes this system different from a
plain interpolation library.

When `animate(new_factory)` runs on a value that's already
animating, the framework:

1. Reads the value handle's current `(value, velocity)`.
2. Calls `new_factory.build(value, velocity)` — the factory
   receives the *current* state, not what it was at construction.
3. Replaces the animator. The first frame of the new animator
   reflects the inherited motion.

Concretely: a gesture drag updates the value via `set` each
frame. On release, the gesture system measures throw velocity and
calls `value.animate(SpringTo::new(rest_target).initial_velocity(v))`.
The spring's first frame moves *in the direction of the throw*,
then settles toward the target. That's the "iOS feels right"
behaviour, derived from one architectural seam.

`cancel()` stops the running animator but preserves velocity in
the value handle — useful when you want to defer animator
selection ("which spring depends on something we don't know yet")
but keep momentum alive for the eventual `animate(...)` call.

---

## Backend integration

### `AnimProp`

The vocabulary of animatable properties, in
[`animation::prop`](../crates/framework/core/src/animation/prop.rs):

```rust
pub enum AnimProp {
    // Scalar (f32)
    Opacity,
    TranslateX, TranslateY,
    Scale, ScaleX, ScaleY,
    RotateZ,

    // Color ([f32; 4] sRGB)
    BackgroundColor,
    ForegroundColor,
}
```

Split into scalar and color families; `is_scalar()` / `is_color()`
test which `Backend::set_animated_*` method receives the value.
Mis-routing (sending a color prop through the f32 path) is a
silent no-op — author bug, not a runtime crash.

### `Backend` trait methods

```rust
fn set_animated_f32(
    &mut self,
    node: &Self::Node,
    prop: AnimProp,
    value: f32,
);
fn set_animated_color(
    &mut self,
    node: &Self::Node,
    prop: AnimProp,
    value: [f32; 4],
);
```

Both default to no-op. Backends opt into animation support by
overriding; the contract is "translate one `AnimProp` to one or
more native property writes, every frame the value handle ticks."

#### Per-backend status

| Backend | Status | Composition strategy |
| --- | --- | --- |
| `backend-web` | All props | Modern CSS individual transform properties (`translate`, `scale`, `rotate`) — composed pair state for `translate` and `scale`, single scalar for `rotate` and `opacity`. |
| `backend-ios-mobile` | All props | Per-view `AnimatedTransformState` composes a `CGAffineTransform` (`T(tx,ty) * R(θ) * S(sx,sy)`) and writes via `setTransform:` on any transform-component update. Opacity → `setAlpha:`, colors → `setBackgroundColor:` / `setTintColor:`. |
| `backend-android-mobile` | All scalar + `BackgroundColor`; `ForegroundColor` best-effort | Android `View` exposes separate native properties (`translationX/Y`, `scaleX/Y`, `rotation`, `alpha`) so each `AnimProp` is a direct setter — no composition state needed. `ForegroundColor` maps to `setTextColor` which works on TextView subclasses and silently fails elsewhere. |

#### Static-style interaction

When both a static style (`transform: scale(0.5)`) and an animated
component (`AnimProp::Scale`) target the same node, the animated
write wins on web and iOS — inline CSS over class CSS on web,
`setTransform:` directly clobbers any prior matrix on iOS. To
preserve a static base, bind a value seeded at the static value.
A future iteration could read the static base at attach time and
compose; not yet done.

---

## Threading

Single-threaded by design. `AnimatedValue` is `Rc + RefCell` —
same model as `Signal`. The animation clock is a `thread_local!`
registry.

Off-thread animation comes for free from the platform compositors
(CSS transitions, Core Animation render server, Android
RenderThread) — and the `Backend::set_animated_*` design leaves
room for backends to delegate to those paths when the animation
is non-interruptible. Today no backend does this; the simplest
path was per-frame writes from the framework clock to native
setters, which works on every platform without needing a separate
acceleration path.

The off-thread escape hatch is in the design notes for a future
"native acceleration" tier; not implemented in the current
release.

---

## What's intentionally not in core

- **Per-primitive builder methods** (`view().opacity_animated(&v)`).
  These would touch the primitive enum and the walker. Best built
  as a peripheral library above core.
- **Automatic walker subscription lifetime**. Today the
  `Subscription` returned by `subscribe` / `subscribe_and_apply`
  is owned by the caller. A walker-integrated wrapper would tie it
  to the scope's lifetime.
- **Reanimated-style native-resident shared values** (the design
  in [`motion-value-plan.md`](./motion-value-plan.md)). That's a
  more ambitious tier that would skip the per-frame Rust→backend
  round-trip entirely for gesture-bound properties. The current
  system is a prerequisite — the value handle, animator factories,
  and `AnimProp` vocabulary are reused.

---

## File map

| File | Contents |
| --- | --- |
| [`animation/animatable.rs`](../crates/framework/core/src/animation/animatable.rs) | `Animatable` trait + impls for `f32`, tuples, `[f32; N]` |
| [`animation/curve.rs`](../crates/framework/core/src/animation/curve.rs) | `apply_easing` + cubic-Bézier Newton-Raphson solver |
| [`animation/animator.rs`](../crates/framework/core/src/animation/animator.rs) | `Animator` trait, `AnimatorFactory` trait, `Sample`, `MAX_FRAME_DT` |
| [`animation/tween.rs`](../crates/framework/core/src/animation/tween.rs) | `TweenTo` / `Tween` |
| [`animation/spring.rs`](../crates/framework/core/src/animation/spring.rs) | `SpringTo` / `Spring`, default constants |
| [`animation/decay.rs`](../crates/framework/core/src/animation/decay.rs) | `DecayFrom` / `Decay` |
| [`animation/combinators.rs`](../crates/framework/core/src/animation/combinators.rs) | `Wait`, `SnapTo`, `ErasedFactory`, `stagger` |
| [`animation/sequence.rs`](../crates/framework/core/src/animation/sequence.rs) | `SequenceFactory.then(...)` |
| [`animation/repeat.rs`](../crates/framework/core/src/animation/repeat.rs) | `LoopFactory` + `Repeat` enum |
| [`animation/keyframes.rs`](../crates/framework/core/src/animation/keyframes.rs) | `KeyframesTo.stop(offset, value)` |
| [`animation/prop.rs`](../crates/framework/core/src/animation/prop.rs) | `AnimProp` enum + family helpers |
| [`animation/clock.rs`](../crates/framework/core/src/animation/clock.rs) | Per-thread tick registry + `tick_for_test` |
| [`animation/value.rs`](../crates/framework/core/src/animation/value.rs) | `AnimatedValue<T>` + `Subscription<T>` |
