// Theme-toggle suite: mounts a row list once, then alternates
// `setTheme('light')` and `setTheme('dark')` for `iterations`
// measured toggles. The hot path here is each framework's
// per-element style re-apply / cascade re-resolve when the
// theme changes — completely different shape from `rebuild`,
// which exercises mount + unmount.
//
// Each variant must expose `setTheme(name)` where `name` is
// either `'light'` or `'dark'`. The resolution contract is the
// same as `setRows`: when the promise resolves, the DOM must
// reflect the new theme. Microtask flush OK; rAF wait
// forbidden (bakes ~16ms into apply that other variants
// don't pay).
//
// Bucket convention for the runner: `bucket = 0` means the
// transition WENT FROM light TO dark (the page is now dark);
// `bucket = 1` means dark→light (page is now light). The
// runner labels these "L→D" and "D→L" in the table header.

const TRANSITION_MS = 250;
const SLACK_MS = 50;

export const meta = {
  name: 'toggle',
  title: 'Theme toggle',
  description:
    "Mounts N rows once, then alternates between LIGHT and DARK for "
    + "`iterations` measured toggles. Stresses each framework's per-element "
    + "style re-apply / cascade re-resolve path.",
  params: [
    { name: 'rows',         label: 'Rows',           type: 'number', default: 1000, min: 1, max: 100000 },
    { name: 'iterations',   label: 'Iterations',     type: 'number', default: 10,   min: 1, max: 100   },
    // Untimed toggles run back to back (one frame each) before the
    // paced warmup, to get the JIT past its tier-up schedule. V8
    // compiles a function up a tier (Liftoff → TurboFan for wasm,
    // Ignition → Sparkplug → Maglev for JS) after it has run a fixed
    // amount, so in every page load the compile events land on the
    // SAME toggle numbers, ~5–30 on the web variants (traced:
    // Sparkplug batch at toggle 9, Maglev at 19–22). Toggle n's
    // direction is fixed by n's parity, so with only the paced
    // warmup those one-off compiles always fell on one direction's
    // samples and read as a light→dark vs dark→light gap that is not
    // in either toggle's work. Rounded up to even so the measured
    // loop still starts on light→dark.
    { name: 'jitWarmup',    label: 'JIT warmup toggles', type: 'number', default: 50, min: 0, max: 1000 },
    // Two paced warmup toggles — one in each direction — so the first
    // measured toggle of each direction follows the same 300 ms
    // cadence as the rest.
    { name: 'warmupCycles', label: 'Warmup toggles', type: 'number', default: 2,    min: 0, max: 10   },
  ],
};

/// Run the toggle suite.
///
/// `opts`:
///   - `setRows(n)`   mount-rows hook from the variant. Called
///                    once before the measured loop starts to
///                    establish the row count.
///   - `setTheme(t)`  theme-mutator hook. `t` is `'light'` or
///                    `'dark'`. Must resolve when the DOM
///                    reflects the new theme.
///   - `params`       form values.
///   - `onProgress`   optional per-iter callback.
export async function run({ setRows, setTheme, params, onProgress }) {
  if (typeof setTheme !== 'function') {
    throw new Error("toggle suite: variant must expose setTheme(name)");
  }
  const rows = Number(params?.rows ?? 1000);
  const iterations = Number(params?.iterations ?? 10);
  const warmupCycles = Number(params?.warmupCycles ?? 2);
  const jitWarmup = Number(params?.jitWarmup ?? 50);

  // One-time mount. Most variants need a row list to theme; the
  // ones that don't (e.g. css-vars-only variants where the rows
  // would be inert under toggle) still need the DOM to exist so
  // the toggle has *something* to re-style. If the variant
  // doesn't support setRows (older variants), we skip — the
  // toggle suite will measure the chrome-only theme cost.
  if (typeof setRows === 'function') {
    await setRows(rows);
  }

  // Verify the row mount worked before going into the toggle loop.
  // A silent "0 rows in DOM" means later toggle measurements would
  // be theming nothing — meaningless numbers.
  if (typeof setRows === 'function') {
    verifyRowsMounted(rows, 'after setRows in toggle suite');
  }

  // Page starts in light theme by convention. The JIT warmup runs
  // an even number of toggles, so the page is light again after it.
  // One frame per toggle lets style recalc run as it does in the
  // measured loop, without the 300 ms cadence.
  let currentDark = false;
  const jitToggles = jitWarmup + (jitWarmup % 2);
  for (let i = 0; i < jitToggles; i++) {
    currentDark = !currentDark;
    await setTheme(currentDark ? 'dark' : 'light');
    await new Promise(r => requestAnimationFrame(r));
  }
  verifyThemeApplied(currentDark, `after ${jitToggles} JIT warmup toggles`);

  // Paced warmup: both directions once at the measured cadence, to
  // warm font and style caches at both polarities.
  for (let i = 0; i < warmupCycles; i++) {
    currentDark = !currentDark;
    await measureOne(() => setTheme(currentDark ? 'dark' : 'light'));
    verifyThemeApplied(currentDark, `warmup toggle ${i + 1}`);
  }

  const runs = [];
  for (let i = 0; i < iterations; i++) {
    currentDark = !currentDark;
    const direction = currentDark ? 0 : 1;  // 0 = L→D, 1 = D→L
    const m = await measureOne(() => setTheme(currentDark ? 'dark' : 'light'));
    // Verify the toggle actually re-styled the page. If the variant
    // silently no-ops (e.g. setTheme reachable but the framework
    // didn't fan out the change), the bench would otherwise emit
    // sub-millisecond apply numbers for "theme toggles" that
    // changed nothing.
    verifyThemeApplied(currentDark, `iteration ${i + 1} (${currentDark ? 'L→D' : 'D→L'})`);
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

/// Verify the row list is actually mounted. Same shape as
/// `rebuild.js`'s row check — leaf elements whose direct text
/// matches `Row #N`.
function verifyRowsMounted(expected, context) {
  const all = document.querySelectorAll('*');
  let count = 0;
  for (const el of all) {
    if (el.children.length !== 0) continue;
    const txt = el.textContent;
    if (txt && /^Row #\d+$/.test(txt.trim())) count++;
  }
  if (count !== expected) {
    throw new Error(
      `toggle verify failed: ${context} — expected ${expected} rows in DOM, ` +
      `found ${count}. The toggle suite needs a mounted row list to theme; ` +
      `if rows didn't mount, every subsequent setTheme measurement is meaningless.`,
    );
  }
}

/// Verify the theme actually applied by sampling computed styles.
/// We look for ANY element whose computed `background-color` matches
/// the theme's expected background. CSS-variable, class-swap, and
/// inline-style variants all end up with at least one element painted
/// the theme's background color, so this is the most variant-agnostic
/// check that still catches the "setTheme returned but nothing
/// changed" silent-failure shape.
///
/// Colors here mirror the canonical LIGHT/DARK exports in
/// `instrument.js`. Keep these in sync if those change.
const LIGHT_BG_RGB = 'rgb(247, 247, 251)'; // #f7f7fb
const DARK_BG_RGB = 'rgb(15, 17, 21)';     // #0f1115

function verifyThemeApplied(isDark, context) {
  const expected = isDark ? DARK_BG_RGB : LIGHT_BG_RGB;
  const opposite = isDark ? LIGHT_BG_RGB : DARK_BG_RGB;
  let hasExpected = false;
  let hasOpposite = false;
  for (const el of document.querySelectorAll('*')) {
    const bg = window.getComputedStyle(el).backgroundColor;
    if (bg === expected) hasExpected = true;
    else if (bg === opposite) hasOpposite = true;
    if (hasExpected && hasOpposite) break;
  }
  if (!hasExpected) {
    const dir = isDark ? 'dark' : 'light';
    throw new Error(
      `toggle verify failed: ${context} — after setTheme('${dir}'), no element ` +
      `in the DOM has computed background-color ${expected} (the canonical ` +
      `${dir} background). The variant's setTheme likely didn't propagate the ` +
      `change to any styled element. ` +
      (hasOpposite
        ? `(Elements still painted with the OPPOSITE theme's background — ` +
          `the theme change reverted, or never applied at all.)`
        : `(No elements match either theme — the bench may be measuring an ` +
          `unstyled DOM.)`),
    );
  }
}

/// Same `measureOne` as `rebuild.js`. Repeated inline rather
/// than imported to keep each suite a self-contained module.
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
