// Shared instrumentation for arena toggle benchmarks.
//
// Every variant calls runToggle() inside its toggle handler. The harness
// measures the JS-side apply time, the first paint after the click, the
// rAF cadence during the transition window, and the worst inter-frame gap.
// Then it writes the result into <span id="stats">.
//
// Keep this dependency-free and untranspiled — it's loaded as a module by
// the vanilla baseline AND bundled by Vite for the React variants. Both
// paths agree on the same numbers.

const TRANSITION_MS = 250;
const SLACK_MS = 50;

function nextFrame() {
  return new Promise(r => requestAnimationFrame(r));
}

function fmt(n, d = 2) {
  return n.toFixed(d);
}

// Run a toggle and measure it.
//
//   apply       — synchronous JS work inside the handler
//   firstPaint  — time from click start to first rAF
//   frames      — rAFs observed during the 250ms+slack window
//   avg fps     — frames / elapsed
//   worst frame — max gap between consecutive rAFs (jank)
//
// `meta` is { theme, rows } — labels that show up verbatim in the readout
// so it's hard to misread which run you're looking at.
export async function runToggle(applyFn, meta) {
  const statsEl = document.getElementById('stats');

  const t0 = performance.now();
  await applyFn();
  const applyDone = performance.now();

  let frames = 0;
  let lastFrame = applyDone;
  let worstFrame = 0;

  const firstFrame = await nextFrame();
  const firstPaint = firstFrame - t0;
  const gap0 = firstFrame - lastFrame;
  if (gap0 > worstFrame) worstFrame = gap0;
  lastFrame = firstFrame;
  frames++;

  const deadline = t0 + TRANSITION_MS + SLACK_MS;
  while (performance.now() < deadline) {
    const t = await nextFrame();
    const gap = t - lastFrame;
    if (gap > worstFrame) worstFrame = gap;
    lastFrame = t;
    frames++;
  }

  const elapsed = performance.now() - t0;
  const avgFps = frames / (elapsed / 1000);

  const line =
    `theme=${meta.theme} | rows=${meta.rows} | ` +
    `apply: ${fmt(applyDone - t0)}ms | ` +
    `first paint: ${fmt(firstPaint)}ms | ` +
    `frames: ${frames} | ` +
    `avg fps: ${fmt(avgFps, 1)} | ` +
    `worst frame: ${fmt(worstFrame)}ms`;

  if (statsEl) statsEl.textContent = line;
  return line;
}

// Themes — kept here so every variant imports the exact same values
// rather than risking a typo'd hex in one of them.
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

export const ROW_COUNT = 1000;

// Row count — read from the URL query string so the same variant
// can be tested at multiple scales without code changes. Defaults
// to `ROW_COUNT` (1000) when no `?rows=` param is set. Bounded to
// a sane max so a stray `?rows=999999999` doesn't lock the browser
// before the user can type a correction.
const ROW_MAX = 100_000;

/// Read the row count from the URL's `?rows=` param. Returns
/// `ROW_COUNT` when the param is missing or invalid. Clamps to
/// `[1, ROW_MAX]`.
export function getRowCount() {
  const raw = new URLSearchParams(window.location.search).get('rows');
  if (raw == null) return ROW_COUNT;
  const n = parseInt(raw, 10);
  if (!Number.isFinite(n) || n < 1) {
    console.warn(`[arena] invalid ?rows=${raw}, falling back to ${ROW_COUNT}`);
    return ROW_COUNT;
  }
  if (n > ROW_MAX) {
    console.warn(`[arena] ?rows=${n} exceeds cap; clamping to ${ROW_MAX}`);
    return ROW_MAX;
  }
  return n;
}

/// Write the row count back to the URL. Uses `replaceState` so
/// the change doesn't push a back-button entry — the user can hit
/// back to leave the variant and not have to step through every
/// count they tried.
export function setRowCount(n) {
  const url = new URL(window.location.href);
  url.searchParams.set('rows', String(n));
  window.history.replaceState(null, '', url.toString());
}

// ---------------------------------------------------------------
// Automated suite
// ---------------------------------------------------------------
//
// Each variant exposes a single `setRows(n)` function and calls
// `runSuite({ setRows, ... })` to drive it. The suite alternates
// between two row counts (default 1000 ↔ 10000) for `iterations`
// rebuilds, measuring each with the same harness `runToggle` uses.
// A pre-flight rebuild at the first count is discarded as warmup —
// the JIT, wasm caches, and font-rendering pipeline all warm up
// during it and would otherwise skew run 1's numbers high.
//
// Results are written into the passed `resultsEl` as a table:
// per-iteration rows during execution, then a per-count summary
// (median / min / max of `apply` and `first paint`) at the end.
//
// The suite is sequential and waits a full transition window
// between iterations (via runToggle's built-in rAF loop), so the
// browser settles before the next rebuild. Runs synchronously
// w.r.t. JS — there's no concurrent work to confound the numbers.

function median(xs) {
  if (xs.length === 0) return 0;
  const sorted = [...xs].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[mid - 1] + sorted[mid]) / 2
    : sorted[mid];
}

function makeResultsTable() {
  const wrap = document.createElement('div');
  wrap.style.cssText = 'font: 13px/1.4 monospace; margin-top: 16px;';
  return wrap;
}

function renderProgress(resultsEl, runs) {
  // Render a live progress table — one row per completed iteration.
  // Cheap to re-render on each tick; the suite isn't speed-critical
  // on the JS side (the timed work is the rebuild call, not the
  // table render that happens around it).
  let html = '<table style="border-collapse: collapse; width: 100%;">';
  html += '<thead><tr>';
  for (const h of ['#', 'rows', 'apply (ms)', 'first paint (ms)', 'worst frame (ms)']) {
    html += `<th style="text-align: right; padding: 4px 8px; border-bottom: 1px solid #888;">${h}</th>`;
  }
  html += '</tr></thead><tbody>';
  for (const r of runs) {
    html += '<tr>';
    html += `<td style="text-align: right; padding: 2px 8px;">${r.iter}</td>`;
    html += `<td style="text-align: right; padding: 2px 8px;">${r.rows}</td>`;
    html += `<td style="text-align: right; padding: 2px 8px;">${r.apply.toFixed(1)}</td>`;
    html += `<td style="text-align: right; padding: 2px 8px;">${r.firstPaint.toFixed(1)}</td>`;
    html += `<td style="text-align: right; padding: 2px 8px;">${r.worstFrame.toFixed(1)}</td>`;
    html += '</tr>';
  }
  html += '</tbody></table>';
  resultsEl.innerHTML = html;
}

function renderSummary(resultsEl, runs) {
  // Group by row count, drop the warmup (iter 0), report
  // median/min/max of `apply` and `first paint`.
  const byCount = new Map();
  for (const r of runs) {
    if (!byCount.has(r.rows)) byCount.set(r.rows, []);
    byCount.get(r.rows).push(r);
  }
  let html = renderProgress(resultsEl, runs);
  // re-use the existing per-iter table by re-rendering it then
  // appending the summary; renderProgress wrote into innerHTML, so
  // we read it back and append.
  html = resultsEl.innerHTML;
  html += '<div style="margin-top: 16px; font-weight: 600;">summary (median / min / max)</div>';
  html += '<table style="border-collapse: collapse; width: 100%; margin-top: 4px;">';
  html += '<thead><tr>';
  for (const h of ['rows', 'apply median', 'apply min', 'apply max', 'paint median', 'paint min', 'paint max']) {
    html += `<th style="text-align: right; padding: 4px 8px; border-bottom: 1px solid #888;">${h}</th>`;
  }
  html += '</tr></thead><tbody>';
  for (const [rows, rs] of [...byCount.entries()].sort((a, b) => a[0] - b[0])) {
    const applies = rs.map(r => r.apply);
    const paints = rs.map(r => r.firstPaint);
    html += '<tr>';
    html += `<td style="text-align: right; padding: 2px 8px;">${rows}</td>`;
    html += `<td style="text-align: right; padding: 2px 8px;">${median(applies).toFixed(1)}</td>`;
    html += `<td style="text-align: right; padding: 2px 8px;">${Math.min(...applies).toFixed(1)}</td>`;
    html += `<td style="text-align: right; padding: 2px 8px;">${Math.max(...applies).toFixed(1)}</td>`;
    html += `<td style="text-align: right; padding: 2px 8px;">${median(paints).toFixed(1)}</td>`;
    html += `<td style="text-align: right; padding: 2px 8px;">${Math.min(...paints).toFixed(1)}</td>`;
    html += `<td style="text-align: right; padding: 2px 8px;">${Math.max(...paints).toFixed(1)}</td>`;
    html += '</tr>';
  }
  html += '</tbody></table>';
  resultsEl.innerHTML = html;
}

/// Run the automated benchmark suite.
///
///   setRows(n)   — async fn the variant supplies. Should rebuild
///                  the row list at count `n`. The harness wraps
///                  this in a `runToggle`-style measurement, so
///                  the call should be the rebuild work *only* —
///                  no extra setup.
///   iterations   — how many measured rebuilds to run (default 10).
///                  Plus one unmeasured warmup at the start.
///   counts       — array of row counts to alternate between
///                  (default [1000, 10000]). Iteration k uses
///                  `counts[k % counts.length]`.
///   resultsEl    — DOM node to render the live + final tables into.
export async function runSuite({
  setRows,
  iterations = 10,
  counts = [1000, 10000],
  resultsEl,
}) {
  if (typeof setRows !== 'function') {
    throw new Error('runSuite: setRows must be a function');
  }
  if (!resultsEl) {
    throw new Error('runSuite: resultsEl is required');
  }

  // Warmup: rebuild once at the first count. Numbers from this
  // iteration are discarded — the first run after page load
  // includes JIT warmup, font-rendering setup, and a few other
  // one-time costs that would inflate it 50-100%.
  resultsEl.innerHTML = '<div style="opacity: 0.7;">warmup…</div>';
  await runToggle(() => setRows(counts[0]), { theme: 'warmup', rows: counts[0] });

  const runs = [];
  for (let i = 0; i < iterations; i++) {
    const rows = counts[i % counts.length];
    const startedAt = performance.now();
    let applyMs = 0;
    let firstPaintMs = 0;
    let worstFrameMs = 0;
    // Inline the runToggle math here so we can record the
    // individual numbers without parsing them back out of the
    // string the original writes to #stats.
    await new Promise(async (resolve) => {
      const t0 = performance.now();
      await setRows(rows);
      const applyDone = performance.now();
      applyMs = applyDone - t0;

      let lastFrame = applyDone;
      const firstFrame = await new Promise(r => requestAnimationFrame(r));
      firstPaintMs = firstFrame - t0;
      let gap = firstFrame - lastFrame;
      if (gap > worstFrameMs) worstFrameMs = gap;
      lastFrame = firstFrame;

      const deadline = t0 + TRANSITION_MS + SLACK_MS;
      while (performance.now() < deadline) {
        const t = await new Promise(r => requestAnimationFrame(r));
        gap = t - lastFrame;
        if (gap > worstFrameMs) worstFrameMs = gap;
        lastFrame = t;
      }
      resolve();
    });

    runs.push({
      iter: i + 1,
      rows,
      apply: applyMs,
      firstPaint: firstPaintMs,
      worstFrame: worstFrameMs,
    });
    renderProgress(resultsEl, runs);
    // Yield briefly between iterations. Not strictly needed (the
    // transition window above already drained a few frames), but
    // gives the browser a beat to settle queued GC / paint work.
    await new Promise(r => setTimeout(r, 50));
  }

  renderSummary(resultsEl, runs);
  return runs;
}
