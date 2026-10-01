# benchmark

Head-to-head rendering benchmarks across UI frameworks. Same screen, same
work, same instrumentation. See [spec.md](./spec.md) for what the benchmark
actually measures and why those measurements were chosen.

## Running

```bash
benchmark/serve
```

(or `PORT=9000 benchmark/serve` for a different port). That single
command:

- Builds the runner wasm (`benchmark/pkg/`)
- Builds the idealyst-native variant wasm (`benchmark/idealyst-native/wasm/pkg/`)
  and the idealyst-native-anim variant wasm
- Builds the Svelte variant bundle via Vite (`benchmark/svelte/pkg/`)
- Invokes `idealyst-cli serve` on port 8080 (or `$PORT`)

Open [http://localhost:8080/](http://localhost:8080/) and click **Run**.
Subsequent invocations are fast — each build step is idempotent.

Why a real HTTP server (and not `file://`): the variants use
`<script type="module">` and ES module imports, which browsers refuse
to load over `file://`.

## Builds (manual)

If you want to invoke the builds individually:

```bash
# Runner UI and the idealyst-native variants (Rust → wasm), from the repo root
cargo run -q --release -p bench-pack-web -- benchmark
cargo run -q --release -p bench-pack-web -- benchmark/idealyst-native/wasm
cargo run -q --release -p bench-pack-web -- benchmark/idealyst-native-anim/wasm

# Svelte variant (Svelte SFCs → bundled JS via Vite)
cd benchmark/svelte && npm install && npm run build
```

The vanilla / React / Vue variants need no build step — they load
their (production) runtimes from esm.sh.

**Not wasm-pack.** The three wasm crates link backend-web, whose
bindings are web-glue: its JS rides inside the linked module and has to
come back out as `pkg/__idealyst_glue.js`, which only the framework's
build pass writes. `wasm-pack build` still exits 0 on these crates, but
its `pkg/` imports a `__idealyst_glue.js` that isn't there — the page
404s on it, never boots, and the runner times out with no error.
[`pack-web/`](./pack-web/src/main.rs) (`bench-pack-web`) is cargo + that
glue pass (`build_web::own_glue`) + `wasm-bindgen --target web` +
`wasm-opt` with each crate's own
`[package.metadata.wasm-pack.profile.release]` flags — on a crate without
web-glue its output is byte-identical to wasm-pack's. Pass cargo args
after `--` (e.g. `-- --features debug-stats`) and `--out-dir <dir>` for a
second package. A page must import the generated shim by its plain path
(no `?v=N` cache-buster): the glue file imports `initSync` from it, and a
different URL loads a second, uninitialised copy.

## What's here

| Variant                              | What it shows                                                                |
|--------------------------------------|------------------------------------------------------------------------------|
| [vanilla-css-vars/](./vanilla-css-vars/)             | Cascade ceiling — static classNames, no per-row work.        |
| [vanilla-classes/](./vanilla-classes/)               | Honest per-element mount — createElement+appendChild loop.   |
| [vanilla-classes-bulk/](./vanilla-classes-bulk/)     | Physical DOM ceiling — single `innerHTML` write.             |
| [react-naive/](./react-naive/)                       | React + inline `style={...}` props.                          |
| [react-cssvars/](./react-cssvars/)                   | React + CSS variables + static classNames.                   |
| [vue/](./vue/)                                       | Vue 3, `:style` bindings, runtime compile.                   |
| [svelte/](./svelte/)                                 | Svelte 5 with `$state` runes, runtime compile.               |
| [idealyst-native/](./idealyst-native/)               | The framework's own web backend.                             |

The three vanilla variants bracket what the platform can do:

- **`vanilla-css-vars`** is what's possible when the cascade carries the work.
  The rebuild suite still mounts N nodes here, but each row's style comes from
  a static className referencing `:root` variables — no per-row JS-side style
  computation.
- **`vanilla-classes`** is what a component framework can realistically chase:
  every row's className gets stamped individually via `createElement` +
  `setAttribute`, with a DocumentFragment so the N attach calls collapse to
  one layout commit.
- **`vanilla-classes-bulk`** is the physical ceiling — `innerHTML = htmlStr`
  hands the whole subtree to the browser parser in one FFI call. No component
  framework can do this without abandoning its node abstraction. It exists as
  the "no JS-side overhead can be less than this" reference line.

Every framework variant should land somewhere on that spectrum.

## Suites

Two suites ship:

- **Rebuild** — alternates between two row counts (default 1k ↔ 10k),
  measuring mount + unmount. Stresses build/teardown.
- **Theme toggle** — mounts once, then alternates light/dark for the
  declared iteration count. Stresses per-element style re-apply / cascade
  re-resolve.

The runner sidebar picks one. Switching suites clears the result table —
cross-suite numbers aren't comparable.

## Methodology notes

- **Production builds everywhere.** React variants use the production
  esm.sh bundle and call `_jsx`/`_jsxs` from `react/jsx-runtime` directly
  (~3-5% faster than `React.createElement` because it skips runtime
  arg-shape checks); Vue uses the runtime-only `vue.runtime.esm-browser.prod.js`
  with hand-written `h()` render functions (no template compiler shipped);
  Svelte is AOT-compiled by Vite with `dev: false`; the idealyst-native
  variant builds release wasm through `bench-pack-web` and no `debug-stats`
  feature. Don't ship numbers run against any dev-mode equivalents — they're
  all ~2-5× slower.
- **`flushSync` in React.** The React variants wrap `setRowCountState` in
  `flushSync` so the React commit happens *inside* `setRows`'s window.
  Without it React 18 would batch and commit after `setRows` resolved —
  `apply` would look ~0ms but the rows wouldn't yet exist.
- **Microtask vs rAF.** The `setRows` contract requires resolution by
  microtask, never rAF. Svelte's `tick()` and Vue's `nextTick()` are both
  microtask-based and are fine; the framework's signal fan-out is
  synchronous and also fine. A `requestAnimationFrame` wait inside
  `setRows` would bake ~16ms of paint delay into `apply` and is forbidden.
- **DevTools open.** Browser-extension overhead and DevTools sampling slow
  things down considerably. For headline numbers, run with DevTools closed.
  For attribution (which function is slow), open the Performance tab and
  record across a few iterations.
- **Warmup.** The rebuild suite runs an untimed cycle at each row count
  before measurement starts so neither size pays a cold tax on its first
  iteration. Bump `warmupCycles` in the runner sidebar if iter-1 numbers
  still look anomalous. The toggle suite also runs `jitWarmup` (default
  50) untimed toggles first: V8's tier-up compiles fire at fixed call
  counts, and with only two warmup toggles they always fell on the
  light→dark samples.

## Adding a framework

The contract is small. See [spec.md](./spec.md#how-to-add-a-new-framework-variant).
Briefly:

1. `mkdir benchmark/<framework>/`
2. Build the screen described in the spec using that framework's idioms.
3. Expose `setRows(n)` honoring the resolution contract.
4. `autoRunIfRequested({ setRows })` from `../instrument.js`.
5. Add an entry to the `VARIANTS` array in
   [src/lib.rs](./src/lib.rs) (the runner crate) and rebuild it.

Honesty rule: use the framework's idiomatic mount/styling story. If you
reach for a non-idiomatic trick, name the variant accordingly (see how
`vanilla-classes-bulk` is suffixed so its trick is in the label).

## What's missing (yet)

- Cold-start / first-render benchmarks.
- Memory readouts.
- Bundle-size comparison.
- Scroll perf.
- Theme-toggle suite (rebuild is the only suite shipped today).

Each of these is a separate axis with its own measurement shape. Keep each
slot focused on one axis until that's reliable, then add the next.
