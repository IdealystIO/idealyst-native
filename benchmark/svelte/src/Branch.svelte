<script>
  import Leaf from './Leaf.svelte';

  let { node } = $props();
  // `node` is a tree-spec object: { kind, id, children? }
  // Static throughout this component's lifetime — Branch reads
  // no reactive state of its own, so Svelte's scheduler never
  // re-runs this component after initial mount. Counter updates
  // flow straight to the affected Leaf, skipping all
  // intermediate Branches.
</script>

<div class="b">
  {#each node.children as child (child.id)}
    {#if child.kind === 'leaf'}
      <Leaf id={child.id} />
    {:else}
      <svelte:self node={child} />
    {/if}
  {/each}
</div>
