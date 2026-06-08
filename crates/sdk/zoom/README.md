# `zoom`

A sensible, minimal zoom gesture — the scale peer of the [`pan`](../pan) SDK.
It converges the two ways a user expresses "zoom" into **one reactive scale
factor + focal point**, and binds straight to a view's scale:

- **Pinch** — two fingers on a touch screen (iOS, Android, web-touch,
  macOS-touch), via the framework's `pinch` recognizer. Rides the existing
  touch stream, so no per-platform code.
- **Trackpad pinch + scroll-wheel** — desktop, via the framework's wheel
  channel (web `wheel`+`ctrlKey`, macOS `magnify:`). Each backend normalizes
  its native zoom signal into a uniform incremental scale, so app code carries
  no per-platform constant.

Both feed the same `AnimatedValue<f32>`. A pinch on a phone and a trackpad
pinch on a laptop move the identical value.

## What it gives you

A `Zoom` handle that owns the scale (`AnimatedValue<f32>`, starts at `1.0`) and:

- tracks a pinch as `base * gesture_scale`, snapshotting `base` at each
  gesture start so successive pinches **compound** instead of resetting,
- multiplies the scale on each trackpad-pinch / ctrl-wheel event,
- cancels any in-flight scale animation when a new gesture starts,
- reports the **focal point** (pinch midpoint / cursor) on every event,
- exposes `on_start` / `on_change` / `on_end` / `on_cancel`,
- binds to a view's scale with `zoom.bind(view_ref)`.

## What it deliberately leaves to you

Min/max clamping, momentum, snap-to-fit, and focal-point "zoom about the
cursor" translation are **policy** — left to your app or a higher-level SDK,
built on the methods here. The scale is an `AnimatedValue`, so momentum is one
line in `on_end`; clamping is a comparison in `on_change`; and the reported
focus lets you pair this with a `pan` offset to keep the point under the
fingers fixed.

## Usage

```rust
use zoom::Zoom;
use runtime_core::{view, Ref, ViewHandle};

fn pinch_zoomable() -> Element {
    let view_ref: Ref<ViewHandle> = Ref::new();

    let zoom = Zoom::new().on_change(|info| {
        // info.scale, info.focus, info.velocity — clamp / translate here.
    });
    zoom.bind(view_ref); // scale → AnimProp::Scale

    view(vec![/* ... */])
        .on_touch(zoom.pinch_handler())  // touch screens
        .on_wheel(zoom.wheel_handler())  // desktop trackpad / scroll-wheel
        .bind(view_ref)
        .into()
}
```

Call `zoom.bind(view_ref)` during render, inside the active reactive scope —
like any `AnimatedValue::bind`. On web, the app must have called
`backend_web::install_global_self(&backend)` at startup for animated bindings
to take effect (a standard bootstrap step, not specific to this SDK).

## Clamping, momentum, focal zoom

All policy, kept out of the SDK on purpose:

```rust
// Clamp into [1, 4] on every change.
let z = zoom.clone();
let zoom = zoom.on_change(move |info| {
    let clamped = info.scale.clamp(1.0, 4.0);
    if clamped != info.scale { z.set_scale(clamped); }
});

// Momentum: fling the scale on release.
let z = zoom.clone();
let zoom = zoom.on_end(move |end| {
    z.scale().animate(DecayFrom::new(end.velocity).friction(4.0));
});

// Focal "zoom about the cursor": pair with a `pan` offset and translate by
// focus * (1 - scale) in on_change.
```

No permissions required.
