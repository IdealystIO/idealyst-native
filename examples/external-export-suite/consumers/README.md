# Standalone consumers

Each folder is a standalone app that consumes the **same two exported
components** (`<idl-greeter>` and `<idl-stepper>`) from
[`../dist/external`](../dist/external) — one per supported framework.

Generate the components first:

```bash
idealyst export examples/external-export-suite
```

| Framework | Folder | How to run | Verified live |
|-----------|--------|-----------|:---:|
| Vanilla JS | [`vanilla/`](vanilla/) | no build — `idealyst serve .. && open /consumers/vanilla/` | ✅ |
| Vue 3 | [`vue/`](vue/) | no build — Vue from CDN, generated `.js` wrappers | scaffold |
| React 19 | [`react/`](react/) | `npm install && npm run dev` | scaffold |
| Svelte | [`svelte/`](svelte/) | `npm install && npm run dev` | scaffold |
| Angular | [`angular/`](angular/) | drop into an `ng` app (see its README) | scaffold |

"Verified live" = exercised end-to-end in a headless browser: **both**
components render, reactive prop updates apply, and callbacks fire — across
repeated fresh loads (the vanilla consumer is the regression check for
multiple components sharing one wasm module on a page). The "scaffold" rows
are complete, idiomatic projects wired to the generated wrappers; run them
with their own toolchain.

## Two integration paths

1. **The bare custom element** — works in *every* framework with no build
   step. Import [`../dist/external/web/index.js`](../dist/external/web)
   (the universal layer; registers the elements) and use `<idl-greeter>` /
   `<idl-stepper>` directly. The `vanilla/` and `vue/` consumers take this
   path.
2. **The typed wrapper** — import the framework-specific wrapper for
   ergonomic, typed props/events. The `react/`, `svelte/`, and `angular/`
   consumers take this path, resolving the wrappers from the
   `external-export-suite-components` package (a `file:` dependency on
   `dist/external`, which `idealyst export` emits as an installable
   package — `package.json` + `web/index.d.ts` included). Each framework
   folder under `dist/external/` is self-contained (its own `pkg/` + custom
   elements), so the `web/` universal layer never mixes with the wrappers.

## The two components

- **`<idl-greeter>`** — `name` (string) + `onGreet` (void callback).
- **`<idl-stepper>`** — `label` (string), `value` (number), `onStep`
  (callback carrying the requested next value). Controlled: the host owns
  `value` and sets it back in response to `onStep`.
