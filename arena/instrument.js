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
