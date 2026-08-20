Framework regression test apps
==============================

Small, purpose-built idealyst projects that exercise narrow framework
behaviors so we can verify wasm-split pruning, code-splitting, and other
toolchain changes don't break the runtime independently of any
particular example app's evolving content.

Each subdirectory is a normal idealyst project (`pub fn app() -> Element`
+ `pub fn register_scene_extensions(...)`) that builds via `idealyst build
--web --release` like any other scaffold. They're tiny on purpose — if
something regresses here, the surface area to bisect is small.

## Apps

- `vtable-dispatch/` — many `Box<dyn Trait>` impls dispatched at runtime;
  catches data-segment pruning that zeroes vtable bytes (the failure mode
  is `RuntimeError: null function` or wrong dispatch at the first
  indirect call).
- `lazy-chunk-handoff/` — minimal app wrapping a `lazy! { … }` block;
  verifies the main bundle ↔ chunk boundary survives release-mode
  pruning (chunks reach into main-bundle data symbols for shared
  vtables, statics, panic strings).
- `lazy-many-splits/` — 30 `#[component(lazy)]` pages dispatched through
  a static fn-pointer catalog (the idea-ui-docs shape). Guards the
  many-split-point pipeline end-to-end; added for the splitter's
  call-graph misclassification under duplicate mangled names (LLVM under
  `opt-level=z` emits distinct functions sharing one name): edges
  collided in name-keyed maps, a fmt shim main's data still referenced
  got gutted, and release builds trapped at boot with `RuntimeError:
  function signature mismatch`. One or two split points never triggered
  it — dozens did. Which duplicate loses the collision is hash-order
  luck, so the deterministic guard for a recurrence is the
  `emit_main_module` tripwire (see the fixture's lib.rs); this app keeps
  the shape exercised in a browser.
- `lazy-payload-split/` — a **pair** (`eager/`, `lazy/`) plus a shared
  `heavy/` fake third-party SDK whose *mount handler* reaches a 512 KiB
  static. The two apps are identical — same `app()`, same rendered tree,
  same `#[component(lazy)]` chunk body — except for one line in
  `register_scene_extensions`:

  - `eager/`: `heavy::register(registry)`, so main statically reaches
    the handler and its payload.
  - `lazy/`: `registry.defer::<heavy::HeavyProps>()`, which only
    DECLARES the payload kind late-bound (a compile-time `TypeId` — main
    never names the handler); the handler installs itself from inside
    the chunk via `runtime_scene::defer_registration` →
    `Registry::register_deferred`.

  Both apps render `heavy::widget()` from `app()`. In the lazy variant
  realize meets that item before its handler exists, parks it behind a
  layout-transparent placeholder, and completes the mount in place when
  the chunk's registration drains. The runner diffs their `main.wasm`
  and requires the lazy variant to be ≥ 400 KiB smaller.

  That single number asserts three mechanisms at once: `runtime-scene`'s
  post-boot registration seam really does keep a handler out of the boot
  module, wasm-split places the chunk-only symbol outside `main.wasm`,
  and `--data-prune` evicts the now-unreachable static from main's data
  segments. The browser check is load-bearing too — its marker
  (`heavy payload byte:`) is rendered BY the handler, so in the lazy
  variant seeing it proves the parked item was actually drained.

### History of this pair

These fixtures measured the same axis before runtime v2, over the old
`Element::External` + `runtime_core::defer_external_registration` seam
(recorded: `main.wasm` 1294 KiB eager → 781 KiB lazy). Runtime v2 deleted
that seam, and for one wave the pair degraded to measuring plain
call-site reachability because no post-boot registration existed at all.

`runtime_scene::Registry::defer` / `Registry::register_deferred` plus
`runtime_scene::defer_registration` restored the capability — with two
improvements over the old design: parking is **opt-in per payload kind**
(an undeclared unknown payload still panics at realize rather than
degrading to a placeholder box), and a payload that realizes *before* its
handler arrives completes in place instead of being stuck as a permanent
"not supported" node. The semantics are unit-pinned in
`crates/runtime/scene/src/tests.rs`; this directory is where they are
measured in bytes.

## Runner

`prune-regression/` shells out to the installed `idealyst` CLI to build
each app at `--web --release --data-prune` and asserts the expected dist
artifacts exist. In order, it performs:

1. **Build smoke** — `idealyst build --web --release --data-prune` must
   exit 0 for every app. Catches wasm-split-cli crashes, linker errors
   from a chunk that lost a symbol it imports from main, and
   wasm-bindgen failures on the post-split bundle.
2. **Artifact shape** — `index.html`, a > 1 KiB `{stem}_bg[.hash].wasm`,
   and a matching `{stem}[.hash].js` shim must exist in `dist/web/pkg`
   (matched by prefix/suffix, since release bundles are
   content-addressed).
3. **No build-machine paths** — the shipped `.wasm` must not contain
   `$HOME` or the framework workspace root. Panic `Location`s (`file!()`
   behind every `unwrap`/`expect`/bounds check) are live `.rodata`, not
   debug info, so `wasm-opt --strip-debug` cannot remove them; without
   `--remap-path-prefix` a deployed bundle discloses the builder's home
   directory, username, toolchain version, and dependency inventory.
   Release builds pass the remaps via `build_ios::remap_path_flags`.
   Note this checks the binary produced by whichever `idealyst` is on
   `PATH` — a stale installed CLI will fail here until reinstalled.
4. **Browser smoke** (`--browser`, needs a system Chrome/Chromium) —
   serves `dist/web` on an ephemeral port, waits for each app's
   `expected_marker` text, and fails on any `console.error`, on
   `RuntimeError: null function` from a zeroed vtable byte, or on
   `panicked at :` with an empty message from a zeroed panic string.
5. **Handler-registration `main.wasm` delta** — for the
   `lazy-payload-split` pair only, once both variants built in the same
   run. Requires the late-registering variant's main bundle to be at
   least 400 KiB smaller (`MIN_MAIN_SHRINK_BYTES`). This is the runner's
   only bundle-size assertion.

`--data-prune` is passed explicitly because chunk-only data pruning is
OFF by default (its classification under-approximates main's reachability
and corrupts `main.wasm` on real apps). These fixtures are pure-data
payloads with no indirect main→data reachability — the case the heuristic
handles correctly — so the suite opts in to exercise it.

Usage:

    cargo run -p prune-regression                      # build all apps
    cargo run -p prune-regression -- lazy-payload-split/eager
    cargo run -p prune-regression -- --no-clean        # keep dist/ between runs
    cargo run -p prune-regression -- --browser         # add headless Chrome checks

Requires `idealyst` on PATH (`cargo install --path crates/tools/cli
--force`); re-install after touching the splitter so the bin picks up
changes.
