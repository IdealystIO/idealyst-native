#!/usr/bin/env node
// Headless benchmark driver.
//
// Spins variant pages inside a CDP-controlled headless Chrome, listens
// for the suite's `postMessage` results via a host.html shim, and
// prints a per-(variant, suite, bucket) summary.
//
// Prereqs (the CLI checks each and tells you what's missing):
//   - bench server on :8080 serving `benchmark/`. Start with
//     `benchmark/serve` if it isn't already.
//   - Chrome running with --remote-debugging-port=9223 (override via
//     --chrome-port). Any recent Chromium-family browser works.
//
// Examples:
//   node runner/index.mjs list
//   node runner/index.mjs measure --variant idealyst-native --suite rebuild
//   node runner/index.mjs measure --variant all --suite rebuild --iterations 5
//   node runner/index.mjs measure --variants idealyst-native,vanilla-classes --suite toggle
//
// Output is a Markdown table by default. `--json` dumps raw run records.

import { readFileSync, readdirSync, existsSync, statSync } from 'node:fs';
import { resolve, dirname, basename, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocket } from 'ws';

const __dirname = dirname(fileURLToPath(import.meta.url));
const BENCH_DIR = resolve(__dirname, '..');

// -----------------------------------------------------------------
// CLI arg parsing
// -----------------------------------------------------------------
// Minimal hand-rolled parser. We avoid pulling in `commander` /
// `yargs` to keep this tool's dependency footprint to just `ws` —
// the runner has to install cleanly in CI with one `npm install`.

function parseArgs(argv) {
  const out = { _: [], flags: {} };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith('--')) {
      const eq = a.indexOf('=');
      if (eq > 0) {
        out.flags[a.slice(2, eq)] = a.slice(eq + 1);
      } else {
        const next = argv[i + 1];
        if (next == null || next.startsWith('--')) {
          out.flags[a.slice(2)] = true;
        } else {
          out.flags[a.slice(2)] = next;
          i++;
        }
      }
    } else {
      out._.push(a);
    }
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
const cmd = args._[0] || 'help';

// -----------------------------------------------------------------
// Discovery: variants + suites
// -----------------------------------------------------------------

/// A variant is any directory under `benchmark/` containing an
/// `index.html`, EXCEPT the runner/output/build dirs that match
/// none of the variants the bench actually serves.
function listVariants() {
  const skip = new Set([
    'runner', 'suites', 'pkg', 'src', 'target', 'node_modules',
  ]);
  const out = [];
  for (const name of readdirSync(BENCH_DIR)) {
    if (skip.has(name)) continue;
    const p = join(BENCH_DIR, name);
    if (!statSync(p).isDirectory()) continue;
    if (!existsSync(join(p, 'index.html'))) continue;
    out.push(name);
  }
  return out.sort();
}

/// A suite is any `.js` file under `benchmark/suites/` that exports a
/// `meta` object. We parse the file as text (regex-based) to avoid
/// dynamic-importing Node-incompatible browser modules — `suites/*.js`
/// use ESM features that work in the browser but rely on the DOM at
/// import time for some helpers. Cheaper to scrape the metadata.
function listSuites() {
  const dir = join(BENCH_DIR, 'suites');
  const out = [];
  for (const name of readdirSync(dir)) {
    if (!name.endsWith('.js')) continue;
    const src = readFileSync(join(dir, name), 'utf8');
    const meta = parseSuiteMeta(src, name);
    if (meta) out.push(meta);
  }
  return out;
}

/// Pull just enough metadata out of a suite file to populate the
/// param defaults. Matches the literal `export const meta = { … }`
/// block; falls back to a stub if the regex misses (we'd rather run
/// with no-defaults than skip the suite entirely).
function parseSuiteMeta(src, fileName) {
  const m = src.match(/export\s+const\s+meta\s*=\s*\{([\s\S]*?)\n\};/m);
  const stub = { file: fileName, name: fileName.replace(/\.js$/, ''), params: [] };
  if (!m) return stub;
  const body = m[1];
  const nameMatch = body.match(/name:\s*['"]([^'"]+)['"]/);
  const titleMatch = body.match(/title:\s*['"]([^'"]+)['"]/);
  const params = [];
  // Capture: name, default. (label/min/max/type ignored — we only
  // need defaults for filling URL params.)
  const paramRe = /\{\s*name:\s*['"]([^'"]+)['"][^}]*?default:\s*([^,}]+)[^}]*?\}/g;
  let pm;
  while ((pm = paramRe.exec(body)) !== null) {
    const def = pm[2].trim();
    let v;
    if (/^-?\d+(\.\d+)?$/.test(def)) v = Number(def);
    else if (def === 'true') v = true;
    else if (def === 'false') v = false;
    else v = def.replace(/^['"]|['"]$/g, '');
    params.push({ name: pm[1], default: v });
  }
  return {
    file: fileName,
    name: nameMatch ? nameMatch[1] : stub.name,
    title: titleMatch ? titleMatch[1] : (nameMatch ? nameMatch[1] : stub.name),
    params,
  };
}

// -----------------------------------------------------------------
// CDP plumbing
// -----------------------------------------------------------------
// We talk to Chrome's DevTools Protocol over HTTP (for tab create /
// close) plus a per-tab WebSocket (for Page/Runtime messages).

const DEFAULT_CHROME_PORT = Number(args.flags['chrome-port'] ?? 9223);
const DEFAULT_SERVER_PORT = Number(args.flags['server-port'] ?? 8080);

async function cdpHttp(path, init) {
  const res = await fetch(`http://localhost:${DEFAULT_CHROME_PORT}${path}`, init);
  if (!res.ok) throw new Error(`CDP HTTP ${path} → ${res.status} ${res.statusText}`);
  const text = await res.text();
  if (!text) return null;
  try { return JSON.parse(text); } catch { return text; }
}

async function cdpVersion() {
  return cdpHttp('/json/version');
}

async function cdpNewTab(targetUrl) {
  return cdpHttp(`/json/new?${encodeURIComponent(targetUrl)}`, { method: 'PUT' });
}

async function cdpCloseTab(targetId) {
  try { await cdpHttp(`/json/close/${targetId}`); } catch {}
}

/// A `CdpSession` is one tab's WebSocket connection plus a small
/// request/response and event dispatch on top of the bare protocol.
class CdpSession {
  constructor(ws) {
    this.ws = ws;
    this.nextId = 1;
    this.pending = new Map();
    this.eventListeners = new Map();
    ws.on('message', (data) => this._onMessage(data));
  }

  _onMessage(data) {
    let msg;
    try { msg = JSON.parse(data.toString()); } catch { return; }
    if (msg.id != null && this.pending.has(msg.id)) {
      const { resolve: res, reject: rej } = this.pending.get(msg.id);
      this.pending.delete(msg.id);
      if (msg.error) rej(new Error(`${msg.error.code}: ${msg.error.message}`));
      else res(msg.result);
      return;
    }
    if (msg.method) {
      const lst = this.eventListeners.get(msg.method);
      if (lst) for (const cb of lst) cb(msg.params);
    }
  }

  send(method, params = {}) {
    return new Promise((res, rej) => {
      const id = this.nextId++;
      this.pending.set(id, { resolve: res, reject: rej });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  on(method, cb) {
    let lst = this.eventListeners.get(method);
    if (!lst) { lst = []; this.eventListeners.set(method, lst); }
    lst.push(cb);
    return () => {
      const arr = this.eventListeners.get(method);
      if (!arr) return;
      const idx = arr.indexOf(cb);
      if (idx >= 0) arr.splice(idx, 1);
    };
  }

  close() {
    try { this.ws.close(); } catch {}
  }
}

async function openSession(targetUrl) {
  const tab = await cdpNewTab(targetUrl);
  if (!tab || !tab.webSocketDebuggerUrl) {
    throw new Error(`CDP: no webSocketDebuggerUrl in /json/new response: ${JSON.stringify(tab)}`);
  }
  const ws = new WebSocket(tab.webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    ws.once('open', res);
    ws.once('error', rej);
  });
  const session = new CdpSession(ws);
  session.targetId = tab.id;
  return session;
}

// -----------------------------------------------------------------
// Running one (variant, suite) pair
// -----------------------------------------------------------------

/// Run a single (variant, suite) combination and return the runs[]
/// array the suite posted. Times out after `timeoutMs` of silence
/// (no `bench-progress` or `bench-result`); a hung variant looks
/// the same as a crashed one from our side, so a single timeout
/// suffices for both.
async function runOne({
  variant,
  suite,
  paramOverrides,
  timeoutMs,
  serverPort,
  onProgress,
  wantPhases,
}) {
  const url = new URL(`http://localhost:${serverPort}/runner/host.html`);
  url.searchParams.set('variant', variant);
  url.searchParams.set('suite', suite.name);
  url.searchParams.set('runId', String(Date.now()));
  for (const p of suite.params) {
    if (paramOverrides[p.name] != null) {
      url.searchParams.set(p.name, String(paramOverrides[p.name]));
    } else {
      url.searchParams.set(p.name, String(p.default));
    }
  }

  const session = await openSession(url.toString());

  let resolveDone;
  let rejectDone;
  const done = new Promise((res, rej) => { resolveDone = res; rejectDone = rej; });

  let lastTick = Date.now();
  const watchdog = setInterval(() => {
    if (Date.now() - lastTick > timeoutMs) {
      rejectDone(new Error(
        `timeout: no bench events from ${variant}/${suite.name} for ${timeoutMs}ms ` +
        `(URL: ${url.toString()})`,
      ));
    }
  }, 500);

  // When the caller asked for phase counters, we delay resolution by
  // a short grace period after `bench-result` to give the variant a
  // chance to post its `idealyst-phases` follow-up. Variants without
  // debug-stats never post it; the grace times out and we proceed
  // with `phases = null`.
  const PHASES_GRACE_MS = 5000;
  let runs = null;
  let phases = null;
  let resultArrivedAt = null;
  let resolveTimer = null;

  function maybeFinish() {
    if (runs == null) return;
    if (wantPhases && phases == null) {
      // Schedule the final resolve once we have something OR after grace.
      if (resolveTimer != null) return;
      resolveTimer = setTimeout(() => resolveDone({ runs, phases }), PHASES_GRACE_MS);
    } else {
      resolveDone({ runs, phases });
    }
  }

  session.on('Runtime.consoleAPICalled', (params) => {
    const args = params.args || [];
    if (!args.length || args[0].type !== 'string') return;
    const s = args[0].value;
    if (!s || !s.startsWith('BENCH_EVENT:')) return;
    lastTick = Date.now();
    let msg;
    try { msg = JSON.parse(s.slice('BENCH_EVENT:'.length)); } catch { return; }
    if (msg.type === 'bench-progress') {
      if (onProgress) onProgress(msg.runs);
    } else if (msg.type === 'bench-result') {
      runs = msg.runs;
      resultArrivedAt = Date.now();
      maybeFinish();
    } else if (msg.type === 'bench-error') {
      rejectDone(new Error(`variant reported error: ${msg.error}`));
    } else if (msg.type === 'idealyst-phases') {
      phases = msg.phases;
      // Phases arrived — short-circuit any pending grace timer.
      if (resolveTimer != null) { clearTimeout(resolveTimer); resolveTimer = null; }
      maybeFinish();
    }
  });

  // Capture page-level exceptions too — bare uncaughts in the
  // iframe surface here, and they're a common failure mode when a
  // variant's module init throws.
  session.on('Runtime.exceptionThrown', (params) => {
    const text = params.exceptionDetails?.exception?.description
      || params.exceptionDetails?.text
      || 'unknown exception';
    rejectDone(new Error(`page exception in ${variant}/${suite.name}: ${text}`));
  });

  await session.send('Runtime.enable');
  await session.send('Page.enable');

  try {
    return await done;
  } finally {
    clearInterval(watchdog);
    if (resolveTimer != null) clearTimeout(resolveTimer);
    session.close();
    await cdpCloseTab(session.targetId);
  }
}

// -----------------------------------------------------------------
// Summary stats
// -----------------------------------------------------------------

function median(xs) {
  if (!xs.length) return NaN;
  const s = [...xs].sort((a, b) => a - b);
  const mid = s.length >> 1;
  return s.length & 1 ? s[mid] : (s[mid - 1] + s[mid]) / 2;
}

/// Group runs by `bucket` and report median/min/max of `apply` per
/// bucket. We deliberately skip mean — bench numbers are heavy-tailed
/// (one paused GC can 10x a single iteration's apply) and a median
/// is the only summary that holds up across re-runs.
function summarize(runs) {
  const byBucket = new Map();
  for (const r of runs) {
    let arr = byBucket.get(r.bucket);
    if (!arr) { arr = []; byBucket.set(r.bucket, arr); }
    arr.push(r);
  }
  const buckets = [...byBucket.entries()]
    .map(([bucket, rs]) => ({
      bucket,
      n: rs.length,
      apply_p50: median(rs.map(r => r.apply)),
      apply_min: Math.min(...rs.map(r => r.apply)),
      apply_max: Math.max(...rs.map(r => r.apply)),
      worstFrame_p50: median(rs.map(r => r.worstFrame)),
      firstPaint_p50: median(rs.map(r => r.firstPaint)),
    }))
    .sort((a, b) => {
      // Numeric buckets sort by value; symbolic buckets (0/1
      // meaning "direction" or "phase") sort by their numeric
      // index since they're already small ints.
      return Number(a.bucket) - Number(b.bucket);
    });
  return buckets;
}

// -----------------------------------------------------------------
// Output formatting
// -----------------------------------------------------------------

function pad(s, n) { s = String(s); return s.length >= n ? s : s + ' '.repeat(n - s.length); }
function padL(s, n) { s = String(s); return s.length >= n ? s : ' '.repeat(n - s.length) + s; }

function fmtMs(x) {
  if (!isFinite(x)) return 'n/a';
  if (x >= 100) return x.toFixed(0);
  if (x >= 10) return x.toFixed(1);
  return x.toFixed(2);
}

function printTable(suiteName, rows) {
  // Each row: { variant, bucketRows: [{ bucket, apply_p50, apply_min, apply_max, worstFrame_p50 }] }
  const allBuckets = new Set();
  for (const r of rows) for (const b of r.summary) allBuckets.add(b.bucket);
  const buckets = [...allBuckets].sort((a, b) => Number(a) - Number(b));

  const header = ['variant'];
  for (const b of buckets) {
    header.push(`b${b} p50`);
    header.push(`b${b} worst`);
  }
  const out = [];
  const widths = header.map(h => h.length);
  for (const r of rows) {
    widths[0] = Math.max(widths[0], r.variant.length);
    for (let i = 0; i < buckets.length; i++) {
      const b = r.summary.find(x => x.bucket === buckets[i]);
      const p50 = b ? fmtMs(b.apply_p50) : '—';
      const worst = b ? fmtMs(b.worstFrame_p50) : '—';
      widths[1 + i * 2] = Math.max(widths[1 + i * 2], p50.length);
      widths[2 + i * 2] = Math.max(widths[2 + i * 2], worst.length);
    }
  }

  out.push(`# ${suiteName}`);
  out.push('');
  out.push('| ' + header.map((h, i) => i === 0 ? pad(h, widths[i]) : padL(h, widths[i])).join(' | ') + ' |');
  out.push('|' + widths.map((w, i) => i === 0 ? ' :' + '-'.repeat(w - 1) + ' ' : ' ' + '-'.repeat(w - 1) + ': ').join('|') + '|');
  for (const r of rows) {
    if (r.error) {
      out.push('| ' + pad(r.variant, widths[0]) + ' | ' + 'ERROR'.padEnd(widths.slice(1).reduce((a,b)=>a+b,0) + 3*(widths.length-2)) + ' |');
      out.push(`> \`${r.variant}\`: ${r.error}`);
      continue;
    }
    const cells = [pad(r.variant, widths[0])];
    for (let i = 0; i < buckets.length; i++) {
      const b = r.summary.find(x => x.bucket === buckets[i]);
      cells.push(padL(b ? fmtMs(b.apply_p50) : '—', widths[1 + i * 2]));
      cells.push(padL(b ? fmtMs(b.worstFrame_p50) : '—', widths[2 + i * 2]));
    }
    out.push('| ' + cells.join(' | ') + ' |');
  }
  out.push('');
  out.push('Units: ms. `bN p50` = median apply time for bucket N. `bN worst` = median worst-frame.');
  out.push('Bucket meaning depends on the suite — see ../suites/<suite>.js.');
  return out.join('\n');
}

// -----------------------------------------------------------------
// Subcommands
// -----------------------------------------------------------------

function cmdHelp() {
  console.log(`bench-runner — headless variant benchmark driver

Usage:
  node runner/index.mjs <command> [options]

Commands:
  list                       List discovered variants + suites.
  measure                    Run one or more (variant, suite) pairs.

Options for \`measure\`:
  --variant <name>           Single variant (or 'all').
  --variants a,b,c           Comma-separated variant list.
  --suite <name>             Single suite (or 'all').
  --suites a,b,c             Comma-separated suite list.
  --<param> <value>          Override any suite param (rowsA, rowsB, iterations,
                             warmupCycles, rows, nodes, seed, maxDepth, ...).
  --timeout <ms>             Per-run watchdog timeout (default 120000).
  --chrome-port <port>       CDP port (default 9223).
  --server-port <port>       Bench HTTP server port (default 8080).
  --json                     Emit raw run records as JSON instead of tables.
  --raw                      Include raw per-iteration runs in JSON output.

Examples:
  node runner/index.mjs list
  node runner/index.mjs measure --variant idealyst-native --suite rebuild
  node runner/index.mjs measure --variant all --suite rebuild --iterations 5
  node runner/index.mjs measure --variants idealyst-native,svelte --suite all --json
`);
}

async function cmdList() {
  const variants = listVariants();
  const suites = listSuites();
  console.log('Variants (' + variants.length + '):');
  for (const v of variants) console.log('  ' + v);
  console.log('');
  console.log('Suites (' + suites.length + '):');
  for (const s of suites) {
    const params = s.params.map(p => `${p.name}=${p.default}`).join(', ');
    console.log(`  ${s.name}  [${params}]`);
  }
}

async function cmdMeasure() {
  // ---- Discovery + arg resolution ----
  const variants = listVariants();
  const suites = listSuites();
  const variantIdx = new Map(variants.map(v => [v, true]));
  const suiteIdx = new Map(suites.map(s => [s.name, s]));

  function resolveList(single, multi, all, kind) {
    if (single === 'all' || multi === 'all') return all;
    if (multi) {
      const arr = String(multi).split(',').map(s => s.trim()).filter(Boolean);
      for (const a of arr) if (!all.includes(a) && !all.some(x => (x.name ?? x) === a)) {
        throw new Error(`unknown ${kind}: ${a} (have: ${all.map(x => x.name ?? x).join(', ')})`);
      }
      return arr;
    }
    if (single) {
      if (!all.includes(single) && !all.some(x => (x.name ?? x) === single)) {
        throw new Error(`unknown ${kind}: ${single} (have: ${all.map(x => x.name ?? x).join(', ')})`);
      }
      return [single];
    }
    return null;
  }

  const variantNames = resolveList(args.flags.variant, args.flags.variants, variants, 'variant');
  const suiteNames = resolveList(args.flags.suite, args.flags.suites, suites.map(s => s.name), 'suite');

  if (!variantNames) { console.error('error: --variant or --variants required (or "all")'); process.exit(2); }
  if (!suiteNames)   { console.error('error: --suite or --suites required (or "all")'); process.exit(2); }

  // Param overrides: any --<flag> that matches a suite param name is
  // forwarded into the URL. We grab them off args.flags lazily inside
  // the loop so each suite picks up only its own params.
  const paramOverrides = {};
  for (const k of Object.keys(args.flags)) {
    paramOverrides[k] = args.flags[k];
  }

  const timeoutMs = Number(args.flags.timeout ?? 120000);
  const serverPort = DEFAULT_SERVER_PORT;
  const wantJson = Boolean(args.flags.json);
  const wantRaw = Boolean(args.flags.raw);
  const wantPhases = Boolean(args.flags.phases);

  // ---- Preflight ----
  try {
    const v = await cdpVersion();
    process.stderr.write(`[runner] chrome at :${DEFAULT_CHROME_PORT} → ${v.Browser}\n`);
  } catch (err) {
    console.error(`error: can't reach Chrome at :${DEFAULT_CHROME_PORT}/json/version — ${err.message}`);
    console.error(`hint: launch a debug-port browser, e.g.:`);
    console.error(`  /Applications/Google\\ Chrome.app/Contents/MacOS/Google\\ Chrome \\`);
    console.error(`    --remote-debugging-port=${DEFAULT_CHROME_PORT} --headless=new --no-startup-window`);
    process.exit(1);
  }
  try {
    const r = await fetch(`http://localhost:${serverPort}/index.html`);
    if (!r.ok) throw new Error('http ' + r.status);
  } catch (err) {
    console.error(`error: can't reach bench server at :${serverPort} — ${err.message}`);
    console.error(`hint: run \`benchmark/serve\` in another shell.`);
    process.exit(1);
  }

  // ---- Run ----
  const allResults = {}; // suite → [{ variant, summary, runs?, error? }]
  for (const suiteName of suiteNames) {
    const suite = suiteIdx.get(suiteName);
    if (!suite) { console.error(`skipping unknown suite ${suiteName}`); continue; }
    allResults[suiteName] = [];
    for (const variant of variantNames) {
      process.stderr.write(`[runner] ${variant} × ${suiteName} ...`);
      const t0 = Date.now();
      try {
        const { runs, phases } = await runOne({
          variant,
          suite,
          paramOverrides,
          timeoutMs,
          serverPort,
          onProgress: null,
          wantPhases,
        });
        const summary = summarize(runs);
        const entry = { variant, summary };
        if (wantRaw) entry.runs = runs;
        if (wantPhases) entry.phases = phases;
        allResults[suiteName].push(entry);
        process.stderr.write(` ok (${runs.length} iters, ${Date.now() - t0}ms${phases ? `, ${Object.keys(phases).length} phases` : ''})\n`);
      } catch (err) {
        allResults[suiteName].push({ variant, summary: [], error: err.message });
        process.stderr.write(` FAIL (${err.message})\n`);
      }
    }
  }

  // ---- Output ----
  if (wantJson) {
    console.log(JSON.stringify(allResults, null, 2));
    return;
  }
  for (const [suiteName, rows] of Object.entries(allResults)) {
    console.log(printTable(suiteName, rows));
    console.log('');
    if (wantPhases) {
      for (const r of rows) {
        if (r.phases && typeof r.phases === 'object') {
          console.log(printPhases(`${r.variant} × ${suiteName}`, r.phases));
          console.log('');
        }
      }
    }
  }
}

/// Format the framework's debug-stats phase counters as a Markdown
/// table, sorted by total time descending. Trims to the top 25
/// entries unless `BENCH_PHASE_LIMIT` env says otherwise — full
/// table goes to JSON via `--json --phases` if you need it.
function printPhases(title, phases) {
  const limit = Number(process.env.BENCH_PHASE_LIMIT ?? 25);
  const entries = Object.entries(phases).map(([name, c]) => ({
    phase: name,
    calls: c.calls,
    total_ms: c.total_us / 1000,
    max_us: c.max_us,
    avg_us: c.avg_us,
  })).sort((a, b) => b.total_ms - a.total_ms);
  const top = entries.slice(0, limit);

  const widths = {
    phase: Math.max('phase'.length, ...top.map(e => e.phase.length)),
    calls: Math.max('calls'.length, ...top.map(e => String(e.calls).length)),
    total_ms: Math.max('total_ms'.length, ...top.map(e => e.total_ms.toFixed(2).length)),
    max_us: Math.max('max_us'.length, ...top.map(e => String(e.max_us).length)),
    avg_us: Math.max('avg_us'.length, ...top.map(e => String(e.avg_us).length)),
  };
  const out = [`## phases · ${title}`, ''];
  out.push(`| ${pad('phase', widths.phase)} | ${padL('calls', widths.calls)} | ${padL('total_ms', widths.total_ms)} | ${padL('max_us', widths.max_us)} | ${padL('avg_us', widths.avg_us)} |`);
  out.push(`| :${'-'.repeat(widths.phase - 1)} | ${'-'.repeat(widths.calls - 1)}: | ${'-'.repeat(widths.total_ms - 1)}: | ${'-'.repeat(widths.max_us - 1)}: | ${'-'.repeat(widths.avg_us - 1)}: |`);
  for (const e of top) {
    out.push(`| ${pad(e.phase, widths.phase)} | ${padL(e.calls, widths.calls)} | ${padL(e.total_ms.toFixed(2), widths.total_ms)} | ${padL(e.max_us, widths.max_us)} | ${padL(e.avg_us, widths.avg_us)} |`);
  }
  if (entries.length > top.length) {
    out.push('');
    out.push(`(${entries.length - top.length} more — set BENCH_PHASE_LIMIT to see all)`);
  }
  return out.join('\n');
}

// -----------------------------------------------------------------
// Dispatch
// -----------------------------------------------------------------

try {
  switch (cmd) {
    case 'help':
    case '--help':
    case '-h':
      cmdHelp();
      break;
    case 'list':
      await cmdList();
      break;
    case 'measure':
    case 'bench':
      await cmdMeasure();
      break;
    default:
      console.error(`unknown command: ${cmd}`);
      cmdHelp();
      process.exit(2);
  }
} catch (err) {
  console.error(`error: ${err.message}`);
  process.exit(1);
}
