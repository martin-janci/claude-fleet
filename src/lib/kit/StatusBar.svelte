<!-- The 25 px footer (manual: StatusBar): health, downloads, the hub, a slow
     host still connecting; `end` holds the shortcuts sheet. No actions that
     change state; the only loaders here are the Comet and the 16 px Orbit. -->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let { children, end, testid }: { children?: Snippet; end?: Snippet; testid?: string } = $props();
</script>

<footer class="of of-statusbar" data-testid={testid}>
  {@render children?.()}
  {#if end}<span class="end">{@render end()}</span>{/if}
</footer>
