# Automatic batching: every reactive turn is one flush

**Status:** IMPLEMENTED — `crates/runtime/world` (staged writes +
`World::flush`) and the per-backend flush drivers in
`crates/backend/*/src/newcore.rs`.
**Tests:** `crates/runtime/world/src/tests.rs` —
`set_stages_until_flush`, `writes_collapse_into_one_commit`,
`update_composes_on_the_staged_value`,
`an_effect_runs_once_per_flush_even_with_two_changed_deps`,
`redundant_writes_are_net_noops`,
`diamond_effect_runs_once_with_consistent_pair`,
`memo_chain_settles_in_one_flush_with_one_reaction_run`.

## The model

A signal write propagates in two stages, split at the flush boundary:

1. **Stage — synchronous, invisible.** `set(v)` records `v` as the
   signal's *pending* value and enqueues the slot on its world's staged
   queue (`stage_set` in `crates/runtime/world/src/lib.rs`). The
   committed value is untouched, so `get()`, `peek()`, `with()`, and
   `with_untracked()` all keep returning the previous value —
   everywhere, including inside the same handler
   (`impl_read_ops!`; `tests.rs::set_stages_until_flush`).
2. **Commit + fan-out — at the world's flush.** `World::flush` drains
   the staged queue, writes each pending value into its cell, decides
   per signal whether the change notifies, then runs the affected
   effects. Every write staged during the turn commits in that one
   flush (`tests.rs::writes_collapse_into_one_commit`), and a
   subscriber of several changed signals runs **once**
   (`an_effect_runs_once_per_flush_even_with_two_changed_deps`).

So coalescing is not a lever the author pulls — it is the only shape a
turn has. A handler that writes five signals produces one commit, one
deduped effect pass, and one layout/paint.

**What changed from the pre-v2 core.** The old walker's batching
deferred the *notification* only: "`with_signal_mut` writes the new
value immediately; a `get()` on the next line sees it." That sentence
described `runtime-core`'s `reactive::cycle` / `DirtyWindow` mechanism,
which no longer exists. Runtime v2 stages the *value* too, so
read-back-in-the-same-handler is the one authored pattern that changes
behavior — see
[`migrating-to-runtime-v2.md`](migrating-to-runtime-v2.md#reactive-semantics-writes-are-staged).

## Read-modify-write composes on the staged value

`update(|cur| next)` is the exception to "reads see the committed
value": its closure argument is the staged value when one exists, and
falls back to the committed value otherwise (`stage_update`). That is
what makes repeated increments in one turn compose
(`tests.rs::update_composes_on_the_staged_value`):

```rust
// Both get()s read the committed value → net +1.
count.set(count.get() + 1);
count.set(count.get() + 1);

// update composes on the staged value → net +2.
count.update(|n| n + 1);
count.update(|n| n + 1);
```

## `flush` — the turn boundary

`World::flush` is the primitive (`crates/runtime/world/src/lib.rs`).
Its algorithm, per outer round:

1. drain + commit the staged queue, collecting dirty effects (deduped);
2. run dirty **Derivations** (memo recomputes) to settlement — their
   own writes stage, commit, and wake further derivations, while
   Reactions collected along the way are held;
3. run each held **Reaction** exactly once;
4. a Reaction that staged writes opens the next outer round.

The Derivation/Reaction split is what makes the flush glitch-free: a
reaction only runs after every memo reachable from this round's commits
has recomputed, so an effect reading both `s` and `memo(|| s.get() * 10)`
can never see a fresh `s` with a stale memo
(`tests.rs::diamond_effect_runs_once_with_consistent_pair`,
`memo_chain_settles_in_one_flush_with_one_reaction_run`). A round limit
of 100 turns a cyclic update into a panic rather than a hang
(`cyclic_updates_panic_instead_of_hanging`), and re-entering `flush` on
the same world from inside its own effect panics — writes staged during
a flush are committed by that flush's next round
(`reentrant_flush_panics`).

## Where the framework flushes automatically

The author never calls `flush`. Each backend installs a **flush driver**
at the point where it invokes author code, and the driver commits after
that code returns. Two mechanisms, present in every backend's
`newcore.rs`:

1. **Author-callback wrapping.** The capability impls wrap every author
   callback before handing it to the platform machinery — press/click,
   input/change, toggle, slider, scroll, hover, wheel, touch, key,
   focus/blur, file-drop, image load/error, link activation, portal
   dismiss, graphics lifecycle, virtualizer row mount/release, and the
   app-level key handler. The wrapper calls the author fn, then
   `schedule_flush()` (web: one deduped
   `schedule_microtask` → `world.flush()`;
   `crates/backend/web/src/newcore.rs`).
2. **Post-dispatch hook.** Author code also runs from non-event
   surfaces: `after_ms` timers, `after_animation_frame` one-shots,
   `raf_loop` iterations, and executor-spawned future polls. The
   scheduler and the async executor fire a thread-local
   `dispatch_hook` after each such callback, and the boot entry
   installs `schedule_flush` into that slot. Every backend's
   `newcore.rs` carries this pair — `web`, `macos`, `ios`, `android`,
   `terminal`, `cpu`, `linux`, `windows`.

The commit therefore lands in the same tick as the event, before paint:
this is end-of-turn coalescing, not async deferral.

Backends whose host owns the cadence expose a synchronous escape:
`flush_sync()` (web, iOS, Android, terminal, CPU, Linux, Windows)
commits immediately, for bench drivers, robot verbs, and headless hosts
that must settle before reading a frame. Roku names the same contract
`settle()` — drain microtasks, then flush — and its embedder calls it
after dispatching `HandlerTable` events, before draining the command
queue (`crates/backend/roku/src/newcore.rs`). SSR is the degenerate
case: one flush per request after the tree realizes
(`crates/backend/ssr/src/newcore.rs`).

### Adding a new entry point

When you add a primitive with an author callback, or a new place that
schedules signal-writing work, wrap it at the **dispatch site** in the
backend's `newcore.rs` capability impl — call the author fn, then
`schedule_flush()`. Third-party primitive handlers that source events
outside the framework's wrapped sites (a `<form>` submit listener, an
iframe `message` event, an `NSToolbar` action) carry the same
obligation; see
[`external-export.md`](external-export.md).

## `batch` is gone

Staging makes every turn an implicit batch, so the explicit `batch(f)`
function has no job left and is not part of the author surface:
`crates/runtime/vocabulary/src/glue.rs` re-exports the reactive API
(`signal`, `effect`, `memo`, `untrack`, `on_cleanup`, the handle types)
and `batch` is absent. Delete the wrapper — the writes inside it
already coalesce.

## What is *not* coalesced away

Coalescing coalesces the fan-out; whether a committed write notifies at
all is a separate decision, made per signal at commit time
(`crates/runtime/world/src/tests.rs`):

- `set(v)` is equality-guarded (`T: PartialEq`). A staged value equal to
  the committed one notifies nobody, including an `A→B→A` round trip
  inside one turn that nets to zero
  (`set_skips_when_value_unchanged`, `set_net_zero_flush_window_skips_fanout`,
  `redundant_writes_are_net_noops`).
- `set_always(v)` stages and forces notification even on an equal value
  (`set_always_notifies_when_value_unchanged`,
  `set_always_taints_flush_window_forcing_notify`).
- `touch()` forces notification with no value write
  (`touch_notifies_without_writing`,
  `touch_taints_flush_window_forcing_notify`).
- `set_untracked(v)` writes the committed value directly, bypassing both
  staging and notification (`set_untracked_writes_without_notifying`);
  a later guarded `set` compares against that write
  (`set_untracked_then_equal_set_skips_fanout`).

`update` runs through the guarded path too — it stages, and the commit
compares. See [`reactivity.md`](reactivity.md#the-write-surface) for the
author-facing table.

## Reading the boundary from code

- `runtime_world::is_flushing()` — true while any world on the thread is
  inside `flush`, i.e. while effects are running by the reactive
  system's decision rather than an event handler's
  (`tests.rs::is_flushing_reports_active_flush`). This is the
  replacement for the old core's `is_reactive_busy`.
- `runtime_world::is_entered()` — true while a live world is ambient, so
  creation-side APIs (`signal`, `effect`, `provide`/`inject`) are legal.
  Handlers run with this `false`; see
  [`reactivity.md`](reactivity.md#handlers-run-outside-the-world).
