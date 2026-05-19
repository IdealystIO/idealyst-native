// Theme-toggle suite: mounts a row list once, then alternates
// `setTheme('light')` and `setTheme('dark')` for `iterations`
// measured toggles. The hot path here is each framework's
// per-element style re-apply / cascade re-resolve when the
// theme changes — completely different shape from `rebuild`,
// which exercises mount + unmount.
//
// Each variant must expose `setTheme(name)` where `name` is
// either `'light'` or `'dark'`. The resolution contract is the
// same as `setRows`: when the promise resolves, the DOM must
// reflect the new theme. Microtask flush OK; rAF wait
// forbidden (bakes ~16ms into apply that other variants
// don't pay).
//
// Bucket convention for the runner: `bucket = 0` means the
// transition WENT FROM light TO dark (the page is now dark);
// `bucket = 1` means dark→light (page is now light). The
// runner labels these "L→D" and "D→L" in the table header.

const TRANSITION_MS = 250;
const SLACK_MS = 50;

export const meta = {
  name: 'toggle',
  title: 'Theme toggle',
  description:
    "Mounts N rows once, then alternates between LIGHT and DARK for "
    + "`iterations` measured toggles. Stresses each framework's per-element "
    + "style re-apply / cascade re-resolve path.",
  params: [
    { name: 'rows',         label: 'Rows',           type: 'number', default: 1000, min: 1, max: 100000 },
    { name: 'iterations',   label: 'Iterations',     type: 'number', default: 10,   min: 1, max: 100   },
    // Two warmup toggles by default — one in each direction.
    // The first measured iteration would otherwise pay a cold
    // tax for whichever direction it happens to go.
    { name: 'warmupCycles', label: 'Warmup toggles', type: 'number', default: 2,    min: 0, max: 10   },
  ],
};

/// Run the toggle suite.
///
/// `opts`:
///   - `setRows(n)`   mount-rows hook from the variant. Called
///                    once before the measured loop starts to
///                    establish the row count.
///   - `setTheme(t)`  theme-mutator hook. `t` is `'light'` or
///                    `'dark'`. Must resolve when the DOM
///                    reflects the new theme.
///   - `params`       form values.
///   - `onProgress`   optional per-iter callback.
export async function run({ setRows, setTheme, params, onProgress }) {
  if (typeof setTheme !== 'function') {
    throw new Error("toggle suite: variant must expose setTheme(name)");
  }
  const rows = Number(params?.rows ?? 1000);
  const iterations = Number(params?.iterations ?? 10);
  const warmupCycles = Number(params?.warmupCycles ?? 2);

  // One-time mount. Most variants need a row list to theme; the
  // ones that don't (e.g. css-vars-only variants where the rows
  // would be inert under toggle) still need the DOM to exist so
  // the toggle has *something* to re-style. If the variant
  // doesn't support setRows (older variants), we skip — the
  // toggle suite will measure the chrome-only theme cost.
  if (typeof setRows === 'function') {
    await setRows(rows);
  }

  // Page starts in light theme by convention. Warmup toggles
  // hit both directions to warm JIT, font, and style caches at
  // both polarities before measurement starts.
  let currentDark = false;
  for (let i = 0; i < warmupCycles; i++) {
    currentDark = !currentDark;
    await measureOne(() => setTheme(currentDark ? 'dark' : 'light'));
  }

  const runs = [];
  for (let i = 0; i < iterations; i++) {
    currentDark = !currentDark;
    const direction = currentDark ? 0 : 1;  // 0 = L→D, 1 = D→L
    const m = await measureOne(() => setTheme(currentDark ? 'dark' : 'light'));
    runs.push({
      iter: i + 1,
      bucket: direction,
      apply: m.apply,
      firstPaint: m.firstPaint,
      worstFrame: m.worstFrame,
    });
    if (onProgress) onProgress(runs);
    await new Promise(r => setTimeout(r, 50));
  }

  return runs;
}

/// Same `measureOne` as `rebuild.js`. Repeated inline rather
/// than imported to keep each suite a self-contained module.
async function measureOne(work) {
  const t0 = performance.now();
  await work();
  const applyDone = performance.now();
  const apply = applyDone - t0;

  let lastFrame = applyDone;
  let worstFrame = 0;

  const firstFrame = await new Promise(r => requestAnimationFrame(() => r(performance.now())));
  const firstPaint = firstFrame - t0;
  let gap = firstFrame - lastFrame;
  if (gap > worstFrame) worstFrame = gap;
  lastFrame = firstFrame;

  const deadline = applyDone + TRANSITION_MS + SLACK_MS;
  while (performance.now() < deadline) {
    const t = await new Promise(r => requestAnimationFrame(() => r(performance.now())));
    gap = t - lastFrame;
    if (gap > worstFrame) worstFrame = gap;
    lastFrame = t;
  }

  return { apply, firstPaint, worstFrame };
}
