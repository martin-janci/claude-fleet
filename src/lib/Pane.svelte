<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    id,
    title = '',
    empty = '',
    children,
  }: {
    id: 'sidebar' | 'center' | 'terminal';
    title?: string;
    empty?: string;
    children?: Snippet;
  } = $props();
</script>

<section data-testid="pane-{id}" class="pane pane-{id}">
  {#if title}
    <header class="pane-header">{title}</header>
  {/if}
  <div class="pane-body">
    {#if children}
      {@render children()}
    {:else if empty}
      <p class="empty">{empty}</p>
    {/if}
  </div>
</section>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--bg-pane);
    color: var(--fg);
    border-right: 1px solid var(--border);
  }
  .pane-terminal { border-right: none; }
  .pane-header {
    padding: 0.5rem 0.75rem;
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--fg-muted);
    border-bottom: 1px solid var(--border);
  }
  .pane-body {
    flex: 1;
    overflow: auto;
    padding: 0.75rem;
  }
  .empty {
    color: var(--fg-muted);
    font-size: 0.9rem;
  }
</style>
