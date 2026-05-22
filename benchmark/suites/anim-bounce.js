// Animation suite: bouncing balls. See benchmark/anim/spec.md.
//
// For each N in the sweep:
//
//   1. setupAnim('bounce', n, seed) — variant mounts N nodes and
//      seeds initial state from the shared bounceInitial(n, seed)
//      generator.
//   2. On iter 0, run the cross-variant determinism check: drive
//      stepTo(REF_FRAME) then diff getState() against a reference
//      vector computed from the harness's own physics. A variant
//      that diverges is failing physics — fail loud.
//   3. Re-setup, then startAnim(); sample for `windowMs`;
//      stopAnim() returns { jsPerFrame, frameDt } arrays.
//   4. Reduce to one iteration record with columns mapped per
//      anim/spec.md:
//        apply      = js_p95
//        firstPaint = frame_p95
//        worstFrame = frame_max
//
// Multiple iterations per N → the runner medians across iterations.
//
// Note the dual hook style for stop. Variants are free to capture
// their own frame log in a ring buffer and return it from stopAnim;
// the suite doesn't try to instrument from outside. This is the
// only honest way — the variant knows when its own update function
// runs and what its rAF cadence looks like; an outer rAF observer
// would only see the gaps, not the work.

import {
  generateBounceReference,
  countAbove,
  assertReference,
} from '../anim/harness.js';

export const meta = {
  name: 'anim-bounce',
  title: 'Animation · bouncing balls',
  description:
    'N circles bouncing inside a fixed viewport. Each frame: integrate ' +
    'position with constant velocity, reflect on edges. Measures the ' +
    'framework\'s per-frame write path at scale (compute is trivial). ' +
    'See benchmark/anim/spec.md for the contract.',
  params: [
    // Comma-separated N values. `0` is special — it skips the
    // determinism check and measures the framework's per-frame
    // floor (rAF wrapper, JS→wasm boundary crossing, etc.) with
    // no per-ball write work to attribute. The slope between N=0
    // and N=10000 is the per-write cost.
    { name: 'nValues',    label: 'N values (CSV)', type: 'string', default: '0,100,1000,5000,10000' },
    { name: 'iterations', label: 'Iterations per N', type: 'number', default: 3, min: 1, max: 20 },
    { name: 'windowMs',   label: 'Sample window (ms)', type: 'number', default: 3000, min: 500, max: 30000 },
    { name: 'seed',       label: 'Seed', type: 'number', default: 1, min: 1, max: 0x7fffffff },
  ],
};

// Reference frame the determinism check uses. 60 frames @ 1/60 dt =
// 1 second of simulated time. Long enough for several wall bounces
// at our viewport / velocity range, short enough that FP drift stays
// well below the 1e-3 tolerance.
const REFERENCE_FRAME = 60;

// Threshold (ms) for "dropped frame". 18ms = 16.67ms vsync + tiny
// slack so vsync-aligned-but-late frames don't all count as dropped.
const DROPPED_FRAME_THRESHOLD_MS = 18;

export async function run({
  setupAnim,
  stepTo,
  getState,
  startAnim,
  stopAnim,
  params,
  onProgress,
}) {
  if (typeof setupAnim !== 'function')  throw new Error('anim-bounce: setupAnim hook missing');
  if (typeof stepTo !== 'function')     throw new Error('anim-bounce: stepTo hook missing');
  if (typeof getState !== 'function')   throw new Error('anim-bounce: getState hook missing');
  if (typeof startAnim !== 'function')  throw new Error('anim-bounce: startAnim hook missing');
  if (typeof stopAnim !== 'function')   throw new Error('anim-bounce: stopAnim hook missing');

  const nValues = parseNValues(params?.nValues ?? '0,100,1000,5000,10000');
  const iterations = Math.max(1, Number(params?.iterations ?? 3));
  const windowMs = Math.max(500, Number(params?.windowMs ?? 3000));
  const seed = Math.max(1, Number(params?.seed ?? 1)) >>> 0;

  // Pre-compute reference vectors for the determinism check. One per
  // N (except N=0 — no balls to compare, so the check is skipped).
  // Computed here (in the suite, using the harness's own physics)
  // rather than embedded as static data so adding a new N to the
  // sweep doesn't require regenerating a constant table. The cost
  // is tiny — generateBounceReference for the largest N takes <5ms.
  const references = new Map();
  for (const n of nValues) {
    if (n === 0) continue;
    references.set(n, generateBounceReference(n, seed, REFERENCE_FRAME));
  }

  const runs = [];
  for (const n of nValues) {
    for (let iter = 0; iter < iterations; iter++) {
      // Setup first — every iteration starts from clean initial
      // state. Skipping this between iterations would drift the
      // sim further from the reference and let small bugs hide.
      await setupAnim('bounce', n, seed);

      // Determinism check on iter 0 only (skip for N=0 — no balls).
      // Once we know the variant passes for this N + seed, repeating
      // the check across iterations adds nothing.
      if (iter === 0 && n > 0) {
        assertReference({ stepTo, getState }, n, seed, REFERENCE_FRAME, references.get(n));
        // assertReference mutated the variant's state by stepping
        // through 60 frames. Reset before the perf run.
        await setupAnim('bounce', n, seed);
      }

      await startAnim();
      await sleep(windowMs);
      const log = await stopAnim();
      const { jsPerFrame, frameDt } = log ?? {};
      if (!Array.isArray(jsPerFrame) || !Array.isArray(frameDt)) {
        throw new Error(
          `anim-bounce: stopAnim must return { jsPerFrame: number[], frameDt: number[] } ` +
          `(got ${log === undefined ? 'undefined' : typeof log})`,
        );
      }

      // Field mapping — see benchmark/anim/spec.md "How animation
      // results map onto the runner table". The runner labels
      // these as APPLY/PAINT/WORST internally, but the runner's
      // SUITE_COLUMN_LABELS override surfaces them as
      // "µs/FRAME / FPS / MAX ms" in the table header.
      //
      //   apply      = mean per-frame variant work, in MICROSECONDS.
      //                Mean (not p95) because steady-state mean is
      //                the most honest "how much does each frame
      //                cost?" number. µs scale because most
      //                framework-overhead deltas land below 1ms.
      //   firstPaint = mean frame rate (frames per second), computed
      //                from the sample window. fps = samples *
      //                1000 / windowMs gives the rate the user
      //                actually saw — vsync caps it at 60.
      //   worstFrame = worst single frame interval in MILLISECONDS.
      //                Surfaces hitches that the mean smooths over.
      runs.push({
        iter: runs.length + 1,
        bucket: n,
        apply: meanOf(jsPerFrame) * 1000,
        firstPaint: (frameDt.length * 1000) / windowMs,
        worstFrame: maxOf(frameDt),
        // Extra fields — runner ignores unknown fields; useful when
        // viewing posted JSON in devtools for triage.
        dropped: countAbove(frameDt, DROPPED_FRAME_THRESHOLD_MS),
        samples: frameDt.length,
      });
      if (onProgress) onProgress(runs);
      // Brief settle so a long sample-window's residual GC doesn't
      // bleed into the next setupAnim.
      await sleep(50);
    }
  }
  return runs;
}

function parseNValues(s) {
  return String(s)
    .split(',')
    .map(p => Number(p.trim()))
    // Allow N=0 (idle-rAF boundary measurement); reject negatives
    // and absurd ceilings.
    .filter(n => Number.isFinite(n) && n >= 0 && n < 1_000_000)
    .map(n => Math.floor(n));
}

function sleep(ms) {
  return new Promise(r => setTimeout(r, ms));
}

function maxOf(arr) {
  if (arr.length === 0) return 0;
  let m = arr[0];
  for (let i = 1; i < arr.length; i++) if (arr[i] > m) m = arr[i];
  return m;
}

function meanOf(arr) {
  if (arr.length === 0) return 0;
  let s = 0;
  for (let i = 0; i < arr.length; i++) s += arr[i];
  return s / arr.length;
}
