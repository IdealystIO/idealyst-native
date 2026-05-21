<script>
  import { state } from './state.svelte.js';
  import Branch from './Branch.svelte';
  import Leaf from './Leaf.svelte';

  const pageStyle = (t) =>
    "display: flex; flex-direction: column; padding: 32px; gap: 24px; min-height: 100%;"
    + " background: " + t.background + "; color: " + t.text + ";"
    + " transition: background 250ms ease-in-out, color 250ms ease-in-out;";
  const listStyle = (t) =>
    "display: flex; flex-direction: column; border-radius: 10px;"
    + " background: " + t.surface + "; border: 1px solid " + t.border + ";"
    + " height: 500px; overflow: hidden; overflow-y: auto;"
    + " transition: background 250ms ease-in-out, border-color 250ms ease-in-out;";
  const rowStyle = (t, parity) =>
    "display: flex; flex-direction: column;"
    + " background: " + (parity === 'odd' ? t.surface_alt : t.surface) + ";"
    + " color: " + t.text + ";"
    + " padding: 8px 16px;"
    + " border-bottom: 1px solid " + t.border + ";"
    + " font-size: 13px; height: 36px; justify-content: center;"
    + " transition: background 250ms ease-in-out, color 250ms ease-in-out, border-bottom-color 250ms ease-in-out;";
  // reactive-style row template — flat inline style, bg comes from the
  // resolved (shared OR point) color.
  const rstyleRowStyle = (t, bg) =>
    "display: flex; flex-direction: column; justify-content: center;"
    + " padding: 8px 16px; font-size: 13px; height: 36px; color: " + t.text + ";"
    + " background: " + bg + ";";
  // Canonical colors must match the verifier in suites/reactive-style.js.
  const COLOR_A = 'rgb(91, 108, 255)';
  const COLOR_B = 'rgb(255, 91, 108)';
</script>

{#if state.mode === 'tree' && state.treeRoot}
  <div style="padding: 16px; font: 12px/1.4 monospace;">
    {#if state.treeRoot.kind === 'leaf'}
      <Leaf id={state.treeRoot.id} />
    {:else}
      <Branch node={state.treeRoot} />
    {/if}
  </div>
{:else if state.mode === 'counters'}
  <div style={pageStyle(state.theme)}>
    <div style={listStyle(state.theme)}>
      {#each state.counters as _v, i (i)}
        <div style={rowStyle(state.theme, i % 2 === 0 ? 'even' : 'odd')}>
          <span>row {i}: c={state.counters[i]}</span>
        </div>
      {/each}
    </div>
  </div>
{:else if state.mode === 'rstyle'}
  <div style={pageStyle(state.theme)}>
    <div style={listStyle(state.theme)}>
      {#each state.points as _p, i (i)}
        <div style={rstyleRowStyle(
          state.theme,
          (state.points[i] ?? state.shared) === 'B' ? COLOR_B : COLOR_A,
        )}>
          <span>rstyle {i}</span>
        </div>
      {/each}
    </div>
  </div>
{:else}
  <div style={pageStyle(state.theme)}>
    <div style={listStyle(state.theme)}>
      {#each Array(state.rowCount) as _, i (i)}
        <div style={rowStyle(state.theme, i % 2 === 0 ? 'even' : 'odd')}>
          <span>Row #{i}</span>
        </div>
      {/each}
    </div>
  </div>
{/if}
