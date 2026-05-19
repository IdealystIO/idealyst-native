# Benchmark spec

A head-to-head benchmark of UI frameworks rendering the **same** screen, doing
the **same** work, measured by the **same** instrumentation. The goal is
apples-to-apples comparison of the hot path each framework actually walks when
it mounts a list of nodes — not a contrived microbenchmark.

## The screen

A vertical column with one region:

- A 500px-tall scroller containing **N rows** (N is suite-controlled — see
  below). Each row is `36px` tall, padded `8px 16px`, with text `Row #N`.
  Even and odd rows have different backgrounds (`theme.surface` vs
  `theme.surface_alt`). All rows transition `background`, `color`, and
  `border-bottom-color` over `250ms ease-in-out`.

Visual output across every variant must be pixel-equivalent.

## Themes

Two themes — `LIGHT` and `DARK` — exported as the canonical palettes from
[instrument.js](./instrument.js). Every variant imports from there rather than
inlining hex values so a typo can't drift between variants:

```js
const LIGHT = {
  background:  '#f7f7fb',
  surface:     '#ffffff',
  surface_alt: '#eef0f7',
  text:        '#1a1a1f',
  border:      '#e4e6ef',
  primary:     '#5b6cff',
  primary_text:'#ffffff',
};
const DARK = { /* dark variant, same shape */ };
```

The current `rebuild` suite doesn't toggle between them — all variants render in
LIGHT. Both palettes stay shipped so the upcoming theme-toggle suite can use
them without each variant having to re-declare them.

## Variant operations

Variants expose two hooks the suites drive:

- **`setRows(n)`** — mount `n` rows. Used by suites that exercise mount /
  unmount paths.
- **`setTheme(name)`** — switch to LIGHT or DARK. `name` is the string
  `'light'` or `'dark'`. Used by suites that exercise per-element style
  re-apply / cascade re-resolve paths.

Both share the same resolution contract:

- **When the returned promise resolves, the DOM must reflect the new state.**
- Resolution may include microtask flushes (Vue's `nextTick()`, Svelte's
  `tick()`, React's `flushSync`, framework's synchronous effect fan-out — all
  fine).
- Resolution **must not** include a `requestAnimationFrame` wait — that would
  bake ~16ms of paint delay into `apply` that other variants don't pay.

Variants declare which hooks they support via the
`autoRunIfRequested({ setRows, setTheme })` call from
[instrument.js](./instrument.js). Either is optional from the runner's side;
the active suite asserts on the one it needs and errors loud if absent.

Each variant defines what happens inside its hooks in the idioms of its
framework. The benchmark does not prescribe an implementation — that's the
whole point. The naive React variant re-renders every row; the CSS-variable
variants use static classNames; the framework's web backend updates CSS
variables on `:root`. All of those are valid answers — we want to see what
each costs.

## Suites

A "suite" is a measurement script that drives the variant hooks and reports
per-iteration numbers. Shipped:

- **`rebuild`** — alternates between two row counts (default 1000 ↔ 10000) for
  `iterations` rebuilds (default 10). Measures mount + unmount cost.
  Stresses the framework's build/teardown path and the backend's per-node
  FFI surface. Bucket = row count → columns labeled `LOW` / `HIGH`.

  Params: `rowsA`, `rowsB`, `iterations`, `warmupCycles` (one untimed cycle
  at each count before measurement starts; defaults to 1).

- **`toggle`** — mounts `rows` rows once (untimed), then alternates
  `setTheme('light')` and `setTheme('dark')` for `iterations` measured
  toggles. Each toggle is one measured iteration; `iterations=10` means 5
  light→dark + 5 dark→light. Bucket = direction (0 = light→dark,
  1 = dark→light) → columns labeled `L→D` / `D→L`.

  Params: `rows`, `iterations`, `warmupCycles` (untimed warmup toggles; one
  in each direction by default).

  Note that some variants (vanilla-css-vars, react-cssvars, the
  framework's web backend) implement theme swap as a single `<html>` class
  flip and do ~zero per-row work. Others (vanilla-classes, react-naive,
  vue, svelte) restamp every row's styling on toggle. The 10×+ spread that
  results is exactly the data the suite is built to surface.

## What we measure

For each iteration:

| Field            | What it means                                                                  |
|------------------|--------------------------------------------------------------------------------|
| `apply`          | Synchronous JS time from the variant hook call to the promise resolving.       |
| `firstPaint`     | `performance.now()` at the first `requestAnimationFrame` callback fired after `applyDone`. |
| `worstFrame`     | Largest gap between consecutive rAFs during the 300ms (`TRANSITION_MS + SLACK_MS`) window AFTER `applyDone`. Long-task jank inside `apply` itself shows up in the `apply` column, not here. |

`firstPaint` and `worstFrame` are anchored on `applyDone`, not `t0` — see
[rebuild.js](./suites/rebuild.js) for the reasoning (TL;DR: anchoring on `t0`
when `apply` exceeds the window leaves zero room for post-apply measurement).

## What we deliberately do NOT measure (yet)

- **Cold start / first render.** Important, but separate concern — different
  shape of measurement.
- **Memory.** Same.
- **Bundle size.** Useful and easy, but orthogonal to render perf.
- **Scroll perf.** The list is 500px tall on purpose; we don't want scrolling
  to confound the rebuild measurement.
These belong in their own benchmark slots once the existing slots stabilize.

## How to add a new framework variant

1. Create `benchmark/<framework>/` (or `benchmark/<framework>-<variant>/` if
   more than one idiom is worth measuring).
2. Build the screen above using that framework's idioms. Use its styling
   story honestly — don't reach for a non-idiomatic escape hatch just to win.
3. Expose `setRows(n)` and `setTheme(name)` honoring the resolution contract
   above. Either may be omitted if your variant genuinely can't service the
   operation — but expect suites that need it to fail loud.
4. Call `autoRunIfRequested({ setRows, setTheme })` from
   [instrument.js](./instrument.js) once your setup is done. That handles the
   `?suite=NAME` URL handshake the runner uses, dynamic-imports the suite, and
   posts results back via `postMessage`.
5. Add an entry to the `VARIANTS` array in
   [src/lib.rs](./src/lib.rs) (the runner crate) and rebuild it.

## How to add a new suite

1. Create `benchmark/suites/<name>.js`. Export `meta` (name, title, params)
   and `async run({ setRows, setTheme, params, onProgress })`. The function
   should call the variant hook(s) it needs in measured loops and return an
   array of per-iteration records:
   `{ iter, bucket, apply, firstPaint, worstFrame }`.
2. The runner groups records into result-table columns by `bucket` (sorted
   numerically). Pick stable bucket values that make sense for your suite —
   rebuild uses row count, toggle uses direction (0/1).
3. Add a `SuiteInfo` entry to `SUITES` in [src/lib.rs](./src/lib.rs) with
   the column labels for your buckets, then rebuild the runner.

The contract is small on purpose: same screen, same `setRows` semantics, same
reporter. Everything else is the variant author's call — and that *is* the
data we want.

### Honesty rule

Use the framework's idiomatic styling/mount story, not an escape hatch chosen
to win the benchmark. If you reach for a non-idiomatic trick (e.g. direct DOM
manipulation in a React variant, or an `innerHTML` bulk write that no
component framework can use), call it out in a header comment and consider
naming the variant accordingly — `vanilla-classes` is the honest
per-element mount; `vanilla-classes-bulk` carries the `innerHTML` ceiling
explicitly in its name so a reader knows what they're comparing.
