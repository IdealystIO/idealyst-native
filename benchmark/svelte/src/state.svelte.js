// Module-scope reactive state. `.svelte.js` files get processed
// by the Svelte compiler so `$state` works at module scope. The
// outer JS (main.js) writes to these fields; every Svelte
// component that reads them participates in the reactive graph.
import { LIGHT } from '../../instrument.js';

export const state = $state({
  // rebuild/toggle suites
  theme: LIGHT,
  rowCount: 1000,
  // mode discriminator: 'rows' (rebuild/toggle) or 'tree' (hierarchy)
  mode: 'rows',
  // hierarchy suite
  treeRoot: null,           // tree spec from `genTreeShape`
  targetLeafId: -1,         // which leaf reads branchCounter
  globalCounter: 0,         // every leaf reads
  branchCounter: 0,         // only the target leaf reads
});
