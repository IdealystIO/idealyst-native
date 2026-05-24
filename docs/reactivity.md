# Reactivity

The reactivity system is the substrate everything else assumes. The
render walker uses it to wire backend updates to signal changes; the
styling system uses it to re-resolve stylesheets when the theme
changes; refs are arena-allocated alongside signals. Understanding
this layer makes the rest of the framework legible.

The implementation lives in `runtime_core::reactive`.

## Model

The framework's reactivity is **fine-grained, single-threaded, and
arena-backed**.

- **Fine-grained**: a signal change re-runs only the effects that read
  it on their last run. No virtual DOM, no diff pass, no
  component-level re-render. The unit of update is "the closure that
  read this signal."
- **Single-threaded**: all signal reads, effect runs, and arena
  operations happen on one thread. UIs aren't compute-bound; the
  ergonomics gain from skipping `Send` / `Sync` is enormous.
- **Arena-backed**: signals, effects, and refs live in a thread-local
  arena. Handles (`Signal<T>`, `Ref<H>`) are `Copy` indices into that
  arena, not `Rc`-style owning references. This eliminates the manual
  `.clone()` boilerplate at closure boundaries that's typical of
  Rust reactive systems.

## The three primitives

### `Signal<T>: Copy`

A `Copy` handle to an arena-stored cell of type `T`. Reads subscribe
the currently-running `Effect`; writes notify subscribers.

```rust
let count = signal!(0);                         // creates a Signal<i32>
let _ = count.get();                            // reads, subscribes current effect
count.set(5);                                   // writes, notifies subscribers
count.update(|n| *n += 1);                      // in-place mutation
```

`Copy` is the ergonomic centerpiece: `count` can be moved into every
closure that needs it without `.clone()`. This is what makes
`move || count.update(...)` work without `let count = count.clone()`
ceremony.

The cell is owned by whichever `Scope` was active when `signal!` ran.
When the scope drops, the slot is freed; subsequent `.get()` on a
freed signal panics with "signal used after its scope was dropped".

### `Effect`

A unit of reactive work: a closure that re-runs whenever a signal
it read on its last run changes.

```rust
let _e = Effect::new(move || {
    let n = count.get();
    log::info!("count is {}", n);
});
```

`Effect::new` runs the closure once immediately (establishing initial
subscriptions), then registers it for future runs. The returned
`Effect` handle's drop frees the effect slot — **unless** a `Scope`
took ownership during creation, in which case the handle is a no-op
and the scope frees the slot at its own drop.

This dual ownership is what makes effects inside the render walker
"just work": the framework wraps render in `with_scope(&mut owner_scope,
…)`, every effect created during the walk gets adopted by the owner's
scope, and the bare `let _e = Effect::new(...)` idiom at primitive
construction does the right thing without explicit handoff.

### `Ref<H>: Copy`

An arena-allocated slot for a typed handle. Created empty, filled at
mount time, read at any later point.

```rust
let r: Ref<ButtonHandle> = Ref::new();
ui! { Button(label = "Click", on_click = …).bind(r) }
// later:
r.with(|h| h.click());                          // None until mount; Some after
```

`Ref<H>` shares the `Copy` ergonomic with `Signal<T>`, and the scope
lifecycle with both signals and effects. See
[`ui-layer.md` § Refs](./ui-layer.md#refs) for the author-facing API.

---

## The arena

`reactive::ARENA` is a thread-local `RefCell<Arena>`:

```rust
struct Arena {
    signals: Vec<Option<Box<dyn Any>>>,
    effects: Vec<Option<Box<dyn Any>>>,
    refs:    Vec<Option<Option<Box<dyn Any>>>>,
}
```

Three flat slot tables. Slots are `None` after freeing — the index
stays valid (so dangling handles can be detected and panicked on)
even though the contents are gone.

`SignalId(u32)`, `EffectId(u32)`, `RefId(u32)` are the index types
the `Copy` handles wrap.

The arena is intentionally simple:

- **No generation counters.** If you read a freed slot, you panic;
  if a previously-freed slot has been reused by an unrelated handle,
  the inner downcast catches the type mismatch. We pay a small
  consistency tax for not maintaining generations; the audit surface
  is small.
- **`Box<dyn Any>` storage.** The arena doesn't statically know
  signal types. The inner downcast at `.get()` / `.set()` recovers
  the type. This is the cost of `Signal<T>: Copy` — we can't store
  `Vec<T>` per type without `T` parameters on the arena itself.
- **Linear growth.** Slots are never freed, only their contents.
  Long-lived UIs accumulate slot tombstones at the rate of
  signal/effect creation; in practice this is small (re-renders
  inside `when`/`switch` are the main churn source, and each
  per-branch scope is a few slots).

---

## `Scope` — the unit of lifetime

```rust
pub(crate) struct Scope {
    signals: Vec<SignalId>,
    effects: Vec<EffectId>,
    refs:    Vec<RefId>,
}
```

A `Scope` is "the set of arena slots that should be freed together."
The active scope, set by `with_scope`, registers new signals / effects
/ refs as they're created. When a scope drops, it walks its three
lists and frees the matching arena slots.

Scopes form a hierarchy through the call stack — `with_scope` pushes
a pointer onto `ACTIVE_SCOPE` (a thread-local Vec), runs the closure,
pops. So a `when` branch built inside a parent component's body
creates a nested scope; that scope's slots free when the branch
rebuilds even though the parent scope outlives it.

### Owning vs adopting

`Signal::new` and `Effect::new` check `ACTIVE_SCOPE`. If there's an
active scope, the slot is registered there (the scope "adopts" it)
and the returned handle's drop is a no-op. If no scope is active —
typical for "leaf" signals created at construction time outside any
render call — the handle owns the slot and its drop frees it.

This means signal/effect creation code is identical whether or not a
scope is around:

```rust
let count = signal!(0);                         // owns it if no scope; otherwise scope owns
let _e = Effect::new(|| …);                     // same
```

The framework's `render` function wraps the build walk in
`with_scope(&mut owner.scope, …)`, so everything that's part of a
rendered tree ends up owned by the owner's scope. When the
`Owner` is dropped, the whole tree's reactive state goes with it.

---

## Drop order: effects-first, signals-second

`Scope::drop` looks like this (paraphrased):

```rust
impl Drop for Scope {
    fn drop(&mut self) {
        // Step 1: take every slot's contents out under the ARENA borrow.
        let (taken_effects, taken_signals, taken_refs) = ARENA.with(|a| {
            // …drain self.effects / signals / refs from `a`…
        });

        // Step 2 (borrow released):
        //   effects first, signals second.
        drop(taken_effects);
        drop(taken_signals);
        drop(taken_refs);
    }
}
```

The two-step "take out, then drop" sidesteps a re-entrancy hazard: an
`EffectInner`'s captured state can transitively own *nested* scopes
(an inner `when`'s `Effect` captured a `Rc<RefCell<Option<Box<Scope>>>>`).
Those nested scopes' `Drop` calls re-enter `ARENA.borrow_mut`. If we
were still holding the outer borrow when those inner drops fire, we'd
panic with "RefCell already borrowed."

**Drop order between the categories is load-bearing.** Backend cleanup
hooks (`release_virtualizer`, `release_graphics`, navigator release)
run from inside an `EffectInner` drop — they tear down JS-side
listeners and drop the wasm-bindgen closures the platform holds. While
that teardown runs, a queued browser event (scroll, ResizeObserver
callback, microtask-deferred refresh) can fire synchronously into a
Rust callback that reads a user signal.

If we'd freed the signal first, that read would panic with "signal
used after its scope was dropped." By draining effects first, every
cleanup hook executes while signals are still live; once all
effects are gone, no Rust code holds a `Signal<T>` reference into
this scope, so signal drops become harmless.

This invariant is the resolution of an entire class of teardown
bugs. New cleanup hooks added to the `Backend` trait should rely on
it being preserved.

---

## Notification flow

A `Signal::set` does:

1. Replace the cell's `value`.
2. Drain the cell's `subscribers: Vec<EffectId>`.
3. Run each subscribed effect.

Each effect's run is:

1. Move the effect's closure out of its arena slot (replace with a
   no-op while running, to avoid double-borrowing the arena if the
   closure itself touches the arena).
2. Set the thread-local `CURRENT = Some(effect_id)` — any `Signal::get`
   reached during this run subscribes that signal's cell to this
   effect.
3. Run the closure.
4. Restore `CURRENT` and put the closure back in its slot.

Subscriptions are **rebuilt on every run** — there's no incremental
add/remove. So a branch inside an effect that stops reading a signal
naturally stops being notified by it. Conversely, every read inside
an effect subscribes; `untrack(|| …)` is the escape hatch to read a
signal without subscribing.

The arena moves the closure out before invoking it so that signal
callbacks invoked during the run can re-borrow the arena without
conflict. This is also why **a freed effect during its own run is
fine** — `run_effect`'s restore-closure step checks the slot first.

---

## Reactivity at the framework's seams

The framework uses reactivity at three layers:

1. **Primitive props.** When a primitive carries a closure for
   reactive content (`label: TextSource::Reactive(Fn -> String)`,
   `src: Box<dyn Fn() -> String>`, `disabled: Option<Box<dyn Fn() ->
   bool>>`), the walker wraps it in an `Effect` that calls the
   matching backend update method. Signal reads inside the closure
   subscribe naturally.

2. **Reactive conditionals.** `when` and `switch` wrap their decision
   closure in an `Effect`. When the closure's signals change, the
   effect re-runs, compares the new branch identity against the
   previous, and rebuilds the subtree (under a fresh nested scope)
   if it changed.

3. **Style resolution.** Each styled node has a dedicated `Effect`
   that calls `style_source()`, resolves the resulting `StyleApplication`
   against the active theme, and applies the resulting `StyleRules`
   to the backend. The active theme is itself a signal; signals read
   inside variant-source / override-source closures subscribe
   naturally. See [`styling.md`](./styling.md).

In all three cases, the framework doesn't track dependencies
explicitly. Signal reads inside the closure are the dependencies, by
construction.

---

## `schedule_microtask` and deferred teardown

`runtime_core::scheduling::schedule_microtask` exposes a
single-shot microtask helper. The web build uses
`js_sys::Promise::resolve().then(...)`; native builds use a
trampoline equivalent (the implementation is gated per target).

The framework uses microtask deferral when *synchronous* teardown
would create lifecycle hazards:

- **`switch` rebuild.** The rebuild is deferred so the triggering
  closure (the click handler or whatever fired `signal.set`) returns
  before the old subtree's closures are dropped. The platform may
  still have queued events for those closures; deferring lets the
  events drain first.
- **`release_virtualizer`** (web). The two-phase release sets a JS
  `_released` flag synchronously, then defers the heavy release
  (which calls back into Rust to drop per-item scopes) so that the
  outer cleanup `Effect`'s borrow on the backend `RefCell` is
  released before re-entry.
- **Navigator initial mount** (some backends). Avoids re-entrant
  `borrow_mut` when `mount_screen` is called inside a path that
  already holds the backend borrow.

The general rule: **if your cleanup invokes platform code that may
synchronously call back into Rust, defer the cleanup to a microtask.**
The framework provides the helper so backends don't need to roll
their own.

---

## Pitfalls

- **Reading a freed signal panics.** This is a hard error, not a
  warning. If you see it, look for an effect outliving its enclosing
  scope — typically a leaked closure handed to a platform API that
  fired after the scope dropped. Fix: register a cleanup hook
  (`release_virtualizer` etc.) that detaches the listener before
  the scope drops; effects-first drop order then guarantees signal
  reads inside any callback fired during teardown are safe.

- **Re-entrant `RefCell::borrow_mut`.** Common with backends that
  store state on a `Rc<RefCell<B>>` and have JS/JVM callbacks that
  re-enter the framework. Fix: either restructure to avoid the
  re-entry, or defer to a microtask so the outer borrow releases
  first.

- **Effect leak when no scope is active.** `let _e = Effect::new(|| …)`
  outside any `with_scope` creates an effect owned by the local
  `_e` binding — when that binding goes out of scope at the end of
  the enclosing function, the effect is freed. If you build a
  primitive subtree outside a scope (e.g. inside a `mount_screen`
  callback that the backend doesn't wrap in `with_scope`), the
  walker's per-primitive effects (style, label, etc.) are immediately
  freed at the end of `build`, and the rendered widget stops
  responding to signals. Fix: wrap the build site in
  `reactive::with_scope`.

- **`Signal<T>` is `Copy` but `T` is moved on `get()`** — `get()`
  clones the value out of the cell, so `T: Clone` is required.
  If you want to inspect without cloning, use `with` (a borrowed
  read) instead.
