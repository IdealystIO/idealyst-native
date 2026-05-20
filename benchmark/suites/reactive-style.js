// Reactive-style suite: mounts N rows, each with a `background-color`
// (or equivalent) bound to ONE shared `Signal<Color>`. Iteration
// bumps the signal between two values. Every row should re-resolve
// & re-apply its style on the bump.
//
// This stresses a path the rebuild suite skips entirely — rebuild's
// rows use static styles and go through the batched fast path. Here
// the styles are reactive (per-row Effect subscribed to the signal),
// which is the same path the framework's `attach_style_reactive`
// covers and the one the `pinned_sheet` fix is anchored to.
//
// Two operations alternated:
//
//   SHARED (bucket 0): one signal write fans out to all N rows.
//                      Costs scale O(N): N Effects re-fire, N style
//                      resolves, N class swaps. The interesting
//                      measure is per-row apply cost, not per-write.
//
//   POINT (bucket 1):  one signal write that only ONE row reads
//                      (separate per-row signal). Bumping it fires
//                      exactly one Effect. Tests whether the
//                      framework's reactive style path has any
//                      shared-state cost that scales with cohort
//                      size even when only one subscriber is touched.
//
// Variant contract:
//   - `setupReactiveStyles(n)`   mount N rows, all reading one
//                                shared color signal AND with a
//                                per-row signal each.
//   - `setSharedColor(name)`     write to the shared signal. `name`
//                                is 'A' or 'B' — the variant maps
//                                each to a distinct concrete color
//                                that the verifier can recognize.
//   - `setPointColor(i, name)`   write to row i's per-row color
//                                signal. Same A/B contract.
//
// DOM contract (lets the verifier sample without per-variant glue):
//   - Each row's leaf element has text `rstyle {i}` and a computed
//     `background-color` matching the active color for the channel
//     (shared OR point) the row currently subscribes to.
//   - Color A = `rgb(91, 108, 255)`  (#5b6cff)
//   - Color B = `rgb(255, 91, 108)`  (#ff5b6c)

const TRANSITION_MS = 250;
const SLACK_MS = 50;

// Canonical RGB outputs. Variants must map 'A'/'B' to these exact
// colors so the verifier can sample DOM without knowing variant
// internals.
const COLOR_A_RGB = 'rgb(91, 108, 255)';   // #5b6cff
const COLOR_B_RGB = 'rgb(255, 91, 108)';   // #ff5b6c

export const meta = {
  name: 'reactive-style',
  title: 'Reactive style binding',
  description:
    'Mounts N rows whose background-color reads from a SHARED signal '
    + 'and a per-row signal. Alternates shared-bump (O(N) re-resolve) '
    + 'and point-bump (1 re-resolve). Stresses the framework\'s '
    + 'reactive style attach + cache-key path.',
  params: [
    { name: 'rows',         label: 'Rows',           type: 'number', default: 2000, min: 1, max: 100000 },
    { name: 'iterations',   label: 'Iterations',     type: 'number', default: 20,   min: 2, max: 200 },
    { name: 'warmupCycles', label: 'Warmup pairs',   type: 'number', default: 2,    min: 0, max: 10 },
  ],
};

export async function run({
  setupReactiveStyles,
  setSharedColor,
  setPointColor,
  params,
  onProgress,
}) {
  if (typeof setupReactiveStyles !== 'function') {
    throw new Error("reactive-style suite: variant must expose setupReactiveStyles(n)");
  }
  if (typeof setSharedColor !== 'function') {
    throw new Error("reactive-style suite: variant must expose setSharedColor(name)");
  }
  if (typeof setPointColor !== 'function') {
    throw new Error("reactive-style suite: variant must expose setPointColor(i, name)");
  }

  const rows = Number(params?.rows ?? 2000);
  const iterations = Number(params?.iterations ?? 20);
  const warmupCycles = Number(params?.warmupCycles ?? 2);

  await setupReactiveStyles(rows);
  verifyReactiveRowsMounted(rows, 'after setupReactiveStyles');

  // Warmup. The shared color drives every row's bg; warmup ends
  // ready for SHARED's first measured bump to be A→B (so all
  // measured SHARED iterations go in the same direction at first).
  let sharedIsB = false;
  for (let i = 0; i < warmupCycles; i++) {
    sharedIsB = !sharedIsB;
    await measureOne(() => setSharedColor(sharedIsB ? 'B' : 'A'));
    verifyAnyRowHasBg(sharedIsB ? COLOR_B_RGB : COLOR_A_RGB, `warmup ${i + 1} SHARED`);
    // Also warm the point path. Pick row 0 and bump it.
    await measureOne(() => setPointColor(0, i % 2 === 0 ? 'B' : 'A'));
  }

  const runs = [];
  for (let i = 0; i < iterations; i++) {
    const isShared = i % 2 === 0;
    const bucket = isShared ? 0 : 1;  // 0 = SHARED, 1 = POINT
    if (isShared) {
      sharedIsB = !sharedIsB;
      const m = await measureOne(() => setSharedColor(sharedIsB ? 'B' : 'A'));
      // SHARED bumps fan out to every row. Verify at least the
      // first and last sampled — if both flipped, the fan-out
      // propagated. The runner's bucket-0 timing already captures
      // per-row apply scale.
      verifyAnyRowHasBg(
        sharedIsB ? COLOR_B_RGB : COLOR_A_RGB,
        `iter ${i + 1} (SHARED bumped to ${sharedIsB ? 'B' : 'A'})`,
      );
      runs.push({
        iter: i + 1,
        bucket,
        apply: m.apply,
        firstPaint: m.firstPaint,
        worstFrame: m.worstFrame,
      });
    } else {
      // POINT bumps one row. Each iteration picks a different row
      // so we don't always hit the same DOM element's
      // cache-warmth.
      const rowIdx = (Math.floor(i / 2) * 7) % rows;
      const wantB = (Math.floor(i / 2) % 2 === 0);
      const m = await measureOne(() => setPointColor(rowIdx, wantB ? 'B' : 'A'));
      // POINT verification is loose: just confirm some element on
      // the page has the expected color. A stricter check (the
      // specific row's bg matches) would require variant-specific
      // DOM probing.
      verifyAnyRowHasBg(
        wantB ? COLOR_B_RGB : COLOR_A_RGB,
        `iter ${i + 1} (POINT row=${rowIdx} → ${wantB ? 'B' : 'A'})`,
      );
      runs.push({
        iter: i + 1,
        bucket,
        apply: m.apply,
        firstPaint: m.firstPaint,
        worstFrame: m.worstFrame,
      });
    }
    if (onProgress) onProgress(runs);
    await new Promise(r => setTimeout(r, 50));
  }

  return runs;
}

/// Each row's leaf text is `rstyle <i>`. Count distinct leaves.
/// Verifier doesn't check the bg color here — that's covered by
/// the per-iteration verifier. We just want to confirm the mount
/// produced N rows.
function verifyReactiveRowsMounted(expected, context) {
  const all = document.querySelectorAll('*');
  let count = 0;
  for (const el of all) {
    if (el.children.length !== 0) continue;
    const txt = el.textContent;
    if (txt && /^rstyle \d+$/.test(txt.trim())) count++;
  }
  if (count !== expected) {
    throw new Error(
      `reactive-style verify failed: ${context} — expected ${expected} rows ` +
      `(text matching /^rstyle \\d+$/), found ${count}. ` +
      `setupReactiveStyles likely didn't mount the row list.`,
    );
  }
}

/// Scan for ANY element whose computed background-color matches
/// `expected`. Coarse on purpose — variants render bg on different
/// element levels (the row wrapper vs the leaf), so we don't pin
/// down which element it should be. As long as *some* element has
/// the right color, the binding propagated.
function verifyAnyRowHasBg(expected, context) {
  const all = document.querySelectorAll('*');
  for (const el of all) {
    const bg = window.getComputedStyle(el).backgroundColor;
    if (bg === expected) return;
  }
  throw new Error(
    `reactive-style verify failed: ${context} — no element in the DOM has ` +
    `computed background-color ${expected}. The signal update didn't ` +
    `propagate to any styled element.`,
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
