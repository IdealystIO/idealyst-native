// Granular-update suite: mounts N rows once, each with its OWN
// counter signal/value. The body alternates two operations:
//
//   POINT (bucket 0): bump one specific row's counter. Fine-grained
//                     frameworks (signals, refs, runes) update one
//                     leaf. Coarse re-render frameworks (naive React)
//                     re-render the whole list.
//
//   SPREAD (bucket 1): bump a small batch of rows' counters in ONE
//                      reactive batch. Tests batch coalescing — a
//                      well-behaved framework fans out exactly K
//                      effects, one per touched row, regardless of
//                      how the writes are timed.
//
// Different from `hierarchy`: that's a TREE of varying depth with
// branch vs global updates. This is a FLAT list, with per-row
// signals — closer to "list of inboxes with unread counters",
// where one item should be cheap to bump regardless of list size.
//
// Variant contract:
//   - `setupCounters(n)`     mount N rows. Each row text reads its
//                            own `Signal<u64>`-style counter and
//                            renders `"row {i}: c={value}"`.
//   - `bumpCounter(i, v)`    set row `i`'s counter to `v`. After
//                            return, the DOM must reflect the new
//                            value.
//   - `bumpRange(s, e, v)`   set counters for rows [s, e) to `v`
//                            inside a single batch. Variants
//                            without explicit batching can do a
//                            naive loop — the suite catches the
//                            difference in apply time.

export const meta = {
  name: 'granular',
  title: 'Granular per-row updates',
  description:
    'Mounts N rows each owning a counter signal. Alternates POINT '
    + '(bump one row) and SPREAD (batch-bump K rows). POINT exposes '
    + 'per-leaf reactive isolation; SPREAD tests batch coalescing.',
  params: [
    { name: 'rows',         label: 'Rows',           type: 'number', default: 2000, min: 1, max: 100000 },
    // SPREAD bumps this many rows per iteration. Default 50 (small
    // fan-out) so the cost still dwarfs POINT for naive frameworks
    // but stays bounded for fine-grained ones.
    { name: 'spread',       label: 'Spread (K)',     type: 'number', default: 50,   min: 1, max: 10000 },
    { name: 'iterations',   label: 'Iterations',     type: 'number', default: 20,   min: 2, max: 200 },
    { name: 'warmupCycles', label: 'Warmup pairs',   type: 'number', default: 2,    min: 0, max: 10 },
  ],
};

const TRANSITION_MS = 250;
const SLACK_MS = 50;

export async function run({ setupCounters, bumpCounter, bumpRange, params, onProgress }) {
  if (typeof setupCounters !== 'function') {
    throw new Error("granular suite: variant must expose setupCounters(n)");
  }
  if (typeof bumpCounter !== 'function') {
    throw new Error("granular suite: variant must expose bumpCounter(i, v)");
  }
  if (typeof bumpRange !== 'function') {
    throw new Error("granular suite: variant must expose bumpRange(start, end, v)");
  }

  const rows = Number(params?.rows ?? 2000);
  const spread = Number(params?.spread ?? 50);
  const iterations = Number(params?.iterations ?? 20);
  const warmupCycles = Number(params?.warmupCycles ?? 2);

  await setupCounters(rows);
  verifyCounterRowsMounted(rows, 'after setupCounters');

  // Warmup. Bump row 0 then a small range; both untimed.
  let value = 0;
  for (let i = 0; i < warmupCycles; i++) {
    value++;
    await measureOne(() => bumpCounter(0, value));
    verifyRowHasValue(0, value, `warmup ${i + 1} after bumpCounter(0)`);
    value++;
    await measureOne(() => bumpRange(0, Math.min(spread, rows), value));
    // Each spread row should reflect the new value. Sample row 0
    // and the last in the range — if both have the new value, the
    // batch fanned out.
    verifyRowHasValue(0, value, `warmup ${i + 1} after bumpRange(0,${spread})`);
    verifyRowHasValue(Math.min(spread, rows) - 1, value, `warmup ${i + 1} after bumpRange(0,${spread})`);
  }

  const runs = [];
  // Each iteration bumps a fresh row index for POINT so we don't
  // keep hitting cache state from the previous bump. SPREAD always
  // starts at the same offset so the test is reproducible.
  const pointSpacing = Math.max(1, Math.floor(rows / iterations));

  for (let i = 0; i < iterations; i++) {
    const isPoint = i % 2 === 0;
    const bucket = isPoint ? 0 : 1;  // 0 = POINT, 1 = SPREAD
    value++;
    if (isPoint) {
      const rowIdx = (Math.floor(i / 2) * pointSpacing) % rows;
      const m = await measureOne(() => bumpCounter(rowIdx, value));
      verifyRowHasValue(rowIdx, value, `iter ${i + 1} (POINT row=${rowIdx})`);
      runs.push({
        iter: i + 1,
        bucket,
        apply: m.apply,
        firstPaint: m.firstPaint,
        worstFrame: m.worstFrame,
      });
    } else {
      // Wrap the spread within the row range. Different start each
      // SPREAD iteration so we don't always re-bump the same set.
      const start = (Math.floor(i / 2) * spread) % rows;
      const end = Math.min(start + spread, rows);
      const m = await measureOne(() => bumpRange(start, end, value));
      verifyRowHasValue(start, value, `iter ${i + 1} (SPREAD start=${start})`);
      verifyRowHasValue(end - 1, value, `iter ${i + 1} (SPREAD end=${end - 1})`);
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

/// Count elements whose direct text matches `row N: c=V`. Same
/// shape as `hierarchy`'s leaf check — direct-text-only, leaf
/// elements only — so it works across variant DOM shapes.
function verifyCounterRowsMounted(expected, context) {
  const all = document.querySelectorAll('*');
  let count = 0;
  for (const el of all) {
    if (el.children.length !== 0) continue;
    const txt = el.textContent;
    if (txt && /^row \d+: c=\d+$/.test(txt.trim())) count++;
  }
  if (count !== expected) {
    throw new Error(
      `granular verify failed: ${context} — expected ${expected} counter rows ` +
      `(text matching /^row \\d+: c=\\d+$/), found ${count}. ` +
      `setupCounters likely didn't mount the row list.`,
    );
  }
}

/// Sample one row by index — its text must end with `c=<expected>`.
/// `i` is the row's index inside the cohort (0-based), which the
/// variant must encode into the leaf text.
function verifyRowHasValue(i, expected, context) {
  const all = document.querySelectorAll('*');
  for (const el of all) {
    if (el.children.length !== 0) continue;
    const txt = el.textContent;
    if (!txt) continue;
    const t = txt.trim();
    const m = t.match(/^row (\d+): c=(\d+)$/);
    if (!m) continue;
    if (Number(m[1]) !== i) continue;
    const got = Number(m[2]);
    if (got !== expected) {
      throw new Error(
        `granular verify failed: ${context} — row ${i} should display ` +
        `c=${expected}, but its text is "${t}". Variant's bump function ` +
        `didn't propagate to this row's binding.`,
      );
    }
    return;
  }
  throw new Error(
    `granular verify failed: ${context} — row index ${i} not found in DOM. ` +
    `Either the row list was torn down or the variant doesn't expose row ${i}.`,
  );
}

/// Same measureOne as the other suites — apply + post-apply frame
/// gaps inside the transition window.
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
