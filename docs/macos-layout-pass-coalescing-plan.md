# macOS layout-pass coalescing — navigation multi-pass

Status: **open.** A navigation on the AppKit backend can run several
full-tree layout passes; the guard that collapsed them is no longer wired
to anything (see "Current state").

## Symptom

Changing screens on macOS is laggy, and worse for reactive-heavy pages.
Diagnosed with the Robot bridge + `IDEALYST_LAYOUT_TRACE=1`: a single
navigation fired several full-tree layout passes.

## Root mechanism

The macOS reactive-idle hook (`install_global_self` in
[`crates/backend/macos/src/imp/mod.rs`](../crates/backend/macos/src/imp/mod.rs))
runs `flush_pending_layout_pass()` at **every** reactive-busy → idle boundary
whose `LAYOUT_PASS_QUEUED` flag is armed. A navigation crosses many such
boundaries, and each flush is a **full-tree** `run_layout_pass_global`
(compute from the host root + apply frames). So passes = (number of reactive
windows that arm the flag) × (full-tree cost per pass).

## Current state

- **`LayoutCoalesceGuard`** (`coalesce_layout_passes()` in `imp/mod.rs`):
  while held, `run_pending_layout_pass` early-returns (the flag stays armed)
  and one pass runs on drop. The decision is the pure
  `layout_policy::coalesced_swap_suppresses_pass`, pinned by
  `coalesce_suppresses_only_while_a_swap_is_in_flight`. Its only caller was a
  macOS drawer-navigator dispatcher that no longer exists; the swap and stack
  navigators do not hold it, so nothing coalesces a screen swap today.
- **Duplicate route fan-out** is gone at the kernel level: `set` is
  equality-guarded and commits once per flush, so a route written twice in
  one turn wakes its subscribers once.

## Options

1. **Hold the guard across the navigator's screen swap.** The cheapest fix:
   wrap mount + insert + active-route change in `coalesce_layout_passes()`
   wherever the navigator SDKs swap a screen on macOS.
2. **Per-run-loop-turn layout coalescing.** Replace the per-reactive-window
   synchronous idle flush with a `CFRunLoopObserver` (`beforeWaiting`) that
   runs at most **one** `run_layout_pass_global` per run-loop turn. The
   synchronous flush exists for flicker-free dynamic updates (rows inserted by
   an event must be sized before the same turn paints — see the
   `flush_pending_layout_pass` doc comment); a `beforeWaiting` observer still
   runs before paint, but needs verification across the flicker cases (tree
   expand/collapse, presence / `when` mounts, scroll-document sizing).
3. **Subtree-incremental layout.** Relayout only the changed subtree instead
   of the whole host tree each pass, making the pass *count* matter far less.
   A larger change to the layout pass and `runtime-layout`; also benefits
   every reactive restyle.

## How to measure

1. Enable `debug-stats` on the app (forward to
   `runtime-vocabulary/debug-stats` and the backend's `debug-stats`; see
   CLAUDE.md §6 — there is no `runtime-core/debug-stats`).
2. Run with `IDEALYST_LAYOUT_TRACE=1`; each `[layout-trace] pass …` line is
   one full-tree pass.
3. Drive navigation over the Robot bridge (`find_element` → the `link` →
   `click`) and count passes per navigation.
