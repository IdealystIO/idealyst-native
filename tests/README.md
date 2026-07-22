Framework regression test apps
==============================

Small, purpose-built idealyst projects that exercise narrow framework
behaviors so we can verify wasm-split pruning, code-splitting, and other
toolchain changes don't break the runtime independently of any
particular example app's evolving content.

Each subdirectory is a normal idealyst project (`pub fn app() -> Element`
+ `pub fn register_extensions(...)`) that builds via `idealyst build
--web --release` like any other scaffold. They're tiny on purpose — if
something regresses here, the surface area to bisect is small.

## Apps

- `vtable-dispatch/` — many `Box<dyn Trait>` impls dispatched at runtime;
  catches data-segment pruning that zeroes vtable bytes (the failure mode
  is `RuntimeError: null function` or wrong dispatch at the first
  indirect call).
- `theme-swap/` — many tokens + light/dark toggle that exercises the
  reactive token-cohort and `update_tokens` batching path.
- `lazy-chunk-handoff/` — minimal app wrapping a `lazy! { … }` block;
  verifies the main bundle ↔ chunk boundary survives release-mode
  pruning (chunks reach into main-bundle data symbols for shared
  vtables, statics, panic strings).
- `lazy-external-split/` — a **pair** (`eager/`, `lazy/`) plus a shared
  `heavy/` fake External SDK carrying a 512 KiB payload reachable only
  through its handler. The two apps are identical except where the
  handler is registered: `eager/` at boot (`register_extensions`), `lazy/`
  from inside the `lazy!` chunk body via
  `runtime_core::defer_external_registration`. The runner diffs their
  `main.wasm` and requires the lazy variant to be ≥ 400 KiB smaller —
  proving deferred registration keeps a heavy SDK out of the main bundle.
  The runtime behavior itself is unit-tested in `runtime-core` and
  `backend-web`; this pair is the end-to-end byte measurement.

## Runner

`prune-regression/` shells out to the installed `idealyst` CLI to build
each app at `--web --release` (which turns on data-segment pruning by
default) and asserts the expected dist artifacts exist. It does not
drive a browser yet — that's a follow-up. The build itself catches:

- linker errors from a chunk that lost a symbol it imports from main
- panics during wasm-split's post-processing pass

Browser-level assertions (the page actually rendered without
`null function` traps) are still a manual smoke test today: open
`tests/<app>/dist/web/index.html` via `idealyst serve` and check the
devtools console.
