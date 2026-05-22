// Headless driver for the animation suite.
//
// Usage:
//   node benchmark/anim/run-headless.mjs [variant ...]
//
// With no args, runs both `vanilla-anim` and `idealyst-native-anim`.
// Expects `benchmark/serve` already running on :8080.
//
// What this does:
//   1. Spawns headless Chrome with --remote-debugging-port=9223.
//   2. For each variant, opens `?suite=anim-bounce&...` and polls
//      `window.__benchResult` via Runtime.evaluate.
//   3. Prints the resulting runs[] as JSON for downstream interp.
//
// Why CDP and not puppeteer: avoids a dependency install. Node 22 has
// a built-in WebSocket; everything else is HTTP + JSON. ~150 lines.
//
// Why headless Chrome and not jsdom: the suite needs requestAnimationFrame
// with real frame cadence + DOM transforms; jsdom's rAF is a setTimeout
// shim and the perf measurements would be meaningless.

import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const PORT = 9223;
const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const BENCH_BASE = 'http://localhost:8080';
const DEFAULT_SUITE_PARAMS = {
  nValues: '0,100,1000,5000,10000',
  iterations: '3',
  windowMs: '3000',
  seed: '1',
};

// CLI: `node run-headless.mjs [--suite=NAME] [variant ...]`. When no
// --suite is passed, runs anim-bounce. When no variant args, runs both
// anim variants. Multiple --suite flags chain the suites sequentially
// per variant.
const args = process.argv.slice(2);
const suiteFlags = [];
const variantArgs = [];
for (const a of args) {
  if (a.startsWith('--suite=')) suiteFlags.push(a.slice(8));
  else variantArgs.push(a);
}
const suites = suiteFlags.length ? suiteFlags : ['anim-bounce'];
const variants = variantArgs.length ? variantArgs : ['vanilla-anim', 'idealyst-native-anim'];

// Module-scope CDP-call counter. Must be initialized BEFORE the
// for-loop below calls runVariant → sendCmd; previously declared
// after the loop, which triggered a TDZ error on first call.
let nextCmdId = 1;

// ─────────────────────────────────────────────────────────────────────
// Verify bench server up — chrome will just give us a blank page
// otherwise and we'd wait forever.

const serverProbe = await fetch(BENCH_BASE + '/').catch(() => null);
if (!serverProbe?.ok) {
  console.error(`bench server not reachable at ${BENCH_BASE}. Start it: benchmark/serve`);
  process.exit(1);
}

// ─────────────────────────────────────────────────────────────────────
// Spawn headless Chrome.
// --headless=new is the modern headless mode; old --headless quirks
// don't apply. --no-sandbox not needed on macOS but harmless if reused.
// --disable-gpu is necessary on macOS for stable headless rAF cadence.

const userDataDir = mkdtempSync(join(tmpdir(), 'bench-chrome-'));
const chromeArgs = [
  '--headless=new',
  `--remote-debugging-port=${PORT}`,
  `--user-data-dir=${userDataDir}`,
  '--no-first-run',
  '--no-default-browser-check',
  '--disable-gpu',
  '--hide-scrollbars',
  '--mute-audio',
  'about:blank',
];

const chrome = spawn(CHROME, chromeArgs, { stdio: ['ignore', 'pipe', 'pipe'] });
let chromeAlive = true;
chrome.on('exit', code => {
  chromeAlive = false;
  if (code !== 0 && code !== null) console.error(`chrome exited ${code}`);
});

const cleanup = async () => {
  if (chromeAlive) chrome.kill('SIGTERM');
  // Best-effort temp-dir cleanup. Don't wait for chrome to release locks.
  try { rmSync(userDataDir, { recursive: true, force: true }); } catch {}
};
process.on('exit', cleanup);
process.on('SIGINT', () => { cleanup(); process.exit(130); });

// Wait for Chrome's DevTools endpoint to come up. Poll /json/version
// instead of sleeping — usually ready in <500ms but cold starts can
// take longer.
const debugUrl = await waitForDebugger(PORT, 10000);
console.error(`[driver] devtools up at ${debugUrl}`);

// ─────────────────────────────────────────────────────────────────────
// Drive each variant sequentially.

const results = {};
for (const suite of suites) {
  results[suite] = {};
  for (const variant of variants) {
    console.error(`[driver] ${suite} · ${variant}...`);
    const t0 = Date.now();
    try {
      const result = await runVariant(variant, suite);
      const seconds = ((Date.now() - t0) / 1000).toFixed(1);
      console.error(`[driver] ${suite} · ${variant} done in ${seconds}s`);
      results[suite][variant] = result;
    } catch (err) {
      console.error(`[driver] ${suite} · ${variant} failed: ${err.message}`);
      results[suite][variant] = { error: err.message };
    }
  }
}

await cleanup();
console.log(JSON.stringify(results, null, 2));

// ─────────────────────────────────────────────────────────────────────
// helpers

async function waitForDebugger(port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (r.ok) {
        const j = await r.json();
        return j.webSocketDebuggerUrl;
      }
    } catch {}
    await sleep(100);
  }
  throw new Error('chrome devtools never came up');
}

async function runVariant(variant, suite) {
  const url = buildVariantUrl(variant, suite);
  // /json/new creates a new tab and returns its target info, including
  // its own per-tab webSocketDebuggerUrl. We connect to that — the
  // browser-level WS only handles target management.
  const newR = await fetch(`http://127.0.0.1:${PORT}/json/new?${encodeURIComponent(url)}`,
                           { method: 'PUT' });
  if (!newR.ok) throw new Error(`tab create failed: ${newR.status}`);
  const target = await newR.json();
  const ws = await openWs(target.webSocketDebuggerUrl);

  try {
    // Page.enable so we get Page.loadEventFired (not strictly required
    // since we poll, but helps confirm the page is alive).
    await sendCmd(ws, 'Page.enable', {});
    await sendCmd(ws, 'Runtime.enable', {});

    // Poll window.__benchResult. The suite resolves it after all
    // iterations finish; deadline is generous to allow for the full
    // 5N × 3iter × 3s ≈ 50s run.
    const deadline = Date.now() + 180000;
    while (Date.now() < deadline) {
      const probe = await evaluate(ws, `
        (function() {
          if (window.__benchError) return { err: window.__benchError };
          if (window.__benchResult) return { ok: window.__benchResult };
          return null;
        })()
      `);
      if (probe?.err) throw new Error(probe.err);
      if (probe?.ok) return probe.ok;
      await sleep(500);
    }
    throw new Error('timeout waiting for window.__benchResult (>3min)');
  } finally {
    ws.close();
    await fetch(`http://127.0.0.1:${PORT}/json/close/${target.id}`).catch(() => {});
  }
}

function buildVariantUrl(variant, suite) {
  const qs = new URLSearchParams({ suite, ...DEFAULT_SUITE_PARAMS });
  return `${BENCH_BASE}/${variant}/?${qs}`;
}

function openWs(url) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    ws.addEventListener('open', () => resolve(ws), { once: true });
    ws.addEventListener('error', e => reject(new Error(`ws connect failed: ${e?.message ?? 'unknown'}`)), { once: true });
  });
}

function sendCmd(ws, method, params) {
  return new Promise((resolve, reject) => {
    const id = nextCmdId++;
    const handler = (ev) => {
      let msg;
      try { msg = JSON.parse(ev.data); } catch { return; }
      if (msg.id !== id) return;
      ws.removeEventListener('message', handler);
      if (msg.error) reject(new Error(`${method} failed: ${msg.error.message}`));
      else resolve(msg.result);
    };
    ws.addEventListener('message', handler);
    ws.send(JSON.stringify({ id, method, params }));
  });
}

async function evaluate(ws, expression) {
  const r = await sendCmd(ws, 'Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: false,
  });
  if (r.exceptionDetails) {
    const msg = r.exceptionDetails.exception?.description || r.exceptionDetails.text;
    throw new Error(`page eval threw: ${msg}`);
  }
  return r.result?.value;
}

function sleep(ms) {
  return new Promise(r => setTimeout(r, ms));
}
