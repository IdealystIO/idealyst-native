# macOS layout-pass coalescing — navigation multi-pass

Status: **partially addressed**. Two contributing causes fixed; the dominant
remainder (deferred `switch` builds × full-tree passes) is deferred — captured
here so the next person hits the plan, not the symptom.

## Symptom

Changing screens in a `DrawerNavigator` app (e.g. `examples/idea-ui-docs`) on
macOS is laggy, and the lag is worse for reactive-heavy pages. Diagnosed with
the Robot bridge + `IDEALYST_LAYOUT_TRACE=1` (and a throwaway
`IDEALYST_PASS_ORIGIN=1` backtrace dump in `schedule_layout_pass`): a single
navigation fired **3–9+ full-tree layout passes** (Checkbox page: ~13).

## Root mechanism

The macOS reactive-idle hook (`install_global_self` in
[`crates/backend/macos/src/imp/mod.rs`](../crates/backend/macos/src/imp/mod.rs))
runs `flush_pending_layout_pass()` at **every** `REACTIVE_BUSY → 0` boundary
whose `LAYOUT_PASS_QUEUED` flag is armed. A navigation crosses many such
boundaries, and each flush is a **full-tree** `run_layout_pass_global` (compute
from the host root + apply frames). So passes = (number of reactive windows that
arm the flag) × (full-tree cost per pass).

Three distinct sources, measured on the Checkbox nav:

| # | Source | Passes | Status |
|---|--------|--------|--------|
| 1 | Build-time nested passes — one reactive window per `attach_style` effect first-run while building the incoming screen; a full pass fired *inside* each (seen as a 19ms `attach_style_effect_alloc`, `switch_mount_build_branch` 99ms) | ~4 | **Fixed** |
| 2 | Duplicate route fan-out — `NavigatorControl::dispatch` writes `active_route`/`active_path`, and the SDK's `active_changed` re-writes the same two; `set` always-notifies, so chrome subscribers (sidebar items, header route `switch`) woke twice | ~1–2 | **Fixed** |
| 3 | Deferred `switch` branch builds — `walker::when_switch` builds **every** branch in a `schedule_microtask`, even on first mount, so a screen's N switches build across N separate turns *after* the swap, each build's reactive windows → a full pass via the idle hook | ~8 | **Open** |

A separate, compounding factor — every pass is **full-tree** (~19ms after the
`view_to_layout` reachable-scope fix; see
[`project_macos_apply_frames_reachable_scope`](../) memory) — multiplies all of
the above.

## What's fixed

- **#1 — `LayoutCoalesceGuard`** (`coalesce_layout_passes()` in `imp/mod.rs`,
  used by the drawer dispatcher in
  [`crates/sdk/navigators/drawer/src/macos.rs`](../crates/sdk/navigators/drawer/src/macos.rs)).
  Held across `mount_screen` + `insert` + `active_changed`;
  `run_pending_layout_pass` early-returns while it's held (the flag stays armed),
  and one pass runs on drop. Synchronous build cost dropped ~175ms → ~5ms; the
  19ms stall inside a single effect first-run is gone. Decision is the pure
  `layout_policy::coalesced_swap_suppresses_pass`; regression
  `coalesce_suppresses_only_while_a_swap_is_in_flight`.
- **#2 — batched navigation** (`reactive::batch` around the route/path writes +
  dispatch in
  [`crates/runtime/core/src/primitives/navigator/shared.rs`](../crates/runtime/core/src/primitives/navigator/shared.rs)
  `NavigatorControl::dispatch`). Cross-backend: a signal written twice in one
  window wakes its subscribers once. Regression
  `one_navigation_wakes_route_subscribers_once_despite_duplicate_writes`.

## What's open (#3 + full-tree cost) — recommended fix

The deferred-`switch` passes can't be coalesced from the navigator: each switch
builds on its own microtask turn after the swap guard has dropped. Two ways to
fix it at the root, both substantial and deferred for now:

1. **Per-run-loop-turn layout coalescing (preferred).** Replace the
   per-reactive-window synchronous idle flush with a `CFRunLoopObserver`
   (`beforeWaiting`) that runs at most **one** `run_layout_pass_global` per
   run-loop turn. Then any number of reactive windows / switch microtasks within
   a turn collapse to a single pass.
   - Risk: the synchronous idle flush exists for flicker-free dynamic updates
     (rows inserted by an event must be sized before the same turn paints — see
     the `flush_pending_layout_pass` doc comment). A beforeWaiting observer still
     runs before paint, so this *should* preserve that, but it needs careful
     verification across the existing flicker cases (tree expand/collapse,
     presence/when mounts, scroll-document sizing).
   - Note: only merges switch builds that land in the **same** turn. If
     `schedule_microtask` spreads them across turns, also consider draining all
     pending reactive microtasks before the layout observer fires, or building
     `switch` branches inline (see below).

2. **Subtree-incremental layout.** Relayout only the changed subtree instead of
   the whole host tree each pass (~1ms vs ~19ms), making the pass *count* far
   less important. Larger change to `compute_and_apply_layout` /
   `runtime_layout`; also benefits every reactive restyle, not just navigation.

A smaller, orthogonal option for #3 specifically: have `when_switch` build the
initial branch **inline** during the walker build (like the hydration path
already does) instead of deferring the first build to a microtask — so a
screen's switches build inside the swap guard and coalesce with it. Needs to
preserve the borrow-safety / nav-context reasons the deferral exists.

## How to re-measure

1. Build the generated macOS wrapper with profiling on:
   `target/idealyst/<proj>/macos/` → add `debug-stats = ["runtime-core/debug-stats"]`
   to its `[features]`, `cargo build --features dev,debug-stats`.
2. Run the binary directly with `IDEALYST_BRIDGE_PORT=<port>` +
   `IDEALYST_LAYOUT_TRACE=1` (no relay URL → self-hosts the TCP bridge).
3. Drive navigation over the bridge (newline-delimited JSON `{id,cmd,args}`:
   `find_element` → climb to the `link` → `click`); read `get_perf_counters`
   per nav. Each `[layout-trace] pass …` line is one full-tree pass; the
   `idle-fires` counter is reactive-window boundaries.
