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

autoRunIfRequested({
  setRows,
  setTheme,
  setupHierarchy,
  branchUpdate,
  globalUpdate,
});
