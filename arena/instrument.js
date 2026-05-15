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
