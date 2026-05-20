// Entry point — mounted by the bundled output. AOT-compiled by
// `vite build`, so this file (and the .svelte files it pulls in)
// land in `../pkg/main.js` with the Svelte runtime tree-shaken
// down to only what we use.
import { mount, tick } from 'svelte';
import App from './App.svelte';
import { state } from './state.svelte.js';
import { autoRunIfRequested, LIGHT, DARK } from '../../instrument.js';

mount(App, { target: document.getElementById('root') });

// setRows / setTheme for the runner-iframe path. Writes to the
// shared `$state` re-fire every Svelte reactive read; `tick()`
// resolves once the microtask-scheduled patch flushes. We do
// NOT wait for a rAF — that would bake ~16ms of paint delay
// into `apply` that no other variant pays.
const setRows = async (n) => {
  state.rowCount = n;
  await tick();
};
const setTheme = async (name) => {
  state.theme = name === 'dark' ? DARK : LIGHT;
  await tick();
};

autoRunIfRequested({ setRows, setTheme });
