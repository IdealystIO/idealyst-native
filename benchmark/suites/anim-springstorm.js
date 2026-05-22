// Animation suite: spring storm. See benchmark/anim/spec.md.
//
// N nodes, each with an independent spring driving translateY. On
// start(), every spring kicks toward a fresh target. The variant
// internally re-kicks targets periodically (every ~500ms) so the
// springs stay continuously running rather than settling.
//
// Unlike anim-bounce — which is the framework-as-write-surface test
// (author owns rAF, calls AV.set imperatively) — springstorm tests
// idealyst's actual animation abstraction: AV.animate(SpringTo)
// driving N independent timelines through the framework's clock.
//
// The variant's own rAF closure for idealyst is near-empty (just
// re-kicks); the framework owns the per-frame tick loop. So `apply`
// for idealyst is NOT the per-frame framework cost; it's only the
// re-kick overhead. Use FPS and MAX ms as the headline metrics for
// springstorm. See spec.md "Asymmetric apply measurement" for the
// rationale.

import { countAbove } from '../anim/harness.js';

export const meta = {
  name: 'anim-springstorm',
  title: 'Animation · spring storm',
  description:
    'N independent springs driving translateY, re-kicked every ~500ms ' +
    'to stay continuously running. Tests the framework\'s scheduler + ' +
    'interpolator (idealyst: AV.animate(SpringTo) per ball; vanilla: ' +
    'hand-rolled JS spring integrator). For the framework-as-write-' +
    'surface test, use anim-bounce instead. See benchmark/anim/spec.md.',
  params: [
    // N=0 still meaningful here — measures the variant's per-frame
    // floor when the framework has no animators registered.
    { name: 'nValues',    label: 'N values (CSV)', type: 'string', default: '0,100,1000,5000,10000' },
    { name: 'iterations', label: 'Iterations per N', type: 'number', default: 3, min: 1, max: 20 },
    { name: 'windowMs',   label: 'Sample window (ms)', type: 'number', default: 3000, min: 500, max: 30000 },
    { name: 'seed',       label: 'Seed', type: 'number', default: 1, min: 1, max: 0x7fffffff },
  ],
};

const DROPPED_FRAME_THRESHOLD_MS = 18;

export async function run({
  setupAnim,
  startAnim,
  stopAnim,
  params,
  onProgress,
}) {
  if (typeof setupAnim !== 'function')  throw new Error('anim-springstorm: setupAnim hook missing');
  if (typeof startAnim !== 'function')  throw new Error('anim-springstorm: startAnim hook missing');
  if (typeof stopAnim !== 'function')   throw new Error('anim-springstorm: stopAnim hook missing');
  // Note: stepTo/getState not required — determinism check skipped
  // for spring suites (spring math is per-framework, won't match
  // cross-variant bit-for-bit even when both are correct).

  const nValues = parseNValues(params?.nValues ?? '0,100,1000,5000,10000');
  const iterations = Math.max(1, Number(params?.iterations ?? 3));
  const windowMs = Math.max(500, Number(params?.windowMs ?? 3000));
  const seed = Math.max(1, Number(params?.seed ?? 1)) >>> 0;

  const runs = [];
  for (const n of nValues) {
    for (let iter = 0; iter < iterations; iter++) {
      // Fresh setup per iteration. The variant tears down any prior
      // springs/AVs and seeds N new ones from `seed` so iteration-to-
      // iteration runs are independent. (Re-using state would let an
      // earlier iteration's GC pressure or scheduler residue
      // contaminate later iterations.)
      await setupAnim('springstorm', n, seed);

      await startAnim();
      await sleep(windowMs);
      const log = await stopAnim();
      const { jsPerFrame, frameDt } = log ?? {};
      if (!Array.isArray(jsPerFrame) || !Array.isArray(frameDt)) {
        throw new Error(
          `anim-springstorm: stopAnim must return { jsPerFrame: number[], frameDt: number[] } ` +
          `(got ${log === undefined ? 'undefined' : typeof log})`,
        );
      }

      // Same field mapping as anim-bounce — the runner's column-label
      // override handles the headers, and the asymmetric-apply caveat
      // is documented in spec.md.
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
