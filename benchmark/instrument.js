// Shared variant glue for the benchmark runner.
//
// Variants import:
//
//   - `LIGHT` / `DARK`         the canonical theme palettes. Every
//                              variant pulls colors from here so a
//                              typo in one variant can't drift.
//   - `autoRunIfRequested(...)` the iframe handshake: when a variant
//                              is loaded with `?suite=NAME` (which
//                              the runner does), dynamic-import that
//                              suite and post results to the parent.
//
// Keep this dependency-free and untranspiled — every variant loads it
// as a plain ES module from `<script type="module">`.

export const LIGHT = {
  background:   '#f7f7fb',
  surface:      '#ffffff',
  surface_alt:  '#eef0f7',
  text:         '#1a1a1f',
  border:       '#e4e6ef',
  primary:      '#5b6cff',
  primary_text: '#ffffff',
};

export const DARK = {
  background:   '#0f1115',
  surface:      '#1a1d24',
  surface_alt:  '#262a35',
  text:         '#e8eaf0',
  border:       '#2a2e3a',
  primary:      '#8b9aff',
  primary_text: '#0f1115',
};

// ---------------------------------------------------------------
// Runner integration: auto-run a suite when invoked from an iframe
// ---------------------------------------------------------------
//
// The benchmark runner loads each variant inside an iframe with a
// `?suite=NAME` query param. The variant calls
// `autoRunIfRequested(...)` after its own setup; this helper:
//
//   1. Reads the suite name + params from the URL.
//   2. Dynamic-imports the suite module from `../suites/NAME.js`
//      (relative to the variant page, which lives one level below
//      `benchmark/`).
//   3. Calls the suite's `run(...)` with the variant-supplied
//      `setRows` and the URL params.
//   4. Posts the resulting per-iteration records back to the
//      runner via `window.parent.postMessage(...)`.
//
// Variants that aren't loaded inside an iframe (i.e. opened
// directly in a tab) see no `?suite=` and the helper returns
// immediately.

const PARENT_ORIGIN = '*';  // runner + variant are same-origin in
                            // dev, but '*' avoids breaking if
                            // anyone serves via different hosts
                            // (CI, GitHub Pages). The payload
                            // has nothing sensitive.

/// Auto-run handler called by each variant page. The variant must
/// have already finished its own setup (component mounted, theme
/// installed, etc.) — the suite assumes its hooks are ready to
/// call the moment we invoke it.
///
/// Each hook is optional from the *runner's* perspective: the
/// suite picks which it needs and asserts internally. A variant
/// that doesn't yet support a given operation just omits the
/// hook; suites that depend on it will fail loud rather than
/// silently produce garbage numbers.
///
/// `opts`:
///   - `setRows(n)`    the variant's row-count mutator. Contract:
///                     when the returned promise resolves, the DOM
///                     reflects `n` rows. Microtask-flush is
///                     enough; do NOT wait for a rAF (that bakes
///                     ~16ms of paint delay into `apply` that
///                     other variants don't pay).
///   - `setTheme(t)`   the variant's theme mutator. `t` is
///                     `'light'` or `'dark'`. Same resolution
///                     contract as `setRows`.
///   - `suitesBase`    optional override for the suites directory
///                     URL. Defaults to `../suites/` relative to
///                     the variant page (the layout the benchmark
///                     ships with).
export async function autoRunIfRequested({ setRows, setTheme, suitesBase } = {}) {
  const url = new URL(window.location.href);
  const suiteName = url.searchParams.get('suite');
  if (!suiteName) {
    // Not in iframe-runner mode — variant is being viewed standalone.
    return;
  }

  // Collect every other query param as a string→string map for the
  // suite. The suite's `run()` casts them to whatever shape it
  // wants. `runId` is dropped — it exists purely to differentiate
  // sequential iframe loads in devtools, the variant never reads it.
  const params = {};
  for (const [k, v] of url.searchParams.entries()) {
    if (k === 'suite' || k === 'runId') continue;
    params[k] = v;
  }

  // Dynamic-import the suite module. Default base assumes the
  // variant page lives at `benchmark/<variant>/index.html` and the
  // suite at `benchmark/suites/<name>.js`.
  const base = suitesBase ?? new URL('../suites/', window.location.href).href;
  let suite;
  try {
    suite = await import(`${base}${suiteName}.js`);
  } catch (err) {
    postBack({ type: 'bench-error', error: `failed to load suite ${suiteName}: ${err?.message ?? err}` });
    return;
  }

  if (typeof suite.run !== 'function') {
    postBack({ type: 'bench-error', error: `suite ${suiteName} has no run() export` });
    return;
  }

  try {
    const runs = await suite.run({
      setRows,
      setTheme,
      params,
      onProgress: (progress) => {
        postBack({ type: 'bench-progress', suite: suiteName, runs: progress });
      },
    });
    postBack({ type: 'bench-result', suite: suiteName, runs });
  } catch (err) {
    postBack({ type: 'bench-error', error: `suite ${suiteName} threw: ${err?.message ?? err}` });
  }
}

function postBack(msg) {
  if (window.parent && window.parent !== window) {
    window.parent.postMessage(msg, PARENT_ORIGIN);
  }
}
