# benchmark · idealyst-native

The framework's web backend in the benchmark head-to-head.

## What this measures

Same rebuild benchmark as every other variant. The runner alternates
between two row counts (default 1000 ↔ 10000); on each `set_rows(n)`
the framework's `Switch` re-fires, drops the previous row scope, and
builds a fresh tree at the new size — recording the synchronous JS
time + post-apply frame cadence.

## Build

Unlike the vanilla / React variants, this one requires a one-time
wasm build. From this directory:

```bash
cd wasm
wasm-pack build --target web --release
```

That produces `wasm/pkg/` with the JS shim + `.wasm`, which
`index.html` imports directly. Rebuild after any change to
`wasm/src/lib.rs` or anything in `crates/`.

The wasm is built with `opt-level = "z"` + `lto = true` + `wasm-opt
-Oz` (see `wasm/Cargo.toml`). The `debug-stats` feature is on for
the diagnostic exports (`bench_stats_json`, `debug_take_counters_json`)
which the runner doesn't read but help when poking from the devtools
console.

## What's idiomatic here

- A `stylesheet! { … }` block per stylized region. The `parity`
  variant on `PerfRow` is the same shape the example app uses.
- A reactive `Signal<usize>` for `rowCount`; the screen's `match`
  re-builds the row subtree on change.
- The page chrome (`Page` / `PerfList`) is framework-rendered too;
  participates in the rebuild on each `set_rows`.

## Old-core vs new-core gate (idea-lite migration)

The same bench source builds against either core (see
`wasm/Cargo.toml`):

```bash
cd wasm
wasm-pack build --target web --release                                  # old core → pkg/
wasm-pack build --target web --release --out-dir pkg-new \
  -- --no-default-features --features new-core                          # new core → pkg-new/
```

`measure.html` is the A/B harness: serve this directory
(`python3 -m http.server PORT`) and open
`measure.html?pkg=pkg` / `measure.html?pkg=pkg-new`, then run
`await window.bench.run()` from devtools (or Playwright).

### Methodology

- Every wasm export returns with the reactive work committed on both
  cores: the old core applies writes synchronously; the new-core
  exports stage writes then call `backend_web::newcore::flush_sync()`
  before returning (the `commit()` helper in `src/lib.rs`).
- One measured iteration = optional untimed state prep → settle →
  `t0 = performance.now()` → the op call → settle until a
  DOM/computed-style predicate holds → `t1`. Settling drains the
  microtask queue first (both cores deliver batched text updates in a
  microtask; the drain avoids rAF's ~16 ms quantization) and only
  falls back to rAF polling if the predicate isn't yet true.
- Per op: 2 warmup iterations discarded, 9 measured, **median**
  reported. Same browser, same machine, both variants in one session.
- Sub-0.3 ms ops (single-row bumps) sit at the timer's noise floor;
  for those the gate is judged with an absolute floor of ±0.1 ms
  rather than ±5 % relative.

### Gate status (2026-07-28 third wave, headless Chromium, M-series macOS)

Medians of 9 (`bench.run()`), same machine, both variants back-to-back;
create/teardown re-measured with 21 isolated samples
(`bench.measure(op, 3, 21)`, fresh page — a full `run()` leaves the app
in hierarchy mode and `set_rows` doesn't reset it) for the verdicts.

| op | old core (ms) | new core (ms) | delta | gate ±5 % |
|---|---|---|---|---|
| create_1k | 1.7 | 1.6 | new faster (21-sample) | **pass**¹ (was +24 %) |
| create_10k | 15.4 | 14.0 | −9 % (21-sample) | **pass**¹ (was +23 %) |
| teardown_10k | 3.0 | 3.0 | 0 % (21-sample) | **pass** (was +253 %) |
| theme_toggle_1k | 10.8 | 11.0 | +1.9 %; ±40 % run-to-run | **pass**² |
| granular_bump_1k | 0.0 | 0.0 | — | pass |
| bump_range_100 | 0.1 | 0.1 | — | pass (floor) |
| rstyle_shared_1k | 2.9 | 2.9 | 0 % | **pass** (was +164 %) |
| rstyle_point | 0.2 | 0.2 | — | pass (floor) |
| sclass_shared_1k | 0.8 | 0.9 | — | pass (floor) |
| hier_global_1k | 0.1 | 0.1 | — | **pass** (was +1700 %) |
| hier_branch | 0.0 | 0.0 | — | pass |

What closed the gaps (all with tests; scene-parity goldens unchanged and
shared — the new `full_repeat_fallback` scenario passes byte-identical
on both cores):

- **Repeat batch port**: `for i in 0..n { row }` lowers (same
  recognition conditions as the old `Element::Repeat`) to
  `Element::Many(RepeatPrim)`; the vocabulary's `handlers/repeat.rs`
  drives the SAME one-FFI `execute_batch_with_attach` + bulk-cohort
  path the old walker used. Fallback = per-row mounts + one
  `insert_many`, op-stream-identical to the old core.
- **Resolve seeding** (`runtime_core::pregenerate_and_seed`): the
  per-world sheet registry now seeds the per-sheet pointer-keyed
  resolve fast path at registration — before this every `resolve()`
  took the slow ResolutionKey path and returned pointer-distinct rules,
  defeating `mint_style_class`'s pointer cache (≈2× the enqueue loop).
- **JsBinding text port**: f-string slots carry `Signal::raw_id`;
  live-only assemblies produce `TextSourceProp::JsBinding` →
  `register_reactive_text_binding` + ONE world-root notifier effect per
  signal (`notify_signal_text_js`, the signal-class delivery pattern).
  hier_global's per-leaf effects are gone.
- **Kernel stable-deps reconcile** (`runtime-world`): effect re-runs
  collect reads into a pending frame and DIFF against the previous dep
  set — identical deps touch no subscriber list. The old
  unsubscribe-all (`retain`) + resubscribe (`contains`) scheme was
  O(subscribers) each — quadratic for the 1k-effects-on-1-signal
  shared-style fan-out.
- **Teardown**: batched rows carry no per-node live structure (no
  10k `LiveNode`/handle clones), no per-row `on_node_unstyled` (a
  guaranteed no-op for batch rows — see the `BatchOps` contract), and
  the bulk-cohort member release rides `after_ms_detached` out of the
  synchronous window (the old core's `install_drop_deferral` parity).

Residual attribution (PhaseTimer, debug-stats builds —
`pkg-prof`/`pkg-new-prof`, CLAUDE.md §6):

1. **create residual — CLOSED (was +23 %, ≈0.33 µs/row)**: the third-wave
   re-profile (symmetric `oc_repeat_row_build`/`nc_repeat_row_build`
   timers) put the whole delta in per-row payload CONSTRUCTION
   (old 0.59 µs/row vs new 0.92 µs/row over 60k rows; everything
   downstream — `execute_batch_*`, resolve, bulk cohort — at parity).
   Root cause was payload SIZE, not the builder-pattern shape:
   `StyleProp::Sheet` carried a `StyleApplication` inline (~2.3 KB —
   the `overrides: StyleRules` field), which set `ViewPrim`/`TextPrim`
   to ~2.5 KB, and every glue-wrapper/builder move + `PrimCell` boxing
   + `take()` memcpy'd the whole struct per row. Fix: box the big
   `StyleProp` variants (`Sheet`, `SignalClass`) and
   `TextSourceProp::JsBinding` — prim payloads dropped to ~200-250 B
   (pinned by `runtime-vocabulary/tests/payload_sizes.rs`) and the
   new core now BEATS the old core on create (the old walker still
   moves its ~2.7 KB `Element` enum inline). Template stamping is no
   longer needed for this gate.
2. **theme toggle**: browser recascade of 1k transitioned rows; ±40 %
   run-to-run — same token-cascade delivery both cores.

### Cross-core rebuild trigger

The pre-migration bench retriggered the top-level Switch with
`MODE.touch()` (notify-without-write). The new core's `switch` keys
the mounted arm on the scrutinee VALUE (`PartialEq` dedup), so a
touch is deliberately inert there. The bench now puts a
`REBUILD_GEN` generation signal into the match scrutinee tuple and
bumps it from every `set_rows`/`setup_*` export — a value change
both cores honor identically.

## Caveats

- **First load is slower than React.** The wasm has to download
  + instantiate. The benchmark's rebuild slot only measures *after*
  initial mount, so it doesn't affect the readout — but a cold-start
  slot would tell a different story. See the
  spec's "What we deliberately do NOT measure" section.
