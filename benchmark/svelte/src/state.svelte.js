// Module-scope reactive state. `.svelte.js` files get processed
// by the Svelte compiler so `$state` works at module scope. The
// outer JS (main.js) writes to these fields; every Svelte
// component that reads them participates in the reactive graph.
import { LIGHT } from '../../instrument.js';

export const state = $state({
  // rebuild/toggle suites
  theme: LIGHT,
  rowCount: 1000,
  // mode discriminator: 'rows' (rebuild/toggle), 'tree' (hierarchy),
  // 'counters' (granular), 'rstyle' (reactive-style).
  mode: 'rows',
  // hierarchy suite
  treeRoot: null,           // tree spec from `genTreeShape`
  targetLeafId: -1,         // which leaf reads branchCounter
  globalCounter: 0,         // every leaf reads
  branchCounter: 0,         // only the target leaf reads
  // granular suite — deep-reactive: writing `counters[i] = v`
  // only invalidates the read of `counters[i]`, so only that row
  // rebuilds. Bumping one row scales O(1).
  counters: [],
  // reactive-style suite
  shared: 'A',              // 'A' or 'B' — fanned out to every row
  points: [],               // null per row = "follow shared",
                            // 'A'/'B' per row = override
});
