<script>
  import { state } from './state.svelte.js';

  let { id } = $props();

  // Computed once per instance — `targetLeafId` is set before
  // any Leaf mounts and never changes afterward. Closing over
  // it avoids checking on every reactive re-run.
  const isTarget = id === state.targetLeafId;
</script>

{#if isTarget}
  <span>leaf {id}: g={state.globalCounter} b={state.branchCounter}</span>
{:else}
  <span>leaf {id}: g={state.globalCounter}</span>
{/if}
