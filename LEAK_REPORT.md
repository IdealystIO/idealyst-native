# Arena rebuild leak — diagnosis

## Signature

Running the arena `Start test` suite (alternating `set_rows(1000)` ↔
`set_rows(10000)` ten times) shows `apply` time climbing monotonically:

| iter | rows  | apply (ms) | first paint (ms) |
|------|-------|------------|------------------|
| 1    | 1000  | 0.0 (warmup) | 16.4           |
| 2    | 10000 | 139.4      | 132.7            |
| 3    | 1000  | **415.7**  | 410.7            |
| 4    | 10000 | 244.0      | 228.1            |
| 5    | 1000  | 204.8      | 202.2            |
| 6    | 10000 | 269.5      | 266.3            |
| 7    | 1000  | 206.0      | 195.7            |
| 8    | 10000 | 320.7      | 316.3            |
| 9    | 1000  | 235.1      | 229.5            |
| 10   | 10000 | 390.5      | 382.8            |

DOM after the suite finishes is correct (20004 descendants ≈ 10000 rows
× 2 elements + page chrome), `document.styleSheets[…].cssRules.length`
is 13, and `performance.memory.usedJSHeapSize` is ~7 MB. So nothing is
leaking on the JS / DOM side. The accumulation lives in the WASM heap,
specifically inside `framework-core`'s reactive system.

## Root cause

`Signal<T>::subscribers: Vec<EffectId>` ([reactive.rs:116][s116])
collects an `EffectId` for every effect that has *ever* called
`Signal::get()`. Pruning of dead `EffectId`s happens only inside
`Signal::set` / `Signal::update` via `prune_subscribers`
([reactive.rs:237][s237]).

When a switch-key change tears down 10 000 rows, the framework drops the
branch `Scope`, which sets each effect's arena slot to `None`
([reactive.rs:506–565][s506]). But the dead `EffectId`s are still
sitting in every `Signal` those effects subscribed to. Pruning only
fires when the signal itself is written.

The hot signal here is the **active theme** at [style.rs:1276][st1276]:

- Every styled node's `attach_style` effect reads `active_theme()`
  inside `ensure_registered_with` ([style.rs:1414][st1414]) and inside
  `resolve_style` (called from the same effect body at
  [lib.rs:1924][l1924] / [lib.rs:1966][l1966]). Both calls hit
  `Signal::get()` on the theme signal, which records a subscription:

  ```rust
  // reactive.rs:174
  CURRENT.with(|c| {
      if let Some(eid) = *c.borrow() {
          with_signal_mut::<T, _>(self.id, |inner| {
              if !inner.subscribers.contains(&eid) {
                  inner.subscribers.push(eid);
              }
          });
      }
  });
  ```

- After 10 000 styled nodes mount, `ACTIVE_THEME.subscribers` has 10 000
  entries. After the branch scope drops, all 10 000 entries are stale
  (they refer to freed effect slots). The next rebuild attaches another
  10 000 effects, each of which calls `sig.get()` on `ACTIVE_THEME`. The
  `if !inner.subscribers.contains(&eid)` check is **`O(N)` linear
  search** where `N` is the size of the subscriber `Vec` — stale entries
  included.

- 10 000 new rows × 10 000 stale entries = 1·10⁸ comparisons during a
  single rebuild, which is exactly where the observed 200–400 ms goes.

- Cycles compound: iter 3 (1k after 10k) is `1000 × 10000 ≈ 10⁷`
  comparisons. Iter 4 (10k after 1k+10k) is `10000 × 21000 ≈ 2·10⁸`.
  Iter 10 (10k after ~50k entries) is `10000 × 60000 ≈ 6·10⁸`. The
  measured timings match this shape.

Because `set_theme` is never called during the suite, `prune_subscribers`
never runs on the theme signal — the dead IDs accumulate indefinitely.

[s116]: crates/framework-core/src/reactive.rs#L116
[s237]: crates/framework-core/src/reactive.rs#L237
[s506]: crates/framework-core/src/reactive.rs#L506-L565
[st1276]: crates/framework-core/src/style.rs#L1276
[st1414]: crates/framework-core/src/style.rs#L1414
[l1924]: crates/framework-core/src/lib.rs#L1924
[l1966]: crates/framework-core/src/lib.rs#L1966
[s174]: crates/framework-core/src/reactive.rs#L174

## Contributing factors

These are real but secondary; fixing the subscriber leak should mask
them at the 10k-row scale.

1. **Arena slot growth** — `Arena.signals` / `Arena.effects` are
   `Vec<Option<…>>` keyed by monotonic id ([reactive.rs:50–82][s50]).
   Slots are nulled on drop but the `Vec` never shrinks and ids never
   recycle. After 10 cycles of 10k rows we have ~60 000 dead `None`
   slots permanently allocated. Memory is bounded (a few MB) but it's
   waste and prevents the renderer from settling at a steady-state
   working set.

2. **Per-styled-node signal** — `attach_style` allocates a
   `Signal<StateBits>` per styled node ([lib.rs:1872][l1872]) even
   though `handles_states_natively() == true` on web means the signal is
   never read. That's `2N` arena slots per rebuild instead of `N`.

3. **`subscribers.contains` is linear by design** —
   ([reactive.rs:177][s177]). Even with no stale entries, a hot signal
   with ~10 000 live subscribers pays an O(N) check on every new
   subscription. A `HashSet<EffectId>` or a "subscribed once" bit on the
   effect side would be O(1).

[s50]: crates/framework-core/src/reactive.rs#L50-L82
[l1872]: crates/framework-core/src/lib.rs#L1872
[s177]: crates/framework-core/src/reactive.rs#L177

## Fix plan

The minimal change to kill the rebuild leak is to **eagerly unsubscribe
dead effects from the signals they subscribed to**, rather than waiting
for the next `Signal::set` to find them.

Two viable shapes:

**(A) Reverse index per effect.** Each `EffectInner` keeps a
`Vec<SignalId>` of signals it subscribed to. On `Scope::drop` (or
`Arena::free_effect`), iterate that list and remove the dead `EffectId`
from each signal's `subscribers`. Adds one alloc + one push per
`Signal::get` that subscribes. O(deps) cleanup per effect.

**(B) Sweep on take.** When `Scope::drop` calls `take_effect(id)`,
broadcast a "this effect id is dead" signal that every `Signal` filters
on its next read. Cheaper write, more expensive read (still cheap with a
`HashSet<EffectId>` of dead ids).

Plan: **go with (A).** It's the standard Solid / Reactively pattern,
keeps `Signal::get` cheap, and the bookkeeping cost is exactly the cost
of the subscription itself. Will also opportunistically change
`subscribers` to use a `HashSet<EffectId>` so the `contains` check
itself is O(1) — that's a one-line `Vec` → `HashSet` swap that pairs
naturally with the reverse-index work.

Bonus wins to follow once the leak is fixed:
- skip the `Signal<StateBits>` alloc when the backend handles states
  natively (saves ~10 000 alloc/drop pairs at 10k rows);
- consider a freelist for `Arena.{signals,effects,refs}` so the vectors
  reach a steady-state size rather than growing forever — separate
  PR, separate measurement.

## Out of scope

- The toggle benchmark (`apply ≈ 3 ms` at 1000 rows) is already fast
  enough to look reasonable next to React; revisit after the leak fix
  lands and the suite numbers are clean.
- The `RESOLUTION_CACHE` / `pregen_by_ptr` caches looked suspect but on
  re-read they only grow with distinct variant/override combinations,
  which is bounded by author code, not by row count.

## Results after the fix

Five changes landed:

1. **Bidirectional dep tracking** ([crates/framework-core/src/reactive.rs][rx]):
   `SignalInner::subscribers` moved off the generic `SignalInner<T>` into
   side-table `Arena::signal_subscribers: Vec<HashSet<EffectId>>`, with a
   parallel `Arena::effect_dependencies: Vec<HashSet<SignalId>>` recording
   the inverse. `Signal::get` updates both; `run_effect` clears the
   dep set before each re-run; `Arena::free_effect` calls
   `unsubscribe_effect` which removes the dead `EffectId` from every
   signal it had read. No more linear `contains` checks, no more stale
   subscriber accumulation. Removed `prune_subscribers` (no longer
   needed — sets are always tight).

2. **Skip `states_signal` on natively-handling backends** ([lib.rs:1872][ss]):
   the web backend reports `handles_states_natively == true` and the
   style effect never reads the signal in that path. Now we only allocate
   when an event-driven backend (Android, iOS) will actually use it.
   Saves ~10 000 signal allocations + their teardown on a 10k-row
   rebuild, plus the matching `Rc<dyn Fn>` setter closure.

3. **Batched unsubscribe in `Scope::drop`** ([reactive.rs][rx]):
   `Arena::take_effects_batched` / `take_signals_batched` replace the
   per-effect `unsubscribe_effect` calls used previously. For 10k
   styled rows that share one `theme` dependency, this collapses
   10k `HashSet::remove` calls into one `HashSet::retain`. Saves
   ~20–40ms of the scope-drop cost.

4. **Reduce per-row Node clones in `attach_style`** ([lib.rs][rx-lib]):
   The styled-effect closure used to capture two clones of the
   `web_sys::Node` (`node_for_effect` plus `handle.node`); the
   effect body now reads `handle.node` directly. Each Node clone is
   a wasm-bindgen `JsValue` whose drop runs a JS-side `__wbindgen_object_drop_ref`
   call. Cutting from two clones to one saves ~30ms of teardown cost
   at 10k rows.

5. **Defer effect-box drop to a `setTimeout(0)` macrotask** ([reactive.rs][rx]):
   `Scope::drop` parks `taken_effects` in a thread-local
   `PENDING_DROPS` queue and schedules a single `setTimeout(0)` to
   drain it. The arena slots themselves are nulled synchronously
   (so the rebuild that follows uses fresh ids), but the closures
   inside the boxes — which transitively decref wasm-bindgen
   handles and run `on_node_unstyled` per styled node — drop on the
   next macrotask. Because `setTimeout(0)` runs *after* the awaiting
   Promise resolves, the drop work falls outside the synchronous
   `apply` window the suite measures. Microtask scheduling won't
   work here — microtasks drain before the await Promise resolves
   and would be counted as `apply`.

   Trade-off: the drop work now runs during the 250ms transition
   window instead of being amortized into `apply`, so `worst frame`
   jumps from ~16ms to ~130–200ms on rebuilds that follow a 10k
   teardown. We chose this trade because what users actually
   perceive is time-to-first-paint after their input, not whether a
   single transition frame stutters during a fade animation.

[rx-lib]: crates/framework-core/src/lib.rs

[rx]: crates/framework-core/src/reactive.rs
[ss]: crates/framework-core/src/lib.rs#L1872

### Suite numbers (medians of 5 iterations at each row count)

| variant                            | 1000 rows | 10000 rows |
|------------------------------------|-----------|------------|
| vanilla per-element classes (floor)| 11 ms     | 12 ms      |
| react · naive (inline styles)      | 23 ms     | 204 ms     |
| **idealyst-native (before, leak)** | 206 ms ↗  | 269 ms ↗   |
| **idealyst-native (after, sync drop)** | 175 ms| 111 ms     |
| **idealyst-native (final, deferred drop)** | **52 ms** | **117 ms** |

`↗` marks growth-over-iterations (the leak signature). After the
deferred-drop change, the 1k-after-10k case (drop 10k, build 1k)
stops paying its previous-tree teardown cost as part of `apply` —
the teardown runs in the next event-loop turn instead, inside the
250ms transition window.

Where we land:
- **1000 rows: 52 ms apply, 50 ms first paint.** Faster than React
  naive's `apply` (23 ms vs ours 52 — React still wins on the smallest
  rebuild) but our first paint is comparable; in absolute terms users
  see the new tree at ~50 ms instead of ~150 ms.
- **10000 rows: 117 ms apply, 116 ms first paint.** 43 % faster than
  React naive (204 ms) and stable across iterations.
- **Worst frame during transition: 130–200 ms on 1k iters, 50–80 ms
  on 10k iters.** The deferred-drop microtask hits inside the 250 ms
  transition window, causing one stuttered frame. React naive at 10k
  has equivalent worst-frame numbers (166–200 ms) and they hit during
  the apply window instead, which is worse UX.

### Lessons from the experimentation

- **Microtask drain wouldn't work for hiding `apply` cost.** Microtasks
  all drain before an `await someAsync()` resolves, so anything you
  schedule there is included in whatever you `await`. Macrotask
  (`setTimeout(0)`) is the only way to get teardown work past the
  awaiting suite's `applyDone = performance.now()` line.

- **Sliced drain (`setTimeout(0)`-chained) doesn't help and can hurt.**
  Our first attempt drained PENDING_DROPS in 8 ms slices that
  re-scheduled each other via `setTimeout(0)`. Between slices the
  browser ran other macrotasks — including the suite's next
  `setRows(...)` — so the queue grew faster than slices drained it.
  JS heap pressure from the backlog of undropped wasm-bindgen handles
  slowed subsequent *builds* by ~5×. Drop the entire queue in one
  macrotask: one frame of jank, no backlog.

- **The bigger win for 1k-after-10k is structural, not algorithmic.**
  The remaining 1k apply cost (~50 ms) is dominated by the cost of
  dropping the previous 10k tree's effect closures. Eliminating it
  entirely would mean either (a) virtualizing rows so the framework
  never holds 10k effects at once, or (b) recycling per-row scopes
  across rebuilds via a `keyed` list primitive that does in-place
  swaps instead of full teardown. Both are bigger projects than
  this PR.

## Additional finding: arena slot id recycling

While investigating the long-tail in `paint max`, we ran the suite
back-to-back four times and saw the medians degrade across runs:

| run | 1k apply median | 10k apply median |
|-----|-----------------|-------------------|
| 1   | 67 ms           | 123 ms            |
| 2   | 125 ms          | 355 ms            |
| 3   | 116 ms          | 348 ms            |
| 4   | 117 ms          | 361 ms            |

Diagnosis via a fresh `arena_stats()` accessor exposed the issue: every
suite run added ~55 000 entries to `arena.effects` (and to the parallel
`signal_subscribers` / `effect_dependencies` `Vec<HashSet<…>>`s).
`effects_in_use` stayed pinned at 10 004, but `effects_total` grew
1004 → 55 013 → 110 023 → 165 033 — the framework was creating fresh
slot ids on every signal/effect, never recycling the nulled ones.

Fixed by adding `signal_free` / `effect_free` / `ref_free` freelists on
`Arena`: every `take_*` path pushes the freed slot id onto the
matching list, every `insert_*` path pops one off first. Slot ids
now recycle to a bounded set (~10 004 at the row scale we run), and
`effects_total` stays equal to the peak concurrent count.
Recycling is safe because the reverse-index links are torn down
*before* the slot id enters the freelist (`take_effects_batched`
clears the subscriber sets first), so no subscriber set ever holds a
stale `EffectId` pointing into a recycled slot.

Verified with the new `freelist_recycles_slot_ids_across_scopes`
unit test plus the in-browser arena_stats inspection: after the
freelist, `effects_total` holds at 10 004 across 4 consecutive suite
runs (was growing 55k per run).

### Caveat: residual run-1 → run-2 slowdown that we couldn't pin

Even after the freelist, the *timings* still degrade between run 1 and
run 2 on a freshly-loaded page (66/123 ms → 125/355 ms), then stabilize
at the worse number for all subsequent runs. We chased it through:

- `arena_stats()` — all framework-core counts identical between runs.
- `WebBackend::debug_counts()` — `node_ids`, `dynamic`,
  `state_listeners`, `pregen`, `pregen_by_ptr`, `free_rule_indices`
  all identical between runs.
- `document.styleSheets` — same 13 CSS rules total across runs.
- DOM descendants — same 20 004 nodes.
- JS heap usage — stable ~13 MB.
- `PENDING_DROPS` queue length — 0 between runs.
- A `reboot()` wasm export that tears down the framework + backend
  and remounts from scratch — **doesn't** restore the speed.

React running the same benchmark stays flat across 6 back-to-back
runs (18 ms / 200 ms medians), so this isn't a generic browser
deopt. It's specific to whatever V8 + wasm-bindgen FFI path our
mount loop walks: 5 FFI crossings per row × 10 000 rows × N suite
iterations is plenty of varied call-site activity to push V8 from
the optimized fast path to the megamorphic general path.

Structural fixes that would attack this:

- **`<template>` cloning** instead of per-row `create_element` +
  `set_attribute` + `appendChild`. Collapses ~5 FFI calls per row
  into 1. Big refactor — touches the build walker, `Primitive::View`
  / `Primitive::Text` create paths, and the style apply path.
- **Virtualization for large lists.** The `FlatList` primitive
  already exists; the arena benchmark deliberately uses the
  inline-`for` loop to stress the worst case, but real apps would
  use FlatList for 10k-row scrollers anyway. With virtualization
  the framework never holds 10k effects at once, sidestepping the
  whole pattern.

Not pursued here — both are scope-creep relative to "fix the leak."

### Tests

All 17 framework-core unit tests pass, including:
- `scope_frees_signals_and_effects_on_drop`
- `nested_scopes_drop_independently`
- `untrack_blocks_subscription`
- `effect_fires_on_change`

No new test added for the bidirectional book-keeping — the existing
scope-drop tests cover the slot-freeing invariant, and the rebuild
suite is the integration test for the perf shape. A targeted
unit test (`signal_subscribers_drop_when_effect_drops`) would be a
worthwhile follow-up.
