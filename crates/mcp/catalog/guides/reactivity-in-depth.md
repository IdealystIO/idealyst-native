+++
title = "Reactivity In Depth"
order = 32
tags = ["core", "reactivity", "advanced", "patterns"]
+++

# Reactivity In Depth

[[reactivity]] covers the everyday surface — `signal`, `effect!`, `watch`.
This guide is the layer beneath it: the derived-state primitives, the tracking
controls, the reactive control-flow builders, and the sharp edges you only hit
sometimes but need to recognize when you do. The kernel is
`crates/runtime/world/src/lib.rs` (`signal`, `effect`, `memo`, `untrack`,
`on_cleanup`, `provide`/`inject`) and the author surface that wraps it is
`crates/runtime/vocabulary/src/glue.rs` (`memo_with`, `reducer`, `watch`,
`when`/`switch`) plus `async_reactive.rs` (`resource`, `mutation`); everything
is re-exported under `runtime_core::…`. Reach for these when the plain
signal/effect pair isn't enough.

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

### Coalescing — automatic, and there is no `batch` to call

`a.set(…); b.set(…); c.set(…);` already fans out to each subscriber **once**.
A write *stages* a pending value; the world commits every staged write and runs
the affected effects in one **flush** at the end of the turn, so intermediate
states are never observed. There is no explicit `batch(f)` on the author
surface — the writes inside one already coalesce.

Every backend installs a **flush driver** that closes the turn: the capability
impls wrap each author callback (press, input, toggle, scroll, key, …) and call
`schedule_flush()` after it returns, and a post-dispatch hook does the same for
`after_ms` timers, animation-frame callbacks, and async continuations. So a
handler that writes five signals produces one commit, one deduped effect pass,
and one paint.

Change detection happens **at commit**, against the committed value: a signal
set `A → B → A` within one turn nets to no change and never wakes its
subscribers. `set_always` / `touch` force-taint the entry, so the flush
notifies regardless of the net comparison.

> Edge: reads see the **committed** value, so `s.set(v)` followed by `s.get()`
> in the same handler still returns the old value. `update(|cur| …)` is the
> exception — it composes on the staged value, which is why two increments in
> one turn net `+2`.

Full model: the `automatic-batching` design doc
(`docs/automatic-batching.md`).

## Derived-state machines

### `reducer` — action-dispatched state

`reducer(initial, |&state, action| next_state)` returns `(Signal<S>, dispatch)`.
Each `dispatch(action)` folds against the **staged** value, so two dispatches in
one turn compose (`0 → 1 → 2`) instead of both reading the committed `0`, and it
always notifies — a fold back to an equal state still wakes subscribers, even
though the underlying write is equality-guarded. It never subscribes the caller,
so dispatching from inside an effect doesn't make that effect depend on the
state.

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

These build `Element`s whose subtree the scene's structural drivers rebuild
reactively. All three
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
  `Element` — realize splices the children into the parent with no wrapper
  view (so `flex: 1` / absolute overlays aren't broken by a box). Built once,
  **not** reconciled — for a reactive child set use `switch`/`when`/keyed `for`
  (`for item in items, key = item.id` — the SIGNAL itself in the header, never
  `items.get()`, which freezes a build-time snapshot; see the
  `keyed_list_add_remove` recipe).

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

- **`set` is equality-guarded — a same-value write wakes nobody.** The default
  `.set(v)` (`T: PartialEq`) skips the fan-out when the value is unchanged.
  Code that used a same-value write as a *retrigger* (re-firing a `switch`
  discriminant, forcing a re-render) must say so explicitly: `touch()`
  notifies without writing, `set_always(v)` writes and always notifies (and is
  the only setter for non-`PartialEq` types). `set_untracked(v)` is the
  inverse — write silently, notify never. `update(|&cur| next)` is guarded the
  same way, and composes on the *staged* value, which is why two increments in
  one turn net `+2`. This deliberately diverges from Leptos, whose `set` never
  compares.
- **A write after the world is gone is a no-op; a stale handle in a live world
  panics.** An async callback completing after its world dropped writes
  harmlessly into the void (reads from a dead world do panic). But a write
  through a handle whose slot was recycled *while the world is still alive* is
  a use-after-unmount logic error, and panics rather than silently poking the
  slot's new occupant.
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
  condition dispatches to the reactive path by type), or inline the read into
  the condition — under the 0.4.0 inverted gate any `ui!` `if`/`match` condition
  that *might read a signal* (a `.get()`, or **any** call anywhere in the
  condition — `if name.get().len() < 3`, `if is_short(name)`) lowers to a
  reactive `when`/`switch`; only a *provably signal-free* condition (a literal,
  a bare `bool` path, or a comparison of call-free operands) stays static. Rule
  of thumb: **a `let` freezes, a closure — or a call in the condition — flows.** Debug builds warn at runtime on an untracked
  `.get()` during a component build (naming the component), and the
  `snapshot-condition` lint flags the pattern at the `let`. Build-time
  snapshots are legitimate when intentional — a structural choice that
  shouldn't rebuild — declare them with `.get_untracked()` (reads without
  subscribing, silences both diagnostics).
- **A mutual write-loop panics, it doesn't hang.** A > B > A cascade trips the
  flush's round limit (100 outer rounds) with a recognizable backtrace instead
  of spinning forever.
- **Reactive text interpolation is an f-string literal** (`text { "count:
  {count}" }`) — slots are live by TYPE. For a live prop value, use `rx!` or
  pass the signal itself.
- **On wasm, don't self-rewake a task in a poll loop.** A future that
  `wake_by_ref()`s itself and returns `Pending` spins the single-threaded event
  loop so DOM events never fire (the tab freezes). Park the task and wake it
  from the event instead.

See [[idiomatic-components]] for how these plug into a component body, and
[[reactivity]] for the everyday surface.
