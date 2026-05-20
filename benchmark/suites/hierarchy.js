// Hierarchy suite: measures how each framework propagates state
// changes through a deeply nested component tree. Two operations
// alternated per iteration:
//
//   BRANCH (bucket 0): bump a counter that ONLY one specific leaf
//                      reads. Fine-grained-reactive frameworks
//                      (signals, refs, runes) update one node.
//                      Coarse re-render frameworks (naive React)
//                      re-run the whole subtree from the state
//                      owner downward.
//
//   GLOBAL (bucket 1): bump a counter that EVERY leaf reads.
//                      All frameworks pay O(N) — fine-grained
//                      pays per-subscriber Effect cost; React
//                      with Context pays one big re-render. The
//                      cost shapes differ but the order of
//                      magnitude is comparable.
//
// Tree generation is deterministic per seed — every variant
// implements the same algorithm so the tree shape is identical
// across stacks. Same seed → same tree → numbers are comparable.
//
// ┌─────────────────────────────────────────────────────────────┐
// │ TREE-GENERATION SPEC (must be ported identically per variant) │
// └─────────────────────────────────────────────────────────────┘
//
// PRNG: Mulberry32. Same algorithm in JS and Rust. Operates on
// u32 state; outputs u32.
//
//   function next(state):
//     state = (state + 0x6D2B79F5) | 0
//     t = state * (state ^ (state >>> 15) | 1)   // Math.imul
//     t = t + (t * (t ^ (t >>> 7) | 61) ^ t)     // Math.imul
//     return (t ^ (t >>> 14)) >>> 0
//
// Tree builder:
//   walk(rng, depth, maxDepth, nodeCounter, target):
//     id = nodeCounter.next++
//     if depth >= maxDepth or nodeCounter.total >= target:
//       return { kind: 'leaf', id }
//     r = rng() % 100
//     if r < 30:  // 30% leaf-at-this-depth probability
//       return { kind: 'leaf', id }
//     nChildren = 2 + (rng() % 3)   // 2-4 children
//     children = []
//     for i in 0..nChildren:
//       children.push(walk(...))
//     return { kind: 'branch', id, children }
//
// Defaults: seed=42, target=2000 leaves, maxDepth=8.
//
// After tree gen, pick the BRANCH-update target: the leaf whose
// id is the largest leaf id < target/2. This is a stable choice
// (deepest-discovered leaf in the first half of the traversal),
// reproducible from the seed alone.

const TRANSITION_MS = 250;
const SLACK_MS = 50;

export const meta = {
  name: 'hierarchy',
  title: 'Hierarchical render',
  description:
    'Tree of ~N nested components built deterministically from a seed. '
    + 'Alternates branch-update (one leaf re-reads) and global-update '
    + '(every leaf re-reads). Surfaces what fine-grained reactivity '
    + "buys vs. naive coarse re-render.",
  params: [
    { name: 'seed',         label: 'Seed',          type: 'number', default: 42,   min: 1,    max: 4294967295 },
    { name: 'nodes',        label: 'Target nodes',  type: 'number', default: 2000, min: 100,  max: 200000 },
    // `maxDepth = 0` means auto (sized from `nodes`). Bump this
    // to force a deeper tree at a given leaf count — useful for
    // testing how each framework handles deep nesting rather
    // than wide fanout.
    { name: 'maxDepth',     label: 'Max depth',     type: 'number', default: 0,    min: 0,    max: 30 },
    { name: 'iterations',   label: 'Iterations',    type: 'number', default: 20,   min: 2,    max: 200 },
    { name: 'warmupCycles', label: 'Warmup pairs',  type: 'number', default: 2,    min: 0,    max: 10 },
  ],
};

/// Run the suite. Variants must expose:
///   - `setupHierarchy(seed, nodes)`  mount the tree once
///   - `branchUpdate(n)`              bump branch-counter, only target leaf re-reads
///   - `globalUpdate(n)`              bump global-counter, every leaf re-reads
export async function run({ setupHierarchy, branchUpdate, globalUpdate, params, onProgress }) {
  if (typeof setupHierarchy !== 'function') {
    throw new Error("hierarchy suite: variant must expose setupHierarchy(seed, nodes)");
  }
  if (typeof branchUpdate !== 'function') {
    throw new Error("hierarchy suite: variant must expose branchUpdate(n)");
  }
  if (typeof globalUpdate !== 'function') {
    throw new Error("hierarchy suite: variant must expose globalUpdate(n)");
  }

  const seed = Number(params?.seed ?? 42);
  const nodes = Number(params?.nodes ?? 2000);
  const maxDepth = Number(params?.maxDepth ?? 0);
  const iterations = Number(params?.iterations ?? 20);
  const warmupCycles = Number(params?.warmupCycles ?? 2);

  // One-time mount. Builds the tree deterministically from the
  // seed; same seed → same tree → comparable numbers across
  // variants and across runs. `maxDepth=0` lets the variant
  // auto-size it from `nodes` (log_2.55(nodes) + 2).
  await setupHierarchy(seed, nodes, maxDepth);

  // Warmup: do warmupCycles pairs of (branch, global) updates,
  // untimed. Burns in JIT, font caches, and any first-update
  // overhead at both polarities.
  let counter = 0;
  for (let i = 0; i < warmupCycles; i++) {
    counter++;
    await measureOne(() => branchUpdate(counter));
    counter++;
    await measureOne(() => globalUpdate(counter));
  }

  const runs = [];
  for (let i = 0; i < iterations; i++) {
    counter++;
    const isBranch = i % 2 === 0;
    const bucket = isBranch ? 0 : 1;  // 0 = BRANCH, 1 = GLOBAL
    const op = isBranch
      ? () => branchUpdate(counter)
      : () => globalUpdate(counter);
    const m = await measureOne(op);
    runs.push({
      iter: i + 1,
      bucket,
      apply: m.apply,
      firstPaint: m.firstPaint,
      worstFrame: m.worstFrame,
    });
    if (onProgress) onProgress(runs);
    // Brief settle gap between iterations to drain any queued GC /
    // paint work.
    await new Promise(r => setTimeout(r, 50));
  }

  return runs;
}

/// Same `measureOne` shape as `rebuild.js` and `toggle.js`.
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
