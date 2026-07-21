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
let count = signal(0_i32);
count.set(1);
let value = count.get(); // 1
```

Inside `ui!`, signals participate in reactivity automatically:

```rust
ui! {
    view {
        text { "Count: {count}" }
        button("+1", move || count.update(|v| *v += 1))
    }
}
```

`{count}` interpolates live because `count` is a *signal* — f-string slots
classify by the value's type (a plain `Display` value would bake in
statically), and signal slots get the web backend's optimized binding
automatically. The `button`'s on-click is a regular closure that captures the
signal. Reactive text can also be written as a closure —
`text(move || format!("Count: {}", count.get()))` — the general form for
positional/Debug formatting or arbitrary expressions. (The 0.2-era
`text_fmt!`/`bind!` macros were removed in 0.3 — f-strings produce the same
optimized binding without the sentinel.)

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

> Using `effect!` outside a scope panics in debug builds (it's a sign the logic should either move into a component or use `watch`). The raw `Effect::new` constructor is private to `runtime_core` (`pub(crate)`) — writing it is a compile error; `effect!` and `watch` are the surface.

Pair either form with `on_cleanup(…)` for teardown — the callback fires before the next re-run *and* on disposal.

## Animations

[[AnimatedValue]] is the per-frame motion handle. Construct one with `animated!`:

```rust
let opacity = animated!(0.0_f32);
opacity.animate(TweenTo::new(1.0, Duration::from_millis(400)).ease_out());
```

Use `animate_at!` to schedule animations at a specific offset, or `timeline!` for choreographed sequences.

## Three pitfalls

1. **Don't `.get()` outside an effect** unless you want the current value once. Inside a reactive context (closure inside `ui!`, `effect!`, an f-string slot), `.get()` registers a dependency.
2. **The hoisted-snapshot trap** — pitfall 1 in disguise, and the easiest bug to ship:

   ```rust
   let too_short = name.get().len() < 3;   // runs ONCE at build — frozen bool
   ui! { if too_short { … } }              // static branch: silently never updates
   ```

   The component body runs once and is not a tracked context. Keep derivations behind `move ||` — `let too_short = memo(move || name.get().len() < 3);` gives a live `ReadSignal<bool>` condition — or inline the read (`if name.get().len() < 3` is auto-promoted to a reactive branch). **A `let` freezes, a closure flows.** Debug builds warn at runtime when this happens (naming the component), and the `snapshot-condition` lint flags the hoisted-`let` form — inline forms inside `ui!` (including a `for` header, next pitfall) are NOT linted; verify list reactivity by running the app. For an *intentional* build-time snapshot, declare it: `.get_untracked()` reads without subscribing and silences both diagnostics.

4. **Lists: iterate the signal, not `.get()`.** `for item in items.get()` inside `ui!` iterates a build-time snapshot — the loop renders once and NEVER re-runs, even though `items` is a signal. Unlike `if` headers, a `for` header's `.get()` is NOT auto-promoted. The reactive form iterates the `Signal<Vec<T>>` itself, with a key for reconciliation:

   ```rust
   for item in items, key = item.id { Row(item = item) }
   ```

   See [[components]] § Rendering collections and `describe_recipe("keyed_list_add_remove")` for the full working form.
3. **`HashMap::get()` is not a signal read** — the reactivity detector keys on `.get()` calls and false-positives benignly here. Don't worry about it; it just means an extra effect run that immediately settles.

## See also

- [[concepts|Primitives, Components, Style]] — the structural layer signals operate on.
- [[primitives|Primitives reference]] — every primitive's reactive props.
- The [[Signal]] type entry for the full method surface.
