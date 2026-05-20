# bench-runner

Headless driver for the variant benchmarks. Opens each variant's page in
a CDP-controlled Chrome, runs a suite, and prints a summary table.

## Prereqs

1. **Bench server on port 8080.** From the repo root:

   ```sh
   benchmark/serve
   ```

   This rebuilds the wasm variants and serves `benchmark/` via
   `idealyst-cli serve`. Override the port with `PORT=… benchmark/serve`
   and pass `--server-port` to this CLI to match.

2. **A Chrome with `--remote-debugging-port=9223`.** Any recent
   Chromium-family browser works. Headless is fine and faster:

   ```sh
   /Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome \
     --remote-debugging-port=9223 --headless=new --no-startup-window
   ```

   Override the port with `--chrome-port` if 9223 is in use.

3. **`npm install` once** inside `benchmark/runner/` to pick up `ws`.

## Usage

```sh
# Inspect what the runner can drive.
node benchmark/runner/index.mjs list

# One variant × one suite at defaults.
node benchmark/runner/index.mjs measure --variant idealyst-native --suite rebuild

# Every variant against the rebuild suite, smaller iteration count.
node benchmark/runner/index.mjs measure --variant all --suite rebuild --iterations 5

# Two variants, all suites, JSON output.
node benchmark/runner/index.mjs measure \
  --variants idealyst-native,svelte --suite all --json
```

Any `--<param>` flag whose name matches a suite param (e.g. `--rowsA`,
`--rowsB`, `--iterations`, `--warmupCycles`, `--nodes`, `--seed`,
`--maxDepth`, `--rows`) is forwarded into the variant URL. Suite param
names come from each `benchmark/suites/<name>.js`.

## Output

Default output is a Markdown table per suite. Columns:

- `bN p50` — median `apply` time (ms) for bucket *N*.
- `bN worst` — median worst-frame gap (ms) for bucket *N*.

Bucket meaning depends on the suite:

- `rebuild` — bucket = row count (`rowsA`, `rowsB`).
- `toggle`  — bucket 0 = L→D, bucket 1 = D→L.
- `hierarchy` — bucket 0 = BRANCH update, bucket 1 = GLOBAL update.

`--json` dumps the summary structure as JSON. `--raw` adds the
per-iteration run records under `runs[]`.

## Failure modes

A variant that throws or hangs is reported as `FAIL` on stderr and as
`{ error: '…' }` in JSON output; other variants still run. Common cases:

- **Variant 404** — typo in `--variant` or a missing build (the
  `idealyst-native` wasm has to be rebuilt after Rust changes; the
  Svelte variant needs `npm run build`).
- **Suite verify failed** — the suite's DOM check tripped. Means the
  variant's hook silently no-op'd. See `suites/<name>.js` for the
  exact check.
- **Timeout** — no `bench-progress` or `bench-result` events for
  `--timeout` ms (default 120s). Bump it for slow variants or low
  iteration counts on huge `rowsB`.

## Files

- `index.mjs` — the CLI itself.
- `host.html` — relays `postMessage` from the variant iframe to the
  CLI via `console.log('BENCH_EVENT:…')` lines, which the CDP session
  picks up.
- `package.json` — declares `ws` dependency.
