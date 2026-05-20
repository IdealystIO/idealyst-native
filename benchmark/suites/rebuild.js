// Rebuild suite: alternates mounting `rowsA` and `rowsB` rows for
// `iterations` rebuilds, measuring the per-iteration apply, first paint,
// and worst frame. The shape is the same as the original `runSuite` from
// `instrument.js`; this file extracts it as a self-contained module so
// the benchmark runner can load it as data + a `run` function.
//
// The module is consumed in two places:
//
//   - In the runner page (benchmark/index.html): for the metadata (title,
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
  // discarded. Order matters: warmup MUST end on `rowsB` so
  // iter 1's `setRows(rowsA)` measures the same transition
  // direction (rowsB → rowsA) that every later rowsA iteration
  // measures. The warmup also leaves JIT, font/cache pipelines,
  // and CSSOM rules warmed at BOTH counts so neither size pays
  // a cold tax on its first measured iteration.
  for (let i = 0; i < warmupCycles; i++) {
    await measureOne(() => setRows(rowsA));
    verifyRowCount(rowsA, `warmup cycle ${i + 1} (rowsA=${rowsA})`);
    await measureOne(() => setRows(rowsB));
    verifyRowCount(rowsB, `warmup cycle ${i + 1} (rowsB=${rowsB})`);
  }

  const runs = [];
  for (let i = 0; i < iterations; i++) {
    const rows = counts[i % counts.length];
    const m = await measureOne(() => setRows(rows));
    // Verify the variant actually mounted `rows` DOM nodes. A
    // silent failure here (variant reports apply=2ms but DOM still
    // has the previous iteration's rows) used to ship as a
    // legitimate-looking benchmark number — that's how the
    // idealyst-native rebuild bug went unnoticed across multiple
    // bench sessions. Throwing here causes the variant to fail
    // loudly via the runner's `bench-error` path; the runner
    // displays "error" status on the variant row instead of a
    // fake-fast median.
    verifyRowCount(rows, `iteration ${i + 1} (rows=${rows})`);
    runs.push({
      iter: i + 1,
      // `bucket` is the runner's column-grouping key. For the
      // rebuild suite, bucket = row count so the runner reads
      // LOW=rowsA, HIGH=rowsB (sorted numerically). The runner
      // never reads `rows` directly anymore — bucket is the
      // contract.
      bucket: rows,
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

/// Verify the variant's DOM actually contains `expected` row nodes
/// after a `setRows(expected)` call. Throws with a clear message on
/// mismatch — the runner's outer try/catch turns the throw into a
/// `bench-error` postBack so the variant is flagged in the UI.
///
/// Strategy: count every leaf element (zero element children) whose
/// direct text content matches `/^Row #\d+$/`. This works for every
/// current variant — Vue renders `<div>Row #N</div>`, Svelte/React
/// render `<span>Row #N</span>`, idealyst renders `<span>Row #N</span>`,
/// etc. — without needing per-variant cooperation.
///
/// The pattern is anchored (`^` … `$`) so substring matches inside
/// larger labels can't accidentally inflate the count.
function verifyRowCount(expected, context) {
  const found = countRowLeaves();
  if (found !== expected) {
    throw new Error(
      `rebuild verify failed: ${context} — expected ${expected} DOM rows ` +
      `(elements with text matching /^Row #\\d+$/), found ${found}. ` +
      `This usually means setRows didn't actually rebuild the list — the previous ` +
      `iteration's DOM is still mounted. See the framework's reactive-match ` +
      `arm-body tracking limitation (the variant's setRows must trigger the ` +
      `Switch's discriminant, not just bump the count signal).`,
    );
  }
}

function countRowLeaves() {
  const all = document.querySelectorAll('*');
  let count = 0;
  for (const el of all) {
    if (el.children.length !== 0) continue;
    const txt = el.textContent;
    if (txt && /^Row #\d+$/.test(txt.trim())) count++;
  }
  return count;
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

  // `requestAnimationFrame`'s callback argument is a frame-start-
  // aligned timestamp (per spec, snapshotted when the rendering
  // pipeline begins the frame, not when the callback fires). For
  // fast `work()` operations that complete mid-frame, the next
  // frame's start time can be *before* `t0` — yielding negative
  // `firstPaint`. The fix: capture `performance.now()` inside the
  // callback body, which always runs after the work has completed
  // and the frame has been committed.
  //
  // We use callback-fire time everywhere (firstPaint AND the
  // worst-frame loop) to keep all timestamps in one domain;
  // mixing rAF-arg with performance.now() would produce nonsense
  // gaps. The cost is that gap measurements no longer reflect the
  // pure rendering-pipeline timing — they include JS scheduling
  // jitter. In practice that jitter is small relative to real
  // frame drops (1ms vs 16ms+ for a missed frame), so the signal
  // we care about (long frames during transitions) is preserved.
  const firstFrame = await new Promise(r => requestAnimationFrame(() => r(performance.now())));
  const firstPaint = firstFrame - t0;
  let gap = firstFrame - lastFrame;
  if (gap > worstFrame) worstFrame = gap;
  lastFrame = firstFrame;

  // The transition window is measured **from `applyDone`**, not
  // `t0`. Anchoring on `t0` had a subtle measurement bug: when
  // `apply` exceeded the window (e.g. mounting 100k rows takes
  // 300ms+, equal to the window), no time was left for post-apply
  // frame measurement, and `worstFrame` defaulted to 0 — making
  // bad runs look smoother than they are. The `apply` column
  // already exposes how long the synchronous work took; this
  // column should isolate **post-apply rendering smoothness**.
  // Anchoring on `applyDone` does that consistently regardless
  // of how long apply ran.
  const deadline = applyDone + TRANSITION_MS + SLACK_MS;
  while (performance.now() < deadline) {
    const t = await new Promise(r => requestAnimationFrame(() => r(performance.now())));
    gap = t - lastFrame;
    if (gap > worstFrame) worstFrame = gap;
    lastFrame = t;
  }

  return { apply, firstPaint, worstFrame };
}
