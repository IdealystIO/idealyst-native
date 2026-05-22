// Animation suite: N-body gravity sim. See benchmark/anim/spec.md.
//
// N bodies with pairwise gravity + elastic collisions in a 800×600
// reflective box. Each frame:
//   - O(N²) pair-force accumulation
//   - O(N²) collision pass
//   - O(N) integrate + reflect
//   - O(N) writes to backend
//
// Unlike bounce (O(N) compute, write-dominated) and springstorm
// (declarative framework path), n-body is COMPUTE-dominated above
// some N — pair forces grow N² while writes grow N. That's where
// wasm should start to beat V8 on the math, modulo the per-write
// FFI tax that idealyst pays above raw `el.style.transform =`.
//
// Same shape as anim-bounce in terms of variant contract: variant
// owns the rAF, calls xs[i].set(x) / ys[i].set(y) per body. Both
// variants compute their own physics in their host language — wasm
// for idealyst, JS for vanilla — using the shared algorithm
// described in benchmark/anim/harness.js.

import {
  generateNbodyReference,
  countAbove,
  assertReference,
} from '../anim/harness.js';

export const meta = {
  name: 'anim-nbody',
  title: 'Animation · N-body gravity',
  description:
    'N bodies with pairwise gravity + elastic collisions. O(N²) per ' +
    'frame. Compute-dominated above ~200 bodies — where wasm can ' +
    'start to beat V8 on pure math. See benchmark/anim/spec.md.',
  params: [
    // Default sweep is smaller than bounce — O(N²) saturates fast.
    // 500² = 250K pair-computations per frame; both variants struggle
    // past there.
    { name: 'nValues',    label: 'N values (CSV)', type: 'string', default: '0,50,100,200,500' },
    { name: 'iterations', label: 'Iterations per N', type: 'number', default: 3, min: 1, max: 20 },
    { name: 'windowMs',   label: 'Sample window (ms)', type: 'number', default: 3000, min: 500, max: 30000 },
    { name: 'seed',       label: 'Seed', type: 'number', default: 1, min: 1, max: 0x7fffffff },
  ],
};

const REFERENCE_FRAME = 30;     // ½ second of sim — long enough for the FP
                                // ordering to diverge if a port is wrong,
                                // short enough to stay well below the
                                // tolerance under correct math.
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
  if (typeof setupAnim !== 'function')  throw new Error('anim-nbody: setupAnim hook missing');
  if (typeof stepTo !== 'function')     throw new Error('anim-nbody: stepTo hook missing');
  if (typeof getState !== 'function')   throw new Error('anim-nbody: getState hook missing');
  if (typeof startAnim !== 'function')  throw new Error('anim-nbody: startAnim hook missing');
  if (typeof stopAnim !== 'function')   throw new Error('anim-nbody: stopAnim hook missing');

  const nValues = parseNValues(params?.nValues ?? '0,50,100,200,500');
  const iterations = Math.max(1, Number(params?.iterations ?? 3));
  const windowMs = Math.max(500, Number(params?.windowMs ?? 3000));
  const seed = Math.max(1, Number(params?.seed ?? 1)) >>> 0;

  // Reference vectors per N (skipping N=0, no bodies to compare).
  // generateNbodyReference is O(N²·frame); at N=500, frame=30 that's
  // ~7.5M pair ops on the harness side — runs in ~50ms in V8. Fine
  // for once-per-run setup; not in the perf-measured hot path.
  const references = new Map();
  for (const n of nValues) {
    if (n === 0) continue;
    references.set(n, generateNbodyReference(n, seed, REFERENCE_FRAME));
  }

  const runs = [];
  for (const n of nValues) {
    for (let iter = 0; iter < iterations; iter++) {
      await setupAnim('nbody', n, seed);

      if (iter === 0 && n > 0) {
        assertReference({ stepTo, getState }, n, seed, REFERENCE_FRAME, references.get(n));
        await setupAnim('nbody', n, seed);  // reset after stepping
      }

      await startAnim();
      await sleep(windowMs);
      const log = await stopAnim();
      const { jsPerFrame, frameDt } = log ?? {};
      if (!Array.isArray(jsPerFrame) || !Array.isArray(frameDt)) {
        throw new Error(
          `anim-nbody: stopAnim must return { jsPerFrame: number[], frameDt: number[] } ` +
          `(got ${log === undefined ? 'undefined' : typeof log})`,
        );
      }

      // Same metric mapping as anim-bounce — see spec.md "How
      // animation results map onto the runner table":
      //   apply      = mean per-frame variant work, µs
      //   firstPaint = mean FPS over the sample window
      //   worstFrame = worst single frame interval, ms
      runs.push({
        iter: runs.length + 1,
        bucket: n,
        apply: meanOf(jsPerFrame) * 1000,
        firstPaint: (frameDt.length * 1000) / windowMs,
        worstFrame: maxOf(frameDt),
        dropped: countAbove(frameDt, DROPPED_FRAME_THRESHOLD_MS),
        samples: frameDt.length,
      });
      if (onProgress) onProgress(runs);
      await sleep(50);
    }
  }
  return runs;
}

function parseNValues(s) {
  return String(s)
    .split(',')
    .map(p => Number(p.trim()))
    .filter(n => Number.isFinite(n) && n >= 0 && n < 100_000)
    .map(n => Math.floor(n));
}
function sleep(ms) { return new Promise(r => setTimeout(r, ms)); }
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
