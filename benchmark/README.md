# benchmark

Head-to-head rendering benchmarks across UI frameworks. Same screen, same
work, same instrumentation. See [spec.md](./spec.md) for what the benchmark
actually measures and why those measurements were chosen.

## Running

Two wasm crates need to be built first — the runner page itself (at the
root of this directory) and the idealyst-native variant
(`idealyst-native/wasm/`). From the repo root:

```bash
cd benchmark                     && wasm-pack build --target web --release
cd benchmark/idealyst-native/wasm && wasm-pack build --target web --release
```

Rebuild after any change to the corresponding `src/lib.rs`, or to the
framework itself if you want the runner / variant to pick up the
changes.

Then serve the directory with the idealyst CLI:

```bash
cargo run -p idealyst-cli -- serve benchmark --port 8080
```

Open [http://localhost:8080/](http://localhost:8080/) and click **Run**.

Why a real HTTP server (and not `file://`): the variants use
`<script type="module">` and ES module imports, which browsers refuse
to load over `file://`.

The React / Vue / Svelte variants load their runtimes from esm.sh via
an import map — no build step.

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

- **Production builds everywhere.** React variants load production esm.sh
  builds (no `?dev`); Vue loads `vue.esm-browser.prod.js`; Svelte compiles
  with `dev: false`; the idealyst-native variant builds wasm with
  `wasm-pack --release` and no `debug-stats` feature. Don't ship benchmark
  numbers run against any of the dev-mode equivalents — they're all
  ~2-5× slower.
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
  still look anomalous.

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
