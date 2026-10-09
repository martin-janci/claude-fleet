<!-- Test fixture: a column of session rows, as the list renders them, for
     SessionRowDensity.test.ts's twenty-row measurement. -->
<script lang="ts">
  import type { ComponentProps } from 'svelte';
  import SessionRowItem from '../SessionRowItem.svelte';

  let { rows }: { rows: ComponentProps<typeof SessionRowItem>[] } = $props();
</script>

<div role="tree" aria-label="Sessions">
  {#each rows as p (p.sess.id)}
    <SessionRowItem {...p} />
  {/each}
</div>
