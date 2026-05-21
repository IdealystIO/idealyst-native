// Signal-class suite: mounts N rows, each binding its background
// to ONE shared signal via the framework's `signal_class` helper.
// Bumps the shared signal between two values and measures apply
// time.
//
// This suite specifically exercises the JS-side reactive-class
// binding fast path: at mount, each row registers a pre-resolved
// (value → class) table with the JS dispatcher. On a signal write,
// ONE FFI hop fires the dispatcher; the JS shim iterates subscribed
// nodes and `setAttribute('class', …)`s them — zero Rust Effect
// re-runs per node.
//
// Compare against the existing `reactive-style` suite's SHARED
// bucket: same observable behavior (one signal flips N rows'
// backgrounds), but reactive-style goes through N per-row Rust
// Effects while signal-class hands the fan-out to a JS-side
// dispatcher. The headline ratio at 50k rows is the framework's
// per-Effect-overhead vs ~zero per-node Rust work.
//
// Only variants that expose `setupSignalClassRows` participate.
// Today: idealyst-native only. Other frameworks would need their
// own equivalent (memoized component + selector store, signal
// computed, etc.) — out of scope for this bench.
//
// Variant contract:
//   - `setupSignalClassRows(n)` mount N rows. Each row's style is
//                               `signal_class(shared, [0, 1], |v|
//                                 RStyleRow().background(if v == 0 {
//                                   COLOR_A } else { COLOR_B }))`.
//   - `setSharedColor(name)`    same as reactive-style: write 'A'
//                               or 'B' to the shared color signal.

const TRANSITION_MS = 250;
const SLACK_MS = 50;

const COLOR_A_RGB = 'rgb(91, 108, 255)';
const COLOR_B_RGB = 'rgb(255, 91, 108)';

export const meta = {
  name: 'signal-class',
  title: 'Signal-class binding (JS-side fan-out)',
  description:
    'Mounts N rows whose `class` attribute is driven by a JS-side '
    + 'reactive binding (`signal_class`). Pre-resolves the (value → class) '
    + 'table at mount; signal writes ship ONE FFI hop and the JS shim '
    + 'fans out to every subscribed node. No per-row Rust Effect re-runs '
    + 'on a bump — designed to validate the framework\'s JS-binding '
    + 'fast path.',
  params: [
    { name: 'rows',         label: 'Rows',           type: 'number', default: 50000, min: 1, max: 200000 },
    { name: 'iterations',   label: 'Iterations',     type: 'number', default: 10,    min: 2, max: 200 },
    { name: 'warmupCycles', label: 'Warmup toggles', type: 'number', default: 2,     min: 0, max: 10 },
  ],
};

export async function run({ setupSignalClassRows, setSharedColor, params, onProgress }) {
  if (typeof setupSignalClassRows !== 'function') {
    throw new Error("signal-class suite: variant must expose setupSignalClassRows(n)");
  }
  if (typeof setSharedColor !== 'function') {
    throw new Error("signal-class suite: variant must expose setSharedColor(name)");
  }

  const rows = Number(params?.rows ?? 50000);
  const iterations = Number(params?.iterations ?? 10);
  const warmupCycles = Number(params?.warmupCycles ?? 2);

  await setupSignalClassRows(rows);
  verifyRowsMounted(rows, 'after setupSignalClassRows');

  // Warmup: alternate A and B untimed.
  let sharedIsB = false;
  for (let i = 0; i < warmupCycles; i++) {
    sharedIsB = !sharedIsB;
    await measureOne(() => setSharedColor(sharedIsB ? 'B' : 'A'));
    verifyAnyRowHasBg(
      sharedIsB ? COLOR_B_RGB : COLOR_A_RGB,
      `warmup ${i + 1}`,
    );
  }

  const runs = [];
  for (let i = 0; i < iterations; i++) {
    sharedIsB = !sharedIsB;
    const direction = sharedIsB ? 0 : 1;
    const m = await measureOne(() => setSharedColor(sharedIsB ? 'B' : 'A'));
    verifyAnyRowHasBg(
      sharedIsB ? COLOR_B_RGB : COLOR_A_RGB,
      `iter ${i + 1} (${sharedIsB ? 'A→B' : 'B→A'})`,
    );
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

function verifyRowsMounted(expected, context) {
  const all = document.querySelectorAll('*');
  let count = 0;
  for (const el of all) {
    if (el.children.length !== 0) continue;
    const txt = el.textContent;
    if (txt && /^sclass \d+$/.test(txt.trim())) count++;
  }
  if (count !== expected) {
    throw new Error(
      `signal-class verify failed: ${context} — expected ${expected} rows ` +
      `(text matching /^sclass \\d+$/), found ${count}. ` +
      `setupSignalClassRows didn't mount the row list.`,
    );
  }
}

function verifyAnyRowHasBg(expected, context) {
  for (const el of document.querySelectorAll('*')) {
    const bg = window.getComputedStyle(el).backgroundColor;
    if (bg === expected) return;
  }
  throw new Error(
    `signal-class verify failed: ${context} — no element has computed ` +
    `background-color ${expected}. The JS-side binding dispatcher didn't ` +
    `propagate the signal change to any node.`,
  );
}

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
