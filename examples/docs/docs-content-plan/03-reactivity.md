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

let count = signal!(0);
let name = signal!(String::from("Ada"));
let items = signal!(Vec::<Item>::new());
```

`signal!(v)` is shorthand for `Signal::new(v)`. The value is stored
in a thread-local arena; the `Signal<T>` you hold back is a small
Copy token (a couple of u32s) that indexes into the arena. This is
why you can pass signals into closures and child components without
ever calling `.clone()`.

### Reading and writing

```rust
let n = count.get();          // tracked read
count.set(7);                 // replace the value
count.update(|n| *n += 1);    // mutate in place
```

`.get()` returns a clone of the current value (`T: Clone` is the
only bound). `.set(v)` replaces the value. `.update(|v| ...)` is
the same as `set(f(get()))` but skips the clone — useful for
collections.

Both `.set` and `.update` run synchronously and trigger every
dependent Effect to re-run before they return. See **Cascades**
below for what that means in practice.

### Identity

```rust
let id: u64 = count.id();
```

Every signal has a stable arena id. You rarely need this — it's the
hook the wire protocol uses to refer to a signal across processes.
If you find yourself reaching for `.id()` from app code, there's
probably a more direct API you want instead.

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

- **Reactive text** — `Text { format!("count: {}", count.get()) }`.
  The expression is wrapped in an Effect that re-fires on signal
  change.
- **Closure props** — `label = move || format!("...", count.get())`.
  A closure passed where the framework expected a `Derived<T>` or
  reactive source is wrapped in an Effect.
- **Reactive `if` inside `ui!`** — an `if` whose condition contains
  `.get()` lowers to a `When` primitive whose conditional Effect
  re-fires on change.
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
    count.update(|n| *n += 1);
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

let count = signal!(0);
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
component's subtree is replaced or torn down). That's why `effect!`
debug-asserts a scope is active: a scope-owned effect only makes sense
inside the tree.

For reactivity wired up **outside** the component tree — app startup,
an async callback, library or platform setup — use `watch` instead:

```rust
use runtime_core::watch;

let sub = watch(move || apply_class(is_open.get()));
// `sub` is a `Subscription` — YOU own it. Dropping it disposes the
// effect; `Subscription::leak()` pins it for the whole process.
```

`watch(f)` returns a caller-owned `Subscription`: store it where its
lifetime should match (a struct field, the owning service). Unlike
`effect!`, a `watch` is never adopted by an ambient scope, so it
behaves the same whether or not one is active. The raw `Effect::new`
constructor these build on is sealed — use `effect!` or `watch`.

> **From React.** `effect!` is in the family of `useEffect`,
> with three concrete differences:
>
> 1. **No deps array.** Idealyst tracks dependencies by what the
>    closure actually reads on each run. There is nothing to list
>    out, nothing to forget, no exhaustive-deps lint to fight.
>    Adding a signal read to the closure body subscribes to it;
>    removing the read unsubscribes.
> 2. **No cleanup function.** When the effect's scope drops, its
>    subscriptions are released automatically. If you need to undo
>    a side effect on teardown — close a socket, cancel a timer —
>    you do it by writing a destructor on whatever resource the
>    effect created, not by returning a cleanup function from the
>    effect itself.
> 3. **Runs synchronously on the change.** Idealyst effects fire on
>    the same call stack as the `signal.set()` that caused them, not
>    after a commit phase. This is faster and more predictable, but
>    means heavy work inside an effect blocks the writer.

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

Most of the time you don't construct a `Derived<T>` by hand. Inside
`ui!`, the macro recognizes reactive call shapes and emits a
`Derived<T>` for you:

```rust
ui! {
    // Reactive: the macro wraps this in a Derived<bool> Effect.
    if count.get() > 10 {
        Text { "Over ten!" }
    }
}
```

When you do need an explicit derived value — for example, a
computed value used in two places — you can compose the same shape
manually with an effect that writes to a derived signal:

```rust
let count = signal!(0);
let doubled = signal!(0);
effect!({ doubled.set(count.get() * 2) });
```

`doubled` is now a signal that mirrors `count * 2`. Anything that
reads `doubled.get()` re-runs when `count` changes.

The first-class `Derived<T>` type lives in `runtime-core` and
carries both the runtime closure and a structured description
(method name + input signal ids). The structured form is what lets
generator backends like Roku ship the derived expression to the
device without shipping a closure. You only build one explicitly
when you're writing a primitive or working at the wire-protocol
layer.

> **From React.** A pattern like the `doubled` signal above is the
> equivalent of `useMemo(() => count * 2, [count])` — but the
> framework figures out the deps, and the result is a value
> consumable from anywhere, not bound to a render scope.

> **From Solid.** `Derived<T>` is `createMemo`'s structural cousin.
> Solid's `createMemo` lazily caches and recomputes; the
> `Effect + signal` pattern shown above is the manual equivalent.
> An explicit memo primitive is on the roadmap.

> **From Svelte 5.** A derived signal is what `$derived(...)` is in
> a `.svelte` file.

> **From Vue 3.** A derived signal corresponds to `computed(...)`.

## Refs

A `Ref<H>` is a programmatic handle to a primitive. It's allocated
in the same arena as signals and effects, but it's not a value type
— it's a slot that the framework fills with a handle when the
primitive mounts.

```rust
use runtime_core::{Ref, ButtonHandle};

let btn: Ref<ButtonHandle> = Ref::new();

ui! {
    Button(label = "Increment", on_click = on_click).bind(btn)
}

// Later, from any signal-write context:
btn.with(|h| h.trigger());        // call the button's `trigger` method
```

`Ref::with(|h| ...)` runs the closure with the handle if the
primitive is currently mounted; returns `None` if it isn't.
`Ref::get()` and `Ref::is_mounted()` are convenience variants.

A Ref isn't reactive in the signal sense — reading `is_mounted()`
doesn't subscribe to its mount state. If you need to react to a
ref's lifecycle, drive a signal alongside it.

Refs have their own page — see [Refs](#) for the full surface,
including handles for built-in primitives and how to declare
handles on user components via `methods!`.

## Scopes and cleanup

Every Effect and every Signal is owned by a **Scope**. Scopes form
a tree:

- The renderer's `Owner` holds the **root scope**. When the owner
  drops, the entire app's reactive state is freed in one shot.
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

## Cascades — what happens on a signal change

The cascade machinery is documented in detail on the Overview's
"How a render happens" section. The summary:

1. `signal.set(v)` writes the new value to the arena.
2. It snapshots the signal's current subscriber set.
3. Each subscriber Effect re-runs in turn:
   - Its previous dependency set is cleared.
   - The closure runs with `CURRENT` set to its id, so any read
     records as a fresh dependency.
   - The closure usually makes one backend call.
4. If a subscriber's run writes another signal, that signal's
   subscribers are run **before** the outer write returns.

Cascades are synchronous, depth-first, and bounded by the
re-entry guard (an Effect that fires the signal it's currently
reading is skipped, matching how Solid, MobX, and Reactively handle
the same pattern).

There is no scheduler queue, no microtask drain, no batch boundary.
By the time `set` returns, every downstream Effect has either run
or been skipped, and every backend call those Effects produced has
been made.

### Order

For a single signal write, subscribers run in arena-id order — the
order they were created. You shouldn't rely on this for correctness
(any Effect should be order-independent given its dependencies),
but it's stable and useful for debugging.

For chained cascades, the order is depth-first: writes from inside
an Effect run their consequences before the outer write returns.

## Dynamic dependencies

Each time an Effect runs, its dependency set is rebuilt from
scratch. Whatever signals the body reads on this run become the
new set; everything from the previous run is dropped.

```rust
let mode = signal!("a");
let a_value = signal!(1);
let b_value = signal!(2);

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
  `Signal<T>` itself is two `u32`s. The closure environment
  doesn't grow with the number of signals captured.
- **Arena storage.** Signals and effects are slots in a thread-local
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
let count = signal!(0);
ui! {
    Text { format!("Count: {}", count.get()) }
    Button(label = "++", on_click = move || count.update(|n| *n += 1))
}
```

### Lifting state to a parent

A child component can take a signal as a prop. The signal is Copy,
so there's no clone bookkeeping; both parent and child read and
write the same arena slot.

```rust
#[component]
fn counter(count: Signal<i32>) -> Element {
    ui! {
        Text { format!("Count: {}", count.get()) }
        Button(label = "++", on_click = move || count.update(|n| *n += 1))
    }
}

#[component]
fn app() -> Element {
    let count = signal!(0);
    ui! {
        counter(count = count)
        Text { format!("Doubled: {}", count.get() * 2) }
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
        Text { "Welcome back!" }
    } else {
        Button(label = "Log in", on_click = move || logged_in.set(true))
    }
}
```

### Computed value used in two places

```rust
let count = signal!(0);
let doubled = signal!(0);
effect!({ doubled.set(count.get() * 2) });

ui! {
    Text { format!("count={}", count.get()) }
    Text { format!("doubled={}", doubled.get()) }
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
fn greeting(name: Signal<String>) -> Element {
    let greeting_text = format!("Hello, {}", name.get());  // computed ONCE
    ui! {
        Text { greeting_text.clone() }
    }
}
```

…the text never updates. `name.get()` was called outside any
tracked context. The fix is to do the read inside the `ui!`:

```rust
ui! {
    Text { format!("Hello, {}", name.get()) }
}
```

### Capturing stale values in closures

Same root cause: the closure runs at construction, the read inside
is what tracks.

```rust
let count = signal!(0);
let initial = count.get();    // 0, frozen
ui! {
    // Wrong: shows "Initial: 0" forever
    Text { format!("Initial: {}", initial) }
}
```

If you want the current value, read the signal inside the tracked
context. If you want a frozen value, that's already what you have
— give it a name that says so.

### Writing a signal from inside its own Effect

```rust
let count = signal!(0);
effect!({
    let v = count.get();
    count.set(v + 1);    // re-entry: this run is skipped, no loop
});
```

The re-entry guard skips an Effect that's already running. The
write happens, but the Effect doesn't loop. If you needed that
write to fire other subscribers, it still does — only the self-fire
is suppressed.

### Reading a signal whose scope has dropped

```rust
let s = signal!(0);          // owned by current scope
let _ = std::thread::spawn(move || s.get());  // wrong: panic on other thread

// Inside ui! { for ... { let inner_signal = signal!(0); ... } }
// If you hold inner_signal past the iteration, .get() panics later.
```

Signals are scope-bound and single-threaded. Reads after the
scope's drop panic with a diagnostic. Don't hold signals past
their owning scope's lifetime.

## Where to read more

- [How a render happens](#how-a-render-happens) — the mechanism
  behind cascades, the walker, and reactive subtrees, on the
  Overview page.
- [Refs](#) — the full ref / handle surface and how `methods!`
  declares user-component handles.
- [Styles](#) — how the styling system uses the reactive substrate
  internally.
- [The wire protocol](#) — how `Derived<T>` and `Action`'s
  structured form ship to generator backends.
