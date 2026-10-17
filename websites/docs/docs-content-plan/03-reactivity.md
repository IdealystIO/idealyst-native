# Reactivity

This page is the long version of what the Overview introduced. If
you only need the gist — "a signal notifies the small places that
read it" — the Overview already has it. This page is for when you
want to know the full surface.

## The model in one paragraph

Idealyst's reactivity is one mechanism applied uniformly. A signal
holds a value. When a closure reads the signal inside a tracked
context, the framework records the dependency. When the signal
changes, the framework re-runs every tracked context that read it,
and only those. There is no virtual DOM, no diff, no top-down
re-render. State, derived values, styles, themes, conditional
rendering, list contents, and navigation all use this same mechanism
underneath.

## Signals

### Making a signal

```rust
use runtime_core::signal;

let count = signal(0);
let name = signal(String::from("Ada"));
let items = signal(Vec::<Item>::new());
```

`signal(v)` allocates a slot in the enclosing world's arena. The
`Signal<T>` you hold back is a small `Copy` handle (three `u32`s:
world, slot, generation) that routes to that slot. This is why you
can pass signals into closures and child components without ever
calling `.clone()`.

### Reading and writing

```rust
let n = count.get();          // tracked read
count.set(7);                 // stage a replacement value
count.update(|n| n + 1);      // read-modify-write on the staged value
```

`.get()` returns a clone of the current value, so it needs
`T: Clone`; every signal also needs `T: PartialEq + 'static`.
`.set(v)` stages a replacement. `.update(|cur| next)` takes the
current value by reference and returns the next one.

Writes **stage**: nothing observable changes until the world
flushes, and a `.get()` on the next line still returns the committed
value. The flush runs automatically when your handler returns. Use
`update` for read-modify-write — `set(get() + 1)` twice in one
handler nets +1, `update(|n| n + 1)` twice nets +2. `.set` is
equality-guarded: a staged value equal to the committed one notifies
nobody. See **The flush** below.

### Identity

```rust
let id: u64 = count.raw_id();
```

Every signal has a stable identity key. You rarely need this — it's
a key for external registries (the web backend's JS text bindings
use it) and can't be turned back into a handle. If you find yourself
reaching for `.raw_id()` from app code, there's probably a more
direct API you want instead.

> **From React.** A `Signal<T>` is a small Copy token. You don't
> wrap it in `useRef` to escape the closure rules, you don't pass
> it through `useCallback` to keep referential equality stable —
> there are no closure rules and no equality dance. The signal is
> the same value everywhere it appears.

> **From Solid.** Signals here behave like `createSignal`, but the
> getter/setter is unified — `count` is the signal, `count.get()`
> reads, `count.set(...)` writes. No separate `[count, setCount]`
> tuple.

> **From Svelte 5.** A signal here is what `$state` produces. The
> ergonomic difference is that Svelte's runes are compiler-rewritten
> inside `.svelte` files — `let count = $state(0); count++` reads
> like a plain variable. Idealyst signals are plain Rust values, so
> reads and writes are method calls.

## What gets tracked

Anywhere a signal is read inside a **tracked context**, the
framework records the dependency. The tracked contexts in everyday
code are:

- **Reactive text** — `text { format!("count: {}", count.get()) }`.
  The expression is wrapped in an Effect that re-fires on signal
  change.
- **Closure props** — `label = move || format!("...", count.get())`.
  A closure passed to a reactive prop is wrapped in an Effect.
- **Reactive `if` inside `ui!`** — an `if` whose condition contains
  `.get()` lowers to a `when` over a condition closure, which
  re-evaluates on change.
- **Reactive `for` inside `ui!`** — a `for` over a signal-backed
  source is wrapped so the list rebuilds when the source changes.
- **Stylesheets** — reading from the active theme is itself a
  tracked read. Theme tokens propagate to styles automatically.
- **A manual effect** — `effect!({ ... })` inside a component, or
  `watch(|| ...)` outside the tree, makes any closure a tracked context.

The underlying primitive in every case is a reactive effect. Everything
in this list is one or another way of installing one.

### What isn't tracked

A signal read **outside** any tracked context is just a value
read — useful when you want to look at the current value without
subscribing to changes. Event handlers are a common example:

```rust
on_click = move || {
    // Not tracked — just a snapshot at click time.
    println!("count is {}", count.get());
    count.update(|n| n + 1);
}
```

The `count.get()` inside the click handler runs at click time, not
at render time. It doesn't subscribe the click handler to anything
— event handlers are not tracked contexts.

## Effects

`effect!({ ... })` is the way to write a reactive effect inside a
component. It runs the body once, recording every signal read, then
re-runs the body whenever any of those signals change. The surrounding
component scope owns it — there is no handle to manage. (Outside the
tree, use `watch(|| ...)` and hold the returned `Subscription`.)

```rust
use runtime_core::{effect, signal};

let count = signal(0);
effect!({
    println!("count is now {}", count.get());
});
```

You rarely write effects by hand — most of the time the framework
installs them for you via `Text`, reactive props, `when`, and so
on. The cases where a manual effect makes sense are:

- **Debug logging** — observe a signal without putting it on screen.
- **External side effects** — write to a database, fire an analytics
  event, sync to local storage.
- **Imperative work on a Ref handle** — read a primitive's frame and
  do something with it whenever a signal changes.

### Lifetime: `effect!` in the tree, `watch` outside it

`effect!` is owned by the surrounding component scope — there is no
handle to manage. The effect lives until the scope drops (when the
component's subtree is replaced or torn down).

`effect!` takes a block — `effect!({ ... })`. A closure argument
(`effect!(move || ...)`) compiles but only builds the closure, so the
body never runs. The macro discards the block's value, so a cleanup
closure returned from it is dropped without running — register
cleanups with `on_cleanup` instead.

For an effect whose lifetime **you** control rather than the
component's — a service set up at app start, library or platform
setup — use `watch` instead:

```rust
use runtime_core::watch;

let sub = watch(move || apply_class(is_open.get()));
// `sub` is a `Subscription` — YOU own it. Dropping it disposes the
// effect; `Subscription::leak()` pins it for the world's lifetime.
```

`watch(f)` returns a caller-owned `Subscription`: store it where its
lifetime should match (a struct field, the owning service). Unlike
`effect!`, a `watch` is never adopted by an ambient scope, so it
behaves the same whether or not one is active. It still creates a
world effect, so it must be called inside the world (a component
build, an effect, a flush) — not from an event handler or an async
continuation. Both build on the public `effect(|| ...)` function,
which returns a non-owning `Effect` handle and, unlike the macro,
accepts a cleanup closure returned from the body.

> **From React.** `effect!` is in the family of `useEffect`,
> with three concrete differences:
>
> 1. **No deps array.** Idealyst tracks dependencies by what the
>    closure actually reads on each run. There is nothing to list
>    out, nothing to forget, no exhaustive-deps lint to fight.
>    Adding a signal read to the closure body subscribes to it;
>    removing the read unsubscribes.
> 2. **Cleanup is separate from the effect.** Call
>    `on_cleanup(callback)` from inside the effect to register a
>    teardown that fires before the next re-run and on final
>    disposal. Don't return a function from an `effect!` block —
>    the macro discards it.
> 3. **Effects run at the flush.** Writes stage; when the handler
>    returns, the flush commits them, settles memos, then runs each
>    affected effect once — so an effect never sees a fresh signal
>    next to a stale memo.

> **From Solid.** `effect!` is `createEffect(...)`. Same
> semantics: runs once eagerly, re-runs on dependency change,
> dependencies recomputed each run.

> **From Vue 3.** `effect!` is `watchEffect`. Both auto-track
> reads, both re-run on change, both lifetime-bound to a scope.

## Untracked reads

Sometimes a tracked context needs to read a signal *without*
subscribing — usually because the read is incidental ("I want the
current value, but I don't want to re-run if it changes").

```rust
use runtime_core::untrack;

effect!({
    let user = current_user.get();              // tracked: re-fire if user changes
    let pref = untrack(|| theme_pref.get());    // untracked: just a snapshot
    log_visit(&user, &pref);
});
```

Anything inside `untrack(|| ...)` runs without recording its reads.
Subscriptions added before the `untrack` call are preserved.

You'll know you need `untrack` when you find an effect re-firing
more often than makes sense.

> **From React.** Closest analog: reading `ref.current` instead of
> a state value. The intent is the same — get the current value
> without participating in the dependency graph.

## Derived values

A **derived value** is a function of one or more signals. Reading
the derived value reads its inputs reactively, so any tracked
context that depends on the derived value re-runs when the inputs
change.

Most of the time you don't build one by hand. Inside `ui!`, a
reactive `if` lowers its condition to a closure that the conditional
re-evaluates when the signals it reads change:

```rust
ui! {
    // Reactive: the condition becomes a closure the branch re-runs.
    if count.get() > 10 {
        text { "Over ten!" }
    }
}
```

Any prop that accepts a value also accepts a closure, so a one-off
derived value is just `move || count.get() * 2`. When the same
derived value is used in several places, or is expensive, use
`memo`:

```rust
let count = signal(0);
let doubled = memo(move || count.get() * 2);
```

`doubled` is a `Memo<i32>`. Anything that reads `doubled.get()`
re-runs when `count` changes — and only when the doubled value
actually differs. Don't write a derived value as an effect that
sets another signal: `memo` settles before effects run, while a
mirror signal updates one round later and can be observed stale.

> **From React.** `memo` is the equivalent of
> `useMemo(() => count * 2, [count])` — but the framework figures
> out the deps, and the result is a value consumable from anywhere,
> not bound to a render scope.

> **From Solid.** `memo` is `createMemo`, with the same caching and
> equality cut on its output.

> **From Svelte 5.** A derived signal is what `$derived(...)` is in
> a `.svelte` file.

> **From Vue 3.** A derived signal corresponds to `computed(...)`.

## Refs

A `Ref<H>` is a programmatic handle to a primitive: a slot that
the framework fills with a handle when the primitive mounts. It
lives in a thread-local ref table, not in the world with your
signals.

```rust
use runtime_core::{Ref, TextInputHandle};

let draft = signal(String::new());
let input: Ref<TextInputHandle> = Ref::new();

ui! {
    text_input(value = draft, on_change = move |s: String| draft.set(s), bind = input)
    button(label = "Focus", on_click = move || {
        if let Some(h) = input.get() {
            h.focus();
        }
    })
}
```

`Ref::get()` returns a clone of the handle once the primitive has
mounted, or `None` before that. `Ref::with(|h| ...)` borrows instead
of cloning, and `Ref::is_mounted()` reports whether the slot is
filled.

A Ref isn't reactive in the signal sense — reading `is_mounted()`
doesn't subscribe to its mount state. If you need to react to a
ref's lifecycle, drive a signal alongside it.

Refs have their own page — see [Refs](refs) for the full surface,
including handles for built-in primitives and how to declare
handles on user components with `#[method]`.

## Scopes and cleanup

Every Effect and every Signal is owned by a **Scope**. Scopes form
a tree:

- The world holds the **root scope**. When the world drops, the
  entire app's reactive state is freed in one shot.
- Reactive subtrees create **nested scopes**:
  - The active branch of a `When` lives in its own scope; flipping
    the condition drops the old scope and builds the new branch in
    a fresh one.
  - The same applies to a `Switch` (multi-way conditional) and to
    each iteration of a reactive `for`.

When a scope drops:

1. Every signal allocated inside it is freed. Reads against a freed
   signal panic with a diagnostic message (no silent corruption).
2. Every Effect allocated inside it is freed. Its subscriptions are
   removed from every signal it was reading.
3. Every primitive built inside it is torn down — the backend gets
   `clear_children` (or the equivalent) on the relevant parent
   nodes.

This is why you don't write component teardown code. The scope
owns the lifecycle.

> **From React.** Closest analog: a component unmounting causes its
> hooks to clean up. The difference: in React, you write the
> cleanup function (`return () => clearInterval(id)`); in
> Idealyst, the scope drop is implicit — every signal, effect, and
> node inside is freed together. If you have a resource (a socket,
> a subscription) that needs an explicit `Drop`, wrap it in a
> Rust type with a `Drop` impl and let the type system handle
> teardown.

## The flush — what happens on a signal change

A write does not run anything. It stages, and the driver flushes:

1. `signal.set(v)` records the pending value in the signal's slot.
   Nothing observable changes; reads still return the committed
   value.
2. When the author-code entry point returns (event handler, timer,
   animation frame, async-task poll), the backend's flush driver
   calls `World::flush`. You never call it yourself.
3. The flush settles **derivations** first — memos recompute in
   dependency order, each one equality-guarded.
4. Then user effects run, once each no matter how many of their
   dependencies changed in the turn. Each effect's dependency set is
   rebuilt from what the body reads on this run; the body usually
   makes one backend call.
5. Writes made from inside an effect are staged too and settle in a
   further round of the same flush.

So every handler is implicitly a batch — there is no `batch(|| ...)`
wrapper. N writes to N signals feeding one effect produce one effect
run, and an effect can never observe a fresh signal beside a stale
memo over it.

A flush that doesn't settle within 100 rounds panics ("cyclic signal
updates?") instead of hanging.

### Order

Within a flush, effects run in slot order — the order they were
created. You shouldn't rely on this for correctness (any effect
should be order-independent given its dependencies), but it's stable
and useful for debugging.

## Dynamic dependencies

Each time an Effect runs, its dependency set is rebuilt from
scratch. Whatever signals the body reads on this run become the
new set; everything from the previous run is dropped.

```rust
let mode = signal("a");
let a_value = signal(1);
let b_value = signal(2);

effect!({
    if mode.get() == "a" {
        println!("a = {}", a_value.get());
    } else {
        println!("b = {}", b_value.get());
    }
});

// Initial run: reads `mode` and `a_value`. Subscribed to both.
// Now flip the branch:
mode.set("b");
// Re-run: reads `mode` and `b_value`. Subscriptions: `mode` + `b_value`.
// `a_value` no longer notifies this effect — it dropped on the re-run.
```

You don't maintain a dependency array. You don't lint for missing
deps. You change what the closure reads and the framework adjusts.

> **From React.** This is the biggest practical difference from
> `useEffect`. The deps array is the framework's only way to know
> what to track; getting it wrong is one of React's classic bug
> classes (stale closures, missed updates, infinite loops). Here
> the framework reads what your closure reads, every run.

## Performance properties

A few characteristics that influence how to think about reactive
code:

- **Signals are Copy.** Passing a signal into a closure, a child
  component, or a slot doesn't clone anything heap-allocated; the
  `Signal<T>` itself is three `u32`s (world, slot, generation). The
  closure environment doesn't grow with the number of signals
  captured.
- **Arena storage.** Signals and effects are slots in the world's
  arena, not individual heap allocations. The cost of making a
  signal is bumping an index.
- **Per-update cost is proportional to changed nodes.** A signal
  change visits only the Effects subscribed to it; each Effect
  usually makes one backend call. The framework does less work as
  your app grows, not more — a 1000-component app and a
  10-component app pay the same cost to update one node.
- **Cleanup is bidirectional.** Subscriber sets and dependency sets
  are kept consistent on both ends — there are no stale entries to
  sweep. Dropping an Effect immediately removes its id from every
  signal it was subscribed to.

## Patterns

### A counter

The smallest reactive thing:

```rust
let count = signal(0);
ui! {
    text { format!("Count: {}", count.get()) }
    button(label = "++", on_click = move || count.update(|n| n + 1))
}
```

### Lifting state to a parent

A child component can take a signal as a prop. The signal is Copy,
so there's no clone bookkeeping; both parent and child read and
write the same arena slot.

```rust
#[component]
fn Counter(count: Signal<i32>) -> Element {
    ui! {
        text { format!("Count: {}", count.get()) }
        button(label = "++", on_click = move || count.update(|n| n + 1))
    }
}

#[component]
fn App() -> Element {
    let count = signal(0);
    ui! {
        Counter(count = count)
        text { format!("Doubled: {}", count.get() * 2) }
    }
}
```

### Effect for a side effect

```rust
effect!({
    let user = current_user.get();
    save_to_local_storage("user", &user);
});
```

### Reactive condition

```rust
ui! {
    if logged_in.get() {
        text { "Welcome back!" }
    } else {
        button(label = "Log in", on_click = move || logged_in.set(true))
    }
}
```

### Computed value used in two places

```rust
let count = signal(0);
let doubled = memo(move || count.get() * 2);

ui! {
    text { format!("count={}", count.get()) }
    text { format!("doubled={}", doubled.get()) }
}
```

## Pitfalls

### Components run once

The component function — the body of a `#[component] fn`,
including everything outside the `ui!` block — runs **once** when
the component mounts. Variables assigned there are computed at that
point and never recomputed.

If you write:

```rust
#[component]
fn Greeting(name: Signal<String>) -> Element {
    let greeting_text = format!("Hello, {}", name.get());  // computed ONCE
    ui! {
        text { greeting_text.clone() }
    }
}
```

…the text never updates. `name.get()` was called outside any
tracked context. The fix is to do the read inside the `ui!`:

```rust
ui! {
    text { format!("Hello, {}", name.get()) }
}
```

### Capturing stale values in closures

Same root cause: the closure runs at construction, the read inside
is what tracks.

```rust
let count = signal(0);
let initial = count.get();    // 0, frozen
ui! {
    // Wrong: shows "Initial: 0" forever
    text { format!("Initial: {}", initial) }
}
```

If you want the current value, read the signal inside the tracked
context. If you want a frozen value, that's already what you have
— give it a name that says so.

### Writing a signal from inside its own Effect

```rust
let count = signal(0);
effect!({
    let v = count.get();
    count.set(v + 1);    // loops: every round stages a new value
});
```

There is no self-write guard. The write stages, the flush commits
it in its next round, the effect re-runs because its dependency
changed, and it stages again — until the flush hits its 100-round
limit and panics. Derive the value with `memo` instead, or read it
with `untrack` / `count.update(...)` if the effect must not depend
on it.

### Reading a signal whose scope has dropped

```rust
// Inside ui! { for ... { let inner_signal = signal(0); ... } }
// If you hold inner_signal past the iteration, .get() panics later.
```

Signals are scope-bound and single-threaded. `Signal<T>` is not
`Send`, so moving one into `std::thread::spawn` is a compile error.
A read **or write** through a handle whose scope has dropped panics
with a stale-handle diagnostic; only a write after the whole world
has dropped is a silent no-op. Don't hold signals past their owning
scope's lifetime, and guard async continuations that might outlive
it.

## Where to read more

- [How a render happens](#how-a-render-happens) — the mechanism
  behind cascades, realize, and reactive subtrees, on the
  Overview page.
- [Refs](refs) — the full ref / handle surface and how `#[method]`
  declares user-component handles.
- [Styles](styles) — how the styling system uses the reactive
  substrate internally.
- The wire protocol — how derived values and structured actions
  ship to generator backends.
