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

## Caveats

- **First load is slower than React.** The wasm has to download
  + instantiate. The benchmark's rebuild slot only measures *after*
  initial mount, so it doesn't affect the readout — but a cold-start
  slot would tell a different story. See the
  spec's "What we deliberately do NOT measure" section.
