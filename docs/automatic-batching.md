# Automatic batching: every reactive turn is one cycle

**Status:** IMPLEMENTED — `crates/runtime/core` (`reactive::cycle`, the
`DirtyWindow` flush, and the per-entry-point wrappers).
**Tests:** `reactive::tests::cycle_coalesces_multiple_writes_to_one_fanout`,
`unbatched_writes_fan_out_per_write`, `pressable_handler_is_born_batched`,
`nested_cycle_joins_outer_window`.

## The model

A signal write propagates in three stages:

1. **Value cell — synchronous, always.** `with_signal_mut` writes the new
   value immediately; a `get()` on the next line sees it. Batching never
   defers the *value*, only the *notification*.
2. **Subscriber fan-out (effect re-runs + the native mutations they push).**
3. **Layout pass** — already coalesced to one pass at `REACTIVE_BUSY → 0`
   (the `ON_REACTIVE_IDLE` hook).

Historically stage 3 was coalesced but stage 2 was **eager**: each write
fanned out synchronously, so a handler that wrote five signals re-ran a
shared subscriber five times and pushed five rounds of native mutations,
only the last of which mattered. Explicit `batch(..)` fixed it where an
author (or a backend) remembered to call it — which was inconsistent
(some backends wrapped event handlers, some didn't).

Now stage 2 is coalesced too, **automatically**. Every *reactive turn* runs
as one `cycle`: writes queue, and the subscriber fan-out happens **once**,
at the end of the turn — still within the same synchronous tick, before
paint, so there is **no added frame latency**. This is end-of-turn
coalescing, not async deferral.

## `cycle()` — the turn boundary

`reactive::cycle(f)` is the primitive. It is mechanically identical to
[`batch`](../crates/runtime/core/src/reactive.rs) (it *is* `batch`); the
separate name marks the framework's automatic call sites and keeps them
greppable. `batch` remains the name author code uses to group writes
manually. Nesting composes: an inner `cycle`/`batch` joins the outer
window and only the outermost flushes — so wrapping a handler a backend
*also* happens to wrap is a harmless no-op.

Under the hood, `cycle`/`batch` opens a `DirtyWindow` that records dirtied
**signals** (not pre-collected subscribers); at the outermost close it
resolves each signal's change decision once, collects subscribers, and
fans out a single deduped effect pass. (This same window is what powers
net-zero [`set_if_changed`](signal-set-change-detection.md) dedup.)

## Where the framework opens a cycle automatically

The author never calls `cycle`/`batch`. The runtime wraps every *cycle
entry point*:

**Input event handlers — wrapped at attach, in core.** Handlers are stored
as `Rc<dyn Fn…>` on the element when a builder/primitive setter runs; each
setter wraps the closure so it is "born batched." Every backend then
invokes `(handler)(ev)` exactly as before and inherits batching with **no
per-platform code** — this is why the previous backend-by-backend `batch()`
wraps could be removed. Covered setters: `View::on_touch` / `on_wheel`,
`pressable`, `toggle` / `slider` / `text_input` / `text_area` `on_change`,
`text_input` / `text_area` `on_key_down`, the app-level key handler
(`set_app_key_handler`), `link` activation, `scroll_view::on_scroll`,
`overlay` / `portal` `on_dismiss`, `graphics` `on_resize` / `on_lost`,
`lazy::on_state`.

**Non-event entry points — wrapped at the scheduling/completion site:**
async completions (`resource`, `mutation`, `async_reducer`), scope-anchored
timers and frame loops (`after_ms_scoped`, `raf_loop_scoped`), the animation
clock (one cycle per frame, so all `AnimatedValue` writes in a frame
coalesce), and `reducer` dispatch.

### Adding a new entry point

When you add a primitive with an author callback, or a new place that
schedules signal-writing work, wrap it in `cycle` at the **attach /
schedule** site (not at the backend invocation), following the existing
setters. The convention: a handler is born batched; the backend just calls
it.

## What is *not* batched by default

`cycle` queues the fan-out; it does **not** dedup unchanged writes. A plain
`set` still always notifies (monotonic counters / force-refresh rely on
it). Net-zero elision is the separate, opt-in
[`set_if_changed`](signal-set-change-detection.md) lever, which composes
with the cycle window.

## Escape hatch

The rare case that needs an effect to have run *before the next line*
within a handler (a `flushSync`-style need) is not provided yet — it has
not been needed. If one arises, add a `flush_sync` that drains the current
`DirtyWindow` immediately rather than reintroducing per-write fan-out.
