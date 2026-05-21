// Entry point — mounted by the bundled output. AOT-compiled by
// `vite build`, so this file (and the .svelte files it pulls in)
// land in `../pkg/main.js` with the Svelte runtime tree-shaken
// down to only what we use.
import { mount, tick } from 'svelte';
import App from './App.svelte';
import { state } from './state.svelte.js';
import { autoRunIfRequested, LIGHT, DARK } from '../../instrument.js';
import { genTreeShape } from '../../tree.js';

mount(App, { target: document.getElementById('root') });

// rebuild / toggle hooks. Writes to the shared `$state` re-fire
// every Svelte reactive read; `tick()` resolves once the
// microtask-scheduled patch flushes. We do NOT wait for a rAF —
// that would bake ~16ms of paint delay into `apply` that no other
// variant pays.
const setRows = async (n) => {
  state.mode = 'rows';
  state.rowCount = n;
  await tick();
};
const setTheme = async (name) => {
  state.theme = name === 'dark' ? DARK : LIGHT;
  await tick();
};

// hierarchy hooks. setupHierarchy is called once per run to mount
// the tree from a deterministic seed; branchUpdate/globalUpdate
// bump the per-leaf and global counters respectively.
const setupHierarchy = async (seed, nodes, maxDepth) => {
  const tree = genTreeShape(seed, nodes, maxDepth || undefined);
  state.targetLeafId = tree.targetLeaf.id;
  state.treeRoot = tree.root;
  state.mode = 'tree';
  await tick();
};
const branchUpdate = async (n) => {
  state.branchCounter = n;
  await tick();
};
const globalUpdate = async (n) => {
  state.globalCounter = n;
  await tick();
};

// granular hooks. Svelte 5's `$state` is deep — mutating
// `state.counters[i]` only invalidates the read at index `i`, so
// POINT updates fan out to exactly one row. SPREAD writes in a
// straight loop; Svelte batches the resulting effects until the
// next microtask flush.
const setupCounters = async (n) => {
  state.mode = 'counters';
  state.counters = new Array(n).fill(0);
  await tick();
};
const bumpCounter = async (i, v) => {
  state.counters[i] = v;
  await tick();
};
const bumpRange = async (s, e, v) => {
  const end = Math.min(e, state.counters.length);
  for (let i = s; i < end; i++) {
    state.counters[i] = v;
  }
  await tick();
};

// reactive-style hooks. Same fine-grained shape as counters —
// each row's `style` derives from `state.shared` and
// `state.points[i]`; bumping `shared` invalidates the derive for
// every row (O(N)), bumping `points[i]` invalidates only row i.
const setupReactiveStyles = async (n) => {
  state.mode = 'rstyle';
  state.shared = 'A';
  state.points = new Array(n).fill(null);
  await tick();
};
const setSharedColor = async (name) => {
  state.shared = name === 'B' ? 'B' : 'A';
  await tick();
};
const setPointColor = async (i, name) => {
  state.points[i] = name === 'B' ? 'B' : 'A';
  await tick();
};

autoRunIfRequested({
  setRows,
  setTheme,
  setupHierarchy,
  branchUpdate,
  globalUpdate,
  setupCounters,
  bumpCounter,
  bumpRange,
  setupReactiveStyles,
  setSharedColor,
  setPointColor,
});
