+++
title = "Reactivity: Signals, Effects, Bindings"
order = 30
tags = ["core", "reactivity"]
+++

# Reactivity

Idealyst's reactive layer is signal-based, similar in shape to SolidJS or Leptos but adapted for cross-platform native rendering.

## Signals

`Signal<T>` is a reactive cell. Read via `.get()`, write via `.set(v)` or `.update(|v| …)`.

```rust
let count = signal!(0_i32);
count.set(1);
let value = count.get(); // 1
```

Inside `ui!`, signals participate in reactivity automatically:

```rust
ui! {
    View {
        Text(text_fmt!("Count: {}", bind!(count)))
        Button(label = "+1", on_click = move || count.update(|v| *v += 1))
    }
}
```

The `bind!(count)` form tells `text_fmt!` to track changes; the `Button`'s `on_click` is a regular closure that captures the signal.

## Effects

`effect!(|| …)` runs a closure now and re-runs it whenever any signal it reads changes. Most authors don't reach for `effect!` directly inside components — `ui!` already wraps reactive parts in effects — but it's the building block underneath.

## Animations

[[AnimatedValue]] is the per-frame motion handle. Construct one with `animated!`:

```rust
let opacity = animated!(0.0_f32);
opacity.animate(TweenTo::new(1.0, Duration::from_millis(400)).ease_out());
```

Use `animate_at!` to schedule animations at a specific offset, or `timeline!` for choreographed sequences.

## Two pitfalls

1. **Don't `.get()` outside an effect** unless you want the current value once. Inside a reactive context (closure inside `ui!`, `effect!`, `text_fmt!`), `.get()` registers a dependency.
2. **`HashMap::get()` is not a signal read** — the reactivity detector keys on `.get()` calls and false-positives benignly here. Don't worry about it; it just means an extra effect run that immediately settles.

## See also

- [[concepts|Primitives, Components, Style]] — the structural layer signals operate on.
- [[primitives|Primitives reference]] — every primitive's reactive props.
- The [[Signal]] type entry for the full method surface.
