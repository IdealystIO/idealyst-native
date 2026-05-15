// Rebuild suite: alternates mounting `rowsA` and `rowsB` rows for
// `iterations` rebuilds, measuring the per-iteration apply, first paint,
// and worst frame. The shape is the same as the original `runSuite` from
// `instrument.js`; this file extracts it as a self-contained module so
// the arena runner can load it as data + a `run` function.
//
// The module is consumed in two places:
//
//   - In the runner page (arena/index.html): for the metadata (title,
//     description, params) used to render the param form, and to
//     interpret each variant's posted results.
//
//   - In each variant page (when the variant is loaded inside an
//     iframe with `?suite=rebuild`): the `run(...)` function actually
//     drives the variant's `setRows` and measures.
//
// Keep this dependency-light — no DOM references, no reading from
// `window.parent`. The variant glue (in instrument.js) wires those.
//
// All timing constants and the `runToggle`-style measurement match
// the original `instrument.js` implementation exactly so historical
// numbers stay comparable across the runner refactor.

const TRANSITION_MS = 250;
const SLACK_MS = 50;

/// Suite metadata. Consumed by the runner to render the param form
/// and to label the results table.
export const meta = {
  name: 'rebuild',
  title: 'Rebuild',
  description:
    'Alternates between two row counts (e.g. 1000 ↔ 10000), measuring the cost ' +
    'of mounting and unmounting many DOM nodes per iteration. Stresses the ' +
    'framework\'s build / teardown path and the backend\'s per-node FFI surface.',
  params: [
    { name: 'rowsA', label: 'Min rows',  type: 'number', default: 1000,  min: 1, max: 100000 },
    { name: 'rowsB', label: 'Max rows',  type: 'number', default: 10000, min: 1, max: 100000 },
    { name: 'iterations', label: 'Iterations', type: 'number', default: 10, min: 1, max: 100 },
    // Untimed cycles run before measurement starts. One cycle =
    // one rebuild at min + one at max, both discarded. Default 1
    // is enough to warm the JIT / font / cache pipeline for both
    // row counts; bump it if you suspect first-measured iterations
    // are running cold.
    { name: 'warmupCycles', label: 'Warmup cycles', type: 'number', default: 1, min: 0, max: 10 },
  ],
};

/// Run the suite against a variant. Returns the per-iteration run
/// records — same shape as the old `runSuite` return value.
///
/// `setRows` is the variant-supplied hook that rebuilds at count `n`.
/// `params` is `{ rowsA, rowsB, iterations }` from the form.
///
/// Optionally calls `onProgress(runs)` after each iteration so the
/// variant page (or the runner) can render a live table. Pass `null`
/// to skip.
export async function run({ setRows, params, onProgress }) {
  if (typeof setRows !== 'function') {
    throw new Error('rebuild suite: setRows must be a function');
  }
  const rowsA = Number(params?.rowsA ?? 1000);
  const rowsB = Number(params?.rowsB ?? 10000);
  const iterations = Number(params?.iterations ?? 10);
  const warmupCycles = Number(params?.warmupCycles ?? 1);
  const counts = [rowsA, rowsB];

  // Warmup. Each cycle = one rebuild at min + one at max, both
  // discarded. Burns in JIT, font/cache pipelines, and any
  // first-mount overhead at *both* row counts so the first
  // measured iteration doesn't run cold against the larger size.
  for (let i = 0; i < warmupCycles; i++) {
    await measureOne(() => setRows(rowsA));
    await measureOne(() => setRows(rowsB));
  }

  const runs = [];
  for (let i = 0; i < iterations; i++) {
    const rows = counts[i % counts.length];
    const m = await measureOne(() => setRows(rows));
    runs.push({
      iter: i + 1,
      rows,
      apply: m.apply,
      firstPaint: m.firstPaint,
      worstFrame: m.worstFrame,
    });
    if (onProgress) onProgress(runs);
    // Brief settle gap — lets the browser drain any queued GC /
    // paint work the previous iteration's transition window left.
    await new Promise(r => setTimeout(r, 50));
  }

  return runs;
}

/// Time a single rebuild. Returns `{ apply, firstPaint, worstFrame }`
/// in ms. Exactly the math the original instrument.js inlined inside
/// `runSuite`'s loop — extracted here so it's testable on its own.
async function measureOne(work) {
  const t0 = performance.now();
  await work();
  const applyDone = performance.now();
  const apply = applyDone - t0;

  let lastFrame = applyDone;
  let worstFrame = 0;

  const firstFrame = await new Promise(r => requestAnimationFrame(r));
  const firstPaint = firstFrame - t0;
  let gap = firstFrame - lastFrame;
  if (gap > worstFrame) worstFrame = gap;
  lastFrame = firstFrame;

  const deadline = t0 + TRANSITION_MS + SLACK_MS;
  while (performance.now() < deadline) {
    const t = await new Promise(r => requestAnimationFrame(r));
    gap = t - lastFrame;
    if (gap > worstFrame) worstFrame = gap;
    lastFrame = t;
  }

  return { apply, firstPaint, worstFrame };
}
