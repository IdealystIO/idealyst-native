+++
title = "Reactivity In Depth"
order = 32
tags = ["core", "reactivity", "advanced", "patterns"]
+++

# Reactivity In Depth

[[reactivity]] covers the everyday surface — `signal`, `effect!`, `watch`.
This guide is the layer beneath it: the derived-state primitives, the tracking
controls, the reactive control-flow builders, and the sharp edges you only hit
sometimes but need to recognize when you do. Everything here is in
`crates/runtime/core/src/reactive.rs` (and `builder.rs` for the control-flow
constructors); reach for these when the plain signal/effect pair isn't enough.

## Derived state

### `memo` — cached, change-gated derivation

`memo(move || expr)` returns a `ReadSignal<T>` that recomputes when the signals
the closure reads change, and **notifies subscribers only when the new value
differs** (`T: PartialEq`). Use it for derived state read in several places or
expensive to compute — the work runs once per dependency change, not once per
read. (A `memo!` macro used to exist; it was removed — write the `move ||`
yourself.)

```rust
let count = signal(0);
let doubled = memo(move || count.get() * 2);   // ReadSignal<i32>, cached until count changes
```

The return is the **read half only** — a memo is a pure derivation, so its
output cannot be `.set()` (the old writable return invited values that were
silently clobbered on the next dependency change).

### `split` / `read_only` / `write_only` — capability halves

Any `Signal<T>` can hand out capability-restricted views over the same slot:

```rust
let (count, set_count) = signal(0).split();   // (ReadSignal<i32>, WriteSignal<i32>)
let view_of = my_state.read_only();           // observe-only handle
let report  = my_state.write_only();          // write-only handle (reads impossible)
```

Semantics are unchanged — same tracking, same generational stale-write no-op,
still `Copy`; only the TYPE narrows. Use `ReadSignal<T>` for props a component
observes without mutating (the signature then *proves* it), `WriteSignal<T>`
for children that report values upward without subscribing themselves. The
unified `Signal<T>` stays the right type for genuinely two-way props
(`TextInput.value` and friends).

- For a type without `PartialEq`, or a "close enough" comparison (float
  tolerance, trait-object contents), call `memo_with(eq, f)` directly.
- A memo's body must be a **pure derivation** — calling `.set()`/`.update()`
  inside it panics loudly (a `MemoComputeGuard` catches the side effect).
- For a cheap one-off derivation, a plain closure or `rx!` is lighter than a
  memo — don't cache what's trivial to recompute.

### `rx!` — an inline reactive prop value

`rx!(expr)` wraps a computed expression as a live `Reactive<T>` (it expands to
`Reactive::derive(move || expr)`). It's the reactive-prop analog of an f-string text slot:
reach for it when a component's `Reactive<T>` prop should track a *computed*
expression rather than a bare signal (a bare `Signal` is already live via
`IntoProp`, no `rx!` needed).

```rust
Typography(content = rx!(format!("clicked {}×", count.get())))
```

### `derived(...)` — the style-sink cousin (don't confuse them)

`derived(f)` (from `style.rs`) produces a `Derive<F>` **variant source** for a
stylesheet sink — this is the one you saw in `divider.rs`/`stack.rs` routing a
reactive style axis. It is a different type from the reactive-condition
`Derived<T>` that `when`/`switch` consume. Rule of thumb: `derived(...)` at a
`sheet.axis(...)` call site = style variant; `rx!`/`memo(...)` = reactive value.

## Tracking control

### `untrack` — read without subscribing

`untrack(|| signal.get())` reads current values **without** subscribing the
enclosing effect to them. Use it when an effect needs a value but shouldn't
re-run when that value changes — e.g. reading "current" state inside a
dispatcher that's meant to *cause* changes, not react to them.

### `batch` — coalesce writes into one fan-out

`batch(|| { a.set(…); b.set(…); c.set(…); })` records all writes and fans out
to each subscriber **once**, in first-write order, when the outer batch
returns. Intermediate states are never observed by any effect. Nested batches
join the outermost window and don't flush early.

You rarely call `batch` by hand: **handlers are born batched.** A closure
attached through a core builder (`pressable`, `Bound::on_*`, `reducer`'s
dispatch) is wrapped in a `cycle` at the point the backend invokes it, so two
writes in one tap wake a shared subscriber once. Reach for explicit `batch`
only for multi-write sequences *outside* an event handler.

> Edge: an effect body that calls `.set()` *during* a flush sees the batch as
> already over — that write fans out synchronously, it doesn't fold back into
> the window being drained.

## Derived-state machines

### `reducer` — action-dispatched state

`reducer(initial, |&state, action| next_state)` returns `(Signal<S>, dispatch)`.
Each `dispatch(action)` runs as one reactive cycle (coalescing sibling writes),
and reads current state under `untrack` so dispatching from inside an effect
doesn't subscribe that effect to the state.

```rust
let (count, dispatch) = reducer(0i32, |&n, a| match a {
    Msg::Inc => n + 1,
    Msg::Reset => 0,
});
button("+", move || dispatch(Msg::Inc));
```

### `resource` — async derived state

`resource(deps, fetcher)` drives an async fetch keyed on reactive `deps`,
exposing a `Resource<T, E>` whose status is `Loading`/`Error`/`Success`/`Idle`
(precedence `Loading > Error > Success > Idle`; a refetch-while-stale collapses
to `Loading`). Use it for data that depends on signals and lives off a future.

## Context — `provide` / `inject`

Pass values down the scope tree without threading props:

```rust
provide(Theme::dark());                       // at a scope root
let theme: Option<Theme> = inject::<Theme>(); // anywhere in the subtree
let locale = inject_or(Locale("en-US".into()));
with_inject::<Theme, _>(|t| t.background);    // borrow, no clone
```

- Keyed by Rust **type** — disambiguate two providers of the same underlying
  type with distinct newtypes (`struct PrimaryColor(Color)` vs
  `struct AccentColor(Color)`).
- Inner provisions shadow outer ones for their subtree.
- `provide` **panics** outside any active scope or inside a memo compute.

## Reactive control flow

These build `Element`s whose subtree the walker rebuilds reactively. All three
follow the **dispose-on-hide** model: when a branch goes away, its effects are
dropped and its signal subscriptions released — **state in the hidden branch is
lost.**

- `when(cond, then, otherwise)` — two-way. `cond` reads signals and returns
  `bool`; the active branch rebuilds from scratch on change.
- `switch(scrutinee, |&key| …)` — multi-way. `scrutinee` returns any
  `PartialEq + 'static` value (usually an enum); the subtree rebuilds only when
  the key actually changes. `ui!`'s `match` lowers to this — write a normal
  `match` in the macro and it emits `switch`.
- `fragment(children)` — a **layout-transparent** sibling group. Return it from
  a `#[component]` that conceptually yields several siblings but must return one
  `Element` — the walker splices the children into the parent with no wrapper
  view (so `flex: 1` / absolute overlays aren't broken by a box). Built once,
  **not** reconciled — for a reactive child set use `switch`/`when`/`for`.

## Lifecycle — `on_cleanup`

Register teardown for the surrounding reactive context:

```rust
effect!({
    let task = after_ms(500, || tick());
    on_cleanup(move || drop(task));   // fires before next re-run AND on disposal
    deps.get();
});
```

- Inside an effect: fires **before the next re-run** and **on disposal** —
  release timers/listeners/in-flight requests acquired last pass.
- Inside a scope (no active effect): fires once when the scope drops.
- Outside any reactive context: dropped immediately (a top-level no-op).

## Out-of-tree reactivity — `watch` / `Subscription`

`watch(f) -> Subscription` is the counterpart to `effect!` for reactivity wired
up *outside* the component tree (app init, async callbacks, platform installs),
where no scope owns the effect. The `Subscription` is **`#[must_use]` and
caller-owned**: store it where its lifetime should match, or call `.leak()` for
a deliberate, greppable process-lifetime pin. Dropping it disposes the effect
and runs its cleanups.

```rust
self.insets_sub = Some(watch(move || apply_insets(safe_area_insets().get())));
```

The raw `Effect::new` constructor is `pub(crate)` — author code can't call it.
`effect!` (in-tree) and `watch` (out-of-tree) are the only surface, and
`idealyst lint`'s `prefer-effect-macro` flags a raw `Effect` where the macro was
intended.

## Sharp edges worth knowing

- **Stale `set` is a safe no-op, not a panic.** Signal handles are generational
  — a `.set()`/`.update()` on a handle whose slot was recycled (or whose owning
  scope was torn down) silently does nothing. This is why a deferred/async
  callback firing after unmount doesn't crash.
- **Never `.set()` inside a `Ref::with` / `handle.with` closure.** That closure
  holds the arena borrow; a signal write inside it aborts with *"RefCell already
  borrowed"* (the `is_reactive_busy` guard does **not** catch this). Read the
  value out, close the `with`, then `set` after.
- **`.get()` outside a reactive context is a one-shot read.** It returns the
  current value and subscribes nothing. Subscription only happens inside an
  effect / `ui!` closure / f-string slot / `rx!` body.
- **The hoisted-snapshot trap.** The commonest form of the previous point,
  and it *looks* reactive:

  ```rust
  let too_short = name.get().len() < 3;   // runs ONCE at build — frozen bool
  ui! { if too_short { … } }              // static branch, silently never updates
  ```

  The component body is not a tracked context, so a derivation hoisted into a
  plain `let` is a build-time snapshot; the `if` then dispatches on a plain
  `bool` → static. Keep derivations behind `move ||`:
  `let too_short = memo(move || name.get().len() < 3);` (a `ReadSignal<bool>`
  condition is live), or inline the read — a visible `.get()` in a `ui!` `if`
  condition is auto-promoted to a reactive `when`. Rule of thumb: **a `let`
  freezes, a closure flows.** Debug builds warn at runtime on an untracked
  `.get()` during a component build (naming the component), and the
  `snapshot-condition` lint flags the pattern at the `let`. Build-time
  snapshots are legitimate when intentional — a structural choice that
  shouldn't rebuild — declare them with `.get_untracked()` (reads without
  subscribing, silences both diagnostics).
- **A mutual write-loop panics, it doesn't hang.** A > B > A cascade trips the
  `MAX_EFFECT_DEPTH` (256) guard with a recognizable backtrace instead of a
  stack overflow.
- **Reactive text interpolation is an f-string literal** (`text { "count:
  {count}" }`) — slots are live by TYPE. For a live prop value, use `rx!` or
  pass the signal itself.
- **On wasm, don't self-rewake a task in a poll loop.** A future that
  `wake_by_ref()`s itself and returns `Pending` spins the single-threaded event
  loop so DOM events never fire (the tab freezes). Park the task and wake it
  from the event instead.

See [[idiomatic-components]] for how these plug into a component body, and
[[reactivity]] for the everyday surface.
