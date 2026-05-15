# Arena benchmark spec

A head-to-head benchmark of UI frameworks rendering the **same** screen, doing
the **same** work, measured by the **same** instrumentation. The goal is
apples-to-apples comparison of the hot path each framework actually walks when
a theme changes — not a contrived microbenchmark.

## The screen

A vertical column with three regions:

1. A controls bar — `<button>Toggle theme</button>` and a `<span>` for stats.
2. A 500px-tall scroller containing **1000 rows**.
3. Each row is `36px` tall, padded `8px 16px`, with text `Row #N`. Even and
   odd rows have different backgrounds (`theme.surface` vs `theme.surface_alt`).
   All rows transition `background`, `color`, and `border-bottom-color` over
   `250ms ease-in-out`.

Visual output across every variant must be pixel-equivalent.

## Themes

Two themes — `LIGHT` and `DARK` — defined exactly as in
[examples/hello/src/lib.rs](../examples/hello/src/lib.rs):

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
const DARK = {
  background:  '#0f1115',
  surface:     '#1a1d24',
  surface_alt: '#262a35',
  text:        '#e8eaf0',
  border:      '#2a2e3a',
  primary:     '#8b9aff',
  primary_text:'#0f1115',
};
```

## The benchmarked operation

A single click on `Toggle theme`. Each variant defines what that click does
**in the idioms of its framework**. The benchmark does not prescribe an
implementation — that's the whole point. The naive React variant re-renders
every row; the CSS-variable variant doesn't; the framework's web backend
re-mints class rules and writes 2000 setAttributes. All of those are valid
answers — we want to see what each costs.

## What we measure

Every variant calls into a shared instrumentation harness
([instrument.js](./instrument.js)). It reports six numbers per toggle:

| Field            | What it means                                                                  |
|------------------|--------------------------------------------------------------------------------|
| `apply (ms)`     | Synchronous JS time spent inside the toggle handler.                           |
| `first paint`    | `t` of the first `requestAnimationFrame` callback after the click started.    |
| `frames`         | Number of rAFs observed during the 250ms transition window + 50ms slack.       |
| `avg fps`        | `frames / elapsed_seconds`.                                                    |
| `worst frame`    | Largest gap between consecutive rAFs during the window — the jank indicator.   |
| `theme / rows`   | Sanity readout so it's obvious you're comparing apples to apples.              |

Numbers are reported into a `<span id="stats">` on the page. Eyeballing is
fine; for repeatable runs, open DevTools' performance tab and record across
a few toggles.

## What we deliberately do NOT measure (yet)

- **Cold start / first render.** Important, but separate concern — different
  shape of measurement.
- **Memory.** Same.
- **Bundle size.** Useful and easy, but orthogonal to render perf.
- **Scroll perf.** The list is 500px tall on purpose; we don't want scrolling
  to confound the toggle measurement.

These belong in their own arena slots once the toggle slot stabilizes.

## How to add a new framework variant

1. Create `arena/<framework>/` (or `arena/<framework>-<variant>/` if more
   than one idiom is worth measuring).
2. Build the screen described above using that framework's idioms. Use its
   styling story honestly — don't reach for a non-idiomatic escape hatch
   just to win.
3. In the toggle handler, wrap the work in:

   ```js
   import { runToggle } from '../instrument.js';
   runToggle(async () => {
     // ...your framework's theme switch...
   }, { theme: nextThemeName, rows: COUNT });
   ```

   `runToggle` does the timing and writes the stats span. The async body is
   where you do whatever the framework needs.
4. Link it from [index.html](./index.html).

The contract is small on purpose: same screen, same toggle, same reporter.
Everything else is the variant author's call — and that *is* the data we want.
