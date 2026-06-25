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

A reactive *effect* runs a body now and re-runs it whenever any signal it read changes. There is no dependency array — dependencies are tracked automatically from the `.get()` calls in the body. Idealyst splits this into two forms by **where the effect lives**:

### `effect!` — inside the component tree

```rust
effect!({
    log("count is {}", count.get());
});
```

`effect!({ … })` is for reactivity **inside a component body** (or another active reactive scope). The surrounding scope owns the effect and frees it on teardown, so there is no handle to manage. It debug-asserts that a scope is active. Most authors rarely reach for it directly — `ui!` already wraps reactive parts in effects — but it's the building block underneath. Wraps the body in `move ||` for you, so `Copy` signal handles are captured by value.

### `watch` — outside the tree

```rust
// app init, an async callback, a platform/service install …
let sub = watch(move || apply_class(is_open.get()));
// `sub` is a `Subscription` — store it; dropping it disposes the effect.
```

`watch(f) -> Subscription` is the counterpart for reactivity wired up **outside** the component tree, where no scope exists to own the effect: app bootstrap, async callbacks, library/SDK setup. The returned `Subscription` is **caller-owned** — keep it alive by storing it (a struct field, a thread-local, the owning service); dropping it disposes the effect and runs its `on_cleanup` callbacks. For a one-time install that should live for the whole process, call `Subscription::leak()` — the honest, greppable "pin forever".

> Using `effect!` outside a scope panics in debug builds (it's a sign the logic should either move into a component or use `watch`). Don't reach for the raw `Effect::new` constructor — it's sealed; `effect!` and `watch` are the surface.

Pair either form with `on_cleanup(…)` for teardown — the callback fires before the next re-run *and* on disposal.

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
