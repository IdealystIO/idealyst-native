// Module-scope reactive state. `.svelte.js` files get processed
// by the Svelte compiler so `$state` works at module scope. The
// outer JS (main.js) writes to these fields; every Svelte
// component that reads them participates in the reactive graph.
import { LIGHT } from '../../instrument.js';

export const state = $state({
  theme: LIGHT,
  rowCount: 1000,
});
