# arena · idealyst-native

The framework's web backend in the arena head-to-head.

## What this measures

Same theme-toggle benchmark as every other arena variant. 1000 rows;
on click, `set_theme(...)` fires; the framework's per-styled-node
`Effect`s re-fire and re-apply against the new theme; `runToggle`
records the synchronous JS time + frame cadence.

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

Note: `debug-stats` is **off** for the benchmark build — the
telemetry would add ~200ms of `performance.now()` × event-log
overhead and skew the numbers. The wasm is built with
`opt-level = "z"` + `lto = true` + `wasm-opt -Oz` (see
`wasm/Cargo.toml`).

## What's idiomatic here

- A `stylesheet! { … }` block per stylized region. The `parity`
  variant on `PerfRow` is the same shape the example app uses.
- `set_theme(...)` is what the framework exposes for theme swaps.
  It fires every subscribed `apply_style` effect; each one calls
  `resolve_style(&app)` (hits the per-sheet variant cache) and
  `apply_styled_states` (hits the pointer-keyed pregen cache).
- The button + stats span live as static HTML — *not* framework-
  rendered. That keeps the click-handler path identical to the
  other variants (the framework would render those too in a real
  app, but they wouldn't participate in the per-row toggle work
  the benchmark cares about).

## Caveats

- **First load is slower than React.** The wasm has to download
  + instantiate. For the toggle benchmark this happens before any
  measurement starts — so it doesn't affect the readout — but a
  cold-start arena slot would tell a different story. See the
  spec's "What we deliberately do NOT measure" section.
- **Page chrome (`Page`/`Controls`/`PerfList`) participates in the
  theme transition.** Those stylesheets also have their own
  per-node effects, so a few extra apply-style calls happen on
  every toggle (negligible at 1000-row scale).
