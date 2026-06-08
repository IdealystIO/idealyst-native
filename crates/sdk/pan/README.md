# `pan`

A sensible, minimal pan / drag gesture that works the same on desktop, web,
and touch screens — because the framework already converges pointer input
below it. A left-button mouse drag, a trackpad drag, a pen drag, and a finger
drag all arrive as the identical `Began → Moved* → Ended` sequence, and the
output binds to a view's native translate on every backend. There is no
per-platform code in this crate.

## What it gives you

A `Pan` handle that owns a **reactive 2D offset** (`x`, `y` as
`AnimatedValue<f32>`) and:

- tracks the finger while dragging (`offset = base + delta`),
- snapshots `base` at each gesture start, so successive grabs accumulate
  instead of snapping back to the origin,
- cancels any in-flight offset animation when you grab, so a fling never
  fights the drag,
- exposes `on_start` / `on_change` / `on_end` / `on_cancel` lifecycle hooks,
- binds straight to a view's translate with `pan.bind(view_ref)`.

## What it deliberately leaves to you

Momentum / fling, snap points, axis locking, bounds clamping, and
swipe-to-dismiss are **policy** — they live in your app or a higher-level SDK,
built on the methods here. The offset is an `AnimatedValue`, so momentum is one
line in `on_end`; snapping and dismissal are a comparison on `PanEnd`.

## Usage

```rust
use pan::Pan;
use runtime_core::animation::DecayFrom;
use runtime_core::{view, Ref, ViewHandle};

fn draggable_card() -> Element {
    let view_ref: Ref<ViewHandle> = Ref::new();

    let pan = Pan::new();
    // Momentum is your call — handed off via the exposed AnimatedValue.
    let momentum = pan.clone();
    let pan = pan.on_end(move |end| {
        momentum.x().animate(DecayFrom::new(end.velocity.x).friction(3.5));
        momentum.y().animate(DecayFrom::new(end.velocity.y).friction(3.5));
    });

    pan.bind(view_ref); // x → TranslateX, y → TranslateY

    view(vec![/* ... */])
        .on_touch(pan.handler())
        .bind(view_ref)
        .into()
}
```

Call `pan.bind(view_ref)` during render, inside the active reactive scope —
the binding anchors to that scope and unbinds when it drops, exactly like any
`AnimatedValue::bind`. On the web backend, the app must have called
`backend_web::install_global_self(&backend)` at startup for animated bindings
to take effect (a standard app-bootstrap step, not specific to this SDK).

## Constraining, snapping, dismissing

All of these are a few lines on top of the hooks — kept out of the SDK on
purpose:

```rust
// Axis lock (horizontal only): ignore the y offset by binding only x.
pan.x().bind(view_ref, AnimProp::TranslateX);

// Clamp into bounds on every move.
let p = pan.clone();
let pan = pan.on_change(move |info| {
    let clamped = info.offset.x.clamp(0.0, 200.0);
    if clamped != info.offset.x { p.set_offset(clamped, info.offset.y); }
});

// Swipe-to-dismiss past a velocity / distance threshold.
let pan = pan.on_end(move |end| {
    if end.velocity.x.abs() > 800.0 || end.offset.x.abs() > 150.0 {
        // dismiss(...)
    } else {
        // spring back: pan.x().animate(SpringTo::new(0.0))
    }
});
```

No permissions required.
