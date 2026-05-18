# arena

Head-to-head rendering benchmarks across UI frameworks. Same screen, same
work, same instrumentation. See [spec.md](./spec.md) for what the benchmark
actually measures and why those measurements were chosen.

## Running

Anything that serves static files works. From the repo root:

```bash
python3 -m http.server 8000 -d arena
```

Then open [http://localhost:8000/](http://localhost:8000/).

Why a real HTTP server (and not `file://`): the variants use
`<script type="module">` and ES module imports, which browsers refuse to
load over `file://`.

The React variants load React from esm.sh via an import map — there's no
build step and no `node_modules` to install. The first load fetches React;
subsequent loads come from the browser cache.

## What's here

| Path                  | What it shows                                                                   |
|-----------------------|---------------------------------------------------------------------------------|
| [vanilla-css-vars/](./vanilla-css-vars/)   | Theoretical ceiling — one className flip on `<html>`.       |
| [vanilla-classes/](./vanilla-classes/)     | Mirrors the framework's web backend — mint+swap classes.    |
| [react-naive/](./react-naive/)             | React + `useState(theme)` + inline styles. Rows re-render.  |
| [react-cssvars/](./react-cssvars/)         | React + CSS variables. Rows are inert on toggle.            |

The two vanilla variants bracket the theoretical bounds: how cheap can a
theme toggle possibly be (CSS variables), and how expensive does it get when
every element's class is rewritten on every toggle (per-element classes).
Every framework variant should land somewhere on that spectrum.

## Methodology notes

- **Production vs dev React.** The React variants currently load React's
  development build, which trades perf for stack traces and warnings. Numbers
  are dev-mode for all React variants, so they're comparable to each other —
  but not directly to a prod-mode framework build. To compare cross-stack,
  swap `?dev` → empty in the import map URLs (esm.sh serves prod by default).
- **`flushSync`.** The React variants wrap the state update in `flushSync`
  so the React commit happens *inside* the `apply` window. Without it, React
  18 batches updates and the commit lands *after* `apply` closes — `apply`
  would show ~0ms, but the user can't see the new theme until the deferred
  commit lands, so that number would lie.
- **DevTools open.** Browser-extension overhead and DevTools sampling slow
  things down considerably. For headline numbers, run with DevTools closed.
  For attribution (which function is slow), open the Performance tab and
  record across a few toggles.
- **Warmup.** First toggle always pays a JIT/cache tax. Toggle two or three
  times before trusting a number.

## Adding a framework

The contract is small. See [spec.md](./spec.md#how-to-add-a-new-framework-variant).
Briefly:

1. `mkdir arena/<framework>/`
2. Build the screen described in the spec using that framework's idioms.
3. Wrap the toggle handler with `runToggle()` from `../instrument.js`.
4. Add a link to [index.html](./index.html).

Honesty rule: use the framework's idiomatic styling story, not an escape
hatch chosen just to win the benchmark. If you reach for a non-idiomatic
trick (e.g. direct DOM manipulation in a React variant), call it out in a
header comment so a reader knows it isn't the framework being measured.

## What's missing (yet)

- Cold-start / first-render benchmarks.
- Memory readouts.
- Bundle-size comparison.
- Scroll perf.
- Other frameworks: Solid, Svelte, Vue, the idealyst-native framework
  itself (via [examples/theme-benchmark.html](../examples/theme-benchmark.html)
  for now — to be folded in once `framework-core` runs in a way that fits
  this directory's static-file model).

Each of these is a separate axis with its own measurement shape. Keep this
slot focused on toggle-cost until that's reliable, then add the next slot.
