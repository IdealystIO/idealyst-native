# Reactivity

The reactivity system is the substrate everything else assumes. Scene
drivers use it to wire backend updates to signal changes; the styling
system uses it to re-resolve stylesheets when the theme changes;
navigation stages its commands into it. Understanding this layer makes
the rest of the framework legible.

The kernel lives in `crates/runtime/world` (`runtime_world`). Author code
reaches it through `runtime_core::…`, whose root is a paper-thin
re-export of `runtime_vocabulary::glue` — see
[`migrating-to-runtime-v2.md` § `runtime_core::` is the author-facing path](migrating-to-runtime-v2.md#runtime_core-is-the-author-facing-path-and-stays-that-way).

## Model

The framework's reactivity is **fine-grained, single-threaded,
arena-backed, and staged-commit**.

- **Fine-grained**: a committed change re-runs only the effects that read
  the signal on their last run. No virtual DOM, no diff pass, no
  component-level re-render. The unit of update is "the closure that
  read this signal."
- **Single-threaded**: all signal reads, effect runs, and arena
  operations happen on one thread. UIs aren't compute-bound; the
  ergonomics gain from skipping `Send` / `Sync` is large.
- **Arena-backed**: signals and effects live in a per-world arena.
  Handles (`Signal<T>`, `ReadSignal<T>`, `WriteSignal<T>`, `Effect`,
  `Memo<T>`) are `Copy` triples of `(world id, slot, generation)` — not
  `Rc`-style owning references. This eliminates the `.clone()`
  boilerplate at closure boundaries typical of Rust reactive systems.
- **Staged-commit**: a write records a *pending* value and changes
  nothing observable. The world's flush commits every staged write as one
  logical update. See
  [`automatic-batching.md`](automatic-batching.md) for the flush
  algorithm and the per-backend drivers.

### `World` — the reactive runtime instance

Reactive state belongs to a `World` (`crates/runtime/world/src/lib.rs`).
A world owns an arena of generational signal/effect slots, a staged
queue, and a typed context map. Several worlds coexist on a thread and
flush independently: `World::enter` sets the *creation* context, while
handle *use* routes through the handle's own world id
(`tests.rs::worlds_are_discrete`, `parallel_worlds_flush_and_drop_independently`).
That is what lets SSR build one world per request and drop it after
serializing (`crates/backend/ssr/tests/newcore_isolation.rs`), and what
lets an embedded app mounted inside a host page write into the host's
world.

A single `thread_local!` holds the whole registry (`world id → arena`)
plus every transient stack. One TLS key matters on Android, where bionic
caps pthread TLS keys at 128.

Slots are **generational**: freeing a slot bumps its generation, so a
stale `Copy` handle whose scope was dropped can never alias the slot's
next occupant — it panics with a diagnostic
(`tests.rs::stale_signal_read_panics`, `stale_signal_write_panics`).

## The primitives

### `Signal<T>: Copy`

A `Copy` handle to an arena cell of type `T`, where `T: PartialEq` (the
guarded-commit bound). Reads see the committed value and subscribe the
running effect; writes stage.

```rust
let count = signal(0);        // Signal<i32>, created in the ambient world
let _ = count.get();          // committed read; subscribes the running effect
count.set(5);                 // stages 5; nothing observable yet
count.update(|n| n + 1);      // read-modify-write on the staged value
```

`signal(v)` creates into the **ambient** world and therefore requires
`World::enter` — component bodies and effect bodies are entered, event
handlers are not (`tests.rs::ambient_api_requires_a_world`). See
[Handlers run outside the world](#handlers-run-outside-the-world).

#### The read surface

| method | tracks | sees |
|---|---|---|
| `get()` | yes | committed value (`T: Clone`) |
| `peek()` | no | committed value (`T: Clone`) |
| `with(\|&T\| …)` | yes | committed value, borrowed — no clone |
| `with_untracked(\|&T\| …)` | no | committed value, borrowed |

No read ever returns a staged value. `peek()` is the untracked read —
the name the old core spelled `get_untracked()`. Use it for
self-referential writes inside an effect (`set(peek() + 1)` with `get()`
would re-trigger the effect forever) and `with()` for large values
(`Vec` rows) where the clone is the cost
(`tests.rs::peek_never_subscribes`, `with_reads_without_cloning_and_tracks`).

Reading the *same* signal again from inside a `with` closure is a
reentrancy error — the storage is moved out for the closure's duration.
Other signals are fine.

#### The write surface

| method | writes | notifies | on |
|---|---|---|---|
| `set(v)` | stages | at commit, only if the committed value changed | `T: PartialEq` |
| `set_always(v)` | stages | at commit, always | `T: PartialEq` |
| `touch()` | no | at commit, always | any signal |
| `set_untracked(v)` | commits directly | never | `T: PartialEq` |
| `update(\|&T\| -> T)` | stages, composing on the staged value | at commit, only if changed | `T: PartialEq` |

`set` being equality-guarded is the default the other fine-grained
frameworks ship (Solid, Vue, React's bail-out): a same-value write wakes
nobody, which also starves the echo loops two-way-bound signal props
could otherwise feed. Guarding is evaluated **at commit**, so an
`A→B→A` round trip inside one turn nets to zero fan-out
(`tests.rs::set_net_zero_flush_window_skips_fanout`). `set_always` is
the retrigger spelling when the value stays equal; `touch()` is the
retrigger spelling when there is no value to write (shared interior
state the signal's value points to). `set_untracked` writes the
committed cell silently — dependents keep a stale view until something
else notifies, so treat it as deliberate bookkeeping only.

`update` takes `&T` and **returns** the new value; it composes on the
staged value, which is what makes two increments in one turn net `+2`
(`tests.rs::update_composes_on_the_staged_value`). This is the shape
event handlers should use. (The old core's `update(|v: &mut T| …)`
in-place form and `update_if_changed` are both gone.)

Leptos note: Leptos's `set` never compares — ported code relying on
same-value sets as retriggers must switch those sites to `set_always` or
`touch`.

`Copy` is the ergonomic centerpiece: `count` moves into every closure
that needs it without `.clone()` ceremony
(`tests.rs::handles_are_copy`).

### `ReadSignal` / `WriteSignal` — capability halves

`signal.split()` (or `.read_only()` / `.write_only()`) hands out
zero-cost views over the same slot whose TYPE permits only reading or
only writing — same tracking, same staleness checks, still `Copy`
(`tests.rs::capability_halves_share_the_signal`). Use them to encode
least privilege in signatures: a `ReadSignal<T>` prop proves the
component observes without mutating (every `Memo` exposes one), a
`WriteSignal<T>` lets a child report upward without subscribing itself.
The unified `Signal<T>` is the right type for genuinely two-way props.
Deliberately no `Deref` between them — deref would hand the other
capability back.

### `Effect`

A unit of reactive work: a closure that re-runs when a signal it read on
its last run commits a change.

```rust
let _e = effect(move || {
    log::info!("count is {}", count.get());
});
```

`effect(f)` creates into the ambient world and runs the body once
immediately to collect dependencies. The returned handle is `Copy` and
**non-owning** — the effect's lifetime belongs to the enclosing
ownership scope (see [Ownership](#ownership-and-teardown)), not to the
binding. Authors normally write `effect!({ … })`, the macro form
re-exported by the `runtime-core` root.

Four properties worth internalizing:

- **Dependencies re-collect every run.** Whatever the body actually read
  on a given run is its dependency set for the next one; a branch not
  taken this run does not subscribe
  (`tests.rs::dependencies_recollect_each_run`,
  `conditional_dep_switch_unsubscribes_the_dropped_signal`). Reads are
  reconciled by diff, so an effect whose reads are stable run-to-run
  touches no subscriber list on re-run
  (`stable_deps_rerun_keeps_delivering`).
- **Effects run during the flush, not inside `set()`**, and once per
  flush no matter how many dependencies changed
  (`an_effect_runs_once_per_flush_even_with_two_changed_deps`).
- **An effect body is its own tracking context.** An effect *created*
  inside `untrack(…)` still tracks its own reads — the body resets the
  untrack window (`effect_created_inside_untrack_still_tracks`).
- **Bodies may return a cleanup.** `()`, a `FnOnce()`, or
  `Option<FnOnce()>` (the sealed `IntoCleanup` trait). A returned
  cleanup runs before the next re-run and at teardown, identically to
  `on_cleanup` (`returned_closures_are_cleanups`,
  `optional_cleanups_only_register_when_some`).

### `Memo<T>`

`memo(move || …)` creates a derived signal: `T: PartialEq + Clone`,
computed once and shared by all consumers. A `Memo<T>` is `Copy` and
exposes the read surface; it is a `ReadSignal` half plus the recompute
effect (`crates/runtime/world/src/lib.rs`).

Memos are a distinct effect class — **Derivations** — and the flush
settles every dirty Derivation before running any user effect, which is
what makes the diamond glitch-free
(`tests.rs::diamond_effect_runs_once_with_consistent_pair`). They are
equality-guarded: a recompute producing an equal value does not wake
consumers (`memo_equality_cut_stops_propagation`).

For a *single* consumer a plain closure (`move || src.get() * 2`) is
lighter and needs no machinery — every reactive prop already accepts one.
`memo` earns its slot when the derivation is shared, expensive, or
should cut propagation.

### `untrack` and `unscope`

`untrack(f)` runs `f` with dependency tracking suspended: reads inside
subscribe nothing, even within an effect. Suspension is global for the
code region rather than per-world, so it means the same thing in every
cross-world composition
(`tests.rs::untrack_suspends_tracking_globally_across_worlds`). It does
not extend into nested effect *bodies* (see above).

`unscope(f)` (kernel: `runtime_world::unscoped`) suspends the *ownership*
collector instead, so everything created inside is world-root-owned. Use
it for world-lifetime services whose first use lands inside some
subtree's build — a per-world theme-version signal collected into that
subtree's scope would be freed on the subtree's unmount, leaving its
`Copy` handles dangling (`tests.rs::unscoped_creations_survive_the_enclosing_collector`).

### `Ref<H>` — the imperative handle slot

`Ref<H>` slots live in the shared substrate's legacy arena
(`crates/runtime/shared/src/reactive.rs`), not in the world kernel.
`.bind(r)` on a primitive builder installs a `ref_fill` closure that the
primitive's handler calls with the minted handle at mount
(`crates/runtime/vocabulary/src/glue.rs`, e.g. `GlueView::bind`;
`ref_fill` slots on the payloads in
`crates/runtime/vocabulary/src/prims/`), so `r.get()` / `r.with(|h| …)`
resolve after mount.

Two caveats that follow from the slot living outside the world:

- **A `Ref::new()` slot is not freed.** Its lifetime was bound to an
  old-core `Scope`, and no old-core scope is ever active in a runtime-v2
  build, so the slot leaks until the thread exits (`Ref::new`'s own docs
  state the rule). Refs are per-component, so this is a bounded leak, not
  a per-update one — but a `Ref` created inside a frequently-remounted
  subtree accumulates slots.
- **Anchoring is the detached sentinel only.** For
  `anchored_overlay(target = …)` the sanctioned surface is
  `AnchorTarget::from(Ref::default())` — the detached sentinel, whose
  `get()` is `None`, so the overlay falls back to unresolved positioning
  (`crates/runtime/vocabulary/src/glue.rs`, the `AnchorTarget`
  re-export comment).

## Ownership and teardown

**Dropping is the entire teardown story.** There is no dispose call.

`collect_owned(f)` runs `f` and gathers every signal and effect created
inside it (in any world) into an `Owned` handle; dropping the `Owned`
runs the collected effects' cleanups and frees the slots
(`tests.rs::dropping_owned_runs_cleanups_and_frees_slots`). Collectors
stack, so an inner `collect_owned`'s creations belong to the inner scope
only (`nested_collectors_own_their_scopes_independently`). Creations with
no active collector are world-root-owned and live until the world drops.

Three consumers of that mechanism:

1. **Component bodies.** `#[component]` wraps the body in
   `component_scope` = "collect everything created, run untracked"
   (`crates/runtime/scene/src/element.rs::component_scope`). The
   collected scope rides on the returned `Element::Owned` and is folded
   into the enclosing `Realized` at realize time, so it lives exactly as
   long as the subtree stays mounted
   (`tests.rs::component_scope_collects_effects_into_owned`,
   `component_bodies_run_untracked`).
2. **Structural holes.** Each reactive region (`if`, `match`, keyed
   `for`, navigator slot) owns its current subtree's scope; swapping the
   region drops it (`tests.rs::dyn_slot_swap_tears_down_previous_scope`,
   `keyed_scopes_reconcile_by_identity`).
3. **`watch(f)`** — a caller-owned subscription. It runs `f` now,
   re-runs it on change, and disposes the effect when the returned
   `Subscription` drops; `.leak()` pins it for the world's lifetime
   (`crates/runtime/vocabulary/src/glue.rs`). Its effect goes into a
   *private* scope, never the caller's, so `watch` is the right tool
   when the lifetime must be explicit.

### Component bodies run once, untracked

A component body runs exactly once. A bare `sig.get()` in a body is a
build-time snapshot that can never subscribe anything, even accidentally
— the same authoring rule the 0.3 snapshot-trap warning taught
(`migration-0.2-to-0.3.md`), now structural. Reactive reads belong in
`ui!` slots, `rx!`, `move ||` closures, memos, and effects.

### `on_cleanup` requires a running effect

`on_cleanup(f)` registers a cleanup on the innermost **running effect**
and panics with `"on_cleanup called outside an effect"` anywhere else,
including a component body
(`crates/runtime/world/src/lib.rs::on_cleanup`). Cleanups run before
that effect's next re-run, or when its owning scope drops, in
registration order (`tests.rs::on_cleanup_runs_before_rerun_and_on_drop`,
`cleanups_run_in_registration_order`). Two shapes:

```rust
// Register inside an effect…
effect!({
    on_cleanup(move || timer.cancel());
});

// …or return the cleanup from the effect body.
effect!({
    let t = start_timer();
    move || t.cancel()
});
```

The effect-owned shape closes a bug class: the cleanup cancels its
timers when the component unmounts, so a detached callback cannot
outlive a remounted world.

## Handlers run outside the world

Event handlers (`on_click`, `on_change`, timers, async continuations) run
*outside* `World::enter` — the world is entered during builds and
flushes only. Because handles are `Copy` and route to their own world,
the everyday surface works in a handler: `get`, `peek`, `with`, `set`,
`set_always`, `touch`, `update` on any signal captured at build time.
What does not work is anything that needs the *ambient* world:

- **Creating state** — `signal(…)`, `effect(…)`, `memo(…)`,
  `provide`/`inject` panic (`with_ambient`'s
  `"called outside World::enter"` diagnostic).
- **Free fns that resolve ambient per-world state** — the theme-install
  family (`install_tokens(…)` & co.) resolves the ambient world's
  `ThemeCtx`. Capture the ctx at build time and call its methods, which
  are handler-safe (`crates/runtime/vocabulary/src/theme.rs`).

`runtime_world::is_entered()` is the probe code that must serve both
callers forks on; `runtime_world::in_effect()` is the equivalent probe
for `on_cleanup` legality (`tests.rs::is_entered_tracks_ambient_world`,
`in_effect_tracks_effect_bodies_only`). For a foreign entry point (JS
interop, a native callback you own) that must create state, web exposes
`backend_web::newcore::with_world_entered(f)`.

### After the world is gone

Per-world lifetime is strict. **Writes** to a dropped world are silent
no-ops (`tests.rs::writes_to_a_dead_world_are_noops`) — an async task
completing after unmount is harmless — while **reads** panic
(`reads_from_a_dead_world_panic`), surfacing the handle leak instead of
returning garbage. Writes through a *stale handle* in a live world still
panic: that is a use-after-unmount logic error worth seeing
(`stale_signal_write_panics`).

## Reactivity at the framework's seams

The framework uses reactivity at four layers, and in each the
dependencies are whatever the closure read — nothing is declared:

1. **Primitive props.** A prop carrying a closure or signal
   (`IntoValue<T>` → `Value::Dyn`) gets a binding effect that calls the
   matching capability method on change: `TextOps::update_text`,
   `ImageOps::update_image_src`, `ToggleOps::update_toggle_value`, and
   so on. The native widget exists once and is mutated in place; there is
   no diff pass (`tests.rs::dyn_values_bind_reactively_until_dropped`,
   `const_values_bind_once_with_no_effect` — a constant prop creates no
   effect at all).
2. **Reactive control flow.** `when(cond, …)` lowers to the scene's
   *guarded* hole keyed on the predicate's boolean, and `switch` /
   reactive `match` keys on the scrutinee **value** with `PartialEq`
   dedup: a dependency re-fire producing an equal key keeps the mounted
   arm instead of rebuilding it
   (`crates/runtime/vocabulary/src/glue.rs::when`/`switch`). Consequence:
   `touch()` on a signal used as a match scrutinee is inert — to force a
   rebuild, change the value (e.g. fold a generation counter into the
   scrutinee tuple). Keyed `for` reconciles rows by `Key` identity, and
   duplicate keys panic (`tests.rs::keyed_scopes_reconcile_by_identity`,
   `duplicate_keys_panic`).
3. **Style resolution.** Each styled node has a binding effect that
   resolves its `StyleApplication` against the active theme and applies
   the resulting `StyleRules` through `StyleOps`. The theme is itself
   per-world reactive state, so signals read inside variant/override
   closures subscribe naturally. See [`styling.md`](./styling.md).
4. **Navigation.** `on_select`, `pop`, and `NavHandle` dispatch stage a
   command into a queue; a driver effect commits it on the flush, so one
   navigation is one logical update
   (`crates/runtime/vocabulary/src/handlers/navigator.rs`).

## `schedule_microtask` and deferred teardown

`runtime_core::scheduling::schedule_microtask` is the single-shot
microtask helper (`crates/runtime/shared/src/scheduling.rs`, re-exported
through glue). The web build uses
`js_sys::Promise::resolve().then(...)`; native builds install a
trampoline equivalent. The flush drivers ride it — `schedule_flush()` is
one deduped microtask — and backends use it when *synchronous* teardown
would create lifecycle hazards:

- **`release_virtualizer`** (web): a two-phase release sets a JS
  `_released` flag synchronously, then defers the heavy release (which
  calls back into Rust to drop per-row scopes) so the outer cleanup's
  borrow on the backend `RefCell` is released before re-entry.
- **Navigator mount** on backends whose platform call would re-enter the
  backend borrow.

The general rule: **if your cleanup invokes platform code that may
synchronously call back into Rust, defer the cleanup to a microtask.**

## Profiling reactive updates

The `debug-stats` feature (`runtime_core::debug`, implemented in
`crates/runtime/shared/src/debug.rs`) carries two instrument families,
and they do not currently have equal coverage on this core:

- **Phase counters** — `PhaseTimer::start("phase_name")` spans, drained
  with `debug::take_phase_counters()`. These are live: the backends'
  apply paths and the new flush driver record into them (e.g.
  `"nc_flush_total"` in `crates/backend/web/src/newcore.rs`). Per-phase
  conventions are in project CLAUDE.md §6.
- **The reactive transaction stream** — `debug::take_events()`,
  `txn_report()`, `slow_signals()`, `format_reactive_profile()`. The
  events these fold (`record_txn_enter`, `record_effect_run`,
  `record_commit`, `record_signal_created`) are emitted from
  `crates/runtime/shared/src/reactive.rs` — the legacy arena — and
  **`runtime-world` emits none of them**. On runtime v2 the transaction
  report is therefore empty of signal/effect data; the surviving
  contributions are `#[component]` enter/exit spans (emitted by
  `crates/runtime/macros/src/lib.rs`) and the style-cache counters.
  Attribute a slow turn with phase counters until the kernel is
  instrumented.

On web, `backend_web::install_time_source()` must run at startup or every
timing reads `0` (the wasm32 monotonic-clock fallback). Turn
`debug-stats` **off** before quoting benchmark numbers — the spans
inflate per-op cost.

## Pitfalls

- **Reading back a signal you just wrote.** `set(v)` then `get()` in the
  same handler returns the *previous* value. Use `update(|cur| …)` for
  read-modify-write, or keep the intended value in a local.
- **Creating state in a handler panics.** Create at build time, capture
  the handle. `signal()`/`effect()`/`memo()`/`provide()`/`inject()` all
  need `World::enter`.
- **`on_cleanup` in a component body panics.** Put it inside an effect,
  or return the cleanup from the effect body.
- **Reading a stale handle panics.** Look for a handle that outlived its
  owning scope — typically a closure handed to a platform API that fired
  after the scope dropped. Fix by registering a cleanup that detaches the
  listener.
- **`touch()` on a `match` scrutinee does nothing.** The reactive
  `match` dedupes equal scrutinees; change the value instead.
- **Re-entrant `RefCell::borrow_mut`.** Common with backends holding
  state behind `Rc<RefCell<B>>` whose platform callbacks re-enter the
  framework. Restructure, or defer to a microtask so the outer borrow
  releases first.
- **`get()` clones.** `Signal<T>::get` requires `T: Clone` and clones out
  of the cell. Use `with(|v| …)` for a borrowed read.
